use annodiff::{
    app::{App, Effect, Modal},
    diff::{CodeLine, FileView, Row, commit_graph},
    review::{self, Comment, Commit, File, Review},
};
use crossterm::event::{Event, KeyCode as K, KeyEvent, KeyModifiers as M};
use ratatui::{Terminal, backend::TestBackend};
use std::{fs, path::Path, process::Command};

fn file(patch: &str) -> File {
    File {
        path: "sample.go".into(),
        patch: patch.into(),
        lines: review::parse(patch),
        comments: Vec::new(),
    }
}
fn fixture() -> File {
    file(
        "diff --git a/sample.go b/sample.go\nindex abc..def 100644\n--- a/sample.go\n+++ b/sample.go\n@@ -1,3 +1,3 @@\n context\n-old()\n+new()\n after\n@@ -30 +30 @@\n-before\n+after\n",
    )
}
fn press(app: &mut App, key: K) -> Effect {
    app.handle(Event::Key(KeyEvent::new(key, M::NONE))).unwrap()
}
fn draw(app: &mut App, t: &mut Terminal<TestBackend>) {
    t.draw(|f| app.draw(f)).unwrap();
}
fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn saved_json_null_fields_validation_and_history() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state.json");
    fs::write(&state,r#"{"Root":"/repo","Files":[{"Path":"empty.go","Patch":"","Lines":null,"Comments":null}],"History":null}"#).unwrap();
    let empty = Review::load(&state).unwrap();
    assert!(empty.files[0].comments.is_empty());
    let mut f = fixture();
    f.comments.push(Comment {
        start: 7,
        end: 7,
        text: "日本語 [red] comment\n```".into(),
        side: "new".into(),
        sent: true,
        ..Comment::default()
    });
    let r = Review {
        root: root.path().to_string_lossy().into_owned(),
        split: true,
        files: vec![f],
        ..Review::default()
    };
    r.save(&state).unwrap();
    assert_eq!(Review::load(&state).unwrap(), r);
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    assert_eq!(value["Files"][0]["Comments"][0]["Sent"], true);
    assert!(value.get("files").is_none());
    let next = r.refresh(
        Review {
            root: r.root.clone(),
            ..Review::default()
        },
        false,
    );
    assert_eq!(next.history.len(), 1);
    assert!(next.history[0].comments[0].sent);
    assert!(next.history[0].patch.is_empty());
    assert_eq!(next.pending(), 0);
    assert_eq!(
        next.refresh(
            Review {
                root: r.root.clone(),
                ..Review::default()
            },
            true
        )
        .count(),
        1
    );
    let mut invalid = r.clone();
    invalid.files[0].comments[0].end = usize::MAX;
    assert!(invalid.save(&state).is_err());
    assert_eq!(Review::load(&state).unwrap(), r);
    invalid = r.clone();
    invalid.files[0].path = "../outside".into();
    assert!(invalid.validate().is_err());
}

#[test]
fn diff_parse_alignment_wrapping_and_comments() {
    let mut f = fixture();
    assert_eq!((f.lines[7].old, f.lines[7].new), (0, 2));
    f.comments.push(Comment {
        start: 7,
        end: 7,
        side: "new".into(),
        text: "first\nsecond".into(),
        ..Comment::default()
    });
    let mut view = FileView::new(&f, true);
    view.layout(80, 0, false);
    assert_eq!(view.source(1, 0), Some(6));
    assert_eq!(view.source(1, 1), Some(7));
    assert!(matches!(
        view.rows[2],
        Row::Comment {
            part: 0,
            side: 1,
            ..
        }
    ));
    assert!(!view.selectable(2, 0));
    assert!(view.selectable(2, 1));
    assert_eq!(
        view.rows
            .iter()
            .filter(|r| matches!(r, Row::Gap(_)))
            .count(),
        3
    );
    let text = "日本語 e\u{301} 👨‍👩‍👧‍👦\t[red] long text ";
    let line = CodeLine::new(text);
    for width in [1, 2, 7, 20, 80] {
        let parts = line.wrap(width);
        assert_eq!(
            parts
                .iter()
                .map(|p| &line.text[p.clone()])
                .collect::<String>(),
            text.replace('\t', "    ")
        );
        assert!(parts.iter().all(|r| !r.is_empty()));
    }
    for split in [false, true] {
        for wrap in [false, true] {
            view.rebuild_rows(&f, split);
            for width in [0, 1, 40, 100] {
                view.layout(width, 40, wrap);
                for row in 0..view.len() {
                    let (logical, part) = view.locate(row).unwrap();
                    assert_eq!(view.starts[logical] + part, row);
                }
            }
        }
    }
}

#[test]
fn prompt_filters_status_side_and_fences() {
    let mut f = file("@@ -1 +1 @@\n-old ```\n+new ```\n");
    f.comments = vec![
        Comment {
            start: 2,
            end: 2,
            side: "new".into(),
            text: "fix this".into(),
            ..Comment::default()
        },
        Comment {
            file: true,
            text: "done".into(),
            done: true,
            ..Comment::default()
        },
        Comment {
            file: true,
            text: "sent".into(),
            sent: true,
            ..Comment::default()
        },
    ];
    let r = Review {
        root: "/repo".into(),
        files: vec![f],
        ..Review::default()
    };
    let prompt = r.prompt();
    assert!(prompt.contains("* 0/1 +new ```"));
    assert!(prompt.contains("````text"));
    assert!(!prompt.contains("-old"));
    assert!(!prompt.contains("**Comment**\n\nsent"));
    assert_eq!(r.pending(), 1);
}

#[test]
fn file_expand_preserves_diff_comments_and_supports_full_file_navigation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = "日本語 [x]\nfile.go";
    git(root, &["init", "-q"]);
    let original: String = (1..=80)
        .map(|i| format!("line {i}: unchanged context that wraps in a narrow terminal\n"))
        .collect();
    fs::write(root.join(path), &original).unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@x",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "base",
        ],
    );
    let base = git(root, &["rev-parse", "HEAD"]).trim().to_owned();
    let modified = original
        .replace("line 20:", "changed 20:")
        .replace("line 40:", "inserted\nline 40:")
        .replace("line 60:", "changed 60:");
    fs::write(root.join(path), &modified).unwrap();
    let working = review::snapshot(root.to_str().unwrap(), "", "").unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@x",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "target",
        ],
    );
    let revision = review::snapshot(root.to_str().unwrap(), &base, "HEAD").unwrap();
    // Revision expansion must read the compared revisions, not the current worktree.
    fs::write(root.join(path), "unrelated worktree content\n").unwrap();
    for (mut review, split) in [
        (revision.clone(), false),
        (revision, true),
        (working, false),
    ] {
        if review.base.is_empty() {
            git(root, &["reset", "--soft", &base]);
            fs::write(root.join(path), &modified).unwrap();
        }
        review.split = split;
        let source = review.files[0]
            .lines
            .iter()
            .position(|l| l.new > 0 && l.text.starts_with("+changed 20:"))
            .unwrap();
        review.files[0].comments.push(Comment {
            start: source,
            end: source,
            text: "original note".into(),
            side: if split { "new" } else { "" }.into(),
            ..Default::default()
        });
        let saved = review.clone();
        let state = root.join(".git/expand-state.json");
        review.save(&state).unwrap();
        let saved_bytes = fs::read(&state).unwrap();
        let mut app = App::new(review, state.clone());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        app.focus(0);
        app.side = usize::from(split);
        app.wrap = true;
        draw(&mut app, &mut terminal);
        app.cursor[0] = app
            .view()
            .unwrap()
            .visual_for_source(source, app.side)
            .unwrap();
        let hunks = app.view().unwrap().hunk_rows.clone().map(|rows| rows.len());
        for count in [10, 20] {
            press(&mut app, K::Char('z'));
            draw(&mut app, &mut terminal);
            let view = app.view().unwrap();
            let added: Vec<_> = view
                .rows
                .iter()
                .filter_map(|row| {
                    let Row::Code(pair) = row else { return None };
                    let i = *pair.iter().flatten().next()?;
                    view.original_source(i).is_none().then_some(i)
                })
                .collect();
            assert_eq!(added.len(), count);
            assert!(
                !added
                    .iter()
                    .any(|i| view.code[*i].text.starts_with("line 80:"))
            );
            assert_eq!(view.source(app.cursor[0], app.side), Some(source));
            assert!(view.rows.iter().any(|row| matches!(row, Row::Gap(true))));
            for (row, item) in view.rows.iter().enumerate() {
                if *item == Row::Gap(true) {
                    assert_eq!(view.rows.get(row.wrapping_sub(1)), Some(&Row::Gap(false)));
                    assert_eq!(view.rows.get(row + 1), Some(&Row::Gap(false)));
                }
            }
            assert_eq!(app.selected_ref().unwrap().comment, 0);
        }
        let visibility = app.view().unwrap().context_visible.clone();
        app.set_split(!split).unwrap();
        app.set_split(split).unwrap();
        assert_eq!(app.view().unwrap().context_visible, visibility);
        app.side = usize::from(split);
        app.cursor[0] = app
            .view()
            .unwrap()
            .visual_for_source(source, app.side)
            .unwrap();
        press(&mut app, K::Char('Z'));
        draw(&mut app, &mut terminal);
        assert!(app.view().unwrap().expanded.is_none());
        assert_eq!(
            app.view().unwrap().source(app.cursor[0], app.side),
            Some(source)
        );
        press(&mut app, K::Char('Z'));
        draw(&mut app, &mut terminal);
        press(&mut app, K::Char('z'));
        assert!(app.view().unwrap().context_visible.is_none());
        assert_eq!(
            app.view().unwrap().source(app.cursor[0], app.side),
            Some(source)
        );
        assert_eq!(app.selected_ref().unwrap().comment, 0);
        assert_eq!(
            app.view().unwrap().hunk_rows.clone().map(|rows| rows.len()),
            hunks
        );
        let full = app.view().unwrap().expanded.as_ref().unwrap();
        for number in [1, 40, 80] {
            assert!(
                full.lines
                    .iter()
                    .any(|l| l.text.contains(&format!("line {number}:")))
            );
        }
        assert_eq!(app.review, saved);
        assert_eq!(fs::read(&state).unwrap(), saved_bytes);
        app.filter(0, "line 1:".into());
        assert!(
            app.view()
                .unwrap()
                .source(app.cursor[0], app.side)
                .is_none()
        );
        assert!(app.start_edit(None, false).is_err());
        app.set_split(!split).unwrap();
        draw(&mut app, &mut terminal);
        let v = app.view().unwrap();
        assert!(
            v.code[v.display_source(app.cursor[0], app.side).unwrap()]
                .text
                .starts_with("line 1:")
        );
        press(&mut app, K::End);
        draw(&mut app, &mut terminal);
        let v = app.view().unwrap();
        assert!(
            v.code[v.display_source(app.cursor[0], app.side).unwrap()]
                .text
                .starts_with("line 80:")
        );
        app.set_split(split).unwrap();
        app.side = usize::from(split);
        app.cursor[0] = app
            .view()
            .unwrap()
            .visual_for_source(source, app.side)
            .unwrap();
        app.start_edit(app.selected_ref(), false).unwrap();
        app.editor.as_mut().unwrap().input.insert_str(" edited");
        app.save_editor().unwrap();
        draw(&mut app, &mut terminal);
        assert!(app.view().unwrap().expanded.is_some());
        let updated = Review::load(&state).unwrap();
        assert_eq!(updated.files[0].patch, saved.files[0].patch);
        assert_eq!(updated.files[0].lines, saved.files[0].lines);
        assert_eq!(updated.files[0].comments[0].start, source);
        assert!(updated.prompt().contains("changed 20:"));
        press(&mut app, K::Char('Z'));
        draw(&mut app, &mut terminal);
        assert!(app.view().unwrap().expanded.is_none());
        assert_eq!(
            app.view().unwrap().source(app.cursor[0], app.side),
            Some(source)
        );
    }
    let snapshot = review::snapshot(root.to_str().unwrap(), "", "").unwrap();
    fs::write(
        root.join(path),
        modified.replace("line 1:", "edited after snapshot:"),
    )
    .unwrap();
    let mut app = App::new(snapshot.clone(), root.join(".git/stale.json"));
    assert!(
        app.expand_context(true)
            .unwrap_err()
            .to_string()
            .contains("refresh")
    );
    assert!(app.view().unwrap().expanded.is_none());
    assert_eq!(app.review, snapshot);
    // An insertion-only hunk with zero context is not necessarily a new file.
    git(root, &["config", "diff.context", "0"]);
    fs::write(
        root.join(path),
        original.replace("line 40:", "inserted\nline 40:"),
    )
    .unwrap();
    let mut snapshot = review::snapshot(root.to_str().unwrap(), "", "").unwrap();
    let f = &snapshot.files[0];
    assert!(!f.lines.iter().any(|l| l.old > 0));
    let full = review::expand_file(&snapshot, f).unwrap();
    assert!(full.lines.iter().any(|l| l.old == 1 && l.new == 1));
    assert!(FileView::expand(f, full, false).is_ok());
    fs::write(root.join("added.txt"), "new file\n").unwrap();
    fs::remove_file(root.join(path)).unwrap();
    snapshot = review::snapshot(root.to_str().unwrap(), "", "").unwrap();
    for f in &snapshot.files {
        let full = review::expand_file(&snapshot, f).unwrap();
        assert_eq!(full.lines, f.lines);
        assert!(FileView::expand(f, full, true).is_ok());
    }
    let binary = file("Binary files a/x and b/x differ\n");
    assert!(review::expand_file(&snapshot, &binary).is_err());
}

