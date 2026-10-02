use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionOptions {
    pub all: bool,
    pub archived: bool,
    pub created: bool,
}
impl SessionOptions {
    pub fn matching(self, items: &[Session], query: String) -> impl Iterator<Item = &Session> {
        items.iter().filter(move |s| s.matches(!self.all, &query))
    }

    pub fn sort(&self, items: &mut [Session]) {
        items.sort_by_key(|s| {
            std::cmp::Reverse(if self.created {
                s.created_at
            } else {
                s.updated_at
            })
        });
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub name: Option<String>,
    pub preview: String,
    pub cwd: String,
    pub updated_at: i64,
    pub created_at: i64,
    /// Computed once for the directory used to list sessions, never trusted from RPC.
    #[serde(skip)]
    pub current: bool,
}
impl Session {
    pub fn matches(&self, current_only: bool, query: &str) -> bool {
        (!current_only || self.current)
            && format!("{} {} {}", self.title(), self.id, self.cwd)
                .to_lowercase()
                .contains(query)
    }
    pub fn title(&self) -> &str {
        self.name
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or(&self.preview)
    }
}

pub fn response(message: Value, id: u64) -> Result<Option<Value>> {
    let Some(message_id) = message.get("id").and_then(Value::as_u64) else {
        return Ok(None);
    };
    ensure!(
        message.get("method").is_none(),
        "unexpected server request: {}",
        message["method"]
    );
    if message_id != id {
        return Ok(None);
    }
    if let Some(error) = message.get("error") {
        bail!("Codex: {}", error["message"])
    }
    Ok(Some(message.get("result").cloned().unwrap_or(Value::Null)))
}
fn initialize() -> Value {
    json!({"clientInfo":{"name":"annodiff","version":env!("CARGO_PKG_VERSION")}})
}

pub struct SessionClient {
    child: Child,
    input: ChildStdin,
    messages: Receiver<Result<Value>>,
    id: u64,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
}
impl Drop for SessionClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl SessionClient {
    pub fn start(root: &str, cancel: Arc<AtomicBool>) -> Result<Self> {
        let mut child = Command::new("codex")
            .arg("app-server")
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("start codex app-server")?;
        let input = child.stdin.take().context("missing stdin")?;
        let output = child.stdout.take().context("missing stdout")?;
        // Drain stderr so a noisy child cannot deadlock. RPC errors are shown in the UI.
        if let Some(mut err) = child.stderr.take() {
            thread::spawn(move || {
                let _ = std::io::copy(&mut err, &mut std::io::sink());
            });
        }
        let (tx, messages) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let parsed = line
                    .map_err(anyhow::Error::from)
                    .and_then(|s| Ok(serde_json::from_str(&s)?));
                if tx.send(parsed).is_err() {
                    break;
                }
            }
        });
        let mut rpc = Self {
            child,
            input,
            messages,
            id: 0,
            deadline: Instant::now() + Duration::from_secs(30),
            cancel,
        };
        rpc.call("initialize", initialize())?;
        rpc.send(json!({"method":"initialized"}))?;
        Ok(rpc)
    }
    fn send(&mut self, message: Value) -> Result<()> {
        serde_json::to_writer(&mut self.input, &message)?;
        self.input.write_all(b"\n")?;
        self.input.flush()?;
        Ok(())
    }
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.id += 1;
        self.send(json!({"id":self.id,"method":method,"params":params}))?;
        loop {
            ensure!(
                !self.cancel.load(Ordering::Relaxed),
                "session listing cancelled"
            );
            ensure!(Instant::now() < self.deadline, "Codex request timed out");
            match self.messages.recv_timeout(Duration::from_millis(100)) {
                Ok(message) => {
                    if let Some(result) = response(message?, self.id)? {
                        return Ok(result);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => bail!("Codex app-server closed its output"),
            }
        }
    }
}
fn normalized_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn session_directories(root: &Path) -> HashSet<PathBuf> {
    let root = normalized_path(root);
    let mut directories = HashSet::from([root.clone()]);
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| out.stdout)
    };
    let Some(top) =
        git(&["rev-parse", "--show-toplevel"]).and_then(|bytes| String::from_utf8(bytes).ok())
    else {
        return directories;
    };
    let top = normalized_path(Path::new(top.strip_suffix('\n').unwrap_or(&top)));
    let Ok(relative) = root.strip_prefix(&top) else {
        return directories;
    };
    // Codex PWD includes the corresponding directory in linked worktrees, not all descendants.
    if let Some(output) = git(&["worktree", "list", "--porcelain", "-z"]) {
        for field in output.split(|b| *b == 0) {
            if let Some(checkout) = field
                .strip_prefix(b"worktree ")
                .and_then(|p| std::str::from_utf8(p).ok())
            {
                let checkout = normalized_path(Path::new(checkout));
                let candidate = normalized_path(&checkout.join(relative));
                if candidate.is_dir()
                    && candidate.strip_prefix(&checkout).ok() == Some(relative)
                    && candidate.ancestors().find(|p| p.join(".git").exists())
                        == Some(checkout.as_path())
                {
                    directories.insert(candidate);
                }
            }
        }
    }
    directories
}

