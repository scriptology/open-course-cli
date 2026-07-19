use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    Info,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub level: ToastLevel,
    expires_at: Instant,
}

impl Toast {
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(message, ToastLevel::Error, Duration::from_secs(6))
    }

    pub fn info(message: impl Into<String>) -> Self {
        Self::new(message, ToastLevel::Info, Duration::from_secs(4))
    }

    fn new(message: impl Into<String>, level: ToastLevel, ttl: Duration) -> Self {
        Self {
            message: message.into(),
            level,
            expires_at: Instant::now() + ttl,
        }
    }

    pub fn expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

pub struct ToastWidget<'a> {
    toast: &'a Toast,
}

impl<'a> ToastWidget<'a> {
    pub fn new(toast: &'a Toast) -> Self {
        Self { toast }
    }
}

impl Widget for ToastWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let color = match self.toast.level {
            ToastLevel::Error => Color::Red,
            ToastLevel::Info => Color::Green,
        };
        let title = match self.toast.level {
            ToastLevel::Error => "Error",
            ToastLevel::Info => "Info",
        };

        let max_width = area.width.saturating_sub(2).max(10);
        let width = (self.toast.message.chars().count() as u16 + 4)
            .min(max_width)
            .max(20);
        let text_width = width.saturating_sub(2);
        let wrapped_lines = (self.toast.message.chars().count() as u16 / text_width.max(1)) + 1;
        let height = (wrapped_lines + 2).min(area.height.saturating_sub(2)).max(3);

        let x = area.x + area.width.saturating_sub(width + 2);
        let y = area.y + area.height.saturating_sub(height + 1);
        let popup = Rect {
            x,
            y,
            width,
            height,
        };

        Clear.render(popup, buf);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(color));
        let inner = block.inner(popup);
        block.render(popup, buf);

        Paragraph::new(self.toast.message.as_str())
            .style(Style::default().fg(color))
            .wrap(Wrap { trim: true })
            .render(inner, buf);
    }
}
