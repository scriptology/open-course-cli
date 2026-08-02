use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

/// Single source of the error style across the CLI.
pub const ERROR_COLOR: Color = Color::Red;

/// Inline error rendering: the message as-is, styled with the shared error
/// color. This is the one way errors are shown inside views.
pub fn error_lines(message: &str) -> Vec<Line<'static>> {
    message
        .lines()
        .map(|line| {
            Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(ERROR_COLOR),
            ))
        })
        .collect()
}

pub struct ErrorBox {
    message: String,
    title: &'static str,
    footer_hint: &'static str,
}

impl ErrorBox {
    pub fn new(message: impl Into<String>, title: &'static str, footer_hint: &'static str) -> Self {
        Self {
            message: message.into(),
            title,
            footer_hint,
        }
    }
}

impl Widget for ErrorBox {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(self.title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ERROR_COLOR));
        let inner = block.inner(area);
        block.render(area, buf);

        let text = format!("{}\n\n{}", self.message, self.footer_hint);
        Paragraph::new(text)
            .style(Style::default().fg(ERROR_COLOR))
            .wrap(Wrap { trim: true })
            .render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_lines_styles_every_line_as_error_without_changes() {
        let lines = error_lines("boom\nsecond line");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].content, "boom");
        assert_eq!(lines[1].spans[0].content, "second line");
        for line in &lines {
            assert_eq!(line.spans[0].style.fg, Some(ERROR_COLOR));
        }
    }
}
