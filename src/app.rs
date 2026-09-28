use crate::{
    agent,
    diff::{FileView, Row, commit_graph},
    review::{self, Comment, Commit, Review},
};
use anyhow::{Context, Result, ensure};
use crossterm::event::{
    Event, KeyCode as K, KeyEvent, KeyEventKind, KeyModifiers as M, MouseButton, MouseEventKind,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
};
use ratatui_textarea::TextArea;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommentRef {
    pub history: bool,
    pub file: usize,
    pub comment: usize,
}
pub struct Editor {
    pub input: TextArea<'static>,
    pub reference: Option<CommentRef>,
    pub comment: Comment,
    pub after: usize,
    pub return_pane: usize,
    pub selection: usize,
    pub offset: usize,
}
pub struct Inspection {
    pub reference: CommentRef,
    pub recorded: String,
    pub current: String,
    pub offsets: [usize; 2],
    pub pane: usize,
}
pub enum Confirmation {
    Delete(CommentRef),
    Reset,
}
pub enum Modal {
    Help {
        scroll: usize,
        input: TextArea<'static>,
        search: bool,
    },
    Search {
        pane: usize,
        previous: String,
        file: Option<usize>,
        cursor: [usize; 4],
        input: TextArea<'static>,
    },
    Confirm {
        action: Confirmation,
        choice: usize,
    },
    Inspect(Inspection),
    Loading {
        cancel: Arc<AtomicBool>,
        receiver: Receiver<Result<Vec<agent::Session>>>,
        options: agent::SessionOptions,
        input: TextArea<'static>,
        control: usize,
    },
    Sessions {
        offset: usize,
        manual_scroll: bool,
        area: Rect,
        filter_areas: [[Rect; 2]; 3],
        items: Vec<agent::Session>,
        input: TextArea<'static>,
        selection: usize,
        search: bool,
        options: agent::SessionOptions,
        control: usize,
    },
    Preview {
        destination: String,
        id: String,
        copy: bool,
        archived: bool,
        list_offset: usize,
        selection: usize,
        pane: usize,
        offsets: [usize; 2],
    },
}
pub enum Effect {
    None,
    Quit,
    Editor,
    Send { id: String, archived: bool },
    Clipboard(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileRow {
    Directory(String),
    File(usize),
}

pub struct App {
    pub review: Review,
    pub session_cwd: String,
    pub state: PathBuf,
    pub language: String,
    pub file: Option<usize>,
    pub pane: usize,
    pub side: usize,
    pub left_pane: usize,
    pub zoom: u8,
    pub wrap: bool,
    pub bias: i32,
    pub sidebar_percent: i32,
    pub stacked: bool,
    pub cursor: [usize; 4],
    pub offset: usize,
    pub anchor: Option<(usize, usize)>,
    drag_start: Option<(usize, usize)>,
    divider_drag: Option<(u16, i32)>,
    pane_drag: Option<(u16, i32)>,
    pub queries: [String; 4],
    pub labels: [Vec<Line<'static>>; 4],
    pub file_rows: Vec<usize>,
    pub tree_rows: Vec<FileRow>,
    collapsed_dirs: HashSet<String>,
    pub refs: Vec<CommentRef>,
    pub commits: Vec<Commit>,
    pub graphs: Vec<String>,
    pub commit_rows: Vec<Option<usize>>,
    pub open_only: bool,
    pub file_only: bool,
    pub commented_files_only: bool,
    pub status: String,
    pub editor: Option<Editor>,
    pub modal: Option<Modal>,
    pub inspection: Option<Inspection>,
    pub pane_rects: [Rect; 4],
    pub(crate) pane_area: Rect,
    pub diff_inner: Rect,
    pub diff_mode_rects: [Rect; 2],
    pub editor_rect: Rect,
    pub list_offsets: [usize; 4],
    pub manual_scroll: [bool; 4],
    pub highlight_pending: bool,
    pub cache: VecDeque<(usize, FileView)>,
    pub total_changes: usize,
    session_worker: Option<thread::JoinHandle<Option<agent::SessionClient>>>,
    _state_lock: Option<fs::File>,
}
impl Drop for App {
    fn drop(&mut self) {
        self.close_modal();
        if let Some(worker) = self.session_worker.take() {
            let _ = worker.join();
        }
    }
}
impl App {
    pub fn open(dir: &str) -> Result<Self> {
        let root = review::root(dir)?;
        let state = review::state_path(&root)?;
        let state_lock = review::lock_state(&state)?;
        if !state.try_exists()? {
            let legacy = state.with_file_name("anno-diff.json");
            if legacy.try_exists()? {
                fs::rename(legacy, &state)?;
            }
        }
        let saved = if state.try_exists()? {
            Some(Review::load(&state)?)
        } else {
            None
        };
        if let Some(saved) = &saved {
            ensure!(
                saved.root == root,
                "saved review belongs to another repository: {}",
                state.display()
            );
        }
        let next = review::snapshot(
            &root,
            saved.as_ref().map_or("", |r| r.base.as_str()),
            saved.as_ref().map_or("", |r| r.target.as_str()),
        )?;
        let review = if let Some(saved) = saved {
            let changed = !saved.same_diff(&next);
            let next = saved.refresh(next, false);
            if changed {
                next.save(&state)?;
            }
            next
        } else {
            next
        };
        let mut app = Self::new(review, state);
        app.session_cwd = fs::canonicalize(dir)?.to_string_lossy().into_owned();
        app._state_lock = Some(state_lock);
        app.language = review::response_language()?;
        app.commits = review::commits(&app.review.root)?;
        app.graphs = commit_graph(&app.commits);
        app.rebuild_lists();
        Ok(app)
    }
    pub fn new(review: Review, state: PathBuf) -> Self {
        let file = (!review.files.is_empty()).then_some(0);
        let mut app = Self {
            session_cwd: review.root.clone(),
            review,
            state,
            language: String::new(),
            file,
            pane: 1,
            side: 0,
            left_pane: 1,
            zoom: 0,
            wrap: false,
            bias: 0,
            sidebar_percent: 30,
            stacked: false,
            cursor: [0; 4],
            offset: 0,
            anchor: None,
            drag_start: None,
            divider_drag: None,
            pane_drag: None,
            queries: Default::default(),
            labels: Default::default(),
            file_rows: Vec::new(),
            tree_rows: Vec::new(),
            collapsed_dirs: HashSet::new(),
            refs: Vec::new(),
            commits: Vec::new(),
            graphs: Vec::new(),
            commit_rows: Vec::new(),
            open_only: true,
            file_only: true,
            commented_files_only: false,
            status: "Ready".into(),
            editor: None,
            modal: None,
            inspection: None,
            pane_rects: [Rect::default(); 4],
            pane_area: Rect::default(),
            diff_inner: Rect::default(),
            diff_mode_rects: [Rect::default(); 2],
            editor_rect: Rect::default(),
            list_offsets: [0; 4],
            manual_scroll: [false; 4],
            highlight_pending: false,
            cache: VecDeque::new(),
            total_changes: 0,
            session_worker: None,
            _state_lock: None,
        };
        app.rebuild_lists();
        app.ensure_view();
        app
    }
    pub fn set_split(&mut self, split: bool) -> Result<()> {
        if self.review.split == split {
            return Ok(());
        }
        let source = self
            .view()
            .and_then(|v| v.display_source(self.cursor[0], self.side));
        let previous = self.review.split;
        self.review.split = split;
        if let Err(error) = self.review.save(&self.state) {
            self.review.split = previous;
            return Err(error);
        }
        self.anchor = None;
        for (file, view) in &mut self.cache {
            let split = self.review.split_file(*file);
            if view.split != split {
                view.rebuild_rows(&self.review.files[*file], split);
            }
        }
        self.layout_diff(self.diff_inner.width as usize);
        if let (Some(source), Some(v)) = (source, self.view())
            && let Some(row) = v.visual_for_display(source, self.side)
        {
            self.cursor[0] = row;
        }
        Ok(())
    }
    fn set_diff_bias(&mut self, bias: i32) {
        self.bias = bias.clamp(-40, 40);
        self.anchor = None;
        self.status = format!(
            "Diff width · old {}% / new {}%",
            50 + self.bias,
            50 - self.bias
        );
    }
    fn set_sidebar_percent(&mut self, percent: i32) {
        self.sidebar_percent = percent.clamp(10, 90);
        self.status = format!(
            "Pane {} · sidebar {}% / diff {}%",
            if self.stacked { "height" } else { "width" },
            self.sidebar_percent,
            100 - self.sidebar_percent
        );
    }
    pub fn split_for(&self, file: usize) -> bool {
        self.review.split_file(file)
    }
    pub(crate) fn pane_axis(&self) -> (u16, u16) {
        if self.stacked {
            (self.pane_area.y, self.pane_area.height)
        } else {
            (self.pane_area.x, self.pane_area.width)
        }
    }
    pub fn split(&self) -> bool {
        self.file.is_some_and(|file| self.split_for(file))
    }
    pub fn current(&self) -> Option<&review::File> {
        self.file.and_then(|i| self.review.files.get(i))
    }
    pub fn view(&self) -> Option<&FileView> {
        let file = self.file?;
        self.cache.iter().find(|(i, _)| *i == file).map(|(_, v)| v)
    }
    pub fn view_mut(&mut self) -> Option<&mut FileView> {
        let file = self.file?;
        self.cache
            .iter_mut()
            .find(|(i, _)| *i == file)
            .map(|(_, v)| v)
    }
    pub fn ensure_view(&mut self) {
        let Some(file) = self.file else { return };
        if !self.cache.iter().any(|(i, _)| *i == file) {
            // Keep the two most recently selected files, bounding syntax-cache memory.
            if self.cache.len() == 2 {
                self.cache.pop_front();
            }
            self.cache.push_back((
                file,
                FileView::new(&self.review.files[file], self.split_for(file)),
            ));
        } else if let Some(pos) = self.cache.iter().position(|(i, _)| *i == file) {
            let entry = self.cache.remove(pos).unwrap();
            self.cache.push_back(entry);
        }
        if !self.split() {
            self.side = 0;
        }
    }
    pub fn expand_context(&mut self, full: bool) -> Result<()> {
        self.ensure_view();
        let file = self.file.context("no selected file")?;
        let view = self.view().unwrap();
        let collapse = full && view.expanded.is_some();
        if !full && view.expanded.is_some() && view.context_visible.is_none() {
            self.status = "All context is already visible · Z: collapse".into();
            return Ok(());
        }
        let display = view
            .expanded
            .as_ref()
            .and_then(|_| view.display_source(self.cursor[0], self.side));
        let source = view.source(self.cursor[0], self.side).or_else(|| {
            (self.cursor[0]..view.len())
                .chain((0..self.cursor[0]).rev())
                .find_map(|row| view.source(row, self.side))
        });
        let anchor = self
            .anchor
            .and_then(|(row, side)| Some((view.source(row, side)?, side)));
        let screen_row = self.cursor[0].saturating_sub(self.offset);
        let f = &self.review.files[file];
        if collapse || view.expanded.is_none() {
            let next = if collapse {
                FileView::new(f, self.split())
            } else {
                FileView::expand(f, review::expand_file(&self.review, f)?, self.split())?
            };
            *self.view_mut().unwrap() = next;
        }
        let split = self.split();
        let view = &mut self.cache.iter_mut().find(|(i, _)| *i == file).unwrap().1;
        view.layout(self.diff_inner.width as usize, self.bias, self.wrap);
        let display = if collapse { None } else { display }.or_else(|| {
            source
                .and_then(|i| view.visual_for_source(i, self.side))
                .and_then(|row| view.display_source(row, self.side))
        });
        if full {
            view.context_visible = None;
        } else {
            view.expand_near(display.unwrap_or(0));
        }
        view.rebuild_rows(&self.review.files[file], split);
        view.layout(self.diff_inner.width as usize, self.bias, self.wrap);
        self.cursor[0] = display
            .and_then(|i| view.visual_for_display(i, self.side))
            .unwrap_or(0);
        self.anchor = anchor.and_then(|(i, side)| Some((view.visual_for_source(i, side)?, side)));
        self.offset = self.cursor[0].saturating_sub(screen_row);
        self.status = if collapse {
            "Diff context restored"
        } else if full {
            "Full file · Z: collapse · expanded context is read-only"
        } else {
            "Context expanded · z: show 10 more nearby lines · Z: collapse"
        }
        .into();
        Ok(())
    }
    pub fn layout_diff(&mut self, width: usize) {
        self.ensure_view();
        let (wrap, bias, selection) = (self.wrap, self.bias, self.cursor[0]);
        let anchor = self.view().and_then(|v| v.locate(selection));
        let range_anchors = [self.anchor, self.drag_start].map(|anchor| {
            anchor.and_then(|(visual, side)| Some((self.view()?.locate(visual)?, side)))
        });
        let editor_anchor = self
            .editor
            .as_ref()
            .and_then(|e| self.view()?.locate(e.after));
        if let Some(view) = self.view_mut() {
            view.layout(width, bias, wrap);
        }
        if let (Some((row, part)), Some(view)) = (anchor, self.view())
            && row + 1 < view.starts.len()
        {
            self.cursor[0] =
                view.starts[row] + part.min(view.starts[row + 1] - view.starts[row] - 1);
        }
        if let Some(view) = self.view() {
            self.cursor[0] = self.cursor[0].min(view.len().saturating_sub(1));
        }
        // Selection and an in-progress drag must follow the same source rows as the cursor.
        [self.anchor, self.drag_start] = range_anchors.map(|anchor| {
            let ((row, part), side) = anchor?;
            let view = self.view()?;
            Some((
                view.starts[row] + part.min(view.starts[row + 1] - view.starts[row] - 1),
                side,
            ))
        });
        let after = editor_anchor.and_then(|(row, part)| {
            let v = self.view()?;
            Some(v.starts[row] + part.min(v.starts[row + 1] - v.starts[row] - 1))
        });
        if let Some(editor) = &mut self.editor {
            editor.selection = self.cursor[0];
            if let Some(after) = after {
                editor.after = after;
            }
        }
    }
    pub fn select_file(&mut self, file: Option<usize>) {
        if self.file == file {
            return;
        }
        self.file = file;
        self.manual_scroll[0] = false;
        if let Some(pos) = self
            .tree_rows
            .iter()
            .position(|row| matches!(row, FileRow::File(i) if Some(*i) == file))
        {
            self.cursor[1] = pos;
        }
        self.cursor[0] = 0;
        self.offset = 0;
        self.anchor = None;
        self.inspection = None;
        self.ensure_view();
        self.rebuild_comments();
    }
    pub fn rebuild_lists(&mut self) {
        let directory = match self.tree_rows.get(self.cursor[1]) {
            Some(FileRow::Directory(path))
                if self.queries[1].is_empty() && !self.commented_files_only =>
            {
                Some(path.clone())
            }
            _ => None,
        };
        self.total_changes = self
            .review
            .files
            .iter()
            .flat_map(|f| &f.lines)
            .filter(|l| (l.old > 0) != (l.new > 0))
            .count();
        self.file_rows.clear();
        self.tree_rows.clear();
        self.labels[1].clear();
        let mut counts = HashMap::<&str, usize>::new();
        for f in self.review.files.iter().chain(&self.review.history) {
            if !f.comments.is_empty() {
                counts.entry(&f.path).or_default();
            }
            for c in &f.comments {
                if !c.done {
                    *counts.entry(&f.path).or_default() += 1;
                }
            }
        }
        for (i, f) in self.review.files.iter().enumerate() {
            if !f
                .path
                .to_lowercase()
                .contains(&self.queries[1].to_lowercase())
                || self.commented_files_only
                    && counts
                        .get(f.path.as_str())
                        .is_none_or(|open| self.open_only && *open == 0)
            {
                continue;
            }
            self.file_rows.push(i);
        }
        let filtered = !self.queries[1].is_empty() || self.commented_files_only;
        let mut files = self.file_rows.clone();
        files.sort_by(|a, b| {
            self.review.files[*a]
                .path
                .split('/')
                .cmp(self.review.files[*b].path.split('/'))
        });
        let mut directories = HashSet::new();
        for i in files {
            let f = &self.review.files[i];
            let mut hidden = false;
            let mut depth = 0;
            for (end, _) in f.path.match_indices('/') {
                let path = &f.path[..end];
                if directories.insert(path.to_owned()) {
                    self.tree_rows.push(FileRow::Directory(path.to_owned()));
                    self.labels[1].push(Line::from(format!(
                        "{}{} {}/",
                        "  ".repeat(depth),
                        if !filtered && self.collapsed_dirs.contains(path) {
                            "▶"
                        } else {
                            "▼"
                        },
                        path.rsplit('/').next().unwrap_or(path)
                    )));
                }
                depth += 1;
                if !filtered && self.collapsed_dirs.contains(path) {
                    hidden = true;
                    break;
                }
            }
            if hidden {
                continue;
            }
            self.tree_rows.push(FileRow::File(i));
            let added = f.lines.iter().filter(|l| l.new > 0 && l.old == 0).count();
            let removed = f.lines.iter().filter(|l| l.old > 0 && l.new == 0).count();
            let open = counts.get(f.path.as_str()).copied().unwrap_or(0);
            let status = format!(
                "{:>2} ",
                self.review.statuses.get(&f.path).map_or("", String::as_str)
            );
            let mut spans = vec![Span::raw("  ".repeat(depth))];
            spans.extend(status.chars().map(|ch| {
                let color = match ch {
                    'A' => Color::LightGreen,
                    'M' | 'T' => Color::Rgb(255, 255, 0),
                    '?' | 'D' => Color::Rgb(255, 0, 0),
                    'R' | 'C' => Color::Rgb(0, 255, 255),
                    'U' => Color::Rgb(255, 0, 255),
                    _ => Color::Reset,
                };
                Span::styled(ch.to_string(), Style::default().fg(color))
            }));
            spans.extend([
                Span::styled(
                    f.path.rsplit('/').next().unwrap_or(&f.path).to_owned(),
                    if open > 0 {
                        Style::default().fg(Color::Rgb(255, 255, 0))
                    } else {
                        Style::default()
                    },
                ),
                Span::raw(if open > 0 {
                    format!(" · {open} open")
                } else {
                    String::new()
                }),
                Span::raw("  "),
                Span::styled(format!("+{added}"), Style::default().fg(Color::LightGreen)),
                Span::raw(" "),
                Span::styled(
                    format!("-{removed}"),
                    Style::default().fg(Color::Rgb(255, 0, 0)),
                ),
            ]);
            self.labels[1].push(Line::from(spans));
        }
        if !self.file.is_some_and(|i| self.file_rows.contains(&i)) {
            self.file = self.file_rows.first().copied();
            self.cursor[0] = 0;
            self.offset = 0;
        }
        self.cursor[1] = directory
            .and_then(|path| self.tree_rows.iter().position(|row| matches!(row, FileRow::Directory(dir) if *dir == path)))
            .or_else(|| self.tree_rows.iter().position(|row| matches!(row, FileRow::File(i) if Some(*i) == self.file)))
            .or_else(|| {
                let path = &self.review.files.get(self.file?)?.path;
                self.tree_rows.iter().rposition(|row| matches!(row, FileRow::Directory(dir) if path.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'))))
            })
            .unwrap_or(0);
        self.rebuild_comments();
        self.rebuild_commits();
    }
    fn set_directory_expanded(&mut self, expand: Option<bool>) -> bool {
        let Some(FileRow::Directory(path)) = self.tree_rows.get(self.cursor[1]) else {
            return false;
        };
        if !self.queries[1].is_empty() || self.commented_files_only {
            if expand != Some(true) {
                self.status = "Clear Files filters to collapse directories".into();
            }
            return true;
        }
        let path = path.clone();
        if expand.unwrap_or_else(|| self.collapsed_dirs.contains(&path)) {
            self.collapsed_dirs.remove(&path);
        } else {
            self.collapsed_dirs.insert(path);
        }
        self.manual_scroll[1] = false;
        self.rebuild_lists();
        true
    }
    pub fn rebuild_comments(&mut self) {
        self.refs.clear();
        self.labels[2].clear();
        let current = self.current().map(|f| f.path.clone());
        for history in [false, true] {
            let files = if history {
                &self.review.history
            } else {
                &self.review.files
            };
            let indices: Vec<usize> = if history {
                (0..files.len()).rev().collect()
            } else {
                (0..files.len()).collect()
            };
            for file in indices {
                let f = &files[file];
                for (comment, c) in f.comments.iter().enumerate() {
                    if self.file_only && Some(&f.path) != current.as_ref()
                        || self.open_only && c.done
                        || !format!("{}\n{}", f.path, c.text)
                            .to_lowercase()
                            .contains(&self.queries[2].to_lowercase())
                    {
                        continue;
                    }
                    self.refs.push(CommentRef {
                        history,
                        file,
                        comment,
                    });
                    self.labels[2].push(Line::from(vec![
                        Span::raw("["),
                        Span::styled(
                            if c.done { "Done" } else { "Open" },
                            Style::default().fg(if c.done {
                                Color::LightGreen
                            } else {
                                Color::Yellow
                            }),
                        ),
                        Span::raw("/"),
                        Span::styled(
                            if c.sent { "Sent" } else { "Unsent" },
                            Style::default().fg(if c.sent { Color::Cyan } else { Color::LightRed }),
                        ),
                        Span::raw(format!(
                            "]{} {} · {}{}",
                            if c.sent { "  " } else { "" },
                            c.text.split_whitespace().collect::<Vec<_>>().join(" "),
                            if history { "history · " } else { "" },
                            f.path,
                        )),
                    ]));
                }
            }
        }
        self.cursor[2] = self.cursor[2].min(self.refs.len().saturating_sub(1));
    }
    pub fn rebuild_commits(&mut self) {
        self.labels[3].clear();
        self.commit_rows.clear();
        let query = self.queries[3].to_lowercase();
        if query.is_empty() {
            self.labels[3].push("Working tree (clear selection)".into());
            self.commit_rows.push(None);
        }
        for (i, c) in self.commits.iter().enumerate() {
            if !format!("{} {} {}", c.id, c.subject, c.refs)
                .to_lowercase()
                .contains(&query)
            {
                continue;
            }
            self.commit_rows.push(Some(i));
            self.labels[3].push(Line::from(vec![
                Span::raw(if c.id == self.review.base || c.id == self.review.target {
                    "[x] "
                } else {
                    "[ ] "
                }),
                Span::styled(
                    if query.is_empty() {
                        self.graphs.get(i).cloned().unwrap_or_default()
                    } else {
                        String::new()
                    },
                    Style::default().fg(Color::Rgb(0, 255, 255)),
                ),
                Span::raw(format!(" {}", &c.id[..c.id.len().min(8)])),
                Span::styled(
                    if c.refs.is_empty() {
                        String::new()
                    } else {
                        format!(" {}", c.refs)
                    },
                    Style::default().fg(Color::Rgb(255, 255, 0)),
                ),
                Span::raw(format!(" {}", c.subject)),
            ]));
        }
        self.cursor[3] = self.cursor[3].min(self.commit_rows.len().saturating_sub(1));
    }
    pub fn comment(&self, r: CommentRef) -> (&review::File, &Comment) {
        let f = &if r.history {
            &self.review.history
        } else {
            &self.review.files
        }[r.file];
        (f, &f.comments[r.comment])
    }
    pub fn selected_ref(&self) -> Option<CommentRef> {
        if self.pane == 2 {
            return self.refs.get(self.cursor[2]).copied();
        }
        let file = self.file?;
        let view = self.view()?;
        if let Some((r, _)) = view.locate(self.cursor[0])
            && let Row::Comment { index, .. } = view.rows[r]
        {
            return Some(CommentRef {
                history: false,
                file,
                comment: index,
            });
        }
        let source = view.display_source(self.cursor[0], self.side)?;
        view.comments[source][self.side].map(|comment| CommentRef {
            history: false,
            file,
            comment,
        })
    }
    pub fn focus(&mut self, pane: usize) {
        self.pane = pane;
        if pane > 0 {
            self.left_pane = pane;
        }
        if pane == 2 {
            self.preview_comment();
        } else {
            self.inspection = None;
        }
    }
    pub fn move_pane(&mut self, step: isize) {
        let count = if self.split() { 5 } else { 4 };
        let pos = if self.pane == 0 {
            3 + if self.split() { self.side } else { 0 }
        } else {
            self.pane - 1
        };
        let next = (pos as isize + step).rem_euclid(count) as usize;
        if next >= 3 {
            let side = next - 3;
            if self.side != side {
                self.anchor = None;
            }
            self.side = side;
            self.focus(0);
        } else {
            self.focus(next + 1);
        }
    }
    pub fn bounds(&self) -> Option<(usize, usize)> {
        let view = self.view()?;
        let (anchor, side) = self.anchor.unwrap_or((self.cursor[0], self.side));
        if side != self.side {
            return None;
        }
        let range = anchor.min(self.cursor[0])..=anchor.max(self.cursor[0]);
        let start = range.clone().find_map(|v| view.source(v, self.side))?;
        let end = range
            .rev()
            .find_map(|v| view.source(v, self.side))
            .unwrap_or(start);
        Some((start.min(end), start.max(end)))
    }
    pub fn move_selection(&mut self, delta: isize, absolute: Option<usize>) {
        self.manual_scroll[self.pane] = false;
        if self.pane > 0 {
            let max = self.labels[self.pane].len().saturating_sub(1);
            self.cursor[self.pane] = absolute
                .unwrap_or_else(|| self.cursor[self.pane].saturating_add_signed(delta))
                .min(max);
            if self.pane == 1
                && let Some(FileRow::File(i)) = self.tree_rows.get(self.cursor[1])
            {
                self.select_file(Some(*i));
            }
            if self.pane == 2 {
                self.preview_comment();
            }
            return;
        }
        let Some(view) = self.view() else { return };
        let max = view.len().saturating_sub(1);
        let target = absolute
            .unwrap_or_else(|| self.cursor[0].saturating_add_signed(delta))
            .min(max);
        let step = if delta < 0 { -1 } else { 1 };
        let mut i = target;
        loop {
            if view.selectable(i, self.side) {
                self.cursor[0] = i;
                return;
            }
            if i == 0 && step < 0 || i >= max && step > 0 {
                break;
            }
            i = i.saturating_add_signed(step);
        }
        let mut i = target;
        while i != self.cursor[0] {
            if view.selectable(i, self.side) {
                self.cursor[0] = i;
                return;
            }
            i = i.saturating_add_signed(-step);
            if i > max {
                break;
            }
        }
        if delta > 0
            && absolute.is_none()
            && self.split()
            && (self.cursor[0] + 1..view.len()).any(|row| view.selectable(row, 1 - self.side))
        {
            self.side = 1 - self.side;
            self.anchor = None;
            self.move_selection(delta, None);
        }
    }
    pub fn apply(&mut self, next: Review) -> Result<()> {
        let source = self
            .view()
            .and_then(|v| v.source(self.cursor[0], self.side));
        let inline = self
            .view()
            .and_then(|v| v.locate(self.cursor[0]).map(|(r, _)| v.rows[r]))
            .filter(|r| matches!(r, Row::Comment { .. }));
        next.save(&self.state)?;
        let changed = !self.review.same_diff(&next);
        let selected_path = self.current().map(|file| file.path.clone());
        self.review = next;
        if changed {
            self.file = selected_path
                .and_then(|path| self.review.files.iter().position(|file| file.path == path))
                .or_else(|| (!self.review.files.is_empty()).then_some(0));
        }
        self.after_save(changed, source, inline);
        Ok(())
    }
    fn after_save(&mut self, changed: bool, source: Option<usize>, inline: Option<Row>) {
        self.anchor = None;
        if changed {
            self.cache.clear();
            self.cursor[0] = 0;
            self.offset = 0;
        } else {
            for (i, view) in &mut self.cache {
                view.rebuild_rows(&self.review.files[*i], self.review.split_file(*i));
            }
        }
        self.rebuild_lists();
        self.ensure_view();
        self.layout_diff(self.diff_inner.width as usize);
        if !changed && let Some(view) = self.view() {
            if let Some(source) = source {
                if let Some(row) = view.visual_for_source(source, self.side) {
                    self.cursor[0] = row;
                }
            } else if let Some(inline) = inline
                && let Some(row) = view.rows.iter().position(|r| *r == inline)
            {
                self.cursor[0] = view.starts[row];
            }
        }
        self.status = format!("Saved · {} comments", self.review.count());
    }
    pub fn toggle_done(&mut self, r: CommentRef) -> Result<()> {
        let source = self
            .view()
            .and_then(|v| v.source(self.cursor[0], self.side));
        let inline = self
            .view()
            .and_then(|v| v.locate(self.cursor[0]).map(|(r, _)| v.rows[r]))
            .filter(|r| matches!(r, Row::Comment { .. }));
        let files = if r.history {
            &mut self.review.history
        } else {
            &mut self.review.files
        };
        files[r.file].comments[r.comment].done = !files[r.file].comments[r.comment].done;
        if let Err(error) = self.review.save(&self.state) {
            let files = if r.history {
                &mut self.review.history
            } else {
                &mut self.review.files
            };
            files[r.file].comments[r.comment].done = !files[r.file].comments[r.comment].done;
            return Err(error);
        }
        self.after_save(false, source, inline);
        self.review.cleanup_queued(&self.state)?;
        self.preview_comment();
        Ok(())
    }
    pub fn inspect(&self, r: CommentRef) -> Inspection {
        let (f, c) = self.comment(r);
        let mut recorded = format!(
            "[{}/{}]\n{}\n{}\n--------------------\n{}",
            if c.done { "Done" } else { "Open" },
            if c.sent { "Sent" } else { "Unsent" },
            Path::new(&self.review.root).join(&f.path).display(),
            c.location(&f.lines),
            c.text
        );
        use std::fmt::Write;
        let excerpt = c.excerpt(&f.lines);
        if !excerpt.is_empty() {
            recorded.push_str("\n\nRecorded excerpt (old/new):\n");
        }
        for i in excerpt {
            let l = &f.lines[i];
            let _ = writeln!(
                recorded,
                "{} {}/{} {}",
                if c.includes(i, l) { "*" } else { " " },
                l.old,
                l.new,
                l.text
            );
        }
        let current =
            review::current_code(&self.review.root, &f.path).unwrap_or_else(|e| e.to_string());
        let line = f
            .lines
            .get(c.start)
            .map_or(0, |l| if l.new > 0 { l.new } else { l.old });
        Inspection {
            reference: r,
            recorded,
            current,
            offsets: [0, line.saturating_sub(4)],
            pane: 0,
        }
    }
    pub fn preview_comment(&mut self) {
        if self.pane != 2 {
            return;
        }
        self.inspection = None;
        let Some(r) = self.refs.get(self.cursor[2]).copied() else {
            return;
        };
        let (_, c) = self.comment(r);
        let c = c.clone();
        if r.history || c.file {
            self.inspection = Some(self.inspect(r));
            return;
        }
        self.file = Some(r.file);
        if let Some(i) = self
            .tree_rows
            .iter()
            .position(|row| *row == FileRow::File(r.file))
        {
            self.cursor[1] = i;
        }
        self.ensure_view();
        self.side = c.display_side(&self.review.files[r.file].lines);
        self.anchor = None;
        self.layout_diff(self.diff_inner.width as usize);
        if let Some(view) = self.view()
            && let Some(row) = (0..view.len()).find(|row| {
                view.source(*row, self.side)
                    .is_some_and(|i| c.includes(i, &self.review.files[r.file].lines[i]))
            })
        {
            self.cursor[0] = row;
            self.offset = row.saturating_sub(3);
        }
    }
    pub fn start_edit(&mut self, reference: Option<CommentRef>, file_wide: bool) -> Result<()> {
        let return_pane = self.pane;
        if let Some(r) = reference {
            ensure!(!r.history, "historical comments are read-only");
            self.queries[1].clear();
            self.select_file(Some(r.file));
            self.rebuild_lists();
        }
        let f = self.current().context("no selected file")?;
        let comment = if let Some(r) = reference {
            f.comments[r.comment].clone()
        } else if file_wide {
            Comment {
                file: true,
                ..Comment::default()
            }
        } else {
            let (start, end) = self
                .bounds()
                .context("select a line in the original diff; expanded context is read-only")?;
            ensure!(
                f.lines[start..=end].iter().any(|l| l.old > 0 || l.new > 0),
                "select a source line inside a diff hunk"
            );
            Comment {
                start,
                end,
                side: if self.split() {
                    ["old", "new"][self.side].into()
                } else {
                    String::new()
                },
                ..Comment::default()
            }
        };
        let mut after = self.cursor[0];
        if !comment.file
            && let Some(view) = self.view()
        {
            let side = if self.split() {
                comment.display_side(&f.lines)
            } else {
                self.side
            };
            for row in 0..view.len() {
                if view
                    .source(row, side)
                    .is_some_and(|i| comment.includes(i, &f.lines[i]))
                {
                    after = row;
                }
            }
        }
        let input = TextArea::new(comment.text.split('\n').map(String::from).collect());
        self.editor = Some(Editor {
            input,
            reference,
            comment,
            after,
            return_pane,
            selection: self.cursor[0],
            offset: self.offset,
        });
        self.pane = 0;
        Ok(())
    }
    pub fn close_editor(&mut self) {
        if let Some(editor) = self.editor.take() {
            self.cursor[0] = editor.selection;
            self.offset = editor.offset;
            self.focus(editor.return_pane);
        }
    }
    pub fn save_editor(&mut self) -> Result<()> {
        let editor = self.editor.as_ref().context("no editor")?;
        let text = editor.input.lines().join("\n").trim().to_owned();
        if text.is_empty() {
            return Ok(());
        }
        let mut comment = editor.comment.clone();
        comment.text = text;
        comment.sent = false;
        comment.done = false;
        comment.delivery.clear();
        let mut next = self.review.clone();
        let file = self.file.context("no file")?;
        if let Some(r) = editor.reference {
            let old = next.files[file].comments[r.comment].clone();
            if (old.sent || old.done) && old != comment {
                next.history.push(review::File {
                    path: next.files[file].path.clone(),
                    lines: next.files[file].lines.clone(),
                    comments: vec![old],
                    patch: String::new(),
                });
            }
            next.files[file].comments[r.comment] = comment;
        } else {
            next.files[file].comments.push(comment);
        }
        self.apply(next)?;
        self.close_editor();
        Ok(())
    }
    pub fn remove_comment(&mut self, r: CommentRef) -> Result<()> {
        let mut next = self.review.clone();
        let f = &mut if r.history {
            &mut next.history
        } else {
            &mut next.files
        }[r.file];
        f.comments.remove(r.comment);
        self.apply(next)?;
        self.status = "Comment permanently deleted".into();
        self.preview_comment();
        Ok(())
    }
    pub fn filter(&mut self, pane: usize, query: String) {
        self.queries[pane] = query;
        self.anchor = None;
        match pane {
            1 => {
                let previous = self.file;
                self.rebuild_lists();
                if self.file != previous {
                    self.cursor[0] = 0;
                    self.offset = 0;
                }
                self.ensure_view();
            }
            2 => {
                self.rebuild_comments();
                self.preview_comment();
            }
            3 => self.rebuild_commits(),
            _ => self.find_match(1, true),
        }
    }
    pub fn find_match(&mut self, step: isize, include_current: bool) {
        if self.pane > 0 {
            let count = self.labels[self.pane].len();
            if count > 0 && !include_current {
                if self.pane == 1 {
                    for offset in 1..=count {
                        let row = (self.cursor[1] as isize + step * offset as isize)
                            .rem_euclid(count as isize) as usize;
                        if matches!(self.tree_rows[row], FileRow::File(_)) {
                            self.move_selection(0, Some(row));
                            break;
                        }
                    }
                    return;
                }
                self.move_selection(
                    0,
                    Some(
                        (self.cursor[self.pane] as isize + step).rem_euclid(count as isize)
                            as usize,
                    ),
                );
            }
            return;
        }
        let query = self.queries[0].to_lowercase();
        if query.is_empty() {
            return;
        }
        let Some(view) = self.view() else { return };
        let sides = if self.split() { 2 } else { 1 };
        let count = view.len() * sides;
        if count == 0 {
            return;
        }
        let current = self.cursor[0] * sides + if sides == 2 { self.side } else { 0 };
        for offset in usize::from(!include_current)..count + usize::from(!include_current) {
            let index =
                (current as isize + step * offset as isize).rem_euclid(count as isize) as usize;
            let (row, side) = (index / sides, index % sides);
            if let Some(i) = view.display_source(row, side)
                && view.code[i].text.to_lowercase().contains(&query)
            {
                self.cursor[0] = row;
                self.side = side;
                self.status = format!("Search: {}", self.queries[0]);
                return;
            }
        }
        self.status = format!("No matches: {}", self.queries[0]);
    }
    pub fn move_hunk(&mut self, step: isize) {
        let Some(view) = self.view() else { return };
        let starts = &view.hunk_rows[self.side];
        if starts.is_empty() {
            return;
        }
        let i = if step > 0 {
            starts.partition_point(|r| view.starts[*r] <= self.cursor[0]) % starts.len()
        } else {
            let i = starts.partition_point(|r| view.starts[*r] < self.cursor[0]);
            if i == 0 { starts.len() - 1 } else { i - 1 }
        };
        self.cursor[0] = view.starts[starts[i]];
        self.anchor = None;
    }
    pub fn refresh(&mut self, archive: bool) -> Result<()> {
        let next = review::snapshot(&self.review.root, &self.review.base, &self.review.target)?;
        self.apply(self.review.refresh(next, archive))?;
        self.commits = review::commits(&self.review.root)?;
        self.graphs = commit_graph(&self.commits);
        self.rebuild_commits();
        self.status = "Diff refreshed; changed comments remain in history".into();
        Ok(())
    }
    pub fn select_commit(&mut self) -> Result<()> {
        let Some(selected) = self.commit_rows.get(self.cursor[3]).copied() else {
            return Ok(());
        };
        let mut ids = Vec::new();
        if let Some(index) = selected {
            ids.extend(
                [&self.review.base, &self.review.target]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .cloned(),
            );
            let id = &self.commits[index].id;
            if let Some(i) = ids.iter().position(|s| s == id) {
                ids.remove(i);
            } else {
                ids.push(id.clone());
            }
            ensure!(
                ids.len() <= 2,
                "select at most two commits; deselect one with Enter/Space"
            );
        }
        let ids: Vec<_> = self
            .commits
            .iter()
            .filter(|c| ids.contains(&c.id))
            .map(|c| c.id.clone())
            .collect();
        let base = ids.last().map_or("", String::as_str);
        let target = if ids.len() == 2 { ids[0].as_str() } else { "" };
        let next = review::snapshot(&self.review.root, base, target)?;
        self.queries[0].clear();
        self.queries[1].clear();
        self.apply(self.review.refresh(next, false))?;
        self.status = "Comparison updated; previous comments remain in history".into();
        Ok(())
    }
    pub fn fresh(&self) -> Result<()> {
        ensure!(
            self.review.same_diff(&review::snapshot(
                &self.review.root,
                &self.review.base,
                &self.review.target
            )?),
            "diff changed since review; press r to refresh before sending"
        );
        Ok(())
    }
    pub fn prompt(&self) -> String {
        let prompt = self.review.prompt();
        if self.language.is_empty() {
            prompt
        } else {
            format!("Respond in {}.\n\n{prompt}", self.language)
        }
    }
    pub fn pending_refs(&self) -> Vec<CommentRef> {
        [(false, &self.review.files), (true, &self.review.history)]
            .into_iter()
            .flat_map(|(history, files)| {
                files.iter().enumerate().flat_map(move |(file, f)| {
                    f.comments
                        .iter()
                        .enumerate()
                        .filter(move |(_, c)| c.sendable(history))
                        .map(move |(comment, _)| CommentRef {
                            file,
                            comment,
                            history,
                        })
                })
            })
            .collect()
    }
    pub fn start_sessions(&mut self) -> Result<()> {
        ensure!(self.review.pending() > 0, "no unsent Open comments to send");
        self.fresh()?;
        self.load_sessions(agent::SessionOptions::default(), TextArea::default(), 0);
        Ok(())
    }
    fn load_sessions(
        &mut self,
        options: agent::SessionOptions,
        input: TextArea<'static>,
        control: usize,
    ) {
        self.close_modal();
        // Keep the initialized client in the completed worker between filter changes.
        let client = self
            .session_worker
            .take()
            .and_then(|worker| worker.join().ok())
            .flatten();
        let root = self.session_cwd.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tx, receiver) = mpsc::channel();
        self.session_worker = Some(thread::spawn(move || {
            let mut client = match client {
                Some(client) => client,
                None => match agent::SessionClient::start(&root, worker_cancel.clone()) {
                    Ok(client) => client,
                    Err(error) => {
                        let _ = tx.send(Err(error));
                        return None;
                    }
                },
            };
            let result = client.sessions(&root, options, worker_cancel);
            let reusable = result.is_ok();
            if tx.send(result).is_ok() && reusable {
                Some(client)
            } else {
                None
            }
        }));
        self.modal = Some(Modal::Loading {
            cancel,
            receiver,
            options,
            input,
            control,
        });
    }
    pub fn poll(&mut self) -> bool {
        let result = if let Some(Modal::Loading { receiver, .. }) = &self.modal {
            receiver.try_recv().ok()
        } else {
            None
        };
        if let Some(result) = result {
            let items = result.unwrap_or_else(|e| {
                self.status = format!(
                    "Session listing failed: {e}; new session and clipboard remain available"
                );
                Vec::new()
            });
            let Some(Modal::Loading {
                options,
                input,
                control,
                ..
            }) = self.modal.take()
            else {
                return false;
            };
            self.modal = Some(Modal::Sessions {
                offset: 0,
                manual_scroll: false,
                area: Rect::default(),
                filter_areas: [[Rect::default(); 2]; 3],
                items,
                input,
                selection: 0,
                search: false,
                options,
                control,
            });
            true
        } else {
            false
        }
    }
    pub fn close_modal(&mut self) {
        if let Some(Modal::Loading { cancel, .. }) = self.modal.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    pub fn reset_all(&mut self) -> Result<()> {
        let mut next = review::snapshot(&self.review.root, &self.review.base, &self.review.target)?;
        next.split = self.review.split;
        self.apply(next)?;
        for entry in fs::read_dir(self.state.parent().context("no state directory")?)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if review::review_filename(&name)
                || name.starts_with(".annodiff-")
                || name.starts_with(".anno-diff-")
            {
                review::remove_if_exists(&entry.path())?;
            }
        }
        self.status = "Reset comments, history and temporary review files".into();
        Ok(())
    }
    pub fn handle(&mut self, event: Event) -> Result<Effect> {
        if matches!(event, Event::Resize(..)) {
            self.pane_drag = None;
        }
        if let Event::Paste(text) = &event {
            if let Some(editor) = &mut self.editor {
                editor.input.insert_str(text);
                return Ok(Effect::None);
            }
            if let Some(
                Modal::Search { input, .. }
                | Modal::Sessions { input, .. }
                | Modal::Help {
                    input,
                    search: true,
                    ..
                },
            ) = &mut self.modal
            {
                input.insert_str(text.replace(['\n', '\r'], " "));
            }
            if let Some(Modal::Search { pane, input, .. }) = &self.modal {
                let (pane, query) = (*pane, input.lines().join(" "));
                self.filter(pane, query);
            }
            if let Some(Modal::Sessions {
                selection,
                offset,
                manual_scroll,
                ..
            }) = &mut self.modal
            {
                *selection = 0;
                *offset = 0;
                *manual_scroll = false;
            }
            return Ok(Effect::None);
        }
        if let Event::Mouse(mouse) = event {
            if let Some(Modal::Sessions {
                items,
                input,
                options,
                offset,
                manual_scroll,
                area,
                filter_areas,
                control,
                search,
                selection,
            }) = &mut self.modal
            {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                    && let Some(index) = filter_areas
                        .iter()
                        .flatten()
                        .position(|r| r.contains((mouse.column, mouse.row).into()))
                {
                    *control = index / 2;
                    *search = false;
                    let current = [options.all, options.archived, options.created][*control];
                    return if current == (index % 2 == 1) {
                        Ok(Effect::None)
                    } else {
                        self.handle_modal(KeyEvent::new(K::Right, M::NONE))
                    };
                }
                if area.contains((mouse.column, mouse.row).into()) {
                    let query = input.lines().join(" ").to_lowercase();
                    let len = 2 + items
                        .iter()
                        .filter(|s| s.matches(!options.all, &query))
                        .count();
                    match mouse.kind {
                        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                            let delta = if mouse.kind == MouseEventKind::ScrollDown {
                                3
                            } else {
                                -3
                            };
                            *offset = offset
                                .saturating_add_signed(delta)
                                .min(len.saturating_sub(area.height as usize));
                            *manual_scroll = true;
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            let index = *offset + usize::from(mouse.row - area.y);
                            if index < len {
                                *selection = index;
                                *search = false;
                                return self.handle_modal(KeyEvent::new(K::Enter, M::NONE));
                            }
                        }
                        _ => {}
                    }
                }
                return Ok(Effect::None);
            }
            if self.editor.is_some() || self.modal.is_some() {
                return Ok(Effect::None);
            }
            let position = if self.stacked {
                mouse.row
            } else {
                mouse.column
            };
            let (origin, extent) = self.pane_axis();
            if matches!(
                mouse.kind,
                MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
            ) {
                if let Some((start, percent)) = self.pane_drag {
                    if self.zoom < 2 && !self.pane_area.is_empty() {
                        let delta = i32::from(position) - i32::from(start);
                        self.set_sidebar_percent(percent + delta * 100 / i32::from(extent));
                    }
                    if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
                        self.pane_drag = None;
                    }
                    return Ok(Effect::None);
                }
                if self.diff_inner.is_empty() {
                    self.drag_start = None;
                    self.divider_drag = None;
                    return Ok(Effect::None);
                }
                if let Some((start_x, bias)) = self.divider_drag {
                    if let Some(view) = self.view() {
                        let available = (view.widths[0] + view.widths[1]).max(1) as i32;
                        let delta = i32::from(mouse.column) - i32::from(start_x);
                        self.set_diff_bias(bias + delta * 100 / available);
                    }
                    if mouse.kind == MouseEventKind::Up(MouseButton::Left) {
                        self.divider_drag = None;
                    }
                    return Ok(Effect::None);
                }
                if let Some((start, side)) = self.drag_start {
                    let y = mouse.row.clamp(
                        self.diff_inner.y,
                        self.diff_inner.bottom().saturating_sub(1),
                    );
                    let target = self.offset + y.saturating_sub(self.diff_inner.y) as usize;
                    self.side = side;
                    self.move_selection(if target < self.cursor[0] { -1 } else { 1 }, Some(target));
                    if self.cursor[0] != start || self.anchor.is_some() {
                        self.anchor = Some((start, side));
                    }
                }
                if mouse.kind == MouseEventKind::Up(MouseButton::Left)
                    && self.drag_start.take().is_some()
                    && self.anchor.is_some()
                {
                    self.start_edit(None, false)?;
                }
                return Ok(Effect::None);
            }
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                self.drag_start = None;
                self.divider_drag = None;
                self.pane_drag = None;
                let sidebar_size = (i32::from(extent) * self.sidebar_percent / 100) as u16;
                let boundary = origin + sidebar_size;
                if self.zoom < 2
                    && sidebar_size > 0
                    && sidebar_size < extent
                    && self.pane_area.contains((mouse.column, mouse.row).into())
                    && (position == boundary || position == boundary - 1)
                    && !self
                        .diff_mode_rects
                        .iter()
                        .any(|r| r.contains((mouse.column, mouse.row).into()))
                {
                    self.pane_drag = Some((position, self.sidebar_percent));
                    return Ok(Effect::None);
                }
                if self.split()
                    && self.diff_inner.contains((mouse.column, mouse.row).into())
                    && self.view().is_some_and(|v| {
                        usize::from(mouse.column - self.diff_inner.x) == v.digits + v.widths[0] + 2
                    })
                {
                    self.focus(0);
                    self.divider_drag = Some((mouse.column, self.bias));
                    return Ok(Effect::None);
                }
                for mode in 0..2 {
                    if self.diff_mode_rects[mode].contains((mouse.column, mouse.row).into()) {
                        self.set_split(mode == 1)?;
                        self.focus(0);
                        return Ok(Effect::None);
                    }
                }
            }
            for pane in 0..4 {
                if self.pane_rects[pane].contains((mouse.column, mouse.row).into()) {
                    match mouse.kind {
                        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                            let delta = if mouse.kind == MouseEventKind::ScrollDown {
                                3
                            } else {
                                -3
                            };
                            let height = self.pane_rects[pane].height.saturating_sub(2) as usize;
                            let len = if pane == 0 {
                                self.view().map_or(0, |v| v.len())
                            } else {
                                self.labels[pane].len()
                            };
                            let offset = if pane == 0 {
                                &mut self.offset
                            } else {
                                &mut self.list_offsets[pane]
                            };
                            *offset = offset
                                .saturating_add_signed(delta)
                                .min(len.saturating_sub(height));
                            self.manual_scroll[pane] = true;
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            let focused = self.pane == pane;
                            self.focus(pane);
                            if pane == 0 {
                                if !self.diff_inner.contains((mouse.column, mouse.row).into()) {
                                    return Ok(Effect::None);
                                }
                                let side = if self.split() {
                                    usize::from(
                                        mouse.column
                                            >= self.diff_inner.x
                                                + self.view().map_or(0, |v| {
                                                    (v.digits + v.widths[0] + 3) as u16
                                                }),
                                    )
                                } else {
                                    0
                                };
                                self.anchor = None;
                                self.side = side;
                                self.move_selection(
                                    1,
                                    Some(
                                        self.offset
                                            + mouse.row.saturating_sub(self.diff_inner.y) as usize,
                                    ),
                                );
                                if self
                                    .view()
                                    .and_then(|v| v.source(self.cursor[0], side))
                                    .is_some()
                                {
                                    self.drag_start = Some((self.cursor[0], side));
                                }
                            } else {
                                if !self.pane_rects[pane]
                                    .inner(ratatui::layout::Margin::new(1, 1))
                                    .contains((mouse.column, mouse.row).into())
                                {
                                    return Ok(Effect::None);
                                }
                                let index = self.list_offsets[pane]
                                    + mouse.row.saturating_sub(self.pane_rects[pane].y + 1)
                                        as usize;
                                if index >= self.labels[pane].len() {
                                    return Ok(Effect::None);
                                }
                                let activate = pane == 3 || focused && self.cursor[pane] == index;
                                self.move_selection(1, Some(index));
                                if activate {
                                    let key = match pane {
                                        1 if matches!(
                                            self.tree_rows.get(index),
                                            Some(FileRow::File(_))
                                        ) =>
                                        {
                                            K::Char('c')
                                        }
                                        2 | 3 => K::Enter,
                                        _ => return Ok(Effect::None),
                                    };
                                    return self.handle(Event::Key(KeyEvent::new(key, M::NONE)));
                                }
                            }
                        }
                        _ => {}
                    }
                    break;
                }
            }
            return Ok(Effect::None);
        }
        let Event::Key(key) = event else {
            return Ok(Effect::None);
        };
        if key.kind == KeyEventKind::Release {
            return Ok(Effect::None);
        }
        self.drag_start = None;
        self.divider_drag = None;
        self.pane_drag = None;
        if self.editor.is_some() {
            match key.code {
                K::Esc => self.close_editor(),
                _ if save_key(key) => self.save_editor()?,
                _ => {
                    self.editor.as_mut().unwrap().input.input(key);
                }
            }
            return Ok(Effect::None);
        }
        if self.modal.is_some() {
            return self.handle_modal(key);
        }
        if key.modifiers.contains(M::CONTROL) && key.code == K::Char('c') {
            return Ok(Effect::Quit);
        }
        if save_key(key) {
            self.start_sessions()?;
            return Ok(Effect::None);
        }
        if key.modifiers.intersects(M::CONTROL | M::ALT) {
            return Ok(Effect::None);
        }
        match key.code {
            K::Char('f' | 'z' | 'Z' | '[' | ']') => self.focus(0),
            K::Char('v' | 'x' | 'd') if self.pane != 2 => self.focus(0),
            K::Char('c' | 'e') if self.pane == 2 || self.pane == 3 => self.focus(0),
            K::Char('n' | 'N')
                if self.pane == 1 && self.queries[1].is_empty() && !self.commented_files_only =>
            {
                self.focus(0)
            }
            _ => {}
        }
        self.manual_scroll[self.pane] = false;
        let code = match key.code {
            K::Char('j') => K::Down,
            K::Char('k') => K::Up,
            K::Char('h') => K::Left,
            K::Char('l') => K::Right,
            c => c,
        };
        let page = self.pane_rects[self.pane].height.saturating_sub(3).max(1) as isize;
        match code {
            K::Char('q') => return Ok(Effect::Quit),
            K::Left => self.move_pane(-1),
            K::Right => self.move_pane(1),
            K::Tab => self.focus(match self.pane {
                1 => 0,
                0 => 2,
                2 => 3,
                _ => 1,
            }),
            K::Char(c @ '0'..='3') => self.focus(c as usize - '0' as usize),
            K::Up => self.move_selection(-1, None),
            K::Down => self.move_selection(1, None),
            K::PageUp => self.move_selection(-page, None),
            K::PageDown => self.move_selection(page, None),
            K::Home => self.move_selection(1, Some(0)),
            K::End => self.move_selection(-1, Some(usize::MAX)),
            K::Char('t') => {
                self.stacked = !self.stacked;
                self.status = if self.stacked {
                    "Stacked layout · 1/2/3: top pane · 0: Diff · t: side-by-side layout"
                } else {
                    "Side-by-side layout · t: stacked layout"
                }
                .into();
            }
            K::Char('+') if self.stacked || self.pane == 0 => self.zoom = 2,
            K::Char('_') if self.stacked || self.pane == 0 => self.zoom = 0,
            K::Char('+') => self.zoom = (self.zoom + 1).min(2),
            K::Char('_') => self.zoom = self.zoom.saturating_sub(1),
            K::Char('{' | '}') if self.zoom < 2 => {
                let step = if code == K::Char('}') { 5 } else { -5 };
                self.set_sidebar_percent(self.sidebar_percent + step);
            }
            K::Char('r') => self.refresh(false)?,
            K::Char('R') => {
                self.modal = Some(Modal::Confirm {
                    action: Confirmation::Reset,
                    choice: 0,
                })
            }
            K::Char('?') => {
                self.modal = Some(Modal::Help {
                    scroll: 0,
                    input: TextArea::default(),
                    search: false,
                })
            }
            K::Char('/') => {
                self.modal = Some(Modal::Search {
                    pane: self.pane,
                    previous: self.queries[self.pane].clone(),
                    file: self.file,
                    cursor: self.cursor,
                    input: TextArea::new(vec![self.queries[self.pane].clone()]),
                })
            }
            K::Esc => {
                self.anchor = None;
                self.filter(self.pane, String::new());
            }
            K::Char('n' | 'N') => {
                let step = if code == K::Char('n') { 1 } else { -1 };
                if self.pane == 0 {
                    self.move_hunk(step);
                } else {
                    self.find_match(step, false);
                }
            }
            K::Char('s') => self.set_split(!self.review.split)?,
            K::Char('z' | 'Z') => self.expand_context(code == K::Char('Z'))?,
            K::Char('f') => {
                self.focus(0);
                self.wrap = !self.wrap;
                self.anchor = None;
            }
            K::Char('[' | ']') => {
                self.focus(0);
                if self.split() {
                    self.set_diff_bias(self.bias + if code == K::Char(']') { 5 } else { -5 });
                }
            }
            K::Char('u') if self.pane == 1 || self.pane == 2 => {
                self.open_only = !self.open_only;
                self.filter(1, self.queries[1].clone());
                self.preview_comment();
                self.status = format!(
                    "Comments: {} · shared by Files and Comments · u: toggle",
                    if self.open_only { "Open" } else { "All" }
                );
            }
            K::Char('o') if self.pane == 1 => {
                self.commented_files_only = !self.commented_files_only;
                self.filter(1, self.queries[1].clone());
                self.status = if self.commented_files_only {
                    if self.open_only {
                        "Files: with Open comments · u: Open/All · o: all files"
                    } else {
                        "Files: with any comments · u: Open/All · o: all files"
                    }
                } else {
                    "Files: all · o: with comments"
                }
                .into();
            }
            K::Char('o') if self.pane == 2 => {
                self.file_only = !self.file_only;
                self.rebuild_comments();
                self.preview_comment();
            }
            K::Char('X') if self.pane == 2 => {
                let mut next = self.review.clone();
                for r in &self.refs {
                    let f = &mut if r.history {
                        &mut next.history
                    } else {
                        &mut next.files
                    }[r.file];
                    f.comments[r.comment].done = true;
                }
                self.apply(next)?;
                self.review.cleanup_queued(&self.state)?;
                self.preview_comment();
            }
            K::Char('x') => {
                if let Some(r) = self.selected_ref() {
                    self.toggle_done(r)?;
                }
            }
            K::Char('d') => {
                if let Some(r) = self.selected_ref() {
                    if self.comment(r).1.done {
                        self.remove_comment(r)?;
                    } else {
                        self.modal = Some(Modal::Confirm {
                            action: Confirmation::Delete(r),
                            choice: 0,
                        });
                    }
                }
            }
            K::Char('v') if self.pane == 2 => {
                if let Some(r) = self.selected_ref() {
                    self.modal = Some(Modal::Inspect(self.inspect(r)));
                }
            }
            K::Char('v') => {
                self.focus(0);
                self.anchor = if self.anchor.is_some() {
                    None
                } else {
                    Some((self.cursor[0], self.side))
                };
            }
            K::Char('c' | 'e')
                if self.pane == 1
                    && matches!(
                        self.tree_rows.get(self.cursor[1]),
                        Some(FileRow::Directory(_))
                    ) =>
            {
                self.status = "Select a file to comment or open it".into();
            }
            K::Char('c') if self.pane != 2 => self.start_edit(None, self.pane == 1)?,
            K::Char('e') if self.pane != 2 => return Ok(Effect::Editor),
            K::Enter => match self.pane {
                1 => {
                    if !self.set_directory_expanded(None) {
                        self.focus(0);
                    }
                }
                3 => self.select_commit()?,
                2 => {
                    if let Some(r) = self.selected_ref() {
                        if r.history {
                            self.modal = Some(Modal::Inspect(self.inspect(r)));
                        } else {
                            self.start_edit(Some(r), false)?;
                        }
                    }
                }
                _ => self.start_edit(self.selected_ref(), false)?,
            },
            K::Char('-' | '=') if self.pane == 1 => {
                self.set_directory_expanded(Some(code == K::Char('=')));
            }
            K::Char(' ') if self.pane == 3 => self.select_commit()?,
            K::Char(' ') if self.pane == 1 => {
                self.set_directory_expanded(None);
            }
            _ => {}
        }
        Ok(Effect::None)
    }
    fn handle_modal(&mut self, key: KeyEvent) -> Result<Effect> {
        let mut modal = self.modal.take().unwrap();
        if key.code == K::Char('q')
            && key.modifiers.is_empty()
            && matches!(
                modal,
                Modal::Loading { .. }
                    | Modal::Sessions { search: false, .. }
                    | Modal::Preview { .. }
            )
        {
            if let Modal::Loading { cancel, .. } = &modal {
                cancel.store(true, Ordering::Relaxed);
            }
            return Ok(Effect::Quit);
        }
        match &mut modal {
            Modal::Help {
                scroll,
                input,
                search,
            } => {
                if *search {
                    match key.code {
                        K::Enter => *search = false,
                        K::Esc => {
                            *input = TextArea::default();
                            *search = false;
                            *scroll = 0;
                        }
                        _ => {
                            input.input(key);
                            *scroll = 0;
                        }
                    }
                } else {
                    match key.code {
                        K::Esc | K::Char('?' | 'q') => return Ok(Effect::None),
                        K::Char('/') => {
                            *search = true;
                            *scroll = 0;
                        }
                        _ => scroll_key(scroll, key, 10),
                    }
                }
            }
            Modal::Loading { cancel, .. } => {
                if key.code == K::Esc {
                    cancel.store(true, Ordering::Relaxed);
                    return Ok(Effect::None);
                }
                if key.code == K::Char('c') {
                    cancel.store(true, Ordering::Relaxed);
                    self.modal = Some(Modal::Preview {
                        destination: "Clipboard".into(),
                        id: String::new(),
                        copy: true,
                        archived: false,
                        list_offset: 0,
                        selection: 0,
                        pane: 0,
                        offsets: [0; 2],
                    });
                    return Ok(Effect::None);
                }
            }
            Modal::Search {
                pane,
                previous,
                file,
                cursor,
                input,
            } => {
                if key.code == K::Esc {
                    self.file = *file;
                    self.filter(*pane, previous.clone());
                    self.cursor = *cursor;
                    self.ensure_view();
                    return Ok(Effect::None);
                }
                if key.code == K::Enter {
                    self.find_match(1, true);
                    return Ok(Effect::None);
                }
                input.input(key);
                self.filter(*pane, input.lines().join(" "));
            }
            Modal::Confirm { action, choice } => {
                let count = if matches!(action, Confirmation::Reset) {
                    3
                } else {
                    2
                };
                match key.code {
                    K::Esc => return Ok(Effect::None),
                    K::Tab | K::Right => *choice = (*choice + 1) % count,
                    K::Left => *choice = (*choice + count - 1) % count,
                    K::Enter => {
                        match action {
                            Confirmation::Delete(r) if *choice == 1 => self.remove_comment(*r)?,
                            Confirmation::Reset if *choice == 1 => self.refresh(true)?,
                            Confirmation::Reset if *choice == 2 => self.reset_all()?,
                            _ => {}
                        }
                        return Ok(Effect::None);
                    }
                    _ => {}
                }
            }
            Modal::Inspect(inspect) => {
                if key.code == K::Esc {
                    self.focus(2);
                    return Ok(Effect::None);
                }
                if key.code == K::Tab {
                    inspect.pane = 1 - inspect.pane;
                } else if key.code == K::Char('x') {
                    let r = inspect.reference;
                    self.toggle_done(r)?;
                    *inspect = self.inspect(r);
                } else {
                    scroll_key(&mut inspect.offsets[inspect.pane], key, 10);
                }
            }
            Modal::Sessions {
                offset,
                manual_scroll,
                area: _,
                filter_areas: _,
                items,
                input,
                selection,
                search,
                options,
                control,
            } => {
                if key.code == K::Esc {
                    return Ok(Effect::None);
                }
                if !*search
                    && key.modifiers.is_empty()
                    && matches!(key.code, K::Char('a' | 'h' | 'l') | K::Left | K::Right)
                {
                    let target = if key.code == K::Char('a') {
                        0
                    } else {
                        *control
                    };
                    match target {
                        0 | 1 => {
                            if target == 0 {
                                options.all = !options.all;
                            } else {
                                options.archived = !options.archived;
                            }
                            self.load_sessions(*options, std::mem::take(input), *control);
                            return Ok(Effect::None);
                        }
                        _ => {
                            options.created = !options.created;
                            options.sort(items);
                        }
                    }
                    *selection = 0;
                    *offset = 0;
                    *manual_scroll = false;
                }
                let query = input.lines().join(" ").to_lowercase();
                let filtered: Vec<_> = items
                    .iter()
                    .filter(|s| s.matches(!options.all, &query))
                    .collect();
                if key.code == K::Enter || save_key(key) {
                    let copy = *selection == 1;
                    let id = if *selection > 1 {
                        filtered
                            .get(*selection - 2)
                            .map_or(String::new(), |s| s.id.clone())
                    } else {
                        String::new()
                    };
                    let destination = if copy {
                        "Clipboard".into()
                    } else if *selection > 1 {
                        filtered
                            .get(*selection - 2)
                            .map_or("New session".into(), |s| {
                                format!(
                                    "{}\nSession: {}\nDirectory: {}{}",
                                    s.title(),
                                    s.id,
                                    s.cwd,
                                    if options.archived {
                                        "\nArchived session: restored when you confirm sending"
                                    } else {
                                        ""
                                    }
                                )
                            })
                    } else {
                        "New session".into()
                    };
                    self.modal = Some(Modal::Preview {
                        destination,
                        id,
                        copy,
                        archived: options.archived && *selection > 1,
                        list_offset: 0,
                        selection: 0,
                        pane: 0,
                        offsets: [0; 2],
                    });
                    return Ok(Effect::None);
                }
                if key.code == K::Tab || key.code == K::BackTab {
                    if *search {
                        *search = false;
                    } else {
                        *control = (*control + if key.code == K::BackTab { 2 } else { 1 }) % 3;
                    }
                } else if *search {
                    input.input(key);
                    *selection = 0;
                    *offset = 0;
                    *manual_scroll = false;
                } else {
                    match key.code {
                        K::Down | K::Char('j') => {
                            *manual_scroll = false;
                            *selection = (*selection + 1).min(filtered.len() + 1)
                        }
                        K::Up | K::Char('k') => {
                            *manual_scroll = false;
                            *selection = selection.saturating_sub(1);
                        }
                        K::Char('/') => *search = true,
                        _ => {}
                    }
                }
            }
            Modal::Preview {
                destination: _,
                id,
                copy,
                archived,
                list_offset: _,
                selection,
                pane,
                offsets,
            } => {
                if key.code == K::Esc {
                    return Ok(Effect::None);
                }
                if key.code == K::Enter || save_key(key) {
                    ensure!(self.review.pending() > 0, "no pending comments");
                    self.fresh()?;
                    return Ok(if *copy {
                        Effect::Clipboard(self.prompt())
                    } else {
                        Effect::Send {
                            id: id.clone(),
                            archived: *archived,
                        }
                    });
                }
                if key.code == K::Tab {
                    *pane = (*pane + 1) % 3;
                } else if *pane == 0 {
                    match key.code {
                        K::Down | K::Char('j') => {
                            *selection =
                                (*selection + 1).min(self.review.pending().saturating_sub(1));
                            *offsets = [0; 2];
                        }
                        K::Up | K::Char('k') => {
                            *selection = selection.saturating_sub(1);
                            *offsets = [0; 2];
                        }
                        _ => {}
                    }
                } else {
                    scroll_key(&mut offsets[*pane - 1], key, 10);
                }
            }
        }
        self.modal = Some(modal);
        Ok(Effect::None)
    }
    pub fn draw(&mut self, frame: &mut Frame) {
        crate::render::draw(self, frame);
    }
}
pub fn save_key(key: KeyEvent) -> bool {
    // Some terminals send Ctrl+Enter as LF, decoded as Ctrl+J in raw mode.
    key.code == K::F(2)
        || matches!(key.code, K::Enter | K::Char('j')) && key.modifiers == M::CONTROL
}
fn scroll_key(scroll: &mut usize, key: KeyEvent, page: usize) {
    match key.code {
        K::Down | K::Char('j') => *scroll = scroll.saturating_add(1),
        K::Up | K::Char('k') => *scroll = scroll.saturating_sub(1),
        K::PageDown => *scroll = scroll.saturating_add(page),
        K::PageUp => *scroll = scroll.saturating_sub(page),
        K::Home => *scroll = 0,
        _ => {}
    }
}
