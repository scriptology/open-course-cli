use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Widget, Wrap};

pub struct Card<'a> {
    title: String,
    lines: Vec<Line<'a>>,
    padding: Padding,
}

impl<'a> Card<'a> {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            lines: Vec::new(),
            padding: Padding::ZERO,
        }
    }

    /// Inner padding between the border and the content: airy cards use
    /// `Padding::new(2, 2, 1, 1)`.
    pub fn padding(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    pub fn line(mut self, line: impl Into<Line<'a>>) -> Self {
        self.lines.push(line.into());
        self
    }
}

impl<'a> Widget for Card<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(Line::from(self.title))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .padding(self.padding);
        let inner = block.inner(area);
        block.render(area, buf);
        Paragraph::new(Text::from(self.lines))
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: true })
            .render(inner, buf);
    }
}