#[test]
fn commit_selection_survives_non_utf8_diff_content() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    for name in ["legacy.txt", "normal.txt"] {
        fs::write(root.join(name), "old\n").unwrap();
    }
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@x",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "base",
        ],
    );
    let base = git(root, &["rev-parse", "HEAD"]).trim().to_owned();
    fs::write(root.join("legacy.txt"), b"new \x82\xa0\n").unwrap();
    fs::write(root.join("normal.txt"), "new\n").unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@x",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "target",
        ],
    );
    let state = root.join(".git/review.json");
    let mut app = App::new(
        review::snapshot(root.to_str().unwrap(), "", "").unwrap(),
        state.clone(),
    );
    app.commits = review::commits(root.to_str().unwrap()).unwrap();
    app.rebuild_lists();
    app.focus(3);
    let index = app.commits.iter().position(|c| c.id == base).unwrap();
    app.cursor[3] = app
        .commit_rows
        .iter()
        .position(|r| *r == Some(index))
        .unwrap();
    app.select_commit().unwrap();
    assert_eq!(app.review.base, base);
    let legacy = app
        .review
        .files
        .iter()
        .position(|f| f.path == "legacy.txt")
        .unwrap();
    let file = &app.review.files[legacy];
    assert!(file.lines.iter().all(|l| l.old == 0 && l.new == 0));
    assert!(file.lines[0].text.contains("not UTF-8"));
    use base64::{Engine, engine::general_purpose::STANDARD};
    let raw = review::git(root.to_str().unwrap(), &["diff", &base, "--", "legacy.txt"]).unwrap();
    assert_eq!(
        STANDARD
            .decode(file.patch.split_once('\n').unwrap().1)
            .unwrap(),
        raw
    );
    assert!(
        app.review
            .files
            .iter()
            .any(|f| f.path == "normal.txt" && f.patch.contains("+new"))
    );
    app.select_file(Some(legacy));
    app.focus(0);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut terminal);
    assert!(app.start_edit(None, false).is_err());
    app.start_edit(None, true).unwrap();
    app.editor
        .as_mut()
        .unwrap()
        .input
        .insert_str("Check this file");
    app.save_editor().unwrap();
    assert!(Review::load(&state).unwrap().files[legacy].comments[0].file);
    let before = app.review.clone();
    fs::write(root.join("legacy.txt"), b"new \x82\xa2\n").unwrap();
    let after = review::snapshot(root.to_str().unwrap(), &base, "").unwrap();
    assert!(!before.same_diff(&after));
    let refreshed = before.refresh(after, false);
    assert_eq!(refreshed.history[0].comments[0].text, "Check this file");
    let revision = review::snapshot(root.to_str().unwrap(), &base, "HEAD").unwrap();
    assert_eq!(revision.files[legacy].patch, before.files[legacy].patch);
    fs::write(root.join("untracked.txt"), b"\xff\n").unwrap();
    let working = review::snapshot(root.to_str().unwrap(), "", "").unwrap();
    assert!(
        working
            .files
            .iter()
            .find(|f| f.path == "untracked.txt")
            .unwrap()
            .lines[0]
            .text
            .contains("not UTF-8")
    );
}

#[test]
fn git_snapshot_worktree_and_literal_names() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    git(root, &["init", "-q"]);
    fs::write(root.join("日本語 [x].go"), "first\nold\nlast\n").unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@x",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "base",
        ],
    );
    fs::write(root.join("日本語 [x].go"), "first\nstaged\nlast\n").unwrap();
    git(root, &["add", "."]);
    fs::write(root.join("日本語 [x].go"), "first\nnew\nlast\n").unwrap();
    fs::write(root.join("newline\nfile.txt"), "untracked\n").unwrap();
    let root_str = root.to_str().unwrap();
    let r = review::snapshot(root_str, "", "").unwrap();
    assert_eq!(r.files.len(), 2);
    let f = r.files.iter().find(|f| f.path == "日本語 [x].go").unwrap();
    assert!(f.patch.contains("+new"));
    assert!(!f.patch.contains("staged"));
    let state = review::state_path(root_str).unwrap();
    assert_eq!(state, root.join(".git/annodiff.json"));
    let worktree = tempfile::tempdir().unwrap();
    let path = worktree.path().join("worktree");
    git(
        root,
        &[
            "worktree",
            "add",
            "--detach",
            path.to_str().unwrap(),
            "HEAD",
        ],
    );
    let worktree_state = review::state_path(path.to_str().unwrap()).unwrap();
    assert_ne!(worktree_state, state);
    assert!(worktree_state.to_string_lossy().contains("/worktrees/"));
    assert!(review::snapshot(root_str, "--help", "").is_err());
}

#[test]
fn workspace_input_edit_save_history_and_resize() {
    let temp = tempfile::tempdir().unwrap();
    let mut f = fixture();
    f.lines[7].text = format!("+let value = \"{}\";", "日本語 [red] ".repeat(30));
    let r = Review {
        root: temp.path().to_string_lossy().into_owned(),
        split: true,
        files: vec![f],
        ..Review::default()
    };
    let mut app = App::new(r, temp.path().join("state.json"));
    let mut t = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut t);
    assert_eq!(app.total_changes, 4);
    for pane in 0..4 {
        press(&mut app, K::Char(char::from(b'0' + pane as u8)));
        assert_eq!(app.pane, pane);
    }
    app.focus(0);
    app.side = 1;
    app.cursor[0] = 1;
    press(&mut app, K::Char('f'));
    draw(&mut app, &mut t);
    assert!(app.view().unwrap().len() > 10);
    let source = app.view().unwrap().source(app.cursor[0], 1);
    press(&mut app, K::Char('['));
    draw(&mut app, &mut t);
    assert_eq!(app.view().unwrap().source(app.cursor[0], 1), source);
    press(&mut app, K::Char('c'));
    draw(&mut app, &mut t);
    assert!(app.editor_rect.height >= 3);
    let selected = app.editor.as_ref().unwrap().comment.clone();
    for (width, height) in [(40, 15), (160, 50), (80, 25)] {
        t.backend_mut().resize(width, height);
        t.resize(ratatui::layout::Rect::new(0, 0, width, height))
            .unwrap();
        draw(&mut app, &mut t);
        let editor = app.editor.as_ref().unwrap();
        assert_eq!(editor.comment, selected);
        assert_eq!(
            app.view().unwrap().source(editor.after, 1),
            Some(selected.end)
        );
    }
    app.handle(Event::Paste("日本語 comment\n[red] literal".into()))
        .unwrap();
    app.handle(Event::Key(KeyEvent::new(K::Enter, M::CONTROL)))
        .unwrap();
    draw(&mut app, &mut t);
    let saved = Review::load(&app.state).unwrap();
    assert_eq!(
        saved.files[0].comments[0].text,
        "日本語 comment\n[red] literal"
    );
    assert_eq!(saved.files[0].comments[0].start, 7);
    assert!(app.editor.is_none());
    app.review.files[0].comments[0].sent = true;
    app.start_edit(
        Some(annodiff::app::CommentRef {
            history: false,
            file: 0,
            comment: 0,
        }),
        false,
    )
    .unwrap();
    app.editor.as_mut().unwrap().input.insert_str(" revised");
    app.save_editor().unwrap();
    assert_eq!(app.review.history.len(), 1);
    assert!(!app.review.files[0].comments[0].sent);
    for (width, height) in [(1, 1), (3, 2), (20, 8), (80, 25), (160, 50)] {
        t.backend_mut().resize(width, height);
        t.resize(ratatui::layout::Rect::new(0, 0, width, height))
            .unwrap();
        draw(&mut app, &mut t);
    }
    app.focus(1);
    press(&mut app, K::Char('+'));
    assert_eq!(app.zoom, 1);
    press(&mut app, K::Char('+'));
    draw(&mut app, &mut t);
    assert_eq!(app.zoom, 2);
    press(&mut app, K::Right);
    assert_eq!(app.pane, 2);
    press(&mut app, K::Char('?'));
    draw(&mut app, &mut t);
    assert!(matches!(app.modal, Some(Modal::Help { .. })));
    press(&mut app, K::Esc);
    assert!(app.modal.is_none());
    for pane in 0..4 {
        app.focus(pane);
        let left_pane = app.left_pane;
        for _ in 0..2 {
            let split = app.review.split;
            press(&mut app, K::Char('s'));
            draw(&mut app, &mut t);
            assert_eq!(app.review.split, !split);
            assert_eq!(app.pane, pane);
            assert_eq!(app.left_pane, left_pane);
            assert_eq!(app.zoom, 2);
        }
    }
    press(&mut app, K::Char('f'));
    assert_eq!(app.pane, 0);
}

