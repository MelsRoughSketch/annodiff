use annodiff::{
    app::App,
    diff::FileView,
    review::{self, Comment, File, Review},
};
use crossterm::event::{
    Event, KeyCode as K, KeyEvent, KeyModifiers as M, MouseButton, MouseEvent,
    MouseEventKind as Mouse,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{hint::black_box, time::Instant};

fn fixture(lines: usize, files: usize, comments: usize, long: bool) -> Review {
    let suffix = if long {
        "日本語 long text ".repeat(20)
    } else {
        String::new()
    };
    let patch = format!(
        "@@ -1,{lines} +1,{lines} @@\n{}",
        (0..lines)
            .map(|i| format!(" var value{i} = \"日本語\" // comment {suffix}\n"))
            .collect::<String>()
    );
    Review {
        split: true,
        files: (0..files)
            .map(|i| File {
                path: format!("file{i}.go"),
                lines: review::parse(&patch),
                patch: patch.clone(),
                comments: (0..comments)
                    .map(|j| Comment {
                        start: 1 + j * lines / comments,
                        end: 1 + j * lines / comments,
                        side: "new".into(),
                        text: "review comment\nsecond line".into(),
                        ..Default::default()
                    })
                    .collect(),
            })
            .collect(),
        ..Default::default()
    }
}
fn draw(app: &mut App, t: &mut Terminal<TestBackend>) {
    t.draw(|f| app.draw(f)).unwrap();
}
fn press(app: &mut App, key: K) {
    app.handle(Event::Key(KeyEvent::new(key, M::NONE))).unwrap();
}
fn mouse(app: &mut App, kind: Mouse, x: u16, y: u16) {
    app.handle(Event::Mouse(MouseEvent {
        kind,
        column: x,
        row: y,
        modifiers: M::NONE,
    }))
    .unwrap();
}
fn report(name: &str, mut times: Vec<f64>) {
    times.sort_by(f64::total_cmp);
    println!(
        "{name},{:.6},{:.6},{:.6},{}",
        times[times.len() / 2],
        times[0],
        times[times.len() - 1],
        times.len()
    );
}
fn main() {
    println!("benchmark,median_ms,min_ms,max_ms,samples");
    for size in [1000, 10000, 100000] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(fixture(size, 1, 0, false), dir.path().join("state.json"));
        app.focus(0);
        let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
        draw(&mut app, &mut t); // Load syntax definitions outside measurements.
        for op in [
            "end-cold",
            "end-warm",
            "wheel",
            "range-drag",
            "divider-drag",
            "files-filter",
            "mode-toggle",
        ] {
            if !op.starts_with("end-") {
                app.cursor[0] = app.view().unwrap().len() / 2;
                app.offset = app.cursor[0];
                draw(&mut app, &mut t);
            }
            let mut times = Vec::new();
            for i in 0..9 {
                if op == "end-cold" {
                    app.cache.clear();
                    app.cursor[0] = 0;
                    app.offset = 0;
                    draw(&mut app, &mut t);
                }
                if op == "end-warm" {
                    press(&mut app, K::Home);
                    draw(&mut app, &mut t);
                }
                let inner = app.diff_inner;
                if op == "range-drag" {
                    mouse(
                        &mut app,
                        Mouse::Down(MouseButton::Left),
                        inner.x + 2,
                        inner.y + 2,
                    );
                }
                let divider = inner.x
                    + (app.view().unwrap().digits + app.view().unwrap().widths[0] + 2) as u16;
                if op == "divider-drag" {
                    mouse(
                        &mut app,
                        Mouse::Down(MouseButton::Left),
                        divider,
                        inner.y + 2,
                    );
                }
                let start = Instant::now();
                match op {
                    "end-cold" | "end-warm" => press(&mut app, K::End),
                    "wheel" => mouse(
                        &mut app,
                        if i % 2 == 0 {
                            Mouse::ScrollDown
                        } else {
                            Mouse::ScrollUp
                        },
                        inner.x + 2,
                        inner.y + 2,
                    ),
                    "range-drag" => mouse(
                        &mut app,
                        Mouse::Drag(MouseButton::Left),
                        inner.x + 2,
                        inner.y + 15,
                    ),
                    "divider-drag" => mouse(
                        &mut app,
                        Mouse::Drag(MouseButton::Left),
                        (i32::from(divider) + if i % 2 == 0 { 5 } else { -5 }) as u16,
                        inner.y + 2,
                    ),
                    "files-filter" => {
                        app.filter(1, if i % 2 == 0 { "file" } else { "file0" }.into())
                    }
                    "mode-toggle" => press(&mut app, K::Char('s')),
                    _ => unreachable!(),
                }
                draw(&mut app, &mut t);
                times.push(start.elapsed().as_secs_f64() * 1000.);
                if op == "end-cold" && i == 8 {
                    while app.highlight_pending {
                        draw(&mut app, &mut t);
                    }
                    report(
                        &format!("Interaction/lines={size}/end-highlight-complete"),
                        vec![start.elapsed().as_secs_f64() * 1000.],
                    );
                }
                if op.ends_with("drag") {
                    let release_x = if op == "divider-drag" {
                        (i32::from(divider) + if i % 2 == 0 { 5 } else { -5 }) as u16
                    } else {
                        inner.x + 2
                    };
                    mouse(
                        &mut app,
                        Mouse::Up(MouseButton::Left),
                        release_x,
                        inner.y + 15,
                    );
                }
            }
            report(&format!("Interaction/lines={size}/{op}"), times);
        }
    }
    for size in [1000, 10000] {
        for wrap in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let mut app = App::new(fixture(size, 1, 0, true), dir.path().join("state.json"));
            app.wrap = wrap;
            app.focus(0);
            let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
            draw(&mut app, &mut t);
            while app.highlight_pending {
                draw(&mut app, &mut t);
            }
            let mut times = Vec::new();
            for i in 0..9 {
                let start = Instant::now();
                press(&mut app, K::Char(if i % 2 == 0 { ']' } else { '[' }));
                draw(&mut app, &mut t);
                times.push(start.elapsed().as_secs_f64() * 1000.);
            }
            report(
                &format!("LongLines/lines={size}/wrap={wrap}/divider"),
                times,
            );
        }
    }
    for comments in [0, 100, 1000] {
        let review = fixture(10000, 1, comments, false);
        let mut times = Vec::new();
        for _ in 0..9 {
            let start = Instant::now();
            black_box(FileView::new(&review.files[0], true));
            times.push(start.elapsed().as_secs_f64() * 1000.);
        }
        report(&format!("BuildView/lines=10000/comments={comments}"), times);
    }
    for files in [10, 100, 1000] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(fixture(100, files, 1, false), dir.path().join("state.json"));
        let mut t = Terminal::new(TestBackend::new(120, 40)).unwrap();
        draw(&mut app, &mut t);
        while app.highlight_pending {
            draw(&mut app, &mut t);
        }
        for op in ["filter", "toggle-done"] {
            let mut times = Vec::new();
            for i in 0..9 {
                let start = Instant::now();
                if op == "filter" {
                    app.filter(1, if i % 2 == 0 { "file" } else { ".go" }.into());
                } else {
                    app.toggle_done(annodiff::app::CommentRef {
                        history: false,
                        file: 0,
                        comment: 0,
                    })
                    .unwrap();
                }
                draw(&mut app, &mut t);
                times.push(start.elapsed().as_secs_f64() * 1000.);
            }
            report(&format!("ManyFiles/files={files}/{op}"), times);
        }
    }
}
