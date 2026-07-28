use crate::ui::labels::ReportLabels;

pub fn build_footer(entries: &[(&str, &str)]) -> String {
    entries
        .iter()
        .map(|(key, action)| format!("{key}: {action}"))
        .collect::<Vec<_>>()
        .join(" | ")
}

/// Footer entries describing the current mouse mode on scrollable views.
/// While the app captures the mouse the wheel scrolls and `m` switches to
/// native text selection; otherwise the mouse selects text and `m`
/// re-enables wheel scrolling.
pub fn mouse_footer_entries(
    capture_on: bool,
    labels: &ReportLabels,
) -> [(&'static str, &'static str); 2] {
    if capture_on {
        [("wheel", labels.wheel_scroll), ("m", labels.select_text)]
    } else {
        [("mouse", labels.select_text), ("m", labels.wheel_mode)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::labels::get_report_labels;

    #[test]
    fn capture_on_shows_wheel_and_select_toggle() {
        let labels = get_report_labels("en");
        let entries = mouse_footer_entries(true, &labels);
        assert_eq!(entries, [("wheel", "scroll"), ("m", "select text")]);
    }

    #[test]
    fn capture_off_shows_select_and_wheel_toggle() {
        let labels = get_report_labels("en");
        let entries = mouse_footer_entries(false, &labels);
        assert_eq!(entries, [("mouse", "select text"), ("m", "wheel scroll")]);
    }

    #[test]
    fn entries_are_localized() {
        let labels = get_report_labels("ru");
        let entries = mouse_footer_entries(true, &labels);
        assert_eq!(entries[1], ("m", "выделение текста"));
    }
}