#[test]
fn pane_zoom_steps_and_layout() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Review {
            files: vec![fixture()],
            ..Review::default()
        },
        temp.path().join("state.json"),
    );
    let mut t = Terminal::new(TestBackend::new(100, 32)).unwrap();
    for pane in 0..4 {
        app.focus(pane);
        for zoom in 0..=2 {
            for key in ['+', '-'] {
                app.zoom = zoom;
                press(&mut app, K::Char(key));
                let expected = match (pane, key) {
                    (0, '+') => 2,
                    (0, '-') => 0,
                    (_, '+') => (zoom + 1).min(2),
                    _ => zoom.saturating_sub(1),
                };
                assert_eq!(app.zoom, expected);
                draw(&mut app, &mut t);
                assert_eq!(
                    app.pane_rects[pane].width,
                    if expected == 2 {
                        100
                    } else if pane == 0 {
                        70
                    } else {
                        30
                    }
                );
                if expected == 0 {
                    for sidebar in &app.pane_rects[1..] {
                        assert_eq!(sidebar.height, 10);
                    }
                }
            }
        }
    }
}

#[test]
fn save_failure_keeps_editor_and_comment() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Review {
            files: vec![fixture()],
            ..Review::default()
        },
        temp.path().join("absent/state.json"),
    );
    app.focus(0);
    app.layout_diff(80);
    app.start_edit(None, false).unwrap();
    app.editor.as_mut().unwrap().input.insert_str("keep me");
    assert!(app.save_editor().is_err());
    assert!(app.editor.is_some());
    assert!(app.review.files[0].comments.is_empty());
}

#[test]
fn queued_cleanup_never_removes_unrelated_files() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state.json");
    let queued = temp.path().join("annodiff-review-test.md");
    fs::write(&queued, "pending").unwrap();
    let mut r = Review {
        files: vec![File {
            path: "a.go".into(),
            comments: vec![
                Comment {
                    file: true,
                    done: true,
                    delivery: "annodiff-review-test.md".into(),
                    ..Comment::default()
                },
                Comment {
                    file: true,
                    delivery: "annodiff-review-test.md".into(),
                    ..Comment::default()
                },
            ],
            ..File::default()
        }],
        ..Review::default()
    };
    r.cleanup_queued(&state).unwrap();
    assert!(queued.exists());
    r.files[0].comments[1].done = true;
    r.cleanup_queued(&state).unwrap();
    assert!(!queued.exists());
    r.files[0].comments[0].delivery = "../outside.md".into();
    assert!(r.cleanup_queued(&state).is_err());
}

#[cfg(unix)]
#[test]
fn preview_rejects_external_symlinks_and_binary() {
    let root = tempfile::tempdir().unwrap();
    let external = tempfile::NamedTempFile::new().unwrap();
    std::os::unix::fs::symlink(external.path(), root.path().join("link")).unwrap();
    assert!(review::current_code(root.path().to_str().unwrap(), "link").is_err());
    fs::write(root.path().join("binary"), b"a\0b").unwrap();
    assert!(review::current_code(root.path().to_str().unwrap(), "binary").is_err());
}

#[test]
fn graph_preserves_lanes() {
    let commits = [
        ("m", vec!["a", "b"]),
        ("b", vec!["r"]),
        ("a", vec!["r"]),
        ("r", vec![]),
    ]
    .into_iter()
    .map(|(id, parents)| Commit {
        id: id.into(),
        parents: parents.into_iter().map(String::from).collect(),
        ..Commit::default()
    })
    .collect::<Vec<_>>();
    assert_eq!(commit_graph(&commits), vec!["◎─╮", "│ ○", "○─┤", "  ○"]);
}

#[test]
fn highlighting_stops_at_viewport_and_resumes_multiline_context() {
    let patch = format!(
        "@@ -0,0 +1,1000 @@\n+/* open comment\n{}+end */\n+var answer = 42\n",
        "+still in comment\n".repeat(997)
    );
    let f = file(&patch);
    let mut incremental = FileView::new(&f, true);
    incremental.layout(80, 0, false);
    incremental.highlight_visible(&f, 0, 8);
    assert!(
        incremental.tokens(500, 1).is_empty(),
        "offscreen lines must remain unparsed"
    );
    let mut complete = FileView::new(&f, true);
    complete.layout(80, 0, false);
    while complete.highlight_visible(&f, 0, complete.len()) {}
    while incremental.highlight_visible(&f, 990, 20) {}
    for i in 1..f.lines.len() {
        let tokens = |v: &FileView| {
            v.tokens(i, 1)
                .iter()
                .map(|t| (t.range.clone(), t.style))
                .collect::<Vec<_>>()
        };
        assert_eq!(tokens(&incremental), tokens(&complete), "line {i}");
    }
    incremental.highlight_visible(&f, 0, 8);
    while incremental.highlight_visible(&f, 990, 20) {}
    assert_eq!(
        incremental.tokens(1000, 1).len(),
        complete.tokens(1000, 1).len()
    );
}

#[test]
fn comment_range_markers_and_context_help() {
    let mut f = fixture();
    f.comments.push(Comment {
        start: 5,
        end: 8,
        side: "new".into(),
        text: "range".into(),
        ..Default::default()
    });
    for split in [false, true] {
        let v = FileView::new(&f, split);
        let side = usize::from(split);
        assert_eq!(v.comment_marker(5, side, 0, 1), "╭");
        assert_eq!(v.comment_marker(6, side, 0, 1), " "); // deleted line excluded
        assert_eq!(v.comment_marker(7, side, 0, 1), "│");
        assert_eq!(v.comment_marker(8, side, 0, 2), "│");
        assert_eq!(v.comment_marker(8, side, 1, 2), "╰");
        if split {
            assert_eq!(v.comment_marker(5, 0, 0, 1), " ");
        }
    }
    f.comments[0].start = 7;
    f.comments[0].end = 7;
    let v = FileView::new(&f, true);
    assert_eq!(v.comment_marker(7, 1, 0, 1), "◆");
    assert_eq!(v.comment_marker(7, 1, 0, 2), "╭");
    assert_eq!(v.comment_marker(7, 1, 1, 2), "╰");
    assert_eq!(f.comments[0].status(), "[Open · Unsent]");
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Review {
            files: vec![f],
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    press(&mut app, K::Char('?'));
    draw(&mut app, &mut terminal);
    let text = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(text.contains("Current pane"));
    assert!(text.contains("file comment"));
    assert!(text.contains("Global"));
    assert!(!text.contains("   R:"), "one binding per line");
    let buffer = terminal.backend().buffer();
    assert!(
        buffer
            .content
            .iter()
            .any(|c| c.fg == ratatui::style::Color::Rgb(0, 255, 255))
    );
    assert!(
        buffer
            .content
            .iter()
            .any(|c| c.fg == ratatui::style::Color::Rgb(255, 255, 0))
    );
}

#[test]
fn git_status_labels_and_untracked_unified_override() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    fs::write(root.join("modified.go"), "old\n").unwrap();
    fs::write(root.join("deleted.go"), "old\n").unwrap();
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "base",
        ],
    );
    fs::write(root.join("modified.go"), "new\n").unwrap();
    fs::remove_file(root.join("deleted.go")).unwrap();
    fs::write(root.join("added.go"), "added\n").unwrap();
    git(root, &["add", "added.go"]);
    let untracked = "日本語 [x]\nnew.go";
    fs::write(root.join(untracked), "new\n").unwrap();
    let mut r = review::snapshot(root.to_str().unwrap(), "", "").unwrap();
    for (path, status) in [
        ("modified.go", "M"),
        ("deleted.go", "D"),
        ("added.go", "A"),
        (untracked, "??"),
    ] {
        assert_eq!(r.statuses.get(path).map(String::as_str), Some(status));
    }
    r.split = true;
    let mut app = App::new(r, root.join(".git/state.json"));
    let tracked_i = app
        .review
        .files
        .iter()
        .position(|f| f.path == "modified.go")
        .unwrap();
    let new_i = app
        .review
        .files
        .iter()
        .position(|f| f.path == untracked)
        .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    app.select_file(Some(tracked_i));
    app.side = 1;
    draw(&mut app, &mut terminal);
    assert!(app.view().unwrap().split);
    app.select_file(Some(new_i));
    draw(&mut app, &mut terminal);
    assert!(app.review.split);
    assert!(!app.split());
    assert!(!app.view().unwrap().split);
    assert_eq!(app.side, 0);
    let label = app.labels[1][app.cursor[1]].to_string();
    assert!(label.starts_with("?? "));
    app.select_file(Some(tracked_i));
    draw(&mut app, &mut terminal);
    assert!(app.view().unwrap().split);
    app.select_file(Some(new_i));
    git(root, &["add", "--", untracked]);
    app.refresh(false).unwrap();
    let new_i = app
        .review
        .files
        .iter()
        .position(|f| f.path == untracked)
        .unwrap();
    app.select_file(Some(new_i));
    draw(&mut app, &mut terminal);
    assert_eq!(app.review.statuses[untracked], "A");
    assert!(app.review.split && app.view().unwrap().split);
    assert!(
        !serde_json::to_string(&app.review)
            .unwrap()
            .contains("Statuses")
    );
}

