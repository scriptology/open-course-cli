use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use crate::ui::colors;

pub fn draw_confirmation(
    frame: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    message: &str,
    footer: &str,
) {
    let popup_area = centered_rect(60, 30, area);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(colors::BLUE))
        .title_style(
            Style::default()
                .fg(colors::BLUE)
                .add_modifier(Modifier::BOLD),
        );

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(inner);

    let text = ratatui::text::Text::from(message);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(ratatui::widgets::Wrap { trim: true })
            .alignment(Alignment::Center),
        chunks[0],
    );

    let footer_text = Line::from(Span::styled(
        footer,
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(
        Paragraph::new(footer_text).alignment(Alignment::Center),
        chunks[1],
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
