use annodiff::agent;
use serde_json::json;
use std::path::Path;

#[test]
fn protocol_routes_notifications_and_errors() {
    assert_eq!(
        agent::response(json!({"method":"notice"}), 1).unwrap(),
        None
    );
    assert_eq!(
        agent::response(json!({"id":2,"result":42}), 1).unwrap(),
        None
    );
    assert_eq!(
        agent::response(json!({"id":1,"result":{"ok":true}}), 1).unwrap(),
        Some(json!({"ok":true}))
    );
    assert!(agent::response(json!({"id":1,"method":"unexpected"}), 1).is_err());
    assert!(
        agent::response(json!({"id":1,"error":{"message":"failed"}}), 1)
            .unwrap_err()
            .to_string()
            .contains("failed")
    );
    for (id, queued, want) in [
        ("", false, "-C"),
        ("thread", false, "resume"),
        ("thread", true, "queue"),
    ] {
        let cmd = agent::command("/repo with space", id, Path::new("/tmp/review.md"), queued);
        assert_eq!(cmd.get_args().next().unwrap(), want);
        let prompt = cmd.get_args().last().unwrap().to_string_lossy();
        assert!(prompt.contains("/repo with space"));
        assert!(prompt.contains("/tmp/review.md"));
    }
}

#[cfg(unix)]
#[test]
fn websocket_over_unix_loaded_detection() {
    use std::{os::unix::net::UnixListener, thread};
    let temp = tempfile::tempdir().unwrap();
    let absent = temp.path().join("absent.sock");
    assert!(!agent::session_loaded_at(&absent, "thread").unwrap());
    for status in ["notLoaded", "idle", "active", "systemError", "unknown"] {
        let path = temp.path().join(format!("{status}.sock"));
        let listener = UnixListener::bind(&path).unwrap();
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let request: serde_json::Value =
                serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "initialize");
            ws.send(tungstenite::Message::Text(
                json!({"id":request["id"],"result":{}}).to_string().into(),
            ))
            .unwrap();
            let initialized: serde_json::Value =
                serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(initialized["method"], "initialized");
            let request: serde_json::Value =
                serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["method"], "thread/read");
            assert_eq!(request["params"]["threadId"], "thread");
            ws.send(tungstenite::Message::Text(
                json!({"method":"notification"}).to_string().into(),
            ))
            .unwrap();
            ws.send(tungstenite::Message::Text(
                json!({"id":request["id"],"result":{"thread":{"status":{"type":status}}}})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        });
        let loaded = agent::session_loaded_at(&path, "thread");
        worker.join().unwrap();
        if status == "unknown" {
            assert!(loaded.is_err())
        } else {
            assert_eq!(loaded.unwrap(), status != "notLoaded");
        }
    }
}

#[test]
fn session_name_accepts_null_missing_and_empty() {
    for name in [
        None,
        Some(json!(null)),
        Some(json!("")),
        Some(json!("Named session")),
    ] {
        let mut value =
            json!({"id":"thread", "preview":"Preview fallback", "cwd":"/repo", "updatedAt":1});
        if let Some(name) = name {
            value["name"] = name;
        }
        let session: agent::Session = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            session.title(),
            if value["name"] == "Named session" {
                "Named session"
            } else {
                "Preview fallback"
            }
        );
    }
}

#[test]
#[ignore = "reads local Codex session metadata without starting a model turn"]
fn live_session_listing() {
    let root = std::env::current_dir().unwrap();
    let sessions = agent::sessions(
        root.to_str().unwrap(),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .unwrap();
    assert!(sessions.iter().all(|s| !s.id.is_empty()));
    println!("Listed {} sessions", sessions.len());
}
