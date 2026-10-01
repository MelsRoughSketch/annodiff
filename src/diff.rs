use crate::review::{Commit, File, Line};
use anyhow::{Result, ensure};
use ratatui::style::{Color, Style};
use std::{
    collections::HashMap,
    ops::Range,
    sync::{LazyLock, OnceLock},
    time::{Duration, Instant},
};
use syntect::{easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

#[derive(Clone, Debug)]
pub struct CodeLine {
    pub text: String,
    pub width: usize,
    // Build wrapping metrics only when wrapping a long line; reuse on resize.
    units: OnceLock<Vec<(usize, usize, bool)>>,
}
impl CodeLine {
    pub fn new(text: &str) -> Self {
        let text = text.replace('\t', "    ");
        let width = text.width();
        Self {
            text,
            width,
            units: OnceLock::new(),
        }
    }
    pub fn wrap(&self, width: usize) -> Vec<Range<usize>> {
        let mut parts = Vec::new();
        self.wrap_into(width, &mut parts);
        parts
    }
    fn wrap_into(&self, width: usize, parts: &mut Vec<Range<usize>>) {
        parts.clear();
        let width = width.max(1);
        if self.width <= width {
            parts.push(0..self.text.len());
            return;
        }
        let units = self.units.get_or_init(|| {
            let mut total = 0;
            let mut units = vec![(0, 0, false)];
            for (byte, grapheme) in self.text.grapheme_indices(true) {
                total += grapheme.width();
                units.push((
                    byte + grapheme.len(),
                    total,
                    grapheme.chars().all(char::is_whitespace),
                ));
            }
            units
        });
        let mut start = 0;
        while start + 1 < units.len() {
            let mut end = start + 1;
            let mut word = None;
            while end < units.len() && units[end].1 - units[start].1 <= width {
                if units[end].2 {
                    word = Some(end);
                }
                end += 1;
            }
            end = if end == units.len() {
                end - 1
            } else {
                word.unwrap_or((end - 1).max(start + 1))
            };
            parts.push(units[start].0..units[end].0);
            start = end;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Code([Option<usize>; 2]),
    Comment {
        index: usize,
        part: usize,
        side: usize,
    },
    Gap(bool),
}
#[derive(Clone, Debug)]
pub struct Token {
    pub range: Range<usize>,
    pub style: Style,
}

pub struct FileView {
    pub expanded: Option<File>,
    pub trailing_context: bool,
    pub context_visible: Option<Vec<bool>>,
    source_indices: Vec<Option<usize>>,
    display_indices: Vec<Option<usize>>,
    pub code: Vec<CodeLine>,
    pub rows: Vec<Row>,
    pub digits: usize,
    pub comments: Vec<[Option<usize>; 2]>,
    pub comment_text: Vec<Vec<CodeLine>>,
    pub comment_edges: Vec<[Option<(usize, usize)>; 2]>,
    styles: Vec<[Option<Vec<Token>>; 2]>,
    hunks: Vec<Range<usize>>,
    line_hunks: Vec<usize>,
    highlighters: HashMap<(usize, usize), (usize, HighlightLines<'static>)>,
    pub split: bool,
    pub wrap: bool,
    pub widths: [usize; 2],
    pub fragments: Vec<[Vec<Range<usize>>; 2]>,
    pub starts: Vec<usize>,
    pub hunk_rows: [Vec<usize>; 2],
}

pub fn hidden(line: &Line) -> bool {
    line.old == 0
        && line.new == 0
        && [
            "diff --git ",
            "index ",
            "--- ",
            "+++ ",
            "@@",
            "new file mode ",
            "deleted file mode ",
        ]
        .iter()
        .any(|p| line.text.starts_with(p))
}
pub fn split_rows(lines: &[Line]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = &lines[i];
        if hidden(l) {
            i += 1;
            continue;
        }
        if (l.old > 0) == (l.new > 0) {
            rows.push(Row::Code([Some(i), Some(i)]));
            i += 1;
            continue;
        }
        let (mut old, mut new, mut markers) = (Vec::new(), Vec::new(), Vec::new());
        while i < lines.len() {
            let l = &lines[i];
            if l.old > 0 && l.new == 0 {
                old.push(i);
            } else if l.new > 0 && l.old == 0 {
                new.push(i);
            } else if l.text.starts_with('\\') {
                markers.push(i);
            } else {
                break;
            }
            i += 1;
        }
        // Pair changes in source order, preserving comment source coordinates.
        for j in 0..old.len().max(new.len()) {
            rows.push(Row::Code([old.get(j).copied(), new.get(j).copied()]));
        }
        rows.extend(markers.into_iter().map(|i| Row::Code([Some(i), Some(i)])));
    }
    rows
}

impl FileView {
    pub fn new(file: &File, split: bool) -> Self {
        let mut hunks = Vec::<Range<usize>>::new();
        let mut line_hunks = Vec::new();
        for (i, l) in file.lines.iter().enumerate() {
            if hunks.is_empty() || l.text.starts_with("@@ ") {
                if let Some(last) = hunks.last_mut() {
                    last.end = i;
                }
                hunks.push(i..file.lines.len());
            }
            line_hunks.push(hunks.len() - 1);
        }
        let mut view = Self {
            expanded: None,
            trailing_context: false,
            context_visible: None,
            source_indices: Vec::new(),
            display_indices: Vec::new(),
            code: file
                .lines
                .iter()
                .map(|l| {
                    CodeLine::new(if l.old > 0 || l.new > 0 {
                        l.text.get(1..).unwrap_or("")
                    } else {
                        &l.text
                    })
                })
                .collect(),
            styles: vec![[None, None]; file.lines.len()],
            hunks,
            line_hunks,
            highlighters: HashMap::new(),
            digits: file
                .lines
                .iter()
                .map(|l| l.old.max(l.new))
                .max()
                .unwrap_or(0)
                .to_string()
                .len(),
            rows: Vec::new(),
            comments: Vec::new(),
            comment_edges: Vec::new(),
            comment_text: Vec::new(),
            split,
            wrap: false,
            widths: [0, 0],
            fragments: Vec::new(),
            starts: Vec::new(),
            hunk_rows: Default::default(),
        };
        view.rebuild_rows(file, split);
        view
    }
    pub fn rebuild_rows(&mut self, file: &File, split: bool) {
        let Some(mut expanded) = self.expanded.take() else {
            self.rebuild_display_rows(file, split);
            if self.trailing_context {
                self.rows
                    .extend([Row::Gap(false), Row::Gap(true), Row::Gap(false)]);
            }
            return;
        };
        expanded.comments = file.comments.clone();
        for comment in expanded.comments.iter_mut().filter(|c| !c.file) {
            comment.start = self.display_indices[comment.start].unwrap();
            comment.end = self.display_indices[comment.end].unwrap();
        }
        self.rebuild_display_rows(&expanded, split);
        if let Some(visible) = &self.context_visible {
            let mut rows = Vec::new();
            for row in self.rows.drain(..) {
                if matches!(row, Row::Code(pair) if !pair.iter().flatten().any(|i| visible[*i])) {
                    if !matches!(rows.last(), Some(Row::Gap(_))) {
                        rows.extend([Row::Gap(false), Row::Gap(true), Row::Gap(false)]);
                    }
                } else {
                    rows.push(row);
                }
            }
            self.rows = rows;
        }
        // Full context merges Git hunks; retain navigation between the original hunks.
        let mut hunk = 0;
        let hunks: Vec<_> = file
            .lines
            .iter()
            .map(|l| {
                hunk += usize::from(l.text.starts_with("@@ "));
                hunk
            })
            .collect();
        self.hunk_rows = Default::default();
        let mut previous = [None, None];
        for (row, item) in self.rows.iter().enumerate() {
            if let Row::Code(pair) = item {
                for side in 0..2 {
                    if let Some(source) = pair[side].and_then(|i| self.source_indices[i]) {
                        let l = &file.lines[source];
                        if (l.old > 0) != (l.new > 0) && previous[side] != Some(hunks[source]) {
                            self.hunk_rows[side].push(row);
                            previous[side] = Some(hunks[source]);
                        }
                    }
                }
            }
        }
        self.expanded = Some(expanded);
    }
    pub fn expand(file: &File, expanded: File, split: bool) -> Result<Self> {
        let lookup: HashMap<_, _> = expanded
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.old > 0 || l.new > 0)
            .map(|(i, l)| ((l.old, l.new, l.text.as_str()), i))
            .collect();
        let display_indices: Vec<_> = file
            .lines
            .iter()
            .map(|l| lookup.get(&(l.old, l.new, l.text.as_str())).copied())
            .collect();
        ensure!(
            file.lines
                .iter()
                .zip(&display_indices)
                .all(|(l, i)| l.old == 0 && l.new == 0 || i.is_some()),
            "File changed since this diff was loaded; press r to refresh before expanding"
        );
        let changed = |l: &&Line| (l.old > 0) != (l.new > 0);
        ensure!(
            file.lines
                .iter()
                .filter(changed)
                .eq(expanded.lines.iter().filter(changed)),
            "File changed; press r to refresh before expanding"
        );
        ensure!(
            file.comments.iter().all(|c| c.file
                || display_indices.get(c.start).is_some_and(Option::is_some)
                    && display_indices.get(c.end).is_some_and(Option::is_some)),
            "Cannot expand a comment attached to diff metadata"
        );
        let mut view = Self::new(&expanded, split);
        view.source_indices = vec![None; expanded.lines.len()];
        for (source, display) in display_indices.iter().enumerate() {
            if let Some(display) = display {
                view.source_indices[*display] = Some(source);
            }
        }
        view.display_indices = display_indices;
        view.expanded = Some(expanded);
        view.rebuild_rows(file, split);
        Ok(view)
    }
    pub fn original_source(&self, display: usize) -> Option<usize> {
        if self.expanded.is_some() {
            self.source_indices[display]
        } else {
            Some(display)
        }
    }
    pub fn expand_near(&mut self, display: usize) -> Option<usize> {
        let file = self.expanded.as_ref().unwrap();
        let visible = self.context_visible.get_or_insert_with(|| {
            file.lines
                .iter()
                .enumerate()
                .map(|(i, l)| self.source_indices[i].is_some() || l.old == 0 && l.new == 0)
                .collect()
        });
        let next = (0..visible.len())
            .filter(|i| !visible[*i])
            .min_by_key(|i| i.abs_diff(display))?;
        let step = if next < display { -1 } else { 1 };
        let mut i = next;
        let mut last = next;
        for _ in 0..10 {
            if visible[i] {
                break;
            }
            visible[i] = true;
            last = i;
            let Some(next) = i.checked_add_signed(step).filter(|i| *i < visible.len()) else {
                break;
            };
            i = next;
        }
        Some(last)
    }
    fn rebuild_display_rows(&mut self, file: &File, split: bool) {
        self.split = split;
        self.comment_text = file
            .comments
            .iter()
            .map(|c| {
                std::iter::once(CodeLine::new(&format!(
                    "{} [{}]:",
                    if c.done { "Done" } else { "Open" },
                    c.location(&file.lines)
                )))
                .chain(c.text.split('\n').map(CodeLine::new))
                .collect()
            })
            .collect();
        let base = if split {
            split_rows(&file.lines)
        } else {
            file.lines
                .iter()
                .enumerate()
                .filter(|(_, l)| !hidden(l))
                .map(|(i, _)| Row::Code([Some(i), Some(i)]))
                .collect()
        };
        self.comments = vec![[None, None]; file.lines.len()];
        self.comment_edges = vec![[None, None]; file.comments.len()];
        let mut source_rows = vec![[None, None]; file.lines.len()];
        for (row, pair) in base.iter().enumerate() {
            if let Row::Code(pair) = pair {
                for side in 0..2 {
                    if let Some(i) = pair[side] {
                        source_rows[i][side] = Some(row);
                    }
                }
            }
        }
        let mut after = vec![Vec::new(); base.len()];
        for (ci, c) in file.comments.iter().enumerate().filter(|(_, c)| !c.file) {
            let mut last = None;
            for (i, source_row) in source_rows
                .iter()
                .enumerate()
                .take(c.end.saturating_add(1))
                .skip(c.start)
            {
                for side in 0..2 {
                    if c.includes(i, &file.lines[i])
                        && (!split || c.side.is_empty() || c.side == ["old", "new"][side])
                    {
                        self.comments[i][side].get_or_insert(ci);
                        if source_row[side].is_some() {
                            let edge = self.comment_edges[ci][side].get_or_insert((i, i));
                            edge.1 = i;
                        }
                        if let Some(row) = source_row[side] {
                            last = Some(last.map_or(row, |v: usize| v.max(row)));
                        }
                    }
                }
            }
            if let Some(last) = last {
                after[last].push(ci);
            }
        }
        self.rows.clear();
        let file_side = usize::from(file.lines.iter().any(|l| l.new > 0));
        for (index, _) in file.comments.iter().enumerate().filter(|(_, c)| c.file) {
            for part in 0..self.comment_text[index].len() {
                self.rows.push(Row::Comment {
                    index,
                    part,
                    side: file_side,
                });
            }
        }
        let (mut last_hunk, mut old, mut new) = (None, 0, 0);
        for (row, item) in base.into_iter().enumerate() {
            if let Row::Code(pair) = item {
                let hunk = pair.iter().flatten().last().map(|i| self.line_hunks[*i]);
                let o = pair
                    .iter()
                    .flatten()
                    .map(|i| file.lines[*i].old)
                    .max()
                    .unwrap_or(0);
                let n = pair
                    .iter()
                    .flatten()
                    .map(|i| file.lines[*i].new)
                    .max()
                    .unwrap_or(0);
                if last_hunk != hunk && (o > old + 1 || n > new + 1) {
                    self.rows
                        .extend([Row::Gap(false), Row::Gap(true), Row::Gap(false)]);
                }
                last_hunk = hunk;
                old = old.max(o);
                new = new.max(n);
            }
            self.rows.push(item);
            for &index in &after[row] {
                let side = file.comments[index].display_side(&file.lines);
                for part in 0..self.comment_text[index].len() {
                    self.rows.push(Row::Comment { index, part, side });
                }
            }
        }
        self.starts.clear();
        self.hunk_rows = Default::default();
        let mut previous = [None, None];
        for (row, item) in self.rows.iter().enumerate() {
            if let Row::Code(pair) = item {
                for side in 0..2 {
                    if let Some(i) = pair[side] {
                        let l = &file.lines[i];
                        if (l.old > 0) != (l.new > 0) && previous[side] != Some(self.line_hunks[i])
                        {
                            self.hunk_rows[side].push(row);
                            previous[side] = Some(self.line_hunks[i]);
                        }
                    }
                }
            }
        }
    }
    pub fn code_widths(&self, width: usize, bias: i32) -> [usize; 2] {
        if self.split {
            let available = width.saturating_sub(2 * self.digits + 5).max(2);
            let old = (available * (50 + bias) as usize / 100).clamp(1, available - 1);
            [old, available - old]
        } else {
            let w = width.saturating_sub(2 * self.digits + 3).max(1);
            [w, w]
        }
    }
    pub fn layout(&mut self, width: usize, bias: i32, wrap: bool) {
        let widths = self.code_widths(width, bias);
        if !self.starts.is_empty()
            && self.wrap == wrap
            && (widths == self.widths || !wrap && self.comment_text.iter().all(Vec::is_empty))
        {
            self.widths = widths;
            return;
        }
        let reuse_code = !self.starts.is_empty() && !self.wrap && !wrap;
        self.widths = widths;
        self.wrap = wrap;
        if !reuse_code {
            self.fragments
                .resize_with(self.rows.len(), Default::default);
        }
        self.starts.clear();
        self.starts.push(0);
        // ponytail: heights scan rows on resize; use lazy height indexing if this becomes measurable.
        for (row_index, row) in self.rows.iter().enumerate() {
            let parts = &mut self.fragments[row_index];
            if !reuse_code || matches!(row, Row::Comment { .. }) {
                for side in parts.iter_mut() {
                    side.clear();
                }
            }
            match row {
                Row::Code(pair) if !reuse_code => {
                    for side in 0..2 {
                        if let Some(i) = pair[side] {
                            if wrap {
                                self.code[i].wrap_into(widths[side], &mut parts[side]);
                            } else {
                                parts[side].push(0..self.code[i].text.len());
                            }
                        }
                    }
                }
                Row::Comment { index, part, side } => {
                    let side = if self.split { *side } else { 0 };
                    let indent = if *part == 0 { 2 } else { 6 };
                    self.comment_text[*index][*part]
                        .wrap_into(widths[side].saturating_sub(indent).max(1), &mut parts[side]);
                }
                _ => {}
            }
            let height = parts.iter().map(Vec::len).max().unwrap_or(0).max(1);
            self.starts.push(self.starts.last().unwrap() + height);
        }
    }
    pub fn len(&self) -> usize {
        *self.starts.last().unwrap_or(&0)
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn locate(&self, visual: usize) -> Option<(usize, usize)> {
        if visual >= self.len() {
            return None;
        }
        let row = self.starts.partition_point(|n| *n <= visual) - 1;
        Some((row, visual - self.starts[row]))
    }
    pub fn visual_for_source(&self, source: usize, side: usize) -> Option<usize> {
        let source = if self.expanded.is_some() {
            *self.display_indices.get(source)?.as_ref()?
        } else {
            source
        };
        self.visual_for_display(source, side)
    }
    pub fn visual_for_display(&self, source: usize, side: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Code(pair) if pair[side] == Some(source)))
            .map(|row| self.starts[row])
    }
    pub fn source(&self, visual: usize, side: usize) -> Option<usize> {
        self.original_source(self.display_source(visual, side)?)
    }
    pub fn display_source(&self, visual: usize, side: usize) -> Option<usize> {
        let (row, part) = self.locate(visual)?;
        if let Row::Code(pair) = self.rows[row]
            && self.fragments[row][side].get(part).is_some()
        {
            return pair[side];
        }
        None
    }
    pub fn selectable(&self, visual: usize, side: usize) -> bool {
        self.display_source(visual, side).is_some()
            || self.locate(visual).is_some_and(
                |(r, _)| matches!(self.rows[r],Row::Comment{side:s,..} if !self.split || s==side),
            )
    }
    pub fn highlight_visible(&mut self, file: &File, start: usize, height: usize) -> bool {
        let expanded = self.expanded.take();
        let pending = self.highlight_display(expanded.as_ref().unwrap_or(file), start, height);
        self.expanded = expanded;
        pending
    }
    fn highlight_display(&mut self, file: &File, start: usize, height: usize) -> bool {
        let started = Instant::now();
        // Only parse through the last visible source line. A new file can be one
        // enormous hunk; parsing the whole hunk stalls every Files selection.
        let mut ends = HashMap::<(usize, usize), usize>::new();
        for visual in start..start.saturating_add(height).min(self.len()) {
            for side in 0..2 {
                if let Some(i) = self.display_source(visual, side) {
                    ends.entry((self.line_hunks[i], side))
                        .and_modify(|end| *end = (*end).max(i + 1))
                        .or_insert(i + 1);
                }
            }
        }
        for ((hunk, side), end) in ends {
            let (next, highlighter) = self.highlighters.entry((hunk, side)).or_insert_with(|| {
                let extension = std::path::Path::new(&file.path)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                let syntax = SYNTAXES
                    .find_syntax_by_extension(extension)
                    .unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
                (
                    self.hunks[hunk].start,
                    HighlightLines::new(syntax, &THEMES.themes["base16-ocean.dark"]),
                )
            });
            for i in *next..end {
                if (if side == 0 {
                    file.lines[i].old
                } else {
                    file.lines[i].new
                }) == 0
                {
                    continue;
                }
                let text = format!("{}\n", self.code[i].text);
                let tokens = highlighter
                    .highlight_line(&text, &SYNTAXES)
                    .unwrap_or_default();
                let mut offset = 0;
                let styled = tokens
                    .iter()
                    .filter_map(|(s, t)| {
                        let start = offset;
                        offset += t.len();
                        let end = offset.min(self.code[i].text.len());
                        (start < end).then_some(Token {
                            range: start..end,
                            style: Style::default().fg(
                                if Some(s.foreground)
                                    == THEMES.themes["base16-ocean.dark"].settings.foreground
                                {
                                    Color::Rgb(255, 255, 255)
                                } else {
                                    Color::Rgb(s.foreground.r, s.foreground.g, s.foreground.b)
                                },
                            ),
                        })
                    })
                    .collect();
                self.styles[i][side] = Some(styled);
                *next = i + 1;
                // Preserve parser state, but yield to input instead of blocking on a huge hunk.
                if started.elapsed() >= Duration::from_millis(2) {
                    return true;
                }
            }
            *next = (*next).max(end);
        }
        false
    }
    pub fn comment_marker(
        &self,
        index: usize,
        side: usize,
        part: usize,
        parts: usize,
    ) -> &'static str {
        let Some(ci) = self.comments[index][side] else {
            return " ";
        };
        let Some((start, end)) = self.comment_edges[ci][side] else {
            return " ";
        };
        match (
            index == start && part == 0,
            index == end && part + 1 == parts,
        ) {
            (true, true) => "◆",
            (true, false) => "╭",
            (false, true) => "╰",
            _ => "│",
        }
    }
    pub fn tokens(&self, index: usize, side: usize) -> &[Token] {
        self.styles[index][side].as_deref().unwrap_or(&[])
    }
}

