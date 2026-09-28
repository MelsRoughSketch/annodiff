use crate::{
    app::{App, Confirmation, Inspection, Modal},
    diff::{FileView, Row},
    review::File,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

const SELECT: Color = Color::LightCyan;
const LIST_SELECTION_BG: Color = Color::Rgb(224, 255, 255);

fn luminance([r, g, b]: [u8; 3]) -> f64 {
    let linear = |v: u8| {
        let v = f64::from(v) / 255.;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}
fn selected_foreground(color: Color) -> Color {
    // ANSI colors are theme-dependent; use conventional RGB values as estimates.
    // RGB colors (including the selection background) have exact contrast ratios.
    let rgb = match color {
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Black => [0, 0, 0],
        Color::Red => [128, 0, 0],
        Color::Green => [0, 128, 0],
        Color::Yellow => [128, 128, 0],
        Color::Blue => [0, 0, 128],
        Color::Magenta => [128, 0, 128],
        Color::Cyan => [0, 128, 128],
        Color::Gray => [192, 192, 192],
        Color::DarkGray => [128, 128, 128],
        Color::LightRed => [255, 0, 0],
        Color::LightGreen => [0, 255, 0],
        Color::LightYellow => [255, 255, 0],
        Color::LightBlue => [0, 0, 255],
        Color::LightMagenta => [255, 0, 255],
        Color::LightCyan => [0, 255, 255],
        Color::White => [255, 255, 255],
        Color::Reset | Color::Indexed(_) => return Color::Black,
    };
    let Color::Rgb(r, g, b) = LIST_SELECTION_BG else {
        unreachable!()
    };
    let background = luminance([r, g, b]);
    let readable = |rgb| {
        let foreground = luminance(rgb);
        (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05) >= 4.5
    };
    if readable(rgb) {
        return color;
    }
    let inverse = rgb.map(|v| 255 - v);
    if readable(inverse) {
        Color::Rgb(inverse[0], inverse[1], inverse[2])
    } else {
        Color::Black
    }
}

fn block(title: impl Into<Line<'static>>, focus: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if focus { SELECT } else { Color::DarkGray }))
}
fn bordered(frame: &mut Frame, rect: Rect, title: String, focus: bool) -> Rect {
    let b = block(title, focus);
    let inner = b.inner(rect);
    frame.render_widget(b, rect);
    inner
}
fn selected() -> Style {
    Style::default().fg(Color::Black).bg(SELECT)
}
fn text(frame: &mut Frame, rect: Rect, value: &str, scroll: usize, wrap: bool) {
    if wrap {
        frame.render_widget(
            Paragraph::new(value)
                .wrap(Wrap { trim: false })
                .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
            rect,
        );
        return;
    }
    let lines: Vec<Line> = value
        .split('\n')
        .skip(scroll)
        .take(rect.height as usize)
        .map(Line::raw)
        .collect();
    frame.render_widget(Paragraph::new(lines), rect);
}
fn list<'a, T: Clone + Into<Line<'a>>>(
    frame: &mut Frame,
    rect: Rect,
    labels: &[T],
    (cursor, follow, margin): (usize, bool, usize),
    offset: &mut usize,
    focus: bool,
    marker: bool,
) {
    let height = rect.height as usize;
    if height == 0 {
        return;
    }
    let margin = margin.min(height.saturating_sub(1) / 2);
    if follow && cursor < offset.saturating_add(margin) {
        *offset = cursor.saturating_sub(margin);
    }
    if follow && cursor >= offset.saturating_add(height - margin) {
        *offset = cursor.saturating_add(margin + 1).saturating_sub(height);
    }
    *offset = (*offset).min(labels.len().saturating_sub(height));
    for (line, label) in labels.iter().enumerate().skip(*offset).take(height) {
        let y = rect.y + (line - *offset) as u16;
        let gutter = if marker { 2.min(rect.width) } else { 0 };
        let area = Rect::new(rect.x + gutter, y, rect.width - gutter, 1);
        if marker && line == cursor && gutter > 0 {
            frame.render_widget(
                Paragraph::new("▶").style(Style::default().fg(if focus {
                    Color::Rgb(80, 220, 220)
                } else {
                    Color::DarkGray
                })),
                Rect::new(rect.x, y, gutter, 1),
            );
        }
        let style = if !marker && line == cursor {
            if focus {
                Style::default().fg(Color::Black).bg(LIST_SELECTION_BG)
            } else {
                Style::default().fg(Color::Reset).bg(Color::Rgb(72, 72, 72))
            }
        } else {
            Style::default().fg(Color::Reset)
        };
        let mut label: Line = label.clone().into();
        if marker && line == cursor {
            label.style = label.style.add_modifier(ratatui::style::Modifier::BOLD);
        }
        if !marker && focus && line == cursor {
            let foreground = label.style.fg.unwrap_or(Color::Black);
            label.style = label.style.fg(selected_foreground(foreground));
            for span in &mut label.spans {
                span.style = span
                    .style
                    .fg(selected_foreground(span.style.fg.unwrap_or(foreground)));
            }
        }
        frame.render_widget(Paragraph::new(label).style(style), area);
    }
}

pub fn draw(app: &mut App, frame: &mut Frame) {
    app.highlight_pending = false;
    let area = frame.area();
    app.diff_inner = Rect::default();
    app.pane_area = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(2));
    if area.width == 0 || area.height == 0 {
        return;
    }
    let main = app.pane_area;
    app.pane_rects = [Rect::default(); 4];
    app.diff_mode_rects = [Rect::default(); 2];
    let full = app.zoom == 2;
    let (_, extent) = app.pane_axis();
    let sidebar_size = if full {
        if app.pane == 0 { 0 } else { extent }
    } else {
        (i32::from(extent) * app.sidebar_percent / 100) as u16
    };
    let (sidebar, detail) = if app.stacked {
        (
            Rect::new(main.x, main.y, main.width, sidebar_size),
            Rect::new(
                main.x,
                main.y + sidebar_size,
                main.width,
                main.height - sidebar_size,
            ),
        )
    } else {
        (
            Rect::new(main.x, main.y, sidebar_size, main.height),
            Rect::new(
                main.x + sidebar_size,
                main.y,
                main.width - sidebar_size,
                main.height,
            ),
        )
    };
    if !sidebar.is_empty() {
        if app.stacked || app.zoom > 0 {
            app.pane_rects[app.left_pane] = sidebar;
        } else {
            let h = sidebar.height / 3;
            for pane in 1..4 {
                app.pane_rects[pane] = Rect::new(
                    sidebar.x,
                    sidebar.y + h * (pane as u16 - 1),
                    sidebar.width,
                    if pane == 3 { sidebar.height - 2 * h } else { h },
                );
            }
        }
    }
    for pane in 1..4 {
        let rect = app.pane_rects[pane];
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let title = match pane {
            1 => format!(
                " [1] Files{} · Σ{}{} ",
                if app.commented_files_only {
                    if app.open_only {
                        " · Open comments"
                    } else {
                        " · All comments"
                    }
                } else {
                    ""
                },
                app.total_changes,
                if app.queries[1].is_empty() {
                    String::new()
                } else {
                    format!(" · /{}", app.queries[1])
                }
            ),
            2 => format!(
                " [2] {} comments{}{} ",
                if app.open_only { "Open" } else { "All" },
                if app.file_only {
                    " · current file"
                } else {
                    " · all files"
                },
                if app.queries[2].is_empty() {
                    String::new()
                } else {
                    format!(" · /{}", app.queries[2])
                }
            ),
            _ => format!(
                " [3] Commits{} ",
                if app.review.base.is_empty() {
                    " · working tree".into()
                } else {
                    format!(
                        " · {} → {}",
                        &app.review.base[..app.review.base.len().min(8)],
                        if app.review.target.is_empty() {
                            "working tree"
                        } else {
                            &app.review.target[..app.review.target.len().min(8)]
                        }
                    )
                }
            ),
        };
        let inner = bordered(frame, rect, title, app.pane == pane);
        list(
            frame,
            inner,
            &app.labels[pane],
            (app.cursor[pane], !app.manual_scroll[pane], 0),
            &mut app.list_offsets[pane],
            app.pane == pane,
            true,
        );
    }
    if !detail.is_empty() {
        if app.pane == 2
            && let Some(value) = &app.inspection
        {
            inspection(frame, detail, value);
        } else {
            app.pane_rects[0] = detail;
            draw_diff(app, frame);
        }
    }
    let help = matches!(app.modal, Some(Modal::Help { .. })).then(|| help_lines(app));
    if let Some(modal) = &mut app.modal {
        draw_modal(frame, main, modal, &app.review, help, &app.language);
    }
    if area.height >= 2 {
        frame.render_widget(
            Paragraph::new(app.status.as_str()),
            Rect::new(area.x, area.bottom() - 2, area.width, 1),
        );
    }
    let hint = if app.editor.is_some() {
        "Enter: newline · Ctrl+Enter/F2: save · Esc: cancel"
    } else {
        match app.modal {
            Some(Modal::Loading { .. }) => "q: quit · Esc: cancel · c: copy to clipboard",
            Some(Modal::Search { .. }) => "Type to search/filter · Enter: keep · Esc: cancel",
            Some(Modal::Confirm { .. }) => "←→/Tab: choose · Enter: confirm · Esc: cancel",
            Some(Modal::Sessions { search: true, .. }) => {
                "Type to search · Tab: filters · Enter/Ctrl+Enter/F2: preview · Esc: cancel"
            }
            Some(Modal::Sessions { .. }) => {
                "q: quit · Tab: filter · ←→: change · ↑↓: session · /: search · Enter: preview · Esc: cancel"
            }
            Some(Modal::Preview { .. }) => {
                "q: quit · ↑↓/PgUp/PgDn: scroll · Tab: pane · Enter/Ctrl+Enter/F2: send/copy · Esc: cancel"
            }
            Some(_) => "↑↓/PgUp/PgDn: scroll · Tab: pane · Esc: close",
            None => "?: help · t: layout · 0/1/2/3: pane · h/l: pane/side · +/-: zoom · q: quit",
        }
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(SELECT)),
        Rect::new(area.x, area.bottom() - 1, area.width, 1),
    );
}