#[test]
fn quit_from_session_picker_and_preview_but_type_q_in_search() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(Review::default(), dir.path().join("state.json"));
    app.modal = Some(Modal::Sessions {
        items: vec![],
        input: Default::default(),
        selection: 0,
        search: true,
        options: Default::default(),
        control: 0,
    });
    assert!(matches!(press(&mut app, K::Char('q')), Effect::None));
    assert!(matches!(&app.modal, Some(Modal::Sessions { input, .. }) if input.lines() == ["q"]));
    press(&mut app, K::Tab);
    assert!(matches!(press(&mut app, K::Char('q')), Effect::Quit));
    for pane in 0..3 {
        app.modal = Some(Modal::Preview {
            destination: "New session".into(),
            id: String::new(),
            copy: false,
            archived: false,
            selection: 0,
            pane,
            offsets: [0; 2],
        });
        assert!(matches!(press(&mut app, K::Char('q')), Effect::Quit));
    }
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (_, receiver) = std::sync::mpsc::channel();
    app.modal = Some(Modal::Loading {
        cancel: cancel.clone(),
        receiver,
        options: Default::default(),
        input: Default::default(),
        control: 0,
    });
    assert!(matches!(press(&mut app, K::Char('q')), Effect::Quit));
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
    assert!(!app.state.exists());
}

#[test]
fn session_directory_scope_combines_with_search_and_selects_visible_session() {
    use annodiff::agent::Session;
    let dir = tempfile::tempdir().unwrap();
    let here = Session {
        id: "here".into(),
        current: true,
        cwd: "/repo".into(),
        preview: "Fix local".into(),
        ..Default::default()
    };
    let other = Session {
        id: "other".into(),
        cwd: "/repo-child".into(),
        preview: "Fix elsewhere".into(),
        ..Default::default()
    };
    assert!(here.matches(true, "fix"));
    assert!(!other.matches(true, "fix"));
    assert!(other.matches(false, "fix"));
    assert!(!here.matches(false, "elsewhere"));
    let mut app = App::new(
        Review {
            root: "/repo".into(),
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    app.modal = Some(Modal::Sessions {
        items: vec![
            here,
            Session {
                id: "linked".into(),
                cwd: "/repo-worktree".into(),
                preview: "Fix linked".into(),
                current: true,
                ..Default::default()
            },
            other,
        ],
        input: ratatui_textarea::TextArea::new(vec!["fix".into()]),
        selection: 2,
        search: false,
        options: Default::default(),
        control: 0,
    });
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    draw(&mut app, &mut terminal);
    let screen = |t: &Terminal<TestBackend>| {
        t.backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
    };
    assert!(screen(&terminal).contains("[CWD] / All"));
    assert!(screen(&terminal).contains("2 sessions"));
    assert!(screen(&terminal).contains("[here] Fix linked"));
    assert!(!screen(&terminal).contains("Fix elsewhere"));
    // Switching scope now reloads the server-side filtered list.
    let Some(Modal::Sessions { items, .. }) = &app.modal else {
        panic!()
    };
    let items = items.clone();
    press(&mut app, K::Char('a'));
    assert!(
        matches!(&app.modal, Some(Modal::Loading { options, input, control: 0, .. })
        if options.all && input.lines() == ["fix"])
    );
    app.close_modal();
    app.modal = Some(Modal::Sessions {
        items,
        input: ratatui_textarea::TextArea::new(vec!["fix".into()]),
        selection: 0,
        search: false,
        options: annodiff::agent::SessionOptions {
            all: true,
            ..Default::default()
        },
        control: 0,
    });
    draw(&mut app, &mut terminal);
    assert!(screen(&terminal).contains("CWD / [All]"));
    assert!(screen(&terminal).contains("Fix elsewhere"));
    press(&mut app, K::Char('/'));
    press(&mut app, K::Char('a'));
    assert!(
        matches!(&app.modal, Some(Modal::Sessions { options: annodiff::agent::SessionOptions { all: true, .. }, input, .. }) if input.lines()[0].contains('a'))
    );
    if let Some(Modal::Sessions { input, .. }) = &mut app.modal {
        *input = ratatui_textarea::TextArea::new(vec!["elsewhere".into()]);
    }
    press(&mut app, K::Tab);
    press(&mut app, K::Down);
    press(&mut app, K::Down);
    press(&mut app, K::Enter);
    assert!(matches!(&app.modal, Some(Modal::Preview { id, .. }) if id == "other"));
}

#[test]
fn revision_additions_are_unified_without_changing_split_preference() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    fs::write(root.join("existing.go"), "old\n").unwrap();
    git(root, &["add", "."]);
    let commit = |message| {
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                message,
            ],
        )
    };
    commit("base");
    let base = git(root, &["rev-parse", "HEAD"]).trim().to_owned();
    fs::write(root.join("existing.go"), "new\n").unwrap();
    fs::write(root.join("added.go"), "new file\n").unwrap();
    git(root, &["add", "."]);
    let mut working = review::snapshot(root.to_str().unwrap(), "", "").unwrap();
    working.split = true;
    assert_eq!(working.statuses["existing.go"], "M");
    assert_eq!(working.statuses["added.go"], "A");
    assert!(
        working
            .files
            .iter()
            .enumerate()
            .all(|(i, _)| working.split_file(i))
    );
    commit("add and modify");
    let mut range = review::snapshot(root.to_str().unwrap(), &base, "HEAD").unwrap();
    range.split = true;
    let added = range
        .files
        .iter()
        .position(|f| f.path == "added.go")
        .unwrap();
    let modified = range
        .files
        .iter()
        .position(|f| f.path == "existing.go")
        .unwrap();
    assert_eq!(range.statuses["added.go"], "A");
    let mut app = App::new(range, root.join(".git/state.json"));
    let mut t = Terminal::new(TestBackend::new(100, 40)).unwrap();
    for (file, split) in [(added, false), (modified, true), (added, false)] {
        app.select_file(Some(file));
        draw(&mut app, &mut t);
        assert_eq!(app.view().unwrap().split, split);
        assert!(app.review.split);
    }
    // Rebuilding cached views must retain the per-file override too.
    app.apply(app.review.clone()).unwrap();
    assert!(!app.view().unwrap().split);
}

#[test]
fn inline_comment_header_and_body_wrap_independently_of_code() {
    use unicode_width::UnicodeWidthStr;
    let mut f = fixture();
    f.comments.push(Comment {
        start: 7,
        end: 7,
        side: "new".into(),
        text: format!("one\n{}\n\nthree", "日本語 e\u{301} longword ".repeat(8)),
        ..Default::default()
    });
    for split in [false, true] {
        let mut view = FileView::new(&f, split);
        let column = usize::from(split);
        let mut heights = Vec::new();
        for width in [40, 160, 40] {
            view.layout(width, 0, false);
            assert_eq!(view.comment_text[0][0].text, "Open [new 2]:");
            assert_eq!(view.comment_text[0][1].text, "one");
            for (row, item) in view.rows.iter().enumerate() {
                if let Row::Comment { index, part, .. } = item {
                    let line = &view.comment_text[*index][*part].text;
                    let fragments = &view.fragments[row][column];
                    assert_eq!(
                        fragments
                            .iter()
                            .map(|r| &line[r.clone()])
                            .collect::<String>(),
                        *line
                    );
                    let available = view.widths[column] - if *part == 0 { 2 } else { 6 };
                    assert!(
                        fragments
                            .iter()
                            .all(|r| line[r.clone()].width() <= available)
                    );
                    for visual in view.starts[row]..view.starts[row + 1] {
                        assert!(view.selectable(visual, column));
                    }
                }
            }
            heights.push(view.len());
        }
        assert!(heights[0] > heights[1]);
        assert_eq!(heights[0], heights[2]);
    }
}