impl SessionClient {
    pub fn sessions(
        &mut self,
        root: &str,
        options: SessionOptions,
        cancel: Arc<AtomicBool>,
    ) -> Result<Vec<Session>> {
        self.cancel = cancel;
        self.deadline = Instant::now() + Duration::from_secs(30);
        let config = self.call("config/read", json!({"includeLayers":false,"cwd":root}))?;
        let provider = config["config"]["model_provider"]
            .as_str()
            .unwrap_or("openai");
        let worktrees = config["config"]["features"]["worktrees"]
            .as_bool()
            .unwrap_or(true);
        let directories = if worktrees {
            session_directories(Path::new(root))
        } else {
            HashSet::from([normalized_path(Path::new(root))])
        };
        let cwd = if options.all {
            Value::Null
        } else {
            json!(directories)
        };
        let mut sessions = Vec::new();
        let mut cursor = Value::Null;
        let mut db_only = true;
        loop {
            let result = self.call(
                "thread/list",
                json!({
                    "limit": 100,
                    "sortKey": if options.created { "created_at" } else { "updated_at" },
                    "sourceKinds": ["cli", "vscode"],
                    "modelProviders": [provider],
                    "archived": options.archived,
                    "cwd": cwd,
                    "useStateDbOnly": db_only,
                    "cursor": cursor,
                }),
            );
            // Match resume: repair rollouts only if the initial DB result is unusable.
            // An empty later page must not restart a scan or change the list's source.
            if db_only
                && sessions.is_empty()
                && result
                    .as_ref()
                    .map_or(true, |r| r["data"].as_array().is_none_or(Vec::is_empty))
            {
                db_only = false;
                cursor = Value::Null;
                continue;
            }
            let result = result?;
            sessions.extend(
                serde_json::from_value::<Vec<Session>>(result["data"].clone())
                    .context("decode Codex thread/list sessions")?,
            );
            let next = result["nextCursor"].clone();
            if next.is_null() {
                break;
            }
            ensure!(next != cursor, "Codex returned a repeated cursor");
            cursor = next;
        }
        for session in &mut sessions {
            session.current = !session.cwd.is_empty()
                && directories.contains(&normalized_path(Path::new(&session.cwd)));
        }
        options.sort(&mut sessions);
        Ok(sessions)
    }
}
pub fn unarchive_session(root: &str, id: &str) -> Result<()> {
    let mut rpc = SessionClient::start(root, Arc::new(AtomicBool::new(false)))?;
    rpc.call("thread/unarchive", json!({"threadId":id}))?;
    Ok(())
}
pub fn codex_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|s| !s.is_empty()) {
        return Ok(home.into());
    }
    Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".codex"))
}
#[cfg(unix)]
pub fn session_loaded(id: &str) -> Result<bool> {
    session_loaded_at(
        &codex_home()?.join("app-server-control/app-server-control.sock"),
        id,
    )
}
#[cfg(not(unix))]
pub fn session_loaded(_id: &str) -> Result<bool> {
    bail!("loaded-session detection requires Unix sockets on this platform")
}
#[cfg(unix)]
pub fn session_loaded_at(path: &Path, id: &str) -> Result<bool> {
    use std::os::unix::net::UnixStream;
    let stream = match UnixStream::connect(path) {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false);
        }
        Err(e) => return Err(e.into()),
    };
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let (mut socket, _) = tungstenite::client("ws://localhost/", stream)
        .map_err(|e| anyhow::anyhow!("daemon WebSocket: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let call = |socket: &mut tungstenite::WebSocket<UnixStream>,
                request_id,
                method: &str,
                params: Value|
     -> Result<Value> {
        socket.send(tungstenite::Message::Text(
            json!({"id":request_id,"method":method,"params":params})
                .to_string()
                .into(),
        ))?;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("Codex daemon request timed out")?;
            socket.get_ref().set_read_timeout(Some(remaining))?;
            let message = socket.read()?;
            if message.is_close() {
                bail!("Codex daemon closed the connection")
            }
            if (message.is_text() || message.is_binary())
                && let Some(result) = response(
                    serde_json::from_slice(message.into_data().as_ref())?,
                    request_id,
                )?
            {
                return Ok(result);
            }
        }
    };
    call(&mut socket, 1, "initialize", initialize())?;
    socket.send(tungstenite::Message::Text(
        json!({"method":"initialized"}).to_string().into(),
    ))?;
    let result = call(
        &mut socket,
        2,
        "thread/read",
        json!({"threadId":id,"includeTurns":false}),
    )?;
    match result["thread"]["status"]["type"].as_str() {
        Some("notLoaded") => Ok(false),
        Some("idle" | "active" | "systemError") => Ok(true),
        other => bail!("unknown Codex session status: {other:?}"),
    }
}

