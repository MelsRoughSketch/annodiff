use annodiff::{
    app::App,
    review::{self, File, Review},
};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use std::time::Instant;
fn main() {
    for size in [1000, 10000] {
        let patch = format!(
            "@@ -0,0 +1,{size} @@\n{}",
            (0..size)
                .map(|i| format!("+var value{i} = \"日本語\" // comment\n"))
                .collect::<String>()
        );
        let review = Review {
            files: (0..12)
                .map(|i| File {
                    path: format!("file{i}.go"),
                    lines: review::parse(&patch),
                    patch: patch.clone(),
                    comments: vec![],
                })
                .collect(),
            split: true,
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(review, dir.path().join("state.json"));
        let mut terminal = Terminal::new(TestBackend::new(80, 25)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let mut times = Vec::new();
        for _ in 0..11 {
            let start = Instant::now();
            app.handle(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)))
                .unwrap();
            terminal.draw(|f| app.draw(f)).unwrap();
            times.push(start.elapsed().as_secs_f64() * 1000.);
        }
        assert_eq!(app.file, Some(11));
        times.sort_by(f64::total_cmp);
        println!(
            "{size} lines, 12 new files: median={:.3}ms max={:.3}ms",
            times[5], times[10]
        );
    }
}