#[test]
fn unified_comments_follow_source_side_in_split_view() {
    let mut f = fixture();
    for (start, end, explicit, want) in [
        (7, 7, "", 1),
        (6, 6, "", 0),
        (5, 5, "", 1),
        (6, 7, "", 1),
        (5, 5, "old", 0),
        (5, 5, "new", 1),
    ] {
        f.comments = vec![Comment {
            start,
            end,
            side: explicit.into(),
            text: "comment".into(),
            ..Default::default()
        }];
        let mut view = FileView::new(&f, true);
        view.layout(100, 0, false);
        assert!(
            view.rows
                .iter()
                .any(|r| matches!(r, Row::Comment { side, .. } if *side == want))
        );
        assert_eq!(f.comments[0].side, explicit);
    }
    f.comments = vec![Comment {
        start: 7,
        end: 7,
        text: "new line comment".into(),
        ..Default::default()
    }];
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Review {
            split: true,
            files: vec![f],
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut t = Terminal::new(TestBackend::new(100, 40)).unwrap();
    draw(&mut app, &mut t);
    app.focus(2);
    app.preview_comment();
    assert_eq!(app.side, 1);
    assert_eq!(app.view().unwrap().source(app.cursor[0], 1), Some(7));
    app.side = 0;
    app.start_edit(
        Some(annodiff::app::CommentRef {
            history: false,
            file: 0,
            comment: 0,
        }),
        false,
    )
    .unwrap();
    let after = app.editor.as_ref().unwrap().after;
    assert_eq!(app.view().unwrap().source(after, 1), Some(7));
}

#[test]
fn files_comment_filter_includes_done_history_and_combines_with_path_search() {
    let dir = tempfile::tempdir().unwrap();
    let mut files: Vec<_> = ["plain.go", "open.go", "done.go", "history.go"]
        .into_iter()
        .map(|path| File {
            path: path.into(),
            ..fixture()
        })
        .collect();
    files[1].comments.push(Comment {
        file: true,
        text: "open".into(),
        ..Default::default()
    });
    files[2].comments.push(Comment {
        file: true,
        done: true,
        text: "done".into(),
        ..Default::default()
    });
    let history = File {
        path: "history.go".into(),
        comments: vec![Comment {
            file: true,
            done: true,
            text: "past".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut app = App::new(
        Review {
            files,
            history: vec![history],
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    assert_eq!(app.file_rows, [0, 1, 2, 3]);
    press(&mut app, K::Char('o'));
    assert_eq!(app.file_rows, [1]);
    press(&mut app, K::Char('u'));
    assert!(!app.open_only);
    assert_eq!(app.pane, 1);
    assert_eq!(app.file_rows, [1, 2, 3]);
    app.focus(2);
    press(&mut app, K::Char('u'));
    assert!(app.open_only);
    assert_eq!(app.pane, 2);
    assert_eq!(app.file_rows, [1]);
    press(&mut app, K::Char('u'));
    assert_eq!(app.file_rows, [1, 2, 3]);
    app.focus(1);
    assert_eq!(app.file, Some(1));
    press(&mut app, K::Char('n'));
    assert_eq!(app.pane, 1);
    assert_eq!(app.file, Some(2));
    app.filter(1, "done".into());
    assert_eq!(app.file_rows, [2]);
    app.remove_comment(annodiff::app::CommentRef {
        history: false,
        file: 2,
        comment: 0,
    })
    .unwrap();
    assert!(app.file_rows.is_empty());
    assert!(app.file.is_none());
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    draw(&mut app, &mut terminal);
    press(&mut app, K::Char('o'));
    assert_eq!(app.file_rows, [2]);
    assert_eq!(app.queries[1], "done");
    app.filter(1, String::new());
    assert_eq!(app.file_rows, [0, 1, 2, 3]);
    assert!(!app.commented_files_only);
    app.focus(2);
    app.file_only = false;
    app.rebuild_comments();
    assert!(app.refs.iter().any(|r| r.history));
    app.focus(1);
    press(&mut app, K::Char('u'));
    assert!(app.open_only);
    assert!(!app.refs.iter().any(|r| r.history));
    assert_eq!(app.file_rows, [0, 1, 2, 3]); // o is off; u must not hide files.
}

#[test]
fn file_comments_are_inline_and_diff_uses_removed_notes_space() {
    let dir = tempfile::tempdir().unwrap();
    for split in [false, true] {
        let mut f = fixture();
        f.comments.push(Comment {
            file: true,
            text: "Whole file note\nsecond line".into(),
            ..Default::default()
        });
        let mut app = App::new(
            Review {
                split,
                files: vec![f],
                ..Default::default()
            },
            dir.path().join("state.json"),
        );
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        draw(&mut app, &mut terminal);
        assert_eq!(app.pane_rects[0].height, 28);
        assert!(matches!(
            app.view().unwrap().rows[0],
            Row::Comment {
                index: 0,
                part: 0,
                ..
            }
        ));
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("whole file"));
        assert!(text.contains("Whole file note"));
        assert!(!text.contains("Comments · file / selected line"));
        app.focus(0);
        app.side = usize::from(split);
        app.cursor[0] = 0;
        assert_eq!(app.selected_ref().unwrap().comment, 0);
        press(&mut app, K::Enter);
        assert!(app.editor.as_ref().unwrap().comment.file);
    }
}

#[test]
fn sidebar_selection_uses_moving_marker_without_overriding_text_colors() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = fixture();
    f.comments.push(Comment {
        file: true,
        text: "note".into(),
        ..Default::default()
    });
    let mut app = App::new(
        Review {
            files: vec![
                f.clone(),
                File {
                    path: "second.go".into(),
                    ..f
                },
            ],
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    draw(&mut app, &mut terminal);
    let rect = app.pane_rects[1];
    let (x, y) = (rect.x + 1, rect.y + 1);
    let b = terminal.backend().buffer();
    assert_eq!(b[(x, y)].symbol(), "▶");
    assert_eq!(b[(x, y)].fg, ratatui::style::Color::Rgb(80, 220, 220));
    // Marker + space + two status columns + space: file name retains yellow.
    assert_eq!(b[(x + 5, y)].fg, ratatui::style::Color::Rgb(255, 255, 0));
    assert_eq!(b[(x + 5, y)].bg, ratatui::style::Color::Reset);
    press(&mut app, K::Down);
    draw(&mut app, &mut terminal);
    assert_eq!(terminal.backend().buffer()[(x, y)].symbol(), " ");
    assert_eq!(terminal.backend().buffer()[(x, y + 1)].symbol(), "▶");
    for pane in [2, 3] {
        app.focus(pane);
        draw(&mut app, &mut terminal);
        let rect = app.pane_rects[pane];
        let marker = &terminal.backend().buffer()[(rect.x + 1, rect.y + 1)];
        assert_eq!(marker.symbol(), "▶");
        assert_eq!(marker.fg, ratatui::style::Color::Rgb(80, 220, 220));
        assert_eq!(marker.bg, ratatui::style::Color::Reset);
        assert_eq!(
            terminal.backend().buffer()[(x, y + 1)].fg,
            ratatui::style::Color::DarkGray
        );
    }
}

#[test]
fn mouse_scroll_moves_viewport_without_changing_selection() {
    use crossterm::event::{MouseEvent, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    let patch = format!("@@ -1,100 +1,100 @@\n{}", " context\n".repeat(100));
    let files = (0..40)
        .map(|i| File {
            path: format!("{i}.go"),
            ..file(&patch)
        })
        .collect();
    let mut app = App::new(
        Review {
            files,
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    // Give each sidebar enough rows to exercise independent viewport scrolling.
    app.labels[2] = app.labels[1].clone();
    app.labels[3] = app.labels[1].clone();
    draw(&mut app, &mut terminal);
    let cursors = app.cursor;
    let selected_file = app.file;
    for pane in [1, 2, 3, 0] {
        let rect = app.pane_rects[pane];
        let wheel = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: rect.x + 1,
                row: rect.y + 1,
                modifiers: M::NONE,
            })
        };
        app.handle(wheel(MouseEventKind::ScrollDown)).unwrap();
        draw(&mut app, &mut terminal);
        assert_eq!(app.cursor, cursors);
        assert_eq!(app.file, selected_file);
        assert_eq!(app.pane, 1);
        assert_eq!(
            if pane == 0 {
                app.offset
            } else {
                app.list_offsets[pane]
            },
            3
        );
        app.handle(wheel(MouseEventKind::ScrollUp)).unwrap();
        draw(&mut app, &mut terminal);
        assert_eq!(
            if pane == 0 {
                app.offset
            } else {
                app.list_offsets[pane]
            },
            0
        );
        for _ in 0..50 {
            app.handle(wheel(MouseEventKind::ScrollDown)).unwrap();
        }
        draw(&mut app, &mut terminal);
        let len = if pane == 0 {
            app.view().unwrap().len()
        } else {
            app.labels[pane].len()
        };
        assert_eq!(
            if pane == 0 {
                app.offset
            } else {
                app.list_offsets[pane]
            },
            len.saturating_sub(rect.height.saturating_sub(2) as usize)
        );
        assert_eq!(app.cursor, cursors);
    }
    for (pane, down, up) in [(0, K::Down, K::Up), (1, K::Char('j'), K::Char('k'))] {
        app.focus(pane);
        press(&mut app, down);
        draw(&mut app, &mut terminal);
        assert_eq!(app.cursor[pane], 1);
        assert!(!app.manual_scroll[pane]);
        assert!(
            if pane == 0 {
                app.offset
            } else {
                app.list_offsets[pane]
            } <= 1
        );
        press(&mut app, up);
        assert_eq!(app.cursor[pane], 0);
    }
}

#[test]
fn diff_title_mode_clicks_focus_diff_and_mark_selected_mode() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::style::Modifier;
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Review {
            files: vec![File {
                path: "日本語/".repeat(30),
                ..fixture()
            }],
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    draw(&mut app, &mut terminal);
    for mode in [1, 1, 0] {
        app.focus(1);
        let rect = app.diff_mode_rects[mode];
        assert!(rect.width > 0);
        assert_eq!(
            terminal.backend().buffer()[(rect.x, rect.y)].symbol(),
            if mode == 0 { "u" } else { "s" }
        );
        app.handle(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: M::NONE,
        }))
        .unwrap();
        draw(&mut app, &mut terminal);
        assert_eq!(app.review.split, mode == 1);
        assert_eq!(app.pane, 0);
        for i in 0..2 {
            let rect = app.diff_mode_rects[i];
            assert_eq!(
                terminal.backend().buffer()[(rect.x, rect.y)]
                    .modifier
                    .contains(Modifier::BOLD),
                i == mode
            );
        }
    }
    app.focus(1);
    press(&mut app, K::Char('s'));
    assert!(app.review.split);
    assert_eq!(app.pane, 1);
}

#[test]
fn diff_drag_selects_rows_on_starting_side_and_keeps_range_after_release() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    let patch = format!("@@ -1,12 +1,12 @@\n{}", " context\n".repeat(12));
    let mut app = App::new(
        Review {
            files: vec![file(&patch)],
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    for (split, side, reverse) in [(false, 0, false), (true, 0, true), (true, 1, false)] {
        app.set_split(split).unwrap();
        draw(&mut app, &mut terminal);
        let rows: Vec<_> = (0..app.view().unwrap().len())
            .filter(|&r| app.view().unwrap().source(r, side).is_some())
            .collect();
        let (start, end) = if reverse {
            (rows[5], rows[1])
        } else {
            (rows[1], rows[5])
        };
        let x = if side == 1 {
            app.diff_inner.right() - 2
        } else {
            app.diff_inner.x + 1
        };
        let y = app.diff_inner.y;
        let mouse = |kind, column, row| {
            Event::Mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: M::NONE,
            })
        };
        app.handle(mouse(
            MouseEventKind::Down(MouseButton::Left),
            x,
            y + start as u16,
        ))
        .unwrap();
        assert_eq!(app.pane, 0);
        assert!(app.anchor.is_none());
        // Horizontal motion across the split must not change the selected side.
        app.handle(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            app.diff_inner.right() - 1,
            y + end as u16,
        ))
        .unwrap();
        assert_eq!(app.anchor, Some((start, side)));
        assert_eq!(app.cursor[0], end);
        assert_eq!(app.side, side);
        let expected = app.bounds().unwrap();
        app.handle(mouse(
            MouseEventKind::Up(MouseButton::Left),
            0,
            y + end as u16,
        ))
        .unwrap();
        app.handle(mouse(MouseEventKind::Drag(MouseButton::Left), x, y))
            .unwrap();
        assert_eq!(app.bounds(), Some(expected));
        app.start_edit(None, false).unwrap();
        let comment = &app.editor.as_ref().unwrap().comment;
        assert_eq!((comment.start, comment.end), expected);
        app.close_editor();
        draw(&mut app, &mut terminal);
    }
}

#[test]
fn stacked_layout_focus_zoom_resize_and_state() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Review {
            files: vec![fixture()],
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(60, 102)).unwrap();
    let saved = app.review.clone();
    app.focus(0);
    draw(&mut app, &mut terminal);
    press(&mut app, K::Char('v'));
    press(&mut app, K::Down);
    let selection = app.bounds();
    press(&mut app, K::Char('t'));
    for pane in ['1', '2', '3', '0'] {
        press(&mut app, K::Char(pane));
        draw(&mut app, &mut terminal);
        assert!(app.stacked);
        assert_eq!(app.pane_rects[0], ratatui::layout::Rect::new(0, 30, 60, 70));
        assert_eq!(
            app.pane_rects[app.left_pane],
            ratatui::layout::Rect::new(0, 0, 60, 30)
        );
        assert_eq!(app.pane_rects.iter().filter(|r| !r.is_empty()).count(), 2);
        assert_eq!(app.bounds(), selection);
        press(&mut app, K::Char('}'));
        draw(&mut app, &mut terminal);
        assert_eq!(
            app.pane_rects[app.pane].height,
            if pane == '0' { 75 } else { 35 }
        );
        press(&mut app, K::Char('{'));
        press(&mut app, K::Char('+'));
        draw(&mut app, &mut terminal);
        assert_eq!(
            app.pane_rects[app.pane],
            ratatui::layout::Rect::new(0, 0, 60, 100)
        );
        assert_eq!(app.pane_rects.iter().filter(|r| !r.is_empty()).count(), 1);
        press(&mut app, K::Char('-'));
        assert_eq!(app.zoom, 0);
    }
    let mouse = |kind, x, y| {
        Event::Mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: M::NONE,
        })
    };
    for edge in [29, 30] {
        app.sidebar_percent = 30;
        draw(&mut app, &mut terminal);
        app.handle(mouse(MouseEventKind::Down(MouseButton::Left), 1, edge))
            .unwrap();
        app.handle(mouse(
            MouseEventKind::Drag(MouseButton::Left),
            55,
            edge + 10,
        ))
        .unwrap();
        draw(&mut app, &mut terminal);
        assert_eq!(app.sidebar_percent, 40);
        assert_eq!(app.pane_rects[0].y, 40);
        assert_eq!(app.bounds(), selection);
        app.handle(mouse(MouseEventKind::Up(MouseButton::Left), 1, 99))
            .unwrap();
        assert_eq!(app.sidebar_percent, 90);
        app.handle(mouse(MouseEventKind::Drag(MouseButton::Left), 1, 0))
            .unwrap();
        assert_eq!(app.sidebar_percent, 90);
    }
    app.sidebar_percent = 30;
    draw(&mut app, &mut terminal);
    assert_eq!(app.review, saved);
    // Mode controls share the horizontal border and must remain clickable.
    let mode = app.diff_mode_rects[1];
    assert!(!mode.is_empty());
    app.handle(mouse(
        MouseEventKind::Down(MouseButton::Left),
        mode.x,
        mode.y,
    ))
    .unwrap();
    assert!(app.review.split);
    draw(&mut app, &mut terminal);
    press(&mut app, K::Char('t'));
    draw(&mut app, &mut terminal);
    assert!(!app.stacked);
    assert_eq!(app.pane_rects[0].x, 18);
    assert_eq!(app.pane_rects.iter().filter(|r| !r.is_empty()).count(), 4);
    press(&mut app, K::Char('/'));
    press(&mut app, K::Char('t'));
    assert!(!app.stacked);
    press(&mut app, K::Esc);
    press(&mut app, K::Char('t'));
    for (width, height) in [(1, 1), (3, 2), (10, 4), (40, 80)] {
        app.handle(Event::Resize(width, height)).unwrap();
        terminal.backend_mut().resize(width, height);
        terminal
            .resize(ratatui::layout::Rect::new(0, 0, width, height))
            .unwrap();
        draw(&mut app, &mut terminal);
    }
}

