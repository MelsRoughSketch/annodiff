use annodiff::{
    app::{App, CommentRef},
    review::{self, Comment, File, Review},
};
use crossterm::event::{Event, KeyCode as K, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::{fmt::Write, hint::black_box, process::Command, time::Instant};

fn fixture(size: usize) -> Review {
    let mut patch = String::new();
    for i in 0..size {
        if i % 100 == 0 {
            writeln!(patch, "@@ -{},100 +{},100 @@", i + 1, i + 1).unwrap();
        }
        let mut code = format!("\tfmt.Println(\"日本語 message {i:05}\", value)");
        if i % 20 == 0 {
            code.push_str(&format!(" // {}", "long text ".repeat(12)));
        }
        if i % 10 == 0 {
            writeln!(patch, "-{}\n+{code}", code.replacen("value", "oldValue", 1)).unwrap();
        } else {
            writeln!(patch, " {code}").unwrap();
        }
    }
    Review {
        files: (0..2)
            .map(|n| {
                let lines = review::parse(&patch);
                let comments = lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.new % 100 == 1)
                    .map(|(i, _)| Comment {
                        start: i,
                        end: i,
                        side: "new".into(),
                        text: "確認してください\nsecond line".into(),
                        ..Default::default()
                    })
                    .collect();
                File {
                    path: format!("file{n}.go"),
                    patch: patch.clone(),
                    lines,
                    comments,
                }
            })
            .collect(),
        ..Default::default()
    }
}
fn report(name: &str, mut values: [f64; 3]) {
    values.sort_by(f64::total_cmp);
    println!(
        "{name},{:.6},{:.6},{:.6},3",
        values[1], values[0], values[2]
    );
}
fn draw(app: &mut App, terminal: &mut Terminal<TestBackend>) {
    terminal.draw(|f| app.draw(f)).unwrap();
}
fn press(app: &mut App, key: K) {
    app.handle(Event::Key(KeyEvent::new(key, KeyModifiers::NONE)))
        .unwrap();
}
fn git(root: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn main() {
    println!("benchmark,median_ms,min_ms,max_ms,samples");
    for size in [100, 1000, 10000] {
        for mode in ["unified", "split", "split-wrap"] {
            for op in [
                "first-render",
                "redraw",
                "scroll",
                "page",
                "divider",
                "wrap-toggle",
                "view-toggle",
                "terminal-resize",
                "file-switch",
                "search",
                "comment-edit",
            ] {
                if op == "divider" && mode == "unified" {
                    continue;
                }
                let mut times = [0.; 3];
                for time in &mut times {
                    let dir = tempfile::tempdir().unwrap();
                    let mut r = fixture(size);
                    r.root = dir.path().display().to_string();
                    r.split = mode != "unified";
                    let mut app = App::new(r, dir.path().join("state.json"));
                    app.wrap = mode == "split-wrap";
                    app.pane = 0;
                    let mut terminal = Terminal::new(TestBackend::new(80, 25)).unwrap();
                    draw(&mut app, &mut terminal);
                    draw(&mut app, &mut terminal);
                    if op == "scroll" || op == "page" {
                        app.cursor[0] = app.view().unwrap().len() / 2;
                        draw(&mut app, &mut terminal);
                    }
                    while app.highlight_pending {
                        draw(&mut app, &mut terminal);
                    }
                    let start = Instant::now();
                    for i in 0..4 {
                        match op {
                            // Rebuild all derived data, including syntax; Git and library startup excluded.
                            "first-render" => {
                                app.cache.clear();
                                app.ensure_view();
                            }
                            "scroll" => press(&mut app, if i % 2 == 0 { K::Down } else { K::Up }),
                            "page" => {
                                press(&mut app, if i % 2 == 0 { K::PageDown } else { K::PageUp })
                            }
                            "divider" => {
                                press(&mut app, K::Char(if i % 2 == 0 { ']' } else { '[' }))
                            }
                            "wrap-toggle" => press(&mut app, K::Char('f')),
                            "view-toggle" => press(&mut app, K::Char('s')),
                            "terminal-resize" => {
                                let width = 140 + 20 * (i % 2);
                                terminal.backend_mut().resize(width, 50);
                                terminal.resize(Rect::new(0, 0, width, 50)).unwrap();
                            }
                            "file-switch" => app.select_file(Some((i as usize + 1) % 2)),
                            "search" => {
                                app.filter(0, if i % 2 == 0 { "message" } else { "absent" }.into())
                            }
                            "comment-edit" => {
                                app.start_edit(
                                    Some(CommentRef {
                                        history: false,
                                        file: 0,
                                        comment: 0,
                                    }),
                                    false,
                                )
                                .unwrap();
                                draw(&mut app, &mut terminal);
                                press(&mut app, K::Esc);
                            }
                            _ => {}
                        }
                        draw(&mut app, &mut terminal);
                    }
                    *time = start.elapsed().as_secs_f64() * 1000. / 4.;
                }
                report(&format!("BenchmarkUI/lines={size}/{mode}/{op}"), times);
            }
        }
        for op in ["parse", "save", "load", "prompt"] {
            let dir = tempfile::tempdir().unwrap();
            let state = dir.path().join("state.json");
            let r = fixture(size);
            r.save(&state).unwrap();
            let mut times = [0.; 3];
            for time in &mut times {
                let start = Instant::now();
                for _ in 0..4 {
                    match op {
                        "parse" => {
                            black_box(review::parse(&r.files[0].patch));
                        }
                        "save" => r.save(&state).unwrap(),
                        "load" => {
                            black_box(Review::load(&state).unwrap());
                        }
                        _ => {
                            black_box(r.prompt());
                        }
                    }
                }
                *time = start.elapsed().as_secs_f64() * 1000. / 4.;
            }
            report(&format!("BenchmarkReviewData/lines={size}/{op}"), times);
        }
    }
    for count in [1, 20, 100] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        git(root, &["config", "user.name", "Benchmark"]);
        git(root, &["config", "user.email", "bench@example.invalid"]);
        for i in 0..count {
            std::fs::write(
                root.join(format!("file{i:03}.go")),
                (0..100)
                    .map(|j| format!("var value{j} = \"old\" // 日本語\n"))
                    .collect::<String>(),
            )
            .unwrap();
        }
        git(root, &["add", "."]);
        git(
            root,
            &["-c", "commit.gpgsign=false", "commit", "-qm", "initial"],
        );
        for i in 0..count {
            std::fs::write(
                root.join(format!("file{i:03}.go")),
                (0..100)
                    .map(|j| {
                        format!(
                            "var value{j} = \"{}\" // 日本語\n",
                            if j % 10 == 0 { "new" } else { "old" }
                        )
                    })
                    .collect::<String>(),
            )
            .unwrap();
        }
        let mut times = [0.; 3];
        for time in &mut times {
            let start = Instant::now();
            for _ in 0..4 {
                black_box(review::snapshot(root.to_str().unwrap(), "", "").unwrap());
            }
            *time = start.elapsed().as_secs_f64() * 1000. / 4.;
        }
        report(&format!("BenchmarkSnapshot/files={count}"), times);
    }
}
