use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

pub struct ErrorBox {
    message: String,
}

impl ErrorBox {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Widget for ErrorBox {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title("Error")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red));
        let inner = block.inner(area);
        block.render(area, buf);

        let text = format!("{}\n\nr: retry | m: change model | q: quit", self.message);
        Paragraph::new(text)
            .style(Style::default().fg(Color::Red))
            .wrap(Wrap { trim: true })
            .render(inner, buf);
    }
}
