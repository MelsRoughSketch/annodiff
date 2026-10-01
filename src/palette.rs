use ratatui::style::Color;

pub(crate) const FOCUS: Color = Color::LightCyan;
pub(crate) const MUTED: Color = Color::DarkGray;
pub(crate) const TEXT: Color = Color::White;
pub(crate) const SELECTION_TEXT: Color = Color::Black;
pub(crate) const LIST_SELECTION_BG: Color = Color::Rgb(224, 255, 255);
pub(crate) const LIST_CURSOR_BG: Color = Color::Rgb(84, 84, 84);
pub(crate) const LIST_CURSOR_FG: Color = Color::Rgb(80, 220, 220);
pub(crate) const LIST_INACTIVE_SELECTION_BG: Color = Color::Rgb(72, 72, 72);

pub(crate) const ADDED: Color = Color::LightGreen;
pub(crate) const MODIFIED: Color = Color::Rgb(255, 255, 0);
pub(crate) const REMOVED: Color = Color::Rgb(255, 0, 0);
pub(crate) const RENAMED: Color = Color::Rgb(0, 255, 255);
pub(crate) const CONFLICT: Color = Color::Rgb(255, 0, 255);
pub(crate) const NOTICE: Color = MODIFIED;
pub(crate) const KEY: Color = RENAMED;
pub(crate) const COMMIT_GRAPH: Color = RENAMED;
pub(crate) const COMMENT_OPEN: Color = Color::Yellow;
pub(crate) const COMMENT_DONE: Color = ADDED;
pub(crate) const COMMENT_SENT: Color = Color::Cyan;
pub(crate) const COMMENT_UNSENT: Color = Color::LightRed;

pub(crate) const REMOVAL_BG: Color = Color::Rgb(55, 25, 30);
pub(crate) const ADDITION_BG: Color = Color::Rgb(20, 45, 30);
pub(crate) const COMMENT_BG: Color = Color::Rgb(35, 44, 50);
pub(crate) const SEARCH_BG: Color = Color::Rgb(140, 100, 20);
pub(crate) const RANGE_BG: Color = Color::Rgb(55, 62, 68);
pub(crate) const SYNTAX_THEME: &str = "base16-ocean.dark";

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
pub(crate) fn selected_foreground(color: Color) -> Color {
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