pub fn command(root: &str, id: &str, prompt: &str, queued: bool) -> Command {
    let mut command = Command::new("codex");
    if id.is_empty() {
        command.args(["-C", root, "--", prompt]);
    } else if queued {
        command.args(["queue", "--thread", id, "--message", prompt]);
    } else {
        command.args(["resume", "-C", root, id, prompt]);
    }
    command.current_dir(root);
    command
}
pub fn editor_command(root: &str, path: &Path, line: usize) -> Command {
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".into());
    let name = Path::new(editor.split_whitespace().next().unwrap_or("vi"))
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let mut args = Vec::<String>::new();
    let mut path = path.to_string_lossy().into_owned();
    match name.as_ref() {
        "vi" | "vim" | "nvim" | "nano" | "emacs" | "emacsclient" => {
            if line > 0 {
                args.push(format!("+{line}"));
            }
        }
        "code" | "codium" => {
            args.push("--wait".into());
            if line > 0 {
                args.push("--goto".into());
                path = format!("{path}:{line}");
            }
        }
        _ => {}
    }
    args.push(path);
    let mut command = Command::new("sh");
    command
        .args(["-c", &format!("{editor} \"$@\""), "annodiff-editor"])
        .args(args)
        .current_dir(root);
    command
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn pwd_scope_matches_linked_worktrees_at_the_same_relative_directory() {
        let temp = tempfile::tempdir().unwrap();
        let primary = temp.path().join("primary 日本語");
        let linked = temp.path().join("linked checkout");
        let other = temp.path().join("other");
        let git = |root: &Path, args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        std::fs::create_dir_all(primary.join("src")).unwrap();
        std::fs::write(primary.join("src/file"), "content").unwrap();
        git(&primary, &["init", "-q"]);
        git(&primary, &["add", "."]);
        git(
            &primary,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "initial",
            ],
        );
        git(
            &primary,
            &["worktree", "add", "--detach", linked.to_str().unwrap()],
        );
        let roots = HashSet::from([normalized_path(&primary), normalized_path(&linked)]);
        assert_eq!(session_directories(&primary), roots);
        assert_eq!(session_directories(&linked), roots);
        let subdirs = HashSet::from([
            normalized_path(&primary.join("src")),
            normalized_path(&linked.join("src")),
        ]);
        assert_eq!(session_directories(&primary.join("src")), subdirs);
        assert_eq!(session_directories(&linked.join("src")), subdirs);
        assert_eq!(session_directories(&primary.join("src/..")), roots);
        std::fs::create_dir(&other).unwrap();
        git(&other, &["init", "-q"]);
        assert_eq!(
            session_directories(&other),
            HashSet::from([normalized_path(&other)])
        );
        let missing = temp.path().join("missing");
        assert_eq!(session_directories(&missing), HashSet::from([missing]));
        #[cfg(unix)]
        {
            let alias = temp.path().join("alias");
            std::os::unix::fs::symlink(&primary, &alias).unwrap();
            assert_eq!(session_directories(&alias), roots);
            assert_eq!(normalized_path(&alias), normalized_path(&primary));
        }
        std::fs::create_dir(primary.join("only-here")).unwrap();
        assert_eq!(
            session_directories(&primary.join("only-here")),
            HashSet::from([normalized_path(&primary.join("only-here"))])
        );
        // A nested repository at the corresponding path is not the same project directory.
        git(&linked.join("src"), &["init", "-q"]);
        assert_eq!(
            session_directories(&primary.join("src")),
            HashSet::from([normalized_path(&primary.join("src"))])
        );
    }
}
