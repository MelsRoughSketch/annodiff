"""Exercise the real Ratatui event loop through a PTY, with fake local tools."""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import signal
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


def attach_terminal():
    # Give the child session a controlling terminal, as a real terminal would.
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)

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
    payload = pathlib.Path(os.environ["FAKE_PAYLOAD"])
    payload.write_text(pathlib.Path(json.loads(match[1])).read_text() if match else prompt)
    payload.with_suffix(".mode").write_text("file" if match else "direct")
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

    cases = [(failure, save_key, large)
             for failure, save_key in ((0, "\x1b[13;5u"), (0, "\n"), (7, "\x1bOQ"))
             for large in (False, True)]
    cases += [(0, "\x1bOQ", size) for size in (8191, 8192, 8193)]
    cases += [(failure, "\x1bOQ", "nul") for failure in (0, 7)]
    for case, (failure, save_key, size) in enumerate(cases):
        root = directory / f"repo{case}"
        root.mkdir()
        payload = directory / f"payload{case}.md"
        env = dict(os.environ, TERM="xterm-256color", PATH=str(tools) + os.pathsep + os.environ["PATH"],
                   VISUAL=str(editor), CODEX_HOME=str(directory / "codex-home"),
                   XDG_CONFIG_HOME=str(config.parent), FAKE_PAYLOAD=str(payload), FAKE_FAIL=str(failure),
                   GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_SYSTEM="/dev/null")
        subprocess.run(["git", "init", "-q", str(root)], env=env, check=True)
        (root / "sample.go").write_text('package main\nfunc main() {}\n')
        if case == 0:
            subprocess.run(["git", "-C", str(root), "add", "sample.go"], env=env, check=True)
            subprocess.run(["git", "-C", str(root), "-c", "user.name=E2E", "-c",
                            "user.email=e2e@example.invalid", "-c", "commit.gpgsign=false",
                            "commit", "-qm", "initial"], env=env, check=True)
            (root / "sample.go").write_text('package main\nfunc main() { println("changed") }\n')
            (root / "new.txt").write_text("untracked content\n")
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
        original = termios.tcgetattr(slave)
        process = subprocess.Popen([str(binary), str(root)], stdin=slave, stdout=slave, stderr=slave,
                                   env=env, start_new_session=True, preexec_fn=attach_terminal)
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

        def wait_for(predicate, description="expected state"):
            deadline = time.monotonic() + 15
            while not predicate():
                if process.poll() is not None or time.monotonic() > deadline:
                    raise AssertionError(description)
                pump()

        def send(value):
            data = value.encode("utf-8")
            while data:
                data = data[os.write(master, data):]

        def wait_state(predicate):
            wait_for(lambda: state.exists() and predicate(json.loads(state.read_text())),
                     "saved review update")
            return json.loads(state.read_text())

        def check_navigation():
            start = len(output)
            send("t")
            wait_for(lambda: b"Stacked" in output[start:], "stacked layout")
            send("1230t")
            start = len(output)
            send("0z")
            wait_for(lambda: b"full file" in output[start:], "expand file title")
            start = len(output)
            send("z")
            wait_for(lambda: b"context restored" in output[start:], "collapse file")
            send("?")
            send("/wrapping\r")
            send("q")  # Closes help without quitting.
            assert process.poll() is None
            send("1s")
            wait_state(lambda saved: saved.get("Split") is True)
            send("/new.txt\r")
            send("c")
            send("untracked-e2e")
            send("\x1bOQ")
            saved = wait_state(lambda saved: any(
                c["Text"] == "untracked-e2e" for f in saved["Files"] for c in f["Comments"]))
            new_file = next(f for f in saved["Files"] if f["Path"] == "new.txt")
            assert new_file["Comments"][0]["File"] is True
            assert new_file["Comments"][0]["Text"] == "untracked-e2e"
            assert saved["Split"] is True  # New files keep the global SBS preference.
            send("1\x1b[27u")  # Clear the path filter.
            send("/sample.go\r")
            send("0\x1b[Hvj")
            send("c")
            send("range-e2e")
            send("\x1bOQ")
            saved = wait_state(lambda saved: any(
                c["Text"] == "range-e2e" for f in saved["Files"] for c in f["Comments"]))
            sample = next(f for f in saved["Files"] if f["Path"] == "sample.go")
            ranged = next(c for c in sample["Comments"] if c["Text"] == "range-e2e")
            assert not ranged.get("File", False)
            assert ranged["End"] > ranged["Start"]
            # Resize and issue real SGR mouse-wheel events before continuing.
            for width, height in ((60, 15), (120, 30)):
                start = len(output)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
                os.kill(process.pid, signal.SIGWINCH)
                wait_for(lambda: f"\x1b[{height};1H".encode() in output[start:], "resize redraw")
            send("\x1b[<65;90;15M\x1b[<64;90;15M")
            send("2/range-e2e\rx")
            wait_state(lambda saved: any(c.get("Done") and c["Text"] == "range-e2e"
                       for f in saved["Files"] for c in f["Comments"]))
            send("1ou")  # Commented-files filter and shared Open/All filter.
            assert process.poll() is None
            send("ou")
            send("3\x1b[Hj\r")  # Select the initial commit.
            wait_state(lambda saved: saved.get("Base"))
            send("\x1b[H\r")  # Back to the working tree.
            wait_state(lambda saved: not saved.get("Base"))
            send("R\x1b[C\r")  # Archive, not reset.
            saved = wait_state(lambda saved: saved.get("History")
                               and not any(f["Comments"] for f in saved["Files"]))
            assert saved["History"]
            assert not any(f["Comments"] for f in saved["Files"])

        state = root / ".git/annodiff.json"
        try:
            wait_for(lambda: b"Ready" in output)
            send("e")
            wait_for(lambda: b"fake-editor-finished" in output)
            wait_for(lambda: state.exists() and output.count(b"\x1b[>1u") == 2,
                     "editor returned to TUI")
            body = "[red] literal"
            if type(size) is int:
                # Measure the entire generated review, including paths and instructions.
                sample = json.loads(state.read_text())
                sample["Files"][0]["Comments"] = [{"File": True, "Text": "日本語 comment\n" + body}]
                sample_path = directory / "boundary-review.json"
                sample_path.write_text(json.dumps(sample))
                exported = subprocess.check_output([str(binary), "--prompt", str(sample_path)], env=env)
                remaining = size - len(b"Respond in Japanese.\n\n" + exported)
                assert remaining > 0
                body += "界" * (remaining // 3) + "x" * (remaining % 3)
            elif size == "nul":
                body += "\0after NUL"
            elif size:
                body += " large review" * 800
            uses_file = size is True or size == "nul" or type(size) is int and size > 8192
            send("1c")  # Inline file comment; input letters remain literal.
            send("日本語 comment\r")
            send("\x1b[200~" + body + "\x1b[201~")
            send(save_key)
            wait_for(lambda: json.loads(state.read_text())["Files"][0]["Comments"])
            saved = json.loads(state.read_text())
            assert saved["Files"][0]["Comments"][0]["Text"] == "日本語 comment\n" + body, repr(saved["Files"][0]["Comments"][0]["Text"])
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
            result_start = output.index(b"fake-codex-finished")
            wait_for(lambda: any(word in output[result_start:] for word in
                                 ((b"saved", b"retained") if failure else (b"closed.",))),
                     "delivery result")
            assert payload.read_text().startswith("Respond in Japanese.\n")
            assert "日本語 comment\n" + body in payload.read_text()
            if type(size) is int:
                assert len(payload.read_bytes()) == size, (size, len(payload.read_bytes()))
            assert payload.with_suffix(".mode").read_text() == ("file" if uses_file else "direct")
            saved = json.loads(state.read_text())
            comment = saved["Files"][0]["Comments"][0]
            assert bool(comment.get("Sent")) == (failure == 0)
            retained = list((root / ".git").glob("annodiff-review-*.md"))
            assert len(retained) == (1 if failure and uses_file else 0)
            if case == 0:
                check_navigation()
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
            if case == 0:
                before = state.read_bytes()
                output.clear()
                process = subprocess.Popen([str(binary), str(root)], stdin=slave, stdout=slave,
                                           stderr=slave, env=env, start_new_session=True, preexec_fn=attach_terminal)
                wait_for(lambda: b"Ready" in output)
                assert state.read_bytes() == before, "restart changed saved review history"
                send("q")
                process.wait(timeout=5)
                assert process.returncode == 0
                assert termios.tcgetattr(slave) == original
        except Exception:
            print(f"FAIL: case {case} ({failure=}, {size=})", file=sys.stderr)
            print(output.decode("utf-8", errors="replace")[-6000:], file=sys.stderr)
            raise
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)
print("PASS: PTY navigation, help, filters, modes, comments, resize/mouse, commits/archive, delivery and terminal restore")
