use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Sparkline, Widget};
use crate::ui::colors;

pub struct SparklineChart {
    data: Vec<u64>,
    style: Style,
    block: Option<Block<'static>>,
    gap: bool,
}

impl SparklineChart {
    pub fn new(data: &[f64]) -> Self {
        Self {
            data: data
                .iter()
                .map(|v| v.clamp(0.0, 100.0).round() as u64)
                .collect(),
            style: Style::default().fg(colors::BLUE),
            block: None,
            gap: false,
        }
    }

    pub fn color(mut self, color: Color) -> Self {
        self.style = Style::default().fg(color);
        self
    }

    pub fn block(mut self, block: Block<'static>) -> Self {
        self.block = Some(block);
        self
    }

    pub fn gap(mut self) -> Self {
        self.gap = true;
        self
    }
}

impl Widget for SparklineChart {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let data = if self.gap && self.data.len() >= 2 {
            let mut expanded = Vec::with_capacity(self.data.len() * 2 - 1);
            for (i, &v) in self.data.iter().enumerate() {
                expanded.push(v);
                if i < self.data.len() - 1 {
                    expanded.push(0);
                }
            }
            expanded
        } else {
            self.data
        };

        let sparkline = Sparkline::default()
            .data(&data)
            .style(self.style)
            .block(self.block.unwrap_or_default());
        sparkline.render(area, buf);
    }
}
