use ratatui::layout::{Constraint, Layout, Rect};

pub fn select_next_wrapping(state: &mut ratatui::widgets::ListState, len: usize) {
    if len == 0 {
        return;
    }
    let selected = state.selected().unwrap_or(0);
    state.select(Some((selected + 1) % len));
}

pub fn select_previous_wrapping(state: &mut ratatui::widgets::ListState, len: usize) {
    if len == 0 {
        return;
    }
    let selected = state.selected().unwrap_or(0);
    state.select(Some((selected + len - 1) % len));
}

pub fn screen_chunks(area: Rect) -> [Rect; 3] {
    Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area)
}