#[test]
fn pane_width_keys_follow_focus_and_preserve_zoom_and_selection() {
    let dir = tempfile::tempdir().unwrap();
    for split in [false, true] {
        let mut app = App::new(
            Review {
                files: vec![fixture()],
                split,
                ..Default::default()
            },
            dir.path().join("state.json"),
        );
        app.wrap = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        app.focus(0);
        draw(&mut app, &mut terminal);
        press(&mut app, K::Char('v'));
        press(&mut app, K::Down);
        let selection = app.bounds();
        for pane in 0..4 {
            app.focus(pane);
            for zoom in 0..=2 {
                app.zoom = zoom;
                press(&mut app, K::Char('}'));
                draw(&mut app, &mut terminal);
                let percent = if zoom == 2 {
                    30
                } else if pane == 0 {
                    25
                } else {
                    35
                };
                assert_eq!(app.sidebar_percent, percent);
                assert_eq!((app.pane, app.zoom, app.bias), (pane, zoom, 0));
                assert_eq!(app.bounds(), selection);
                assert_eq!(
                    app.pane_rects[pane].width,
                    if zoom == 2 {
                        100
                    } else if pane == 0 {
                        100 - percent as u16
                    } else {
                        percent as u16
                    }
                );
                press(&mut app, K::Char('{'));
                assert_eq!(app.sidebar_percent, 30);
            }
        }
        app.zoom = 0;
        app.focus(0);
        for (key, expected) in [('}', 10), ('{', 90)] {
            for _ in 0..30 {
                press(&mut app, K::Char(key));
            }
            draw(&mut app, &mut terminal);
            assert_eq!(app.sidebar_percent, expected);
            assert_eq!(app.bounds(), selection);
        }
        press(&mut app, K::Char('/'));
        press(&mut app, K::Char('{'));
        press(&mut app, K::Char('}'));
        assert_eq!(app.queries[0], "{}");
        assert_eq!(app.sidebar_percent, 90);
        press(&mut app, K::Esc);
        app.start_edit(None, false).unwrap();
        press(&mut app, K::Char('{'));
        press(&mut app, K::Char('}'));
        assert_eq!(app.editor.as_ref().unwrap().input.lines().join(""), "{}");
        assert_eq!(app.sidebar_percent, 90);
    }
}

#[test]
fn pane_border_drag_resizes_without_changing_focus_or_selection() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    let mut f = fixture();
    f.comments.push(Comment {
        file: true,
        text: "file note".into(),
        ..Default::default()
    });
    let mut app = App::new(
        Review {
            files: vec![f],
            split: true,
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let mouse = |kind, column| {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row: 5,
            modifiers: M::NONE,
        })
    };
    for pane in 0..4 {
        app.focus(pane);
        for zoom in 0..=2 {
            app.zoom = zoom;
            for edge in [29, 30] {
                app.sidebar_percent = 30;
                draw(&mut app, &mut terminal);
                let cursor = app.cursor;
                let anchor = app.anchor;
                let bias = app.bias;
                app.handle(mouse(MouseEventKind::Down(MouseButton::Left), edge))
                    .unwrap();
                app.handle(mouse(MouseEventKind::Drag(MouseButton::Left), edge + 10))
                    .unwrap();
                draw(&mut app, &mut terminal);
                assert_eq!(app.sidebar_percent, if zoom == 2 { 30 } else { 40 });
                if zoom < 2 {
                    assert_eq!(
                        (app.pane, app.cursor, app.anchor, app.bias),
                        (pane, cursor, anchor, bias)
                    );
                    assert_eq!(app.pane_rects[app.left_pane].width, 40);
                    if pane == 2 {
                        assert!(app.inspection.is_some());
                    }
                }
                app.handle(mouse(MouseEventKind::Up(MouseButton::Left), 99))
                    .unwrap();
                assert_eq!(app.sidebar_percent, if zoom == 2 { 30 } else { 90 });
                app.handle(mouse(MouseEventKind::Drag(MouseButton::Left), 0))
                    .unwrap();
                assert_eq!(app.sidebar_percent, if zoom == 2 { 30 } else { 90 });
            }
        }
    }
    app.focus(0);
    app.zoom = 0;
    app.sidebar_percent = 30;
    draw(&mut app, &mut terminal);
    app.handle(mouse(MouseEventKind::Down(MouseButton::Left), 30))
        .unwrap();
    app.handle(mouse(MouseEventKind::Drag(MouseButton::Left), 0))
        .unwrap();
    assert_eq!(app.sidebar_percent, 10);
    app.handle(Event::Resize(1, 1)).unwrap();
    terminal.backend_mut().resize(1, 1);
    terminal
        .resize(ratatui::layout::Rect::new(0, 0, 1, 1))
        .unwrap();
    draw(&mut app, &mut terminal);
    app.handle(mouse(MouseEventKind::Up(MouseButton::Left), 99))
        .unwrap();
    assert_eq!(app.sidebar_percent, 10);
    terminal.backend_mut().resize(200, 30);
    terminal
        .resize(ratatui::layout::Rect::new(0, 0, 200, 30))
        .unwrap();
    draw(&mut app, &mut terminal);
    assert_eq!(app.pane_rects[0].width, 180);
    app.handle(mouse(MouseEventKind::Down(MouseButton::Left), 20))
        .unwrap();
    press(&mut app, K::Char('{'));
    app.handle(mouse(MouseEventKind::Up(MouseButton::Left), 199))
        .unwrap();
    assert_eq!(app.sidebar_percent, 15);
}

#[test]
fn dragging_split_divider_resizes_without_selecting_lines() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Review {
            files: vec![fixture()],
            split: true,
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    draw(&mut app, &mut terminal);
    let view = app.view().unwrap();
    let initial_width = view.widths[0];
    let x = app.diff_inner.x + (view.digits + view.widths[0] + 2) as u16;
    let y = app.diff_inner.y + 1;
    let mouse = |kind, column| {
        Event::Mouse(MouseEvent {
            kind,
            column,
            row: y,
            modifiers: M::NONE,
        })
    };
    let cursor = app.cursor[0];
    app.handle(mouse(MouseEventKind::Down(MouseButton::Left), x))
        .unwrap();
    app.handle(mouse(MouseEventKind::Drag(MouseButton::Left), x + 10))
        .unwrap();
    draw(&mut app, &mut terminal);
    assert_eq!(app.pane, 0);
    assert!(app.bias > 0);
    assert!(app.view().unwrap().widths[0] > initial_width);
    assert_eq!(app.cursor[0], cursor);
    assert!(app.anchor.is_none());
    app.handle(mouse(MouseEventKind::Drag(MouseButton::Left), 0))
        .unwrap();
    assert_eq!(app.bias, -40);
    app.handle(mouse(MouseEventKind::Up(MouseButton::Left), 119))
        .unwrap();
    assert_eq!(app.bias, 40);
    app.handle(mouse(MouseEventKind::Drag(MouseButton::Left), x))
        .unwrap();
    assert_eq!(app.bias, 40);
    press(&mut app, K::Char('['));
    assert_eq!(app.bias, 35);
    press(&mut app, K::Char(']'));
    assert_eq!(app.bias, 40);
}

