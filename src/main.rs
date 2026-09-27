use annodiff::{
    agent,
    app::{App, Effect},
    review,
};
use anyhow::{Context, Result, bail, ensure};
use base64::Engine;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    },
    execute,
};
use std::{
    io::{self, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

#[cfg(unix)]
static KEYBOARD_ENHANCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = disable_input();
        ratatui::restore();
    }
}
fn disable_input() -> io::Result<()> {
    #[cfg(unix)]
    if KEYBOARD_ENHANCED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        execute!(io::stdout(), event::PopKeyboardEnhancementFlags)?;
    }
    execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste)
}
fn init() -> Result<ratatui::DefaultTerminal> {
    let terminal = ratatui::try_init()?;
    // Unix terminals need CSI-u to distinguish Ctrl+Enter from Enter. Windows
    // reports modifiers through its native input API; these commands are unsupported there.
    #[cfg(unix)]
    {
        execute!(
            io::stdout(),
            event::PushKeyboardEnhancementFlags(
                event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        )?;
        KEYBOARD_ENHANCED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    // Pop before Ratatui's panic hook leaves the alternate screen. Popping later
    // in Drop could otherwise change the parent application's keyboard settings.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_input();
        hook(info);
    }));
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    Ok(terminal)
}
fn suspend(terminal: &mut ratatui::DefaultTerminal, command: &mut Command) -> Result<()> {
    disable_input()?;
    ratatui::restore();
    let result = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();
    *terminal = init()?;
    let status = result.context("start external command")?;
    ensure!(status.success(), "external command exited with {status}");
    Ok(())
}
fn send(app: &mut App, terminal: &mut ratatui::DefaultTerminal, id: &str) -> Result<()> {
    app.fresh()?;
    let queued = !id.is_empty() && agent::session_loaded(id)?;
    let mut prompt = app.prompt();
    // Leave room for CLI arguments/quoting on Windows and Unix. NUL cannot be an argument.
    let path = if prompt.len() > 8 * 1024 || prompt.contains('\0') {
        let mut file = tempfile::Builder::new()
            .prefix("annodiff-review-")
            .suffix(".md")
            .tempfile_in(app.state.parent().context("missing state directory")?)?;
        file.write_all(prompt.as_bytes())?;
        file.flush()?;
        let (file, path) = file.keep()?;
        drop(file);
        prompt = format!(
            "Read the review in {:?} and address all user comments in repository {:?}. It contains file-wide comments and annotated excerpts. Read repository files and git diff for additional context as needed.",
            path.to_string_lossy(),
            app.review.root
        );
        Some(path)
    } else {
        None
    };
    let retained = || {
        path.as_ref().map_or_else(
            || "comments remain in the saved review".to_owned(),
            |path| format!("review retained at {}", path.display()),
        )
    };
    let mut command = agent::command(&app.review.root, id, &prompt, queued);
    let deliver = if queued {
        command.output().context("queue review").and_then(|out| {
            ensure!(
                out.status.success(),
                "queue failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            Ok(())
        })
    } else {
        suspend(terminal, &mut command)
    };
    deliver.with_context(retained)?;
    let mut next = app.review.clone();
    for comment in next
        .files
        .iter_mut()
        .flat_map(|f| &mut f.comments)
        .filter(|c| c.pending())
    {
        comment.sent = true;
        if queued && let Some(path) = &path {
            comment.delivery = path.file_name().unwrap().to_string_lossy().into_owned();
        }
    }
    app.apply(next)
        .with_context(|| format!("sent, but status could not be saved; {}", retained()))?;
    if queued {
        app.status = path.as_ref().map_or_else(
            || "Queued. Comments are Sent and Open.".into(),
            |path| {
                format!(
                    "Queued; review retained until all comments are done: {}",
                    path.display()
                )
            },
        );
    } else {
        if let Some(path) = &path {
            review::remove_if_exists(path)?;
        }
        app.status =
            "Codex closed. Comments are Sent and Open; r refreshes, 2 opens history.".into();
    }
    Ok(())
}
fn run(mut app: App) -> Result<()> {
    let _restore = Restore;
    let mut terminal = init()?;
    let mut dirty = true;
    loop {
        dirty |= app.poll() || app.highlight_pending;
        if dirty {
            terminal.draw(|f| app.draw(f))?;
            dirty = false;
        }
        if !event::poll(Duration::from_millis(if app.highlight_pending {
            0
        } else {
            50
        }))? {
            continue;
        }
        let event = event::read()?;
        dirty = true;
        let effect = match app.handle(event) {
            Ok(effect) => effect,
            Err(e) => {
                app.status = format!("{e:#}");
                continue;
            }
        };
        let result = match effect {
            Effect::None => Ok(()),
            Effect::Quit => {
                app.close_modal();
                break;
            }
            Effect::Clipboard(value) => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(value.as_bytes());
                write!(terminal.backend_mut(), "\x1b]52;c;{encoded}\x07")?;
                terminal.backend_mut().flush()?;
                app.status =
                    "Clipboard copy requested (requires terminal support). Comments remain unsent."
                        .into();
                Ok(())
            }
            Effect::Send(id) => {
                app.status = "Sending review…".into();
                terminal.draw(|f| app.draw(f))?;
                send(&mut app, &mut terminal, &id)
            }
            Effect::Editor => (|| -> Result<()> {
                let file = app.current().context("no selected file")?;
                let path = Path::new(&app.review.root).join(&file.path);
                ensure!(path.try_exists()?, "file no longer exists");
                let source = app
                    .view()
                    .and_then(|v| v.source(app.cursor[0], app.side))
                    .or_else(|| app.selected_ref().map(|r| app.comment(r).1.start));
                let line = source
                    .and_then(|i| file.lines.get(i))
                    .map_or(0, |l| if l.new > 0 { l.new } else { l.old });
                suspend(
                    &mut terminal,
                    &mut agent::editor_command(&app.review.root, &path, line),
                )?;
                app.refresh(false)
            })(),
        };
        if let Err(e) = result {
            app.status = format!("{e:#}");
        }
    }
    Ok(())
}
fn main() {
    if let Err(e) = main_result() {
        eprintln!("annodiff: {e:#}");
        std::process::exit(1);
    }
}
fn main_result() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--licenses") => {
            print!(
                "{}\n{}",
                include_str!("../LICENSE"),
                include_str!("../THIRD_PARTY_NOTICES.md")
            );
            Ok(())
        }
        Some("--help" | "-h") => {
            println!(
                "Usage: annodiff [repository-directory]\nReview combined staged/unstaged changes against HEAD, including untracked files.\n\n--snapshot-json [directory]  Export a fresh review snapshot (no writes)\n--check-state PATH           Validate a saved review file\n--prompt PATH                Print pending review comments as Markdown\n--rewrite-state IN OUT       Validate and re-save review JSON\n--licenses                   Print licenses and third-party notices\n--version                    Print version"
            );
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("annodiff {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--snapshot-json") => {
            ensure!(args.len() <= 2, "too many arguments");
            let root = review::root(args.get(1).map_or(".", String::as_str))?;
            serde_json::to_writer_pretty(io::stdout(), &review::snapshot(&root, "", "")?)?;
            println!();
            Ok(())
        }
        Some("--check-state" | "--prompt") => {
            ensure!(args.len() == 2, "expected a review file path");
            let review = review::Review::load(Path::new(&args[1]))?;
            if args[0] == "--prompt" {
                print!("{}", review.prompt())
            } else {
                println!(
                    "Valid review: {} files, {} comments",
                    review.files.len(),
                    review.count()
                )
            }
            Ok(())
        }
        Some("--rewrite-state") => {
            ensure!(args.len() == 3, "expected input and output review paths");
            let _lock = review::lock_state(Path::new(&args[2]))?;
            review::Review::load(Path::new(&args[1]))?.save(Path::new(&args[2]))
        }
        Some(flag) if flag.starts_with('-') => bail!("unknown option: {flag}; use --help"),
        _ => {
            ensure!(args.len() <= 1, "usage: annodiff [repository-directory]");
            run(App::open(args.first().map_or(".", String::as_str))?)
        }
    }
}
