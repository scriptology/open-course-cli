use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};

/// Bordered single-line input box with a block caret at the end of the text.
/// The caller passes the text already prepared for display (e.g. masked for
/// secrets); the box only renders it plus the caret. An optional title is
/// shown on the top border.
pub fn input_paragraph(display: &str, title: Option<&str>, accent: Color) -> Paragraph<'static> {
    let text = Text::from(Line::from(vec![
        Span::raw(display.to_string()),
        Span::styled(
            "█",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
    ]));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent));
    if let Some(title) = title {
        block = block.title(Span::styled(
            title.to_string(),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    }
    Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(Color::White))
}
