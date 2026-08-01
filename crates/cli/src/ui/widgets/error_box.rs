use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

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
            .border_style(Style::default().fg(Color::Red));
        let inner = block.inner(area);
        block.render(area, buf);

        let text = format!("{}\n\n{}", self.message, self.footer_hint);
        Paragraph::new(text)
            .style(Style::default().fg(Color::Red))
            .wrap(Wrap { trim: true })
            .render(inner, buf);
    }
}
