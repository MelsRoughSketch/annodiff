use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
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

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub name: Option<String>,
    pub preview: String,
    pub cwd: String,
    pub updated_at: i64,
}
impl Session {
    pub fn matches(&self, root: &str, current_only: bool, query: &str) -> bool {
        (!current_only || Path::new(&self.cwd) == Path::new(root))
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
    json!({"clientInfo":{"name":"annodiff","version":"0.1.0"}})
}

struct RpcProcess {
    child: Child,
    input: ChildStdin,
    messages: Receiver<Result<Value>>,
    id: u64,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
}
impl Drop for RpcProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl RpcProcess {
    fn start(root: &str, cancel: Arc<AtomicBool>) -> Result<Self> {
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
pub fn sessions(root: &str, cancel: Arc<AtomicBool>) -> Result<Vec<Session>> {
    let mut rpc = RpcProcess::start(root, cancel)?;
    let mut sessions = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let result=rpc.call("thread/list",json!({"limit":100,"sortKey":"updated_at","sourceKinds":["cli","vscode","exec","appServer"],"cursor":cursor}))?;
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
    sessions.sort_by(|a, b| {
        (b.cwd == root)
            .cmp(&(a.cwd == root))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
    });
    Ok(sessions)
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
    let mut call = |request_id, method: &str, params: Value| -> Result<Value> {
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
    call(1, "initialize", initialize())?;
    // End the closure borrow to send the notification between requests.
    socket.send(tungstenite::Message::Text(
        json!({"method":"initialized"}).to_string().into(),
    ))?;
    socket.send(tungstenite::Message::Text(
        json!({"id":2,"method":"thread/read","params":{"threadId":id,"includeTurns":false}})
            .to_string()
            .into(),
    ))?;
    loop {
        socket.get_ref().set_read_timeout(Some(
            deadline
                .checked_duration_since(Instant::now())
                .context("Codex daemon request timed out")?,
        ))?;
        let message = socket.read()?;
        if message.is_close() {
            bail!("Codex daemon closed the connection")
        }
        if !(message.is_text() || message.is_binary()) {
            continue;
        }
        if let Some(result) = response(serde_json::from_slice(message.into_data().as_ref())?, 2)? {
            return match result["thread"]["status"]["type"].as_str() {
                Some("notLoaded") => Ok(false),
                Some("idle" | "active" | "systemError") => Ok(true),
                other => bail!("unknown Codex session status: {other:?}"),
            };
        }
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
