use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};

use crate::ui::help::HelpGroup;

pub struct HelpOverlay<'a> {
    groups: &'a [HelpGroup],
}

impl<'a> HelpOverlay<'a> {
    pub fn new(groups: &'a [HelpGroup]) -> Self {
        Self { groups }
    }
}

impl Widget for HelpOverlay<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let content_height: u16 = self
            .groups
            .iter()
            .filter(|g| !g.entries.is_empty())
            .map(|g| g.entries.len() as u16 + 2)
            .sum::<u16>()
            + 3;

        let width = area.width.min(60);
        let height = area.height.min(content_height).max(5);
        let x = area.x + area.width.saturating_sub(width) / 2;
        let y = area.y + area.height.saturating_sub(height) / 2;
        let popup = Rect {
            x,
            y,
            width,
            height,
        };

        Clear.render(popup, buf);

        let block = Block::default()
            .title("Help")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue));
        let inner = block.inner(popup);
        block.render(popup, buf);

        let mut lines: Vec<Line> = Vec::new();
        for group in self.groups.iter().filter(|g| !g.entries.is_empty()) {
            lines.push(Line::from(Span::styled(
                group.title,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            for entry in &group.entries {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {:<14}", entry.key),
                        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(entry.action.clone()),
                ]));
            }
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            "Esc / ?: close",
            Style::default().fg(Color::DarkGray),
        )));

        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .render(inner, buf);
    }
}
