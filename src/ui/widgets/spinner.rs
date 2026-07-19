use crate::ui::colors;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Clone, Copy)]
pub struct Spinner {
    frame: usize,
}

impl Spinner {
    pub fn new() -> Self {
        Self { frame: 0 }
    }

    pub fn with_frame(frame: usize) -> Self {
        Self { frame }
    }

    pub fn symbol(&self) -> &'static str {
        FRAMES[self.frame % FRAMES.len()]
    }

    pub fn next(&mut self) {
        self.frame = (self.frame + 1) % FRAMES.len();
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Spinner {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let symbol = FRAMES[self.frame % FRAMES.len()];
        Paragraph::new(Span::styled(symbol, Style::default().fg(colors::YELLOW))).render(area, buf);
    }
}
