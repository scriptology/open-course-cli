use ratatui::style::{Color, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;

pub fn logo() -> Paragraph<'static> {
    let text = Text::from(vec![
        Line::from("┌─┐┌─┐┌─┐┌┐┌  ┌─┐┌─┐┬ ┬┬─┐┌─┐┌─┐"),
        Line::from("│ │├─┘├┤ │││  │  │ ││ │├┬┘└─┐├┤ "),
        Line::from("└─┘┴  └─┘┘└┘  └─┘└─┘└─┘┴└─└─┘└─┘"),
    ]);
    Paragraph::new(text).style(Style::default().fg(Color::White))
}
