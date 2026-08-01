use crate::ui::colors;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Gauge, Widget};

pub struct ProgressBar {
    percent: u16,
    label: String,
    fg: Color,
    show_label: bool,
    block: Option<Block<'static>>,
}

impl ProgressBar {
    pub fn new(percent: f64) -> Self {
        Self {
            percent: percent.clamp(0.0, 100.0) as u16,
            label: String::new(),
            fg: colors::GREEN,
            show_label: false,
            block: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self.show_label = true;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.fg = color;
        self
    }

    pub fn block(mut self, block: Block<'static>) -> Self {
        self.block = Some(block);
        self
    }
}

impl Widget for ProgressBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let label_text = if self.show_label {
            self.label.as_str()
        } else {
            ""
        };
        let gauge = Gauge::default()
            .percent(self.percent)
            .label(label_text)
            .style(Style::default().fg(self.fg))
            .block(self.block.unwrap_or_default());
        gauge.render(area, buf);
    }
}