pub fn commit_graph(commits: &[Commit]) -> Vec<String> {
    let glyphs = [
        ' ', '│', '│', '│', '─', '╯', '╮', '┤', '─', '╰', '╭', '├', '─', '┴', '┬', '┼',
    ];
    let mut lanes: Vec<String> = Vec::new();
    let mut result = Vec::new();
    for commit in commits {
        let node = lanes
            .iter()
            .position(|s| s == &commit.id)
            .or_else(|| lanes.iter().position(String::is_empty))
            .unwrap_or(lanes.len());
        if node == lanes.len() {
            lanes.push(commit.id.clone());
        } else {
            lanes[node] = commit.id.clone();
        }
        let before = lanes.clone();
        lanes[node].clear();
        let mut destinations = Vec::new();
        for parent in &commit.parents {
            let lane = lanes.iter().position(|s| s == parent).unwrap_or_else(|| {
                if lanes[node].is_empty() {
                    node
                } else {
                    lanes
                        .iter()
                        .position(String::is_empty)
                        .unwrap_or(lanes.len())
                }
            });
            if lane == lanes.len() {
                lanes.push(parent.clone());
            } else {
                lanes[lane] = parent.clone();
            }
            destinations.push(lane);
        }
        let mut cells = vec![0; lanes.len() * 2 - 1];
        for (lane, id) in before.iter().enumerate() {
            if !id.is_empty() && lane != node {
                cells[lane * 2] |= 1;
            }
        }
        for (lane, id) in lanes.iter().enumerate() {
            if !id.is_empty() {
                cells[lane * 2] |= 2;
            }
        }
        for lane in destinations {
            for x in node.min(lane) * 2..node.max(lane) * 2 {
                cells[x] |= 8;
                cells[x + 1] |= 4;
            }
        }
        let mut row: Vec<char> = cells.iter().map(|i| glyphs[*i]).collect();
        row[node * 2] = if commit.parents.len() > 1 {
            '◎'
        } else {
            '○'
        };
        result.push(row.iter().collect::<String>().trim_end().into());
        while lanes.last().is_some_and(String::is_empty) {
            lanes.pop();
        }
    }
    result
}