#[test]
fn help_search_filters_bindings_and_q_closes_only_outside_search() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Review {
            files: vec![fixture()],
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
    press(&mut app, K::Char('?'));
    press(&mut app, K::Char('/'));
    press(&mut app, K::Char('q'));
    assert!(
        matches!(&app.modal, Some(Modal::Help { input, search: true, .. }) if input.lines()[0] == "q")
    );
    press(&mut app, K::Backspace);
    app.handle(Event::Paste("ARCHIVE".into())).unwrap();
    press(&mut app, K::Enter);
    draw(&mut app, &mut terminal);
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("choose Archive or Reset all comments"));
    assert!(!text.contains("add a file comment"));
    press(&mut app, K::Char('/'));
    press(&mut app, K::Esc);
    draw(&mut app, &mut terminal);
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(text.contains("add a file comment"));
    assert!(matches!(press(&mut app, K::Char('q')), Effect::None));
    assert!(app.modal.is_none());
}

#[test]
fn forced_unified_files_label_new_files_without_changing_preference() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Review {
            files: vec![fixture()],
            split: true,
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    for (status, base, visible) in [
        ("M", "", true),
        ("??", "", false),
        ("A", "HEAD~1", false),
        ("A", "", true),
    ] {
        app.review
            .statuses
            .insert("sample.go".into(), status.into());
        app.review.base = base.into();
        draw(&mut app, &mut terminal);
        assert_eq!(app.diff_mode_rects.iter().all(|r| r.width > 0), visible);
        let rect = app.pane_rects[0];
        let title: String = (rect.x..rect.right())
            .map(|x| terminal.backend().buffer()[(x, rect.y)].symbol())
            .collect();
        assert_eq!(title.contains("unified / side-by-side"), visible);
        assert_eq!(title.contains("new file"), !visible);
        assert_eq!(title.contains("untracked"), status == "??");
        assert!(app.review.split);
    }
}

#[test]
fn display_and_done_changes_roll_back_when_save_fails() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = fixture();
    f.comments.push(Comment {
        start: 5,
        end: 5,
        text: "keep open".into(),
        ..Default::default()
    });
    let mut app = App::new(
        Review {
            files: vec![f],
            ..Default::default()
        },
        dir.path().join("missing/state.json"),
    );
    assert!(app.set_split(true).is_err());
    assert!(!app.review.split);
    assert!(
        app.toggle_done(annodiff::app::CommentRef {
            history: false,
            file: 0,
            comment: 0
        })
        .is_err()
    );
    assert!(!app.review.files[0].comments[0].done);
}

#[test]
fn batched_git_diff_matches_individual_patches_and_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    for name in [
        "z.go",
        "日本語 [x].go",
        "line\nbreak.go",
        "a.bin",
        "deleted.go",
    ] {
        fs::write(
            root.join(name),
            if name.ends_with("bin") {
                b"\0old\n".as_slice()
            } else {
                b"old\n".as_slice()
            },
        )
        .unwrap();
    }
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=t@x",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "base",
        ],
    );
    for name in ["z.go", "日本語 [x].go", "line\nbreak.go", "a.bin"] {
        fs::write(
            root.join(name),
            if name.ends_with("bin") {
                b"\0new\n".as_slice()
            } else {
                b"new\n".as_slice()
            },
        )
        .unwrap();
    }
    fs::remove_file(root.join("deleted.go")).unwrap();
    // Git order can differ from alphabetical path order.
    fs::write(root.join(".git/diff-order"), "z.go\n*.bin\n").unwrap();
    git(root, &["config", "diff.orderFile", ".git/diff-order"]);
    let r = review::snapshot(root.to_str().unwrap(), "", "").unwrap();
    assert_eq!(r.files.len(), 5);
    assert_eq!(r.files[0].path, "z.go");
    for f in r.files {
        let patch = git(
            root,
            &[
                "-c",
                "core.quotePath=false",
                "-c",
                "diff.suppressBlankEmpty=false",
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--output-indicator-new=+",
                "--output-indicator-old=-",
                "--output-indicator-context= ",
                "HEAD",
                "--",
                &format!(":(literal){}", f.path),
            ],
        );
        assert_eq!(f.patch, patch, "{}", f.path);
    }
}

#[test]
fn distant_highlighting_yields_and_input_can_return_to_cached_lines() {
    let dir = tempfile::tempdir().unwrap();
    let patch = format!(
        "@@ -1,10000 +1,10000 @@\n{}",
        " var value = \"日本語\" // comment\n".repeat(10000)
    );
    let mut app = App::new(
        Review {
            files: vec![file(&patch)],
            split: true,
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    app.focus(0);
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    draw(&mut app, &mut terminal);
    while app.highlight_pending {
        draw(&mut app, &mut terminal);
    }
    press(&mut app, K::End);
    draw(&mut app, &mut terminal);
    assert!(app.highlight_pending);
    assert!(app.view().unwrap().tokens(9999, 1).is_empty());
    press(&mut app, K::Home);
    draw(&mut app, &mut terminal);
    assert!(!app.highlight_pending);
    assert!(app.cursor[0] < 5);
}

#[test]
fn wrapping_preserves_unicode_and_word_boundaries_at_all_widths() {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;
    for text in [
        "alpha beta gamma",
        "日本語 👩‍💻 e\u{301} test",
        "abcdefghijk",
        "   a  b   ",
        "",
    ] {
        let line = CodeLine::new(text);
        for width in 1..32 {
            let parts = line.wrap(width);
            let rebuilt: String = parts.iter().map(|r| &line.text[r.clone()]).collect();
            assert_eq!(rebuilt, text);
            for range in parts {
                let part = &line.text[range];
                assert!(part.width() <= width || part.graphemes(true).count() == 1);
            }
        }
    }
    let line = CodeLine::new("alpha beta gamma");
    assert_eq!(
        line.wrap(8)
            .iter()
            .map(|r| &line.text[r.clone()])
            .collect::<Vec<_>>(),
        ["alpha ", "beta ", "gamma"]
    );
}

#[test]
fn drag_is_cancelled_when_resize_empties_diff_area() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(
        Review {
            files: vec![fixture()],
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    draw(&mut app, &mut terminal);
    let (x, y) = (app.diff_inner.x, app.diff_inner.y);
    let mouse = |kind, row| {
        Event::Mouse(MouseEvent {
            kind,
            column: x,
            row,
            modifiers: M::NONE,
        })
    };
    app.handle(mouse(MouseEventKind::Down(MouseButton::Left), y))
        .unwrap();
    terminal.backend_mut().resize(80, 3);
    terminal.autoresize().unwrap();
    draw(&mut app, &mut terminal);
    assert!(app.diff_inner.is_empty());
    app.handle(mouse(MouseEventKind::Drag(MouseButton::Left), 0))
        .unwrap();
    terminal.backend_mut().resize(80, 24);
    terminal.autoresize().unwrap();
    draw(&mut app, &mut terminal);
    let cursor = app.cursor[0];
    app.handle(mouse(MouseEventKind::Drag(MouseButton::Left), y + 2))
        .unwrap();
    app.handle(mouse(MouseEventKind::Up(MouseButton::Left), y + 2))
        .unwrap();
    assert_eq!(app.cursor[0], cursor);
    assert!(app.anchor.is_none());
}

#[test]
fn wrapped_comment_tail_is_visible_in_inspection_and_send_preview() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = fixture();
    f.comments.push(Comment {
        file: true,
        text: format!("{}TAIL_MARKER", "長いコメント ".repeat(60)),
        ..Default::default()
    });
    let mut app = App::new(
        Review {
            files: vec![f],
            ..Default::default()
        },
        dir.path().join("state.json"),
    );
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for preview in [false, true] {
        app.modal = Some(if preview {
            Modal::Preview {
                destination: "Clipboard".into(),
                id: String::new(),
                copy: true,
                archived: false,
                selection: 0,
                pane: 1,
                offsets: [0; 2],
            }
        } else {
            Modal::Inspect(app.inspect(annodiff::app::CommentRef {
                history: false,
                file: 0,
                comment: 0,
            }))
        });
        let mut found = false;
        for _ in 0..100 {
            draw(&mut app, &mut terminal);
            let screen: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect();
            if screen.contains("TAIL_MARKER") {
                found = true;
                break;
            }
            press(&mut app, K::Down);
        }
        assert!(found, "comment tail missing (preview={preview})");
    }
}

#[test]
fn save_binding_distinguishes_control_enter_from_plain_enter() {
    use annodiff::app::save_key;
    assert!(save_key(KeyEvent::new(K::Enter, M::CONTROL)));
    assert!(save_key(KeyEvent::new(K::Char('j'), M::CONTROL)));
    assert!(save_key(KeyEvent::new(K::F(2), M::NONE)));
    for key in [
        KeyEvent::new(K::Enter, M::NONE),
        KeyEvent::new(K::Enter, M::SHIFT),
        KeyEvent::new(K::Enter, M::CONTROL | M::ALT),
        KeyEvent::new(K::Char('j'), M::NONE),
        KeyEvent::new(K::Char('j'), M::CONTROL | M::ALT),
        KeyEvent::new(K::Char('s'), M::CONTROL),
    ] {
        assert!(!save_key(key));
    }
}

#[test]
fn selection_survives_reflow_before_comment_editing() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let dir = tempfile::tempdir().unwrap();
    let patch = format!(
        "@@ -1,5 +1,5 @@\n {}\n second\n third\n fourth\n fifth\n",
        "x".repeat(150)
    );
    for (split, side) in [(false, 0), (true, 0), (true, 1)] {
        for mouse in [false, true] {
            for (start, end) in [(3, 4), (4, 3)] {
                let mut app = App::new(
                    Review {
                        split,
                        files: vec![file(&patch)],
                        ..Default::default()
                    },
                    dir.path().join("state.json"),
                );
                app.wrap = true;
                app.pane = 0;
                app.side = side;
                let mut terminal = Terminal::new(TestBackend::new(240, 40)).unwrap();
                draw(&mut app, &mut terminal);
                let mouse_at = |app: &App, kind, source| {
                    let visual = app.view().unwrap().visual_for_source(source, side).unwrap();
                    Event::Mouse(MouseEvent {
                        kind,
                        column: if side == 1 {
                            app.diff_inner.right() - 2
                        } else {
                            app.diff_inner.x + 1
                        },
                        row: app.diff_inner.y + (visual - app.offset) as u16,
                        modifiers: M::NONE,
                    })
                };
                if mouse {
                    app.handle(mouse_at(
                        &app,
                        MouseEventKind::Down(MouseButton::Left),
                        start,
                    ))
                    .unwrap();
                    app.handle(mouse_at(&app, MouseEventKind::Drag(MouseButton::Left), end))
                        .unwrap();
                } else {
                    app.cursor[0] = app.view().unwrap().visual_for_source(start, side).unwrap();
                    press(&mut app, K::Char('v'));
                    app.cursor[0] = app.view().unwrap().visual_for_source(end, side).unwrap();
                }
                assert_eq!(app.bounds(), Some((3, 4)));
                for width in [100, 240, 100] {
                    terminal.backend_mut().resize(width, 40);
                    terminal.autoresize().unwrap();
                    draw(&mut app, &mut terminal);
                    assert_eq!(app.bounds(), Some((3, 4)));
                    if mouse {
                        app.handle(mouse_at(&app, MouseEventKind::Drag(MouseButton::Left), end))
                            .unwrap();
                        assert_eq!(app.bounds(), Some((3, 4)));
                    }
                }
                if mouse {
                    app.handle(mouse_at(&app, MouseEventKind::Up(MouseButton::Left), end))
                        .unwrap();
                }
                app.start_edit(None, false).unwrap();
                let comment = &app.editor.as_ref().unwrap().comment;
                assert_eq!((comment.start, comment.end), (3, 4));
            }
        }
    }
}

#[test]
fn unsent_comments_remain_sendable_after_reload() {
    for scenario in [
        "sent_only",
        "edit_sent_comment",
        "new_unsent_comment",
        "unrelated_file_change",
        "no_remaining_diff",
        "deleted_file",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        fs::write(root.join("a.txt"), "base\n").unwrap();
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "base",
            ],
        );
        fs::write(root.join("a.txt"), "first change\n").unwrap();
        let mut review = review::snapshot(root.to_str().unwrap(), "", "").unwrap();
        // Model the state written after successful delivery; do not send to a real agent.
        review.files[0].comments.push(Comment {
            file: true,
            sent: true,
            text: "first review".into(),
            ..Default::default()
        });
        let mut app = App::new(review, root.join(".git/review.json"));
        app.select_file(Some(0));
        if scenario == "edit_sent_comment" {
            app.start_edit(
                Some(annodiff::app::CommentRef {
                    file: 0,
                    comment: 0,
                    history: false,
                }),
                true,
            )
            .unwrap();
            app.editor.as_mut().unwrap().input.insert_str(" follow-up");
            app.save_editor().unwrap();
        } else if scenario != "sent_only" {
            app.start_edit(None, true).unwrap();
            app.editor
                .as_mut()
                .unwrap()
                .input
                .insert_str("follow-up review");
            app.save_editor().unwrap();
        }
        let pending_before = app.review.pending();
        if scenario == "unrelated_file_change" {
            fs::write(root.join("b.txt"), "unrelated change\n").unwrap();
        } else if scenario == "no_remaining_diff" {
            fs::write(root.join("a.txt"), "base\n").unwrap();
        } else if scenario == "deleted_file" {
            fs::remove_file(root.join("a.txt")).unwrap();
        } else {
            fs::write(root.join("a.txt"), "second change\n").unwrap();
        }
        assert!(app.fresh().is_err());
        app.refresh(false).unwrap();
        app.fresh().unwrap();
        assert_eq!(app.review.pending(), pending_before);
        assert_eq!(app.pending_refs().len(), pending_before);
        let saved = Review::load(&app.state).unwrap();
        assert_eq!(saved.pending(), pending_before);
        let historical = scenario != "unrelated_file_change";
        assert_eq!(
            saved.pending_comments().filter(|(h, _, _)| *h).count(),
            if historical { pending_before } else { 0 }
        );
        if scenario == "sent_only" {
            assert!(!app.prompt().contains("### Comment"));
            assert_eq!(
                app.start_sessions().unwrap_err().to_string(),
                "no unsent Open comments to send"
            );
            continue;
        }
        assert!(app.prompt().contains("follow-up"));
        assert_eq!(app.prompt().contains("Historical snapshot:"), historical);
        // Repeated reload and manual Archive must not duplicate or revive comments.
        app.refresh(false).unwrap();
        assert_eq!(app.review.pending(), pending_before);
        let archived = app.review.refresh(
            review::snapshot(root.to_str().unwrap(), "", "").unwrap(),
            true,
        );
        assert_eq!(archived.pending(), 0);
        assert!(!archived.prompt().contains("### Comment"));
        // A successful queued delivery updates history and preserves cleanup tracking.
        let mut delivered = app.review.clone();
        delivered.mark_sent("annodiff-review-regression.md");
        assert_eq!(delivered.pending(), 0);
        assert!(
            delivered
                .files
                .iter()
                .chain(&delivered.history)
                .flat_map(|f| &f.comments)
                .filter(|c| c.text.contains("follow-up"))
                .all(|c| c.sent && c.delivery == "annodiff-review-regression.md")
        );
        app.apply(delivered).unwrap();
        if app.review.files.is_empty() {
            assert_eq!(app.review.pending(), 0);
            app.fresh().unwrap();
            continue;
        }
        app.select_file(Some(0));
        app.start_edit(None, true).unwrap();
        app.editor
            .as_mut()
            .unwrap()
            .input
            .insert_str("new comment after reload");
        app.save_editor().unwrap();
        assert_eq!(app.review.pending(), 1);
        assert_eq!(app.pending_refs().len(), 1);
        assert!(app.prompt().contains("new comment after reload"));
        app.fresh().unwrap();
    }
}

