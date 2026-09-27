"""Exercise the real Ratatui event loop through a PTY, with fake local tools."""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import struct
import subprocess
import sys
import tempfile
import termios
import time

repo = Path(__file__).resolve().parents[1]
subprocess.run(["cargo", "build", "--manifest-path", str(repo / "Cargo.toml"),
                "--locked", "--offline"], check=True)
binary = repo / "target/debug/annodiff"

with tempfile.TemporaryDirectory(prefix="annodiff-terminal-") as directory:
    directory = Path(directory)
    tools = directory / "bin"
    tools.mkdir()
    codex = tools / "codex"
    codex.write_text(f"#!{sys.executable}\n" + r'''
import json, os, pathlib, re, sys
if sys.argv[1:] == ["app-server"]:
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        result = {} if request["method"] == "initialize" else {"data": [{"id": "unnamed", "name": None, "preview": "Unnamed session preview", "cwd": os.getcwd(), "updatedAt": 1}], "nextCursor": None}
        print(json.dumps({"id": request["id"], "result": result}), flush=True)
else:
    prompt = sys.argv[-1]
    match = re.search(r'Read the review in (".*?") and address', prompt)
    path = pathlib.Path(json.loads(match[1]))
    pathlib.Path(os.environ["FAKE_PAYLOAD"]).write_text(path.read_text())
    print("fake-codex-finished", flush=True)
    sys.exit(int(os.environ["FAKE_FAIL"]))
''', encoding="utf-8")
    codex.chmod(0o700)
    editor = tools / "editor"
    editor.write_text(f"#!{sys.executable}\n" + r'''
import pathlib, sys
path = pathlib.Path(sys.argv[-1])
path.write_text(path.read_text() + "// external editor returned\n")
print("fake-editor-finished", flush=True)
''', encoding="utf-8")
    editor.chmod(0o700)
    config = directory / "config/annodiff"
    config.mkdir(parents=True)
    (config / "config.toml").write_text('response_language = "jp"\n')

    for case, (failure, save_key) in enumerate(((0, "\x1b[13;5u"), (0, "\n"), (7, "\x1bOQ"))):
        root = directory / f"repo{case}"
        root.mkdir()
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        (root / "sample.go").write_text('package main\nfunc main() {}\n')
        payload = directory / f"payload{case}.md"
        env = dict(os.environ, TERM="xterm-256color", PATH=str(tools) + os.pathsep + os.environ["PATH"],
                   VISUAL=str(editor), CODEX_HOME=str(directory / "codex-home"),
                   XDG_CONFIG_HOME=str(config.parent), FAKE_PAYLOAD=str(payload), FAKE_FAIL=str(failure),
                   GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_SYSTEM="/dev/null")
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
        original = termios.tcgetattr(slave)
        process = subprocess.Popen([str(binary), str(root)], stdin=slave, stdout=slave, stderr=slave,
                                   env=env, start_new_session=True)
        output = bytearray()

        def pump(duration=0.1):
            deadline = time.monotonic() + duration
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], max(0, deadline - time.monotonic()))
                if not ready:
                    break
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise
                if not data:
                    break
                output.extend(data)

        def wait_for(predicate):
            deadline = time.monotonic() + 15
            while not predicate():
                pump()
                if process.poll() is not None or time.monotonic() > deadline:
                    raise AssertionError(output.decode("utf-8", errors="replace")[-6000:])

        def send(value):
            os.write(master, value.encode("utf-8"))
            pump(0.2)

        state = root / ".git/annodiff.json"
        try:
            wait_for(lambda: b"Ready" in output)
            send("e")
            wait_for(lambda: b"fake-editor-finished" in output)
            wait_for(lambda: state.exists())
            send("1c")  # Inline file comment; input letters remain literal.
            send("日本語 comment\r")
            send("\x1b[200~[red] literal\x1b[201~")
            send(save_key)
            wait_for(lambda: json.loads(state.read_text())["Files"][0]["Comments"])
            saved = json.loads(state.read_text())
            assert saved["Files"][0]["Comments"][0]["Text"] == "日本語 comment\n[red] literal", repr(saved["Files"][0]["Comments"][0]["Text"])
            # A second writer must fail before it can load stale state or enter the TUI.
            before = state.read_bytes()
            for args in ([root], ["--rewrite-state", state, state]):
                other = subprocess.run([str(binary), *map(str, args)], env=env,
                                       capture_output=True, timeout=5)
                assert other.returncode != 0
                assert b"cannot lock" in other.stderr, other.stderr
                assert state.read_bytes() == before
            send(save_key)  # Destination selection, not delivery.
            wait_for(lambda: b"Destination" in output)
            assert not payload.exists()
            send("\r")  # Preview.
            wait_for(lambda: b"Comments to send" in output)
            assert not payload.exists()
            send("\r")  # Explicit delivery confirmation.
            wait_for(payload.exists)
            wait_for(lambda: b"fake-codex-finished" in output)
            pump(0.3)
            assert payload.read_text().startswith("Respond in Japanese.\n")
            assert "日本語 comment\n[red] literal" in payload.read_text()
            saved = json.loads(state.read_text())
            comment = saved["Files"][0]["Comments"][0]
            assert bool(comment.get("Sent")) == (failure == 0)
            retained = list((root / ".git").glob("annodiff-review-*.md"))
            assert len(retained) == (1 if failure else 0)
            send("q")
            process.wait(timeout=5)
            assert process.returncode == 0
            pump()
            assert output.count(b"\x1b[>1u") >= 2
            assert output.count(b"\x1b[>1u") == output.count(b"\x1b[<1u"), "keyboard protocol was not restored"
            assert termios.tcgetattr(slave) == original, "terminal modes were not restored"
            # The persistent lock file must not prevent a later writer.
            subprocess.run([str(binary), "--rewrite-state", str(state), str(state)],
                           env=env, check=True, capture_output=True, timeout=5)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)
print("PASS: PTY editing, Japanese input, preview-before-send, terminal restore, failed-delivery recovery")
