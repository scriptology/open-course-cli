use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};

pub fn select_next_wrapping(state: &mut ratatui::widgets::ListState, len: usize) {
    if len == 0 {
        return;
    }
    let selected = state.selected().unwrap_or(0);
    state.select(Some((selected + 1) % len));
}

pub fn select_previous_wrapping(state: &mut ratatui::widgets::ListState, len: usize) {
    if len == 0 {
        return;
    }
    let selected = state.selected().unwrap_or(0);
    state.select(Some((selected + len - 1) % len));
}

pub fn screen_chunks(area: Rect) -> [Rect; 3] {
    Layout::vertical([
        Constraint::Min(3),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(area)
}

/// Wraps a single-line text input to fit the given width and places the cursor
/// highlight on the correct wrapped line.
pub fn wrapped_input_text(input: &str, cursor: usize, width: usize) -> Text<'static> {
    let width = width.max(1);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current = String::new();
    let mut cursor_line = 0;
    let mut cursor_col = 0;
    let mut char_idx = 0;

    for c in input.chars() {
        if char_idx == cursor {
            cursor_line = lines.len();
            cursor_col = current.chars().count();
        }
        if current.chars().count() >= width && !current.is_empty() {
            lines.push(Line::from(current));
            current = String::new();
        }
        current.push(c);
        char_idx += 1;
    }
    if char_idx == cursor {
        cursor_line = lines.len();
        cursor_col = current.chars().count();
    }
    lines.push(Line::from(current));

    if let Some(line) = lines.get_mut(cursor_line) {
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        let before: String = text.chars().take(cursor_col).collect();
        let at = text.chars().nth(cursor_col).unwrap_or(' ');
        let after: String = text.chars().skip(cursor_col + 1).collect();
        *line = Line::from(vec![
            Span::raw(before),
            Span::styled(
                at.to_string(),
                Style::default().bg(Color::White).fg(Color::Black),
            ),
            Span::raw(after),
        ]);
    }
    Text::from(lines)
}
