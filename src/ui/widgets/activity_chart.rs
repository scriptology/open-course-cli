use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Widget};

use crate::core::dashboard::DailyActivity;

const BAR_SYMBOL: &str = "█";
const GAP: u16 = 1;

pub struct ActivityChart {
    data: Vec<DailyActivity>,
    block: Option<Block<'static>>,
}

impl ActivityChart {
    pub fn new(data: &[DailyActivity]) -> Self {
        Self {
            data: data.to_vec(),
            block: None,
        }
    }

    pub fn block(mut self, block: Block<'static>) -> Self {
        self.block = Some(block);
        self
    }
}

impl Widget for ActivityChart {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let inner = self
            .block
            .as_ref()
            .map(|b| b.inner(area))
            .unwrap_or(area);

        if let Some(block) = self.block {
            block.render(area, buf);
        }

        if inner.width == 0 || inner.height == 0 || self.data.is_empty() {
            return;
        }

        let day_count = self.data.len();
        let total_gap = (day_count.saturating_sub(1) as u16) * GAP;
        let bar_width = ((inner.width.saturating_sub(total_gap)) / day_count as u16).max(1);

        let log_values: Vec<(f64, f64, f64)> = self
            .data
            .iter()
            .map(|d| {
                let sessions = (d.sessions + 1) as f64;
                let new_topics = (d.new_topics + 1) as f64;
                let completed = (d.completed_topics + 1) as f64;
                (sessions.ln(), new_topics.ln(), completed.ln())
            })
            .collect();

        let mut max_total = 0.0f64;
        for (s, n, c) in &log_values {
            max_total = max_total.max(s + n + c);
        }

        let available_height = inner.height as f64;

        for (day_idx, (s, n, c)) in log_values.iter().enumerate() {
            let total = s + n + c;
            let bar_height = if max_total > 0.0 {
                ((total / max_total) * available_height)
                    .round()
                    .clamp(0.0, available_height) as u16
            } else {
                0
            };
            let bar_height = bar_height.min(inner.height);

            let bar_x = inner.x + (day_idx as u16) * (bar_width + GAP);
            if bar_x >= inner.x + inner.width {
                break;
            }

            let seg_total = total.max(1e-10);
            let mut h_sessions = ((s / seg_total) * bar_height as f64).round() as u16;
            let mut h_completed = ((c / seg_total) * bar_height as f64).round() as u16;
            let mut h_new_topics = bar_height.saturating_sub(h_sessions).saturating_sub(h_completed);

            // Fix rounding so the sum exactly matches bar_height when possible.
            let drawn = h_sessions + h_completed + h_new_topics;
            if drawn < bar_height {
                h_new_topics += bar_height - drawn;
            } else if drawn > bar_height {
                if h_new_topics >= drawn - bar_height {
                    h_new_topics -= drawn - bar_height;
                } else if h_sessions >= drawn - bar_height {
                    h_sessions -= drawn - bar_height;
                } else {
                    h_completed -= drawn - bar_height;
                }
            }

            let segments = [
                (h_sessions, Color::Rgb(0, 122, 255)),
                (h_new_topics, Color::Yellow),
                (h_completed, Color::Green),
            ];

            let mut current_y = inner.y + inner.height - 1;
            for (h, color) in segments {
                for _ in 0..h {
                    if current_y < inner.y {
                        break;
                    }
                    for dx in 0..bar_width {
                        let x = bar_x + dx;
                        if x >= inner.x + inner.width {
                            break;
                        }
                        let cell = &mut buf[(x, current_y)];
                        cell.set_symbol(BAR_SYMBOL);
                        cell.set_style(Style::default().fg(color));
                    }
                    current_y = current_y.saturating_sub(1);
                }
            }
        }
    }
}
