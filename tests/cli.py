"""Check the public CLI and saved-data compatibility."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[1]
env = dict(os.environ, GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_SYSTEM="/dev/null")


def run(*args, cwd=repo):
    return subprocess.check_output([str(a) for a in args], cwd=cwd, env=env)


def normalize(value):
    if isinstance(value, dict):
        return {k: normalize(v or []) if k in ("Files", "History", "Lines", "Comments")
                else normalize(v) for k, v in value.items()}
    if isinstance(value, list):
        return [normalize(v) for v in value]
    return value


run("cargo", "build", "--locked", "--offline")
binary = repo / "target/debug/annodiff"
fixture = repo / "tests/fixtures/review.json"
assert run(binary, "--version").startswith(b"annodiff ")
assert b"Usage: annodiff " in run(binary, "--help")
assert run(binary, "--licenses", cwd="/tmp") == (
    b"annodiff is licensed under MIT OR Apache-2.0, at your option.\n\n"
    + (repo / "LICENSE-MIT").read_bytes() + b"\n"
    + (repo / "LICENSE-APACHE").read_bytes() + b"\n"
    + (repo / "THIRD_PARTY_NOTICES.md").read_bytes()
)
run(binary, "--check-state", fixture)
assert run(binary, "--prompt", fixture) == fixture.with_name("review-prompt.md").read_bytes()
with tempfile.TemporaryDirectory(prefix="annodiff-cli-") as directory:
    directory = Path(directory)
    saved = directory / "saved.json"
    run(binary, "--rewrite-state", fixture, saved)
    assert normalize(json.loads(saved.read_text())) == normalize(json.loads(fixture.read_text()))
    assert run(binary, "--prompt", saved) == fixture.with_name("review-prompt.md").read_bytes()
    for path in ("../outside", "/outside"):
        invalid = json.loads(fixture.read_text())
        invalid["Files"][0]["Path"] = path
        saved.write_text(json.dumps(invalid))
        assert subprocess.run([str(binary), "--check-state", str(saved)], capture_output=True).returncode != 0
    root = directory / "repo"
    root.mkdir()
    run("git", "init", "-q", cwd=root)
    source = root / "日本語 [x].txt"
    source.write_text("old\n", encoding="utf-8")
    run("git", "add", ".", cwd=root)
    run("git", "-c", "user.name=Test", "-c", "user.email=test@example.com",
        "-c", "commit.gpgsign=false", "commit", "-qm", "base", cwd=root)
    source.write_text("new\n", encoding="utf-8")
    (root / "new\nfile.txt").write_text("untracked\n", encoding="utf-8")
    state = json.loads(run(binary, "--snapshot-json", root))
    files = {f["Path"]: f for f in state["Files"]}
    assert set(files) == {source.name, "new\nfile.txt"}
    assert "+new" in files[source.name]["Patch"]
    # Preserve path whitespace; neither sibling may resolve to the existing repo.
    for suffix in (" ", "\n"):
        spaced = directory / ("repo" + suffix)
        spaced.mkdir()
        run("git", "init", "-q", cwd=spaced)
        (spaced / "requested.txt").write_text("requested repository\n")
        snapshot = json.loads(run(binary, "--snapshot-json", spaced))
        assert snapshot["Root"] == str(spaced.resolve())
        assert [f["Path"] for f in snapshot["Files"]] == ["requested.txt"]
print("PASS: annodiff CLI, saved JSON/prompt fixtures, Git snapshot and invalid paths")
