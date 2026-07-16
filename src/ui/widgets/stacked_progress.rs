use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;
use crate::ui::colors;

const BAR_SYMBOL: &str = "\u{2580}";

pub struct StackedProgressBar {
    not_started: f64,
    in_progress: f64,
    completed: f64,
}

impl StackedProgressBar {
    pub fn new(not_started: f64, in_progress: f64, completed: f64) -> Self {
        let total = not_started + in_progress + completed;
        if total == 0.0 {
            Self {
                not_started: 0.0,
                in_progress: 0.0,
                completed: 0.0,
            }
        } else {
            Self {
                not_started: not_started / total,
                in_progress: in_progress / total,
                completed: completed / total,
            }
        }
    }
}

impl Widget for StackedProgressBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let width = area.width as f64;
        let completed_width = (self.completed * width).round() as u16;
        let in_progress_width = (self.in_progress * width).round() as u16;
        let not_started_width = (self.not_started * width).round() as u16;

        let mut x = area.x;

        if completed_width > 0 {
            for i in 0..completed_width {
                buf[(x + i, area.y)].set_symbol(BAR_SYMBOL);
                buf[(x + i, area.y)].set_style(Style::default().fg(colors::GREEN));
            }
            x += completed_width;
        }

        if in_progress_width > 0 {
            for i in 0..in_progress_width {
                buf[(x + i, area.y)].set_symbol(BAR_SYMBOL);
                buf[(x + i, area.y)].set_style(Style::default().fg(colors::YELLOW));
            }
            x += in_progress_width;
        }

        if not_started_width > 0 {
            for i in 0..not_started_width {
                buf[(x + i, area.y)].set_symbol(BAR_SYMBOL);
                buf[(x + i, area.y)].set_style(Style::default().fg(colors::BLUE));
            }
        }
    }
}
