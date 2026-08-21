//! Terminal-style kill shortcuts (Ctrl+U / Ctrl+W / Ctrl+K) for single-line
//! text inputs. Terminals report these as `Char` + `CONTROL`, and the view
//! handlers only see the `KeyCode`, so without this interception the keys
//! would insert literal `u`/`w`/`k` characters into the input.

use ratatui::crossterm::event::KeyCode;

use crate::app::{AppState, View};
use crate::ui::views::session;
use crate::ui::views::settings::Section;

/// Applies a kill shortcut to the currently active text input. Returns `true`
/// when the key was consumed, so the caller can skip further dispatch.
pub fn handle_kill_shortcut(state: &mut AppState, code: KeyCode) -> bool {
    if !matches!(
        code,
        KeyCode::Char('u') | KeyCode::Char('w') | KeyCode::Char('k')
    ) {
        return false;
    }
    match state.view {
        View::Session if matches!(state.session.mode, session::Mode::Practicing) => {
            apply(code, &mut state.session.input, &mut state.session.cursor)
        }
        View::Settings if state.settings.is_text_input_active() => {
            if state.settings.section == Section::Provider {
                // Provider setup inputs render the caret at the end and do not
                // track a cursor, so edit from the end of the line.
                let mut end = state.settings.input.chars().count();
                apply(code, &mut state.settings.input, &mut end)
            } else {
                apply(code, &mut state.settings.input, &mut state.settings.cursor)
            }
        }
        View::Onboarding if state.onboarding.is_text_step_active() => {
            // Onboarding inputs are append-only; the cursor is always at the end.
            let mut end = state.onboarding.input.chars().count();
            apply(code, &mut state.onboarding.input, &mut end)
        }
        _ => false,
    }
}

fn apply(code: KeyCode, input: &mut String, cursor: &mut usize) -> bool {
    match code {
        KeyCode::Char('u') => kill_before_cursor(input, cursor),
        KeyCode::Char('w') => kill_word_before_cursor(input, cursor),
        KeyCode::Char('k') => kill_after_cursor(input, cursor),
        _ => return false,
    }
    true
}

fn clamp_cursor(input: &str, cursor: &mut usize) {
    let len = input.chars().count();
    if *cursor > len {
        *cursor = len;
    }
}

/// Byte offsets of every char boundary, including the end of the string.
fn char_boundaries(input: &str) -> Vec<usize> {
    input
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(input.len()))
        .collect()
}

/// Deletes everything before the cursor (readline `unix-line-discard`).
fn kill_before_cursor(input: &mut String, cursor: &mut usize) {
    clamp_cursor(input, cursor);
    if *cursor == 0 {
        return;
    }
    let boundaries = char_boundaries(input);
    input.drain(..boundaries[*cursor]);
    *cursor = 0;
}

/// Deletes from the cursor to the end of the line (readline `kill-line`).
fn kill_after_cursor(input: &mut String, cursor: &mut usize) {
    clamp_cursor(input, cursor);
    let boundaries = char_boundaries(input);
    input.truncate(boundaries[*cursor]);
}

/// Deletes the whitespace-separated word before the cursor, plus any
/// whitespace preceding it (readline `unix-word-rubout`).
fn kill_word_before_cursor(input: &mut String, cursor: &mut usize) {
    clamp_cursor(input, cursor);
    if *cursor == 0 {
        return;
    }
    let chars: Vec<char> = input.chars().collect();
    let mut start = *cursor;
    while start > 0 && chars[start - 1].is_whitespace() {
        start -= 1;
    }
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let boundaries = char_boundaries(input);
    input.drain(boundaries[start]..boundaries[*cursor]);
    *cursor = start;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(code: KeyCode, input: &str, cursor: usize) -> (String, usize) {
        let mut input = input.to_string();
        let mut cursor = cursor;
        assert!(apply(code, &mut input, &mut cursor));
        (input, cursor)
    }

    #[test]
    fn ctrl_u_clears_line_before_cursor() {
        assert_eq!(
            run(KeyCode::Char('u'), "hello world", 11),
            (String::new(), 0)
        );
        assert_eq!(
            run(KeyCode::Char('u'), "hello world", 5),
            (" world".to_string(), 0)
        );
    }

    #[test]
    fn ctrl_u_at_start_is_no_op() {
        assert_eq!(
            run(KeyCode::Char('u'), "hello", 0),
            ("hello".to_string(), 0)
        );
    }

    #[test]
    fn ctrl_k_deletes_to_end_of_line() {
        assert_eq!(
            run(KeyCode::Char('k'), "hello world", 5),
            ("hello".to_string(), 5)
        );
        assert_eq!(
            run(KeyCode::Char('k'), "hello", 5),
            ("hello".to_string(), 5)
        );
    }

    #[test]
    fn ctrl_w_deletes_previous_word() {
        // Words are whitespace-separated, like readline's unix-word-rubout.
        assert_eq!(
            run(KeyCode::Char('w'), "https://api.example.com/v1", 26),
            (String::new(), 0)
        );
        assert_eq!(
            run(KeyCode::Char('w'), "hello world  ", 13),
            ("hello ".to_string(), 6)
        );
        assert_eq!(
            run(KeyCode::Char('w'), "hello world", 5),
            (" world".to_string(), 0)
        );
    }

    #[test]
    fn shortcuts_handle_multibyte_chars() {
        let (input, cursor) = run(KeyCode::Char('u'), "привет мир", 10);
        assert_eq!((input.as_str(), cursor), ("", 0));

        let (input, cursor) = run(KeyCode::Char('w'), "привет мир", 10);
        assert_eq!((input.as_str(), cursor), ("привет ", 7));
    }

    #[test]
    fn cursor_beyond_end_is_clamped() {
        assert_eq!(run(KeyCode::Char('k'), "abc", 10), ("abc".to_string(), 3));
        assert_eq!(run(KeyCode::Char('u'), "abc", 10), (String::new(), 0));
    }
}
