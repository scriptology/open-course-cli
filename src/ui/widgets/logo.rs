use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

/// Logo widget.
///
/// Renders only when the allocated area is wide enough to hold the text.
/// On smaller screens the widget draws nothing so it does not fight for
/// space with the actual UI.
pub struct Logo;

impl Widget for Logo {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 22 || area.height < 1 {
            return;
        }

        let line = Line::from("ＯＰＥＮ  ＣＯＵＲＳＥ");
        Paragraph::new(line)
            .style(Style::default().fg(Color::White))
            .render(area, buf);
    }
}
