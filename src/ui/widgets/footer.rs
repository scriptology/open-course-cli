use crate::ui::labels::ReportLabels;

pub fn build_footer(entries: &[(&str, &str)]) -> String {
    entries
        .iter()
        .map(|(key, action)| format!("{key}: {action}"))
        .collect::<Vec<_>>()
        .join(" | ")
}

const SEPARATOR: &str = " | ";

/// Like [`build_footer`], but reflows the entries onto as many lines as the
/// given width requires: an entry that no longer fits moves to the next line
/// instead of being truncated by the terminal. An entry wider than the whole
/// line stays on its own line (and is truncated as before — nothing better
/// exists for that case).
pub fn build_footer_wrapped(entries: &[(&str, &str)], width: usize) -> String {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for (key, action) in entries {
        let entry = format!("{key}: {action}");
        if current.is_empty() {
            current = entry;
            continue;
        }
        let candidate_len = current.chars().count() + SEPARATOR.len() + entry.chars().count();
        if candidate_len > width {
            lines.push(std::mem::take(&mut current));
            current = entry;
        } else {
            current.push_str(SEPARATOR);
            current.push_str(&entry);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines.join("\n")
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
    fn wrapped_footer_fits_on_one_line_when_wide() {
        let entries = [("a", "first"), ("b", "second")];
        assert_eq!(build_footer_wrapped(&entries, 80), build_footer(&entries));
    }

    #[test]
    fn wrapped_footer_reflows_entries_to_next_line() {
        let entries = [("a", "first"), ("b", "second"), ("c", "third")];
        // "a: first | b: second" is 20 chars; adding " | c: third" (30) overflows.
        assert_eq!(
            build_footer_wrapped(&entries, 25),
            "a: first | b: second\nc: third"
        );
    }

    #[test]
    fn wrapped_footer_keeps_oversized_entry_on_own_line() {
        let entries = [("a", "x"), ("long-key", "a very long action description")];
        let wrapped = build_footer_wrapped(&entries, 10);
        assert_eq!(wrapped, "a: x\nlong-key: a very long action description");
    }

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
