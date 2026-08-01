use crate::ui::colors;
use ratatui::style::{Color, Modifier, Style};
use tui_markdown::StyleSheet;

#[derive(Debug, Clone, Copy)]
pub struct OpenCourseStyleSheet;

impl StyleSheet for OpenCourseStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            1 => Style::default()
                .fg(colors::BLUE)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            2 => Style::default()
                .fg(colors::BLUE)
                .add_modifier(Modifier::BOLD),
            3 => Style::default()
                .fg(colors::BLUE)
                .add_modifier(Modifier::BOLD | Modifier::ITALIC),
            _ => Style::default()
                .fg(colors::BLUE)
                .add_modifier(Modifier::ITALIC),
        }
    }

    fn code(&self) -> Style {
        Style::default().fg(Color::White).bg(Color::DarkGray)
    }

    fn link(&self) -> Style {
        Style::default()
            .fg(colors::BLUE)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        Style::default().fg(colors::YELLOW)
    }

    fn heading_meta(&self) -> Style {
        Style::default().add_modifier(Modifier::DIM)
    }

    fn metadata_block(&self) -> Style {
        Style::default().fg(colors::YELLOW)
    }
}