#[test]
fn historical_comment_preview_preserves_archive_and_legacy_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = fixture();
    f.comments.push(Comment {
        start: 7,
        end: 7,
        text: "historical note".into(),
        ..Default::default()
    });
    let current = Review {
        files: vec![f],
        ..Default::default()
    };
    let refreshed = current.refresh(Review::default(), false);
    let mut app = App::new(refreshed, dir.path().join("state.json"));
    app.review.save(&app.state).unwrap();
    app.file_only = false;
    app.open_only = false;
    app.rebuild_comments();
    app.focus(2);
    assert_eq!(app.review.pending(), 1);
    assert!(app.pending_refs()[0].history);
    assert!(app.prompt().contains("Historical snapshot:"));
    assert!(app.prompt().contains("* 0/2 +new()"));
    assert_eq!(Review::load(&app.state).unwrap().pending(), 1);
    // Clipboard preview uses the same history entry and leaves it unsent.
    app.modal = Some(Modal::Preview {
        destination: "Clipboard".into(),
        id: String::new(),
        copy: true,
        archived: false,
        selection: 0,
        pane: 0,
        offsets: [0; 2],
    });
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
    draw(&mut app, &mut terminal);
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(screen.contains("[history]"));
    assert!(screen.contains("Historical comment"));
    press(&mut app, K::Esc);
    press(&mut app, K::Char('x'));
    assert_eq!(app.review.pending(), 0);
    press(&mut app, K::Char('x'));
    assert_eq!(app.review.pending(), 1);
    let archived = app.review.refresh(Review::default(), true);
    assert_eq!(archived.pending(), 0);
    // Legacy history cannot distinguish reload from manual Archive; preserve its exclusion.
    let encoded = serde_json::to_string(&archived).unwrap();
    assert!(!encoded.contains("SendFromHistory"));
    assert_eq!(
        serde_json::from_str::<Review>(&encoded).unwrap().pending(),
        0
    );
}

#[test]
fn session_picker_sort_and_archived_reload_preserve_filters() {
    use annodiff::agent::{Session, SessionOptions};
    let dir = tempfile::tempdir().unwrap();
    let mut items = vec![
        Session {
            id: "local".into(),
            current: true,
            preview: "Find local".into(),
            updated_at: 10,
            created_at: 30,
            ..Default::default()
        },
        Session {
            id: "remote".into(),
            preview: "Find remote".into(),
            updated_at: 20,
            created_at: 5,
            ..Default::default()
        },
    ];
    let mut options = SessionOptions {
        all: true,
        ..Default::default()
    };
    options.sort(&mut items);
    assert_eq!(items[0].id, "remote"); // All does not prioritize local sessions.
    let mut app = App::new(Review::default(), dir.path().join("state.json"));
    app.modal = Some(Modal::Sessions {
        items,
        input: ratatui_textarea::TextArea::new(vec!["find".into()]),
        selection: 3,
        search: false,
        options,
        control: 0,
    });
    press(&mut app, K::Tab);
    press(&mut app, K::Tab);
    press(&mut app, K::Right);
    let Some(Modal::Sessions {
        items,
        options: selected,
        input,
        selection,
        ..
    }) = &app.modal
    else {
        panic!()
    };
    assert!(selected.all && selected.created && !selected.archived);
    assert_eq!(items[0].id, "local");
    assert_eq!(*selection, 0);
    assert_eq!(input.lines(), ["find"]);
    // Loading Archived keeps scope, sort, query and focused control.
    options.created = true;
    options.archived = true;
    let (tx, receiver) = std::sync::mpsc::channel();
    tx.send(Ok(vec![Session {
        id: "archived".into(),
        preview: "Find archived".into(),
        ..Default::default()
    }]))
    .unwrap();
    app.modal = Some(Modal::Loading {
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        receiver,
        options,
        input: ratatui_textarea::TextArea::new(vec!["find".into()]),
        control: 1,
    });
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    draw(&mut app, &mut terminal);
    let loading_screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    for label in [
        "Search Codex sessions",
        "find",
        "Destination",
        "CWD / [All]",
        "Active / [Archived]",
        "Updated / [Created]",
        "Loading sessions…",
    ] {
        assert!(loading_screen.contains(label), "missing {label}");
    }
    assert!(!loading_screen.contains("New session in this directory"));
    assert!(app.poll());
    assert!(
        matches!(&app.modal, Some(Modal::Sessions { options: loaded, input, control: 1, selection: 0, .. }) if *loaded == options && input.lines() == ["find"])
    );
    draw(&mut app, &mut terminal);
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    for label in ["CWD / [All]", "Active / [Archived]", "Updated / [Created]"] {
        assert!(screen.contains(label));
    }
    press(&mut app, K::Down);
    press(&mut app, K::Down);
    press(&mut app, K::Enter);
    assert!(
        matches!(&app.modal, Some(Modal::Preview { id, archived: true, destination, .. }) if id == "archived" && destination.contains("restored when you confirm sending"))
    );
}