fn draw_diff(app: &mut App, frame: &mut Frame) {
    let rect = app.pane_rects[0];
    let path = app.current().map_or("Diff", |f| f.path.as_str());
    let show_modes = app.file.is_some_and(|file| app.review.can_split_file(file));
    let file_note = if show_modes || app.file.is_none() {
        ""
    } else if app.review.statuses.get(path).is_some_and(|s| s == "??") {
        " · new file · untracked"
    } else {
        " · new file"
    };
    let budget = (rect.width as usize).saturating_sub(if show_modes {
        36
    } else {
        8 + file_note.width()
    });
    let mut name = String::new();
    for glyph in unicode_segmentation::UnicodeSegmentation::graphemes(path, true) {
        if name.width() + glyph.width() > budget {
            break;
        }
        name.push_str(glyph);
    }
    let prefix = format!(" [0] {name}{}", if show_modes { " · <" } else { file_note });
    let mut x = rect
        .x
        .saturating_add(1)
        .saturating_add(prefix.width() as u16);
    let mut spans = vec![Span::raw(prefix)];
    if show_modes {
        for (mode, label) in ["unified", "side-by-side"].into_iter().enumerate() {
            let width = label.len() as u16;
            app.diff_mode_rects[mode] = Rect::new(x, rect.y, width, 1)
                .intersection(rect.inner(ratatui::layout::Margin::new(1, 0)));
            spans.push(Span::styled(
                label,
                if app.review.split == (mode == 1) {
                    Style::default().add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    Style::default()
                },
            ));
            x = x.saturating_add(width);
            if mode == 0 {
                spans.push(Span::raw(" / "));
                x = x.saturating_add(3);
            }
        }
        spans.push(Span::raw(">"));
    }
    spans.push(Span::raw(if app.queries[0].is_empty() {
        " ".to_string()
    } else {
        format!(" · /{} ", app.queries[0])
    }));
    if app.view().is_some_and(|v| v.expanded.is_some()) {
        spans.push(Span::raw(
            if app.view().is_some_and(|v| v.context_visible.is_some()) {
                " · expanded context · z: more "
            } else {
                " · full file · Z: collapse "
            },
        ));
    }
    let border = block(Line::from(spans), app.pane == 0);
    app.diff_inner = border.inner(rect);
    frame.render_widget(border, rect);
    let inner = app.diff_inner;
    app.layout_diff(inner.width as usize);
    let height = inner.height as usize;
    if height == 0 || inner.width == 0 {
        return;
    }
    let editor_height = app.editor.as_ref().map_or(0, |e| {
        (e.input.lines().len() + 2)
            .max(3)
            .min((height / 2).max(3))
            .min(height)
    });
    if let Some(editor) = &app.editor {
        app.offset = editor
            .after
            .saturating_add(1)
            .saturating_sub(height.saturating_sub(editor_height + 2));
    } else {
        if !app.manual_scroll[0] && app.cursor[0] < app.offset {
            app.offset = app.cursor[0];
        }
        if !app.manual_scroll[0] && app.cursor[0] >= app.offset + height {
            app.offset = app.cursor[0] + 1 - height;
        }
        app.offset = app
            .offset
            .min(app.view().map_or(0, |v| v.len().saturating_sub(height)));
    }
    let Some(file) = app.file else {
        let message = if app.commented_files_only {
            "No matching files with comments. o shows all files."
        } else if app.queries[1].is_empty() {
            "No changes against HEAD."
        } else {
            "No matching files. Esc clears the filter."
        };
        text(frame, inner, message, 0, true);
        return;
    };
    let view = app
        .cache
        .iter_mut()
        .find(|(i, _)| *i == file)
        .map(|(_, v)| v)
        .unwrap();
    if app.modal.is_none() {
        app.highlight_pending = view.highlight_visible(&app.review.files[file], app.offset, height);
    }
    let view = app.view().unwrap();
    let f = view.expanded.as_ref().unwrap_or(&app.review.files[file]);
    if view.is_empty() && app.editor.is_none() {
        text(
            frame,
            inner,
            "No text changes (empty file or metadata-only change).",
            0,
            true,
        );
        return;
    }
    let bounds = app.bounds();
    let mut screen_row = 0;
    let mut visual = app.offset;
    let mut editor_rect = Rect::default();
    while screen_row < height {
        if app.editor.as_ref().is_some_and(|e| visual == e.after + 1) {
            editor_rect = Rect::new(
                inner.x,
                inner.y + screen_row as u16,
                inner.width,
                editor_height.min(height - screen_row) as u16,
            );
            screen_row += editor_height;
            if screen_row >= height {
                break;
            }
        }
        let Some((row, part)) = view.locate(visual) else {
            break;
        };
        let area = Rect::new(inner.x, inner.y + screen_row as u16, inner.width, 1);
        match view.rows[row] {
            Row::Gap(label) => {
                if label {
                    frame.render_widget(
                        Paragraph::new("⋯ unchanged lines omitted · z: expand nearby ⋯")
                            .style(Style::default().fg(Color::DarkGray)),
                        area,
                    );
                }
            }
            Row::Comment {
                index,
                part: comment_part,
                side,
            } => {
                let column = if view.split { side } else { 0 };
                let fragments = &view.fragments[row][column];
                let Some(range) = fragments.get(part) else {
                    break;
                };
                let line = &view.comment_text[index][comment_part].text[range.clone()];
                let last = comment_part + 1 == view.comment_text[index].len()
                    && part + 1 == fragments.len();
                let prefix = if comment_part == 0 {
                    if part == 0 { "╭ " } else { "│ " }
                } else if last {
                    "╰     "
                } else {
                    "│     "
                };
                let target = if view.split {
                    let x = if side == 0 {
                        area.x + view.digits as u16 + 1
                    } else {
                        area.x + (view.digits * 2 + 4 + view.widths[0]) as u16
                    };
                    Rect::new(
                        x,
                        area.y,
                        view.widths[side].min(area.right().saturating_sub(x) as usize) as u16,
                        1,
                    )
                } else {
                    let gutter = ((2 * view.digits + 3) as u16).min(area.width);
                    Rect::new(area.x + gutter, area.y, area.width - gutter, 1)
                };
                let style = if app.cursor[0] == visual && (side == app.side || !view.split) {
                    selected()
                } else {
                    Style::default()
                        .fg(if f.comments[index].done {
                            Color::LightGreen
                        } else {
                            Color::Yellow
                        })
                        .bg(Color::Rgb(35, 44, 50))
                };
                let prefix: String = prefix
                    .chars()
                    .take(target.width.saturating_sub(1) as usize)
                    .collect();
                frame.render_widget(
                    Paragraph::new(format!("{prefix}{line}")).style(style),
                    target,
                );
            }
            Row::Code(pair) => {
                if view.split {
                    for (side, index) in pair.iter().enumerate() {
                        let x = if side == 0 {
                            area.x
                        } else {
                            area.x + (view.digits + view.widths[0] + 3) as u16
                        };
                        if x >= area.right() {
                            continue;
                        }
                        let code_x = x + view.digits as u16 + 1;
                        let code_width =
                            (view.widths[side] as u16).min(area.right().saturating_sub(code_x));
                        if let (Some(i), Some(range)) =
                            (*index, view.fragments[row][side].get(part))
                        {
                            let bg = background(app, f, i, side, visual, bounds);
                            let n = if side == 0 {
                                f.lines[i].old
                            } else {
                                f.lines[i].new
                            };
                            let number = if part == 0 && n > 0 {
                                format!("{n:>width$}", width = view.digits)
                            } else {
                                " ".repeat(view.digits)
                            };
                            frame.render_widget(
                                Paragraph::new(number).style(bg.fg(Color::DarkGray)),
                                Rect::new(x, area.y, view.digits as u16, 1),
                            );
                            frame.render_widget(
                                Paragraph::new(view.comment_marker(
                                    i,
                                    side,
                                    part,
                                    view.fragments[row][side].len(),
                                ))
                                .style(bg.fg(
                                    if app.cursor[0] == visual && app.side == side {
                                        Color::Black
                                    } else {
                                        Color::Rgb(255, 255, 0)
                                    },
                                )),
                                Rect::new(code_x - 1, area.y, 1, 1),
                            );
                            code(
                                frame,
                                Rect::new(code_x, area.y, code_width, 1),
                                view,
                                (i, side),
                                range.clone(),
                                bg,
                                app.cursor[0] == visual && app.side == side,
                            );
                        }
                    }
                    let x = area.x + (view.digits + view.widths[0] + 2) as u16;
                    if x < area.right() {
                        frame.render_widget(
                            Paragraph::new("│").style(Style::default().fg(Color::DarkGray)),
                            Rect::new(x, area.y, 1, 1),
                        );
                    }
                } else if let (Some(i), Some(range)) = (pair[0], view.fragments[row][0].get(part)) {
                    let l = &f.lines[i];
                    let side = usize::from(l.new > 0);
                    let bg = background(app, f, i, app.side, visual, bounds);
                    let old = if part == 0 && l.old > 0 {
                        l.old.to_string()
                    } else {
                        String::new()
                    };
                    let new = if part == 0 && l.new > 0 {
                        l.new.to_string()
                    } else {
                        String::new()
                    };
                    let numbers = format!(
                        "{old:>width$} {new:>width$} {}",
                        if part == 0 && (l.old > 0 || l.new > 0) {
                            l.text.chars().next().unwrap_or(' ')
                        } else {
                            ' '
                        },
                        width = view.digits
                    );
                    let number_width = (2 * view.digits + 3) as u16;
                    frame.render_widget(
                        Paragraph::new(numbers).style(bg.fg(Color::DarkGray)),
                        Rect::new(area.x, area.y, number_width.min(area.width), 1),
                    );
                    if number_width < area.width {
                        // Use the existing separator column; source text and +/- stay intact.
                        frame.render_widget(
                            Paragraph::new(view.comment_marker(
                                i,
                                0,
                                part,
                                view.fragments[row][0].len(),
                            ))
                            .style(bg.fg(
                                if app.cursor[0] == visual {
                                    Color::Black
                                } else {
                                    Color::Rgb(255, 255, 0)
                                },
                            )),
                            Rect::new(area.x + (2 * view.digits + 1) as u16, area.y, 1, 1),
                        );
                        code(
                            frame,
                            Rect::new(area.x + number_width, area.y, area.width - number_width, 1),
                            view,
                            (i, side),
                            range.clone(),
                            bg,
                            app.cursor[0] == visual,
                        );
                    }
                }
            }
        }
        screen_row += 1;
        visual += 1;
    }
    // A file-wide comment can also be edited when the diff has no source rows.
    if app.editor.is_some() && editor_rect.height == 0 {
        editor_rect = Rect::new(inner.x, inner.y, inner.width, editor_height as u16);
    }
    app.editor_rect = editor_rect;
    if let Some(editor) = &mut app.editor {
        frame.render_widget(Clear, editor_rect);
        editor.input.set_block(block(
            format!(
                " {} comment · Ctrl+Enter/F2: save ",
                if editor.comment.file { "File" } else { "Line" }
            ),
            true,
        ));
        editor.input.set_cursor_line_style(Style::default());
        frame.render_widget(&editor.input, editor_rect);
    }
}
fn background(
    app: &App,
    f: &File,
    index: usize,
    side: usize,
    visual: usize,
    bounds: Option<(usize, usize)>,
) -> Style {
    let l = &f.lines[index];
    let mut style = Style::default().fg(Color::White);
    if l.old > 0 && l.new == 0 {
        style = style.bg(Color::Rgb(55, 25, 30));
    }
    if l.old == 0 && l.new > 0 {
        style = style.bg(Color::Rgb(20, 45, 30));
    }
    if app
        .view()
        .is_some_and(|v| v.comments[index][side].is_some())
    {
        style = style.bg(Color::Rgb(35, 44, 50));
    }
    if !app.queries[0].is_empty()
        && l.text
            .to_lowercase()
            .contains(&app.queries[0].to_lowercase())
    {
        style = style.bg(Color::Rgb(140, 100, 20));
    }
    if app.anchor.is_some()
        && side == app.side
        && bounds.is_some_and(|(a, b)| {
            app.view()
                .and_then(|v| v.original_source(index))
                .is_some_and(|i| i >= a && i <= b)
        })
    {
        style = style.bg(Color::Rgb(55, 62, 68));
    }
    if app.cursor[0] == visual && (!app.split() || side == app.side) {
        style = selected();
    }
    style
}
fn code(
    frame: &mut Frame,
    area: Rect,
    view: &FileView,
    source_position: (usize, usize),
    range: std::ops::Range<usize>,
    style: Style,
    selected: bool,
) {
    let (index, side) = source_position;
    if area.width == 0 {
        return;
    }
    let source = &view.code[index].text;
    let mut spans = Vec::new();
    let mut offset = range.start;
    for token in view.tokens(index, side) {
        let start = token.range.start.max(range.start);
        let end = token.range.end.min(range.end);
        if start >= end {
            continue;
        }
        if offset < start {
            spans.push(Span::styled(&source[offset..start], style));
        }
        spans.push(Span::styled(
            &source[start..end],
            if selected {
                style
            } else {
                style.patch(token.style)
            },
        ));
        offset = end;
    }
    if offset < range.end {
        spans.push(Span::styled(&source[offset..range.end], style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(style), area);
    if source[range].width() > area.width as usize {
        let x = area.right() - 1;
        let buffer = frame.buffer_mut();
        if x > area.x && buffer[(x - 1, area.y)].symbol().width() > 1 {
            buffer[(x - 1, area.y)].set_symbol(" ");
        }
        buffer[(x, area.y)].set_symbol("…").set_style(style);
    }
}
fn inspection(frame: &mut Frame, area: Rect, inspection: &Inspection) {
    let left = Rect::new(area.x, area.y, area.width / 2, area.height);
    let right = Rect::new(left.right(), area.y, area.width - left.width, area.height);
    let recorded = bordered(
        frame,
        left,
        " Recorded comment ".into(),
        inspection.pane == 0,
    );
    text(
        frame,
        recorded,
        &inspection.recorded,
        inspection.offsets[0],
        true,
    );
    let current = bordered(
        frame,
        right,
        " Current file · original lines may have moved ".into(),
        inspection.pane == 1,
    );
    text(
        frame,
        current,
        &inspection.current,
        inspection.offsets[1],
        false,
    );
}
fn draw_modal(
    frame: &mut Frame,
    area: Rect,
    modal: &mut Modal,
    review: &crate::review::Review,
    help: Option<Vec<Line<'static>>>,
    language: &str,
) {
    match modal {
        Modal::Search { input, .. } => {
            let rect = Rect::new(
                area.x,
                area.bottom().saturating_sub(3),
                area.width,
                3.min(area.height),
            );
            frame.render_widget(Clear, rect);
            input.set_block(block(" / Search / filter ", true));
            frame.render_widget(&*input, rect);
        }
        Modal::Help {
            scroll,
            input,
            search,
        } => {
            frame.render_widget(Clear, area);
            let query = input.lines().join(" ").to_lowercase();
            let title = if query.is_empty() {
                " Key bindings · q/Esc/?: close · /: search ".into()
            } else {
                format!(
                    " Key bindings · /{} · q/Esc: close · /: search ",
                    input.lines().join(" ")
                )
            };
            let mut inner = bordered(frame, area, title, true);
            if *search {
                let height = inner.height.min(3);
                let search_rect = Rect::new(inner.x, inner.bottom() - height, inner.width, height);
                inner.height -= height;
                input.set_block(block(" Search help · Enter: apply · Esc: clear ", true));
                frame.render_widget(&*input, search_rect);
            }
            let lines: Vec<_> = help
                .unwrap_or_default()
                .into_iter()
                .filter(|line| query.is_empty() || line.to_string().to_lowercase().contains(&query))
                .collect();
            *scroll = (*scroll).min(lines.len().saturating_sub(inner.height as usize));
            if lines.is_empty() {
                frame.render_widget(Paragraph::new("No matching key bindings"), inner);
            } else {
                frame.render_widget(
                    Paragraph::new(lines.into_iter().skip(*scroll).collect::<Vec<_>>()),
                    inner,
                );
            }
        }
        Modal::Confirm { action, choice } => {
            let rect = centered(area, 76, 10);
            frame.render_widget(Clear, rect);
            let inner = bordered(frame, rect, " Confirm ".into(), true);
            let (message, buttons) = match action {
                Confirmation::Delete(_) => (
                    "Permanently delete this open comment?",
                    vec!["Cancel", "Delete"],
                ),
                Confirmation::Reset => (
                    "Archive current comments, or permanently reset ALL comments, history and temporary review files?\nArchive keeps comments in history, but excludes them from future reviews sent to the agent.\nReset also removes files still needed by queued requests.",
                    vec!["Cancel", "Archive", "Reset all"],
                ),
            };
            text(
                frame,
                Rect::new(
                    inner.x,
                    inner.y,
                    inner.width,
                    inner.height.saturating_sub(1),
                ),
                message,
                0,
                true,
            );
            let spans: Vec<_> = buttons
                .iter()
                .enumerate()
                .flat_map(|(i, s)| {
                    [
                        Span::styled(
                            format!(" {s} "),
                            if i == *choice {
                                selected()
                            } else {
                                Style::default()
                            },
                        ),
                        Span::raw("  "),
                    ]
                })
                .collect();
            if inner.height > 0 {
                frame.render_widget(
                    Paragraph::new(Line::from(spans)),
                    Rect::new(inner.x, inner.bottom() - 1, inner.width, 1),
                );
            }
        }
        Modal::Inspect(value) => {
            frame.render_widget(Clear, area);
            inspection(frame, area, value);
        }
        Modal::Sessions { .. } | Modal::Loading { .. } => {
            let mut loading_selection = 0;
            let (items, input, selection, search, options, control, viewport) = match modal {
                Modal::Sessions {
                    offset,
                    manual_scroll,
                    area,
                    filter_areas,
                    items,
                    input,
                    selection,
                    search,
                    options,
                    control,
                } => (
                    items.as_slice(),
                    input,
                    selection,
                    *search,
                    options,
                    control,
                    Some((offset, manual_scroll, area, filter_areas)),
                ),
                Modal::Loading {
                    input,
                    options,
                    control,
                    ..
                } => (
                    &[][..],
                    input,
                    &mut loading_selection,
                    false,
                    options,
                    control,
                    None,
                ),
                _ => unreachable!(),
            };
            let loading = viewport.is_none();
            frame.render_widget(Clear, area);
            let query = input.lines().join(" ").to_lowercase();
            let mut labels = vec![
                "+ New session in this directory".into(),
                "Copy to clipboard".into(),
            ];
            labels.extend(
                items
                    .iter()
                    .filter(|s| s.matches(!options.all, &query))
                    .map(|s| {
                        format!(
                            "{}{} · {}",
                            if s.current { "[here] " } else { "" },
                            s.title().split_whitespace().collect::<Vec<_>>().join(" "),
                            s.cwd
                        )
                    }),
            );
            *selection = (*selection).min(labels.len().saturating_sub(1));
            let search_area = Rect::new(area.x, area.y, area.width, 3.min(area.height));
            input.set_block(block(" Search Codex sessions ", search));
            frame.render_widget(&*input, search_area);
            let list_area = Rect::new(
                area.x,
                search_area.bottom(),
                area.width,
                area.height - search_area.height,
            );
            let filters = [
                ("Filter", ["CWD", "All"], options.all),
                ("Status", ["Active", "Archived"], options.archived),
                ("Sort", ["Updated", "Created"], options.created),
            ];
            let summary = if loading {
                "Loading sessions… ".into()
            } else {
                format!("{} sessions ", labels.len().saturating_sub(2))
            };
            let title_width = " Destination · ".width()
                + summary.width()
                + usize::from(!search)
                + filters
                    .iter()
                    .map(|(name, values, _)| {
                        name.width() + 2 + values[0].width() + values[1].width() + 5 + " · ".width()
                    })
                    .sum::<usize>();
            let stacked_filters = title_width + 2 > list_area.width as usize;
            let mut title = vec![Span::raw(" Destination · ")];
            let mut filter_labels = Vec::new();
            let mut filter_areas = [[Rect::default(); 2]; 3];
            let hit_bounds =
                list_area.inner(ratatui::layout::Margin::new(1, u16::from(stacked_filters)));
            let mut x = list_area
                .x
                .saturating_add(1 + " Destination · ".width() as u16);
            for (i, (name, values, second)) in filters.iter().enumerate() {
                let y = if stacked_filters {
                    x = hit_bounds.x;
                    hit_bounds.y.saturating_add(i as u16)
                } else {
                    list_area.y
                };
                let focused = !search && *control == i;
                let mut label = format!("{}{name}: ", if focused { ">" } else { "" });
                for (side, value) in values.iter().enumerate() {
                    if side > 0 {
                        label.push_str(" / ");
                    }
                    let value = if *second == (side == 1) {
                        format!("[{value}]")
                    } else {
                        (*value).into()
                    };
                    let start = x.saturating_add(label.width() as u16);
                    filter_areas[i][side] =
                        Rect::new(start, y, value.width() as u16, 1).intersection(hit_bounds);
                    label.push_str(&value);
                }
                x = x.saturating_add(label.width() as u16 + " · ".width() as u16);
                let span = Span::styled(
                    label,
                    if focused {
                        selected()
                    } else {
                        Style::default()
                    },
                );
                if stacked_filters {
                    filter_labels.push(span);
                } else {
                    title.extend([span, Span::raw(" · ")]);
                }
            }
            title.push(Span::raw(summary));
            let border = block(Line::from(title), !search);
            let mut inner = border.inner(list_area);
            frame.render_widget(border, list_area);
            let rows = filter_labels.len().min(inner.height as usize) as u16;
            for (i, label) in filter_labels.into_iter().take(rows as usize).enumerate() {
                frame.render_widget(
                    Paragraph::new(Line::from(label)),
                    Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
                );
            }
            inner.y += rows;
            inner.height -= rows;
            if loading {
                text(frame, inner, "Loading sessions…", 0, true);
            } else if let Some((offset, manual_scroll, list_area, areas)) = viewport {
                *list_area = inner;
                *areas = filter_areas;
                list(
                    frame,
                    inner,
                    &labels,
                    (*selection, !*manual_scroll, 3),
                    offset,
                    !search,
                    false,
                );
            }
        }
        Modal::Preview {
            destination,
            id: _,
            copy: _,
            archived: _,
            list_offset,
            selection,
            pane,
            offsets,
        } => {
            frame.render_widget(Clear, area);
            let header_height = 9.min(area.height);
            let header = bordered(
                frame,
                Rect::new(area.x, area.y, area.width, header_height),
                " Send preview · destination ".into(),
                false,
            );
            text(
                frame,
                header,
                &format!(
                    "Destination: {}\nReview directory: {}\nComments: {}{}",
                    destination,
                    review.root,
                    review.pending(),
                    if language.is_empty() {
                        String::new()
                    } else {
                        format!(" · Response language: {language}")
                    }
                ),
                0,
                true,
            );
            let content = Rect::new(
                area.x,
                area.y + header_height,
                area.width,
                area.height - header_height,
            );
            let left = Rect::new(content.x, content.y, content.width / 4, content.height);
            let right = Rect::new(
                left.right(),
                content.y,
                content.width - left.width,
                content.height,
            );
            let refs: Vec<_> = review.pending_comments().collect();
            let labels: Vec<_> = refs
                .iter()
                .enumerate()
                .map(|(i, (history, f, c))| {
                    format!(
                        "{}. {}{} · {}",
                        i + 1,
                        if *history { "[history] " } else { "" },
                        f.path,
                        c.location(&f.lines)
                    )
                })
                .collect();
            let inner = bordered(frame, left, " Comments to send ".into(), *pane == 0);
            list(
                frame,
                inner,
                &labels,
                (*selection, true, 3),
                list_offset,
                *pane == 0,
                false,
            );
            if let Some((history, file, comment)) = refs.get(*selection) {
                let body = Rect::new(right.x, right.y, right.width, right.height / 3);
                let code = Rect::new(
                    right.x,
                    body.bottom(),
                    right.width,
                    right.height - body.height,
                );
                let inner = bordered(
                    frame,
                    body,
                    if *history {
                        " Historical comment "
                    } else {
                        " Comment "
                    }
                    .into(),
                    *pane == 1,
                );
                text(
                    frame,
                    inner,
                    &format!(
                        "{}\nLines: {}{}\n\n{}",
                        file.path,
                        comment.location(&file.lines),
                        if *history { " (historical)" } else { "" },
                        comment.text
                    ),
                    offsets[0],
                    true,
                );
                let inner = bordered(
                    frame,
                    code,
                    " Code excerpt · * selected ".into(),
                    *pane == 2,
                );
                let mut excerpt = String::new();
                use std::fmt::Write;
                for i in comment.excerpt(&file.lines) {
                    let l = &file.lines[i];
                    let _ = writeln!(
                        excerpt,
                        "{} {:5} / {:5} {}",
                        if comment.includes(i, l) { "*" } else { " " },
                        l.old,
                        l.new,
                        l.text.replace('\t', "    ")
                    );
                }
                if comment.file {
                    excerpt.push_str("File-wide comment (no code excerpt)");
                }
                text(frame, inner, &excerpt, offsets[1], false);
            }
        }
    }
}
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
pub const HELP: &str = r#"

Global
0/1/2/3: focus Diff / Files / Comments / Commits
h/l or ←→: focus previous/next pane or diff side
Tab: move focus to the next pane
t: switch side-by-side / stacked layout (stacked: 1/2/3 select top pane, 0 focuses Diff below)
+/-: expand/shrink pane (Diff or stacked: normal / full; others: normal / tall / full)
{/}: grow/shrink pane 0 by 5% (shrink/grow panes 1–3; width in side-by-side, height in stacked); drag the sidebar/Diff border to resize
Ctrl+Enter/F2: preview and send unsent Open comments
r: reload diff and file/commit lists   R: choose Archive or Reset all comments   q: quit the app

Diff
z: expand 10 nearby lines; Z: collapse if expanded, otherwise full file (additional context is read-only)
n/N: jump to next/previous diff hunk   s: switch unified / side-by-side display   f: toggle wrapping of long diff lines
[: widen NEW side, narrow OLD side   ]: widen OLD side, narrow NEW side
/: search text in the current diff   v: start/clear range; extend with j/k   c/Enter: add a comment to selected lines
x: toggle selected comment Done / Open   d: delete the selected comment   e: edit a comment in your external editor

Files
o: show all files / only files with matching comments
u: filter Open / all comments (shared with Comments)
/: filter files by path   n/N: jump to next/previous filtered file
c: add a file comment (whole file)   Enter: focus Diff for the selected file   e: edit a comment in your external editor

Comments
/: filter comments by text or file path   n/N: jump to next/previous search match   Esc: clear the current search/filter
u: show Open / all comments (shared with Files)   o: show comments for current file / all files   Enter: edit the selected comment; inspect history comments   v: inspect recorded comment and code
x: toggle selected comment Done / Open   X: mark all comments matching filters Done   d: delete the selected comment

Commits
/: filter commits by hash, message or ref   n/N: jump to next/previous search match   Esc: clear the current search/filter
Enter/Space: select/unselect up to two commits
No selection: HEAD → working tree
One selection: commit → working tree
Two selections: older commit → newer commit
First row: select to clear chosen commits
"#;

fn help_lines(app: &App) -> Vec<Line<'static>> {
    let zoom_hint = if app.stacked || app.pane == 0 {
        "+/-: expand/shrink pane (normal / full)"
    } else {
        "+/-: expand/shrink pane (normal / tall / full)"
    };
    let mut hints = Vec::new();
    hints.push("t: switch side-by-side / stacked layout; 1/2/3 select top pane in stacked layout");
    match app.pane {
        3 => hints.extend([
            "/: filter commits by hash, message or ref",
            "n/N: jump to next/previous search match",
            "Esc: clear the current search/filter",
            "↑↓/jk: move the selected commit",
            "Enter/Space: select/unselect up to two commits",
            "0/1/2/3: focus Diff / Files / Comments / Commits",
            zoom_hint,
            "r: reload diff and file/commit lists",
            "?: show key bindings",
            "q: quit the app",
        ]),
        2 => {
            if let Some(r) = app.selected_ref() {
                hints.push(if r.history {
                    "Enter: inspect the recorded comment and code"
                } else {
                    "Enter: edit the selected comment"
                });
                hints.extend([
                    "↑↓/jk: move the selected comment",
                    "v: inspect recorded comment and code",
                    "x: toggle selected comment Done / Open",
                    "X: mark all comments matching filters Done",
                    "d: delete the selected comment",
                ]);
            }
            hints.extend([
                "/: filter comments by text or file path",
                "n/N: jump to next/previous search match",
                "Esc: clear the current search/filter",
                "u: show all / Open comments (shared with Files)",
                "o: show comments for current file / all files",
                "←→/hl/0/1/2/3: focus Diff / Files / Comments / Commits",
                zoom_hint,
                "R: choose Archive or Reset all comments",
                "q: quit the app",
            ]);
        }
        _ => {
            if app.pane == 1 {
                hints.extend([
                    "o: show all files / only commented files",
                    "u: filter Open / all comments (shared with Comments)",
                ]);
            }
            if let Some(file) = app.current() {
                if app.pane == 1 {
                    hints.extend([
                        "↑↓/jk: select a file and show its diff",
                        "Enter: focus Diff for the selected file",
                        "c: add a file comment (whole file)",
                        "v: focus Diff and start/clear range selection",
                    ]);
                } else if app.view().is_some_and(|v| !v.is_empty()) {
                    hints.push("↑↓/jk: move selected line; extend active range");
                    if app.split() {
                        hints.extend([
                            "←→/hl: move focus between panes / diff sides",
                            "[: widen NEW side, narrow OLD side",
                            "]: widen OLD side, narrow NEW side",
                        ]);
                    }
                    let comment = app.selected_ref().is_some();
                    if app.bounds().is_some_and(|(a, b)| {
                        file.lines[a..=b].iter().any(|l| l.old > 0 || l.new > 0)
                    }) {
                        hints.push(if comment {
                            "c: add another comment to selected lines"
                        } else {
                            "c/Enter: add a comment to selected lines"
                        });
                        hints.push("v: start/clear range; extend with j/k");
                    }
                    if comment {
                        hints.extend([
                            "Enter: edit the selected comment",
                            "x: toggle selected comment Done / Open",
                            "d: delete the selected comment",
                        ]);
                    }
                    if app.anchor.is_some() {
                        hints.push("Esc: clear the selected range");
                    }
                }
                hints.extend([
                    "s: switch unified / side-by-side display",
                    "z: expand 10 nearby lines; Z: collapse if expanded, otherwise full file (additional context is read-only)",
                    if app.wrap {
                        "f: disable wrapping of long diff lines"
                    } else {
                        "f: enable wrapping of long diff lines"
                    },
                ]);
                hints.push(
                    if app.pane == 0 || app.queries[1].is_empty() && !app.commented_files_only {
                        "n/N: jump to next/previous diff hunk"
                    } else {
                        "n/N: jump to next/previous search match"
                    },
                );
            }
            hints.push("/: search diff text or filter file paths");
            if !app.queries[app.pane].is_empty() {
                hints.push("Esc: clear the current search/filter");
            }
            if app.review.pending() > 0 {
                hints.push("Ctrl+Enter/F2: preview and send unsent Open comments");
            }
            hints.extend([
                "R: choose Archive or Reset all comments",
                "e: edit a comment in your external editor",
                "←→/hl/0/1/2/3: focus Diff / Files / Comments / Commits",
                "Tab: move focus to the next pane",
                zoom_hint,
                "r: reload diff and file/commit lists",
                "q: quit the app",
            ]);
        }
    }
    let text = format!("Current pane\n\n{}{}", hints.join("\n"), HELP).replace("   ", "\n");
    let width = text
        .lines()
        .filter_map(|l| l.split_once(": ").map(|(k, _)| k.width()))
        .max()
        .unwrap_or(0);
    text.lines()
        .map(|line| {
            if let Some((key, description)) = line.split_once(": ") {
                Line::from(vec![
                    Span::styled(key.to_owned(), Style::default().fg(Color::Rgb(0, 255, 255))),
                    Span::raw(" ".repeat(width - key.width() + 3)),
                    Span::raw(description.to_owned()),
                ])
            } else {
                Line::styled(
                    line.to_owned(),
                    Style::default()
                        .fg(Color::Rgb(255, 255, 0))
                        .add_modifier(ratatui::style::Modifier::BOLD),
                )
            }
        })
        .collect()
}

#[cfg(test)]
mod contrast_tests {
    use super::*;
    #[test]
    fn selected_colors_preserve_readable_colors_and_check_inversion() {
        assert_eq!(
            selected_foreground(Color::Rgb(255, 255, 0)),
            Color::Rgb(0, 0, 255)
        );
        for color in [
            Color::Rgb(128, 0, 0),
            Color::Rgb(0, 128, 0),
            Color::Rgb(0, 0, 128),
        ] {
            assert_eq!(selected_foreground(color), color);
        }
        assert_eq!(selected_foreground(Color::Rgb(255, 0, 0)), Color::Black);
        for r in [0, 64, 128, 192, 255] {
            for g in [0, 64, 128, 192, 255] {
                for b in [0, 64, 128, 192, 255] {
                    let color = selected_foreground(Color::Rgb(r, g, b));
                    let fg = match color {
                        Color::Rgb(r, g, b) => luminance([r, g, b]),
                        Color::Black => 0.,
                        _ => unreachable!(),
                    };
                    let bg = luminance([224, 255, 255]);
                    assert!((fg.max(bg) + 0.05) / (fg.min(bg) + 0.05) >= 4.5);
                }
            }
        }
    }
}
