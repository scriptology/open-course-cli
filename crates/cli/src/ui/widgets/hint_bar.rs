use crate::ui::colors;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

pub struct HintBar {
    hints: Vec<(String, String)>,
    model_name: Option<String>,
    accent_key: Option<String>,
}

impl HintBar {
    pub fn new(hints: &[(&str, &str)]) -> Self {
        Self {
            hints: hints
                .iter()
                .map(|(k, l)| (k.to_string(), l.to_string()))
                .collect(),
            model_name: None,
            accent_key: None,
        }
    }

    pub fn push(&mut self, key: &str, label: &str) {
        self.hints.push((key.to_string(), label.to_string()));
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model_name = Some(model.into());
        self
    }

    /// Highlights one hint (by its key) in green — used for the primary
    /// action of the screen.
    pub fn accent(mut self, key: &str) -> Self {
        self.accent_key = Some(key.to_string());
        self
    }
}

impl Widget for HintBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let hints_spans: Vec<Span> = self
            .hints
            .iter()
            .flat_map(|(key, label)| {
                let accented = self.accent_key.as_deref() == Some(key.as_str());
                let key_style = if accented {
                    Style::default()
                        .fg(colors::GREEN)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(colors::BLUE)
                        .add_modifier(Modifier::BOLD)
                };
                let label_span = if accented {
                    Span::styled(format!("{}  ", label), Style::default().fg(colors::GREEN))
                } else {
                    Span::raw(format!("{}  ", label))
                };
                [Span::styled(format!("{}:", key), key_style), label_span]
            })
            .collect();

        let hints_text: String = self
            .hints
            .iter()
            .map(|(key, label)| format!("{}:{}  ", key, label))
            .collect();

        let mut spans = hints_spans;

        if let Some(model) = &self.model_name {
            let hints_len = hints_text.chars().count();
            let model_text = format!(" [{}]", model);
            let model_len = model_text.chars().count();
            let area_width = area.width as usize;

            if hints_len + model_len < area_width {
                let padding = area_width - hints_len - model_len;
                spans.push(Span::raw(" ".repeat(padding)));
                spans.push(Span::styled(
                    model_text,
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }

        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}
