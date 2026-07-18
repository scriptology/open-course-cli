# Settings UI Fixes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix spacing, navigation, and input controls on the settings screens (Provider list, Profile, Session, Data).

**Architecture:** Keep the existing declarative `FieldDef` table for settings fields, but switch the Session batch size to a discrete selector, remove the Profile CEFR field, add a visible caret to the Profile age input, and enable Up/Down navigation on the Data reset list. Layout spacing is fixed by trimming the header constraints and removing the extra blank body line.

**Tech Stack:** Rust, ratatui, crossterm, serde.

---

### Task 1: Fix settings layout spacing

**Files:**
- Modify: `src/ui/views/settings/mod.rs:124-139`
- Modify: `src/ui/views/settings/mod.rs:190-197`
- Modify: `src/ui/views/settings/mod.rs:240-247`

**Step 1: Remove the leading blank line in the section body**

In `build_body`, change:

```rust
let mut lines = vec![String::new()];
```

to:

```rust
let mut lines = vec![];
```

This removes the extra blank line between the header and the first field.

**Step 2: Tighten the section picker header**

In `draw_section_picker`, the header is only two lines tall (title + blank line). Change:

```rust
.constraints([
    Constraint::Length(4),
    Constraint::Min(0),
    Constraint::Length(1),
])
```

to:

```rust
.constraints([
    Constraint::Length(2),
    Constraint::Min(0),
    Constraint::Length(1),
])
```

**Step 3: Tighten the section page header**

In `draw_section_page`, the header is three lines tall (title + blank line + section label). Change:

```rust
.constraints([
    Constraint::Length(4),
    Constraint::Min(0),
    Constraint::Length(footer_height),
])
```

to:

```rust
.constraints([
    Constraint::Length(3),
    Constraint::Min(0),
    Constraint::Length(footer_height),
])
```

**Step 4: Run the app and visually verify the Provider/Profile/Session/Data screens have no extra blank line after the header**

Run: `cargo run -- --settings` (or open the settings screen in the dashboard)
Expected: the `> Provider` row appears immediately below the header, not two rows below.

**Step 5: Commit**

```bash
git add src/ui/views/settings/mod.rs
git commit -m "fix(settings): remove extra blank line and tighten header spacing"
```

---

### Task 2: Convert Session batch size to a selector and remove Hint mode

**Files:**
- Modify: `src/ui/views/settings/fields.rs`
- Modify: `src/ui/views/settings/mod.rs`

**Step 1: Remove the Hint mode field from `FIELDS`**

Delete the `FieldDef` for `Hint mode` in `src/ui/views/settings/fields.rs`.

**Step 2: Convert Batch size to a selector**

Change the Batch size `FieldDef` so `text_input` is `false` and add a selector-style display:

```rust
FieldDef {
    section: Section::Session,
    index: 0,
    label: "Batch size",
    text_input: false,
    display: |config: &OpenCourseConfig| {
        let size = config.preferences.batch_size;
        let suffix = if size == 3 { " (recommended)" } else { "" };
        format!("{}{}", size, suffix)
    },
    load: |config: &OpenCourseConfig| config.preferences.batch_size.to_string(),
    apply: |config: &mut OpenCourseConfig, value: &str| -> Result<()> {
        let size = value
            .parse::<u32>()
            .map_err(|_| AppError::Config(format!("Invalid batch size: {value}")))?;
        if !(2..=5).contains(&size) {
            return Err(AppError::Config("Batch size must be 2-5".to_string()));
        }
        config.preferences.batch_size = size;
        Ok(())
    },
}
```

**Step 3: Add Up/Down selector handling in `handle_key`**

In `src/ui/views/settings/mod.rs`, inside the `in_section` key handler, after the Tab/BackTab handling, add Up/Down handling for non-text fields:

```rust
KeyCode::Up | KeyCode::Char('k') => {
    if state.settings.section == Section::Session {
        state.settings.prev_batch_size();
    }
}
KeyCode::Down | KeyCode::Char('j') => {
    if state.settings.section == Section::Session {
        state.settings.next_batch_size();
    }
}
```

Add helper methods to `SettingsState`:

```rust
pub(super) fn next_batch_size(&mut self) {
    if let Some(config) = self.config.as_mut() {
        let current = config.preferences.batch_size;
        let next = if current >= 5 { 2 } else { current + 1 };
        config.preferences.batch_size = next;
    }
}

pub(super) fn prev_batch_size(&mut self) {
    if let Some(config) = self.config.as_mut() {
        let current = config.preferences.batch_size;
        let prev = if current <= 2 { 5 } else { current - 1 };
        config.preferences.batch_size = prev;
    }
}
```

Wait: `SettingsState` does not own `config`. In `handle_key`, the `AppState` owns `config`. So implement the helpers on `SettingsState` taking `config` as argument, or just inline the logic in `handle_key` using `state.config.as_mut()`.

Inline version is simpler:

```rust
KeyCode::Up | KeyCode::Char('k') => {
    if state.settings.section == Section::Session {
        if let Some(config) = state.config.as_mut() {
            let current = config.preferences.batch_size;
            config.preferences.batch_size = if current <= 2 { 5 } else { current - 1 };
        }
    }
}
KeyCode::Down | KeyCode::Char('j') => {
    if state.settings.section == Section::Session {
        if let Some(config) = state.config.as_mut() {
            let current = config.preferences.batch_size;
            config.preferences.batch_size = if current >= 5 { 2 } else { current + 1 };
        }
    }
}
```

Also update the footer for Session to mention Up/Down:

Change the footer builder so that for `Section::Session` it shows:

```
↑/↓: change | Tab: field | Enter: save | Esc: back
```

**Step 4: Run the Session settings screen and verify**

Run: `cargo run`, open Settings → Session.
Expected: only one field `Batch size` shown, cycling 2→3→4→5→2 with Up/Down. Value `3` shows suffix `(recommended)`.

**Step 5: Commit**

```bash
git add src/ui/views/settings/fields.rs src/ui/views/settings/mod.rs
git commit -m "feat(settings): batch size selector with 2-5 options and remove hint mode"
```

---

### Task 3: Enable Up/Down navigation on the Data reset screen

**Files:**
- Modify: `src/ui/views/settings/mod.rs`

**Step 1: Add Up/Down handling for the Data section**

In the `in_section` key handler, Up/Down should behave like Tab/BackTab for the Data section (and for all other non-text fields):

```rust
KeyCode::Up | KeyCode::Char('k') => {
    if state.settings.section == Section::Data {
        state.settings.prev_field();
    }
}
KeyCode::Down | KeyCode::Char('j') => {
    if state.settings.section == Section::Data {
        state.settings.next_field();
    }
}
```

**Step 2: Update the Data footer to mention Up/Down**

Change the Data footer from:

```rust
lines[0] = "Tab/Shift+Tab: action | Enter: reset | Esc: back".to_string();
```

to:

```rust
lines[0] = "↑/↓: action | Enter: reset | Esc: back".to_string();
```

**Step 3: Run the Data screen and verify Up/Down moves the `>` marker**

Run: `cargo run`, open Settings → Data.
Expected: pressing Up/Down cycles through the five reset rows.

**Step 4: Commit**

```bash
git add src/ui/views/settings/mod.rs
git commit -m "fix(settings): enable Up/Down navigation on Data reset screen"
```

---

### Task 4: Remove CEFR from Profile and add a caret to the Age input

**Files:**
- Modify: `src/ui/views/settings/fields.rs`
- Modify: `src/ui/views/settings/mod.rs`

**Step 1: Remove the CEFR field from `FIELDS`**

Delete the `FieldDef` for `CEFR` in `src/ui/views/settings/fields.rs`. Profile will then only have Age.

**Step 2: Add a cursor to `SettingsState`**

In `src/ui/views/settings/mod.rs`:

```rust
pub struct SettingsState {
    pub section: Section,
    pub active_field: usize,
    pub input: String,
    pub cursor: usize,
    pub error: Option<String>,
    ...
}
```

Initialize `cursor: 0` in `SettingsState::new`.

**Step 3: Load and reset cursor position**

In `load_input`, after setting `self.input`, set `self.cursor = self.input.chars().count()`.

In `ensure_input_loaded`, when the field changes, load_input resets cursor, so no extra change needed.

**Step 4: Update key handling for text input with cursor movement**

Replace the existing text-input block:

```rust
KeyCode::Char(c) if state.settings.is_text_field() => {
    state.settings.input.push(c);
}
KeyCode::Backspace if state.settings.is_text_field() => {
    state.settings.input.pop();
}
```

with full cursor-aware editing:

```rust
KeyCode::Char(c) if state.settings.is_text_field() => {
    insert_char(&mut state.settings.input, &mut state.settings.cursor, c);
}
KeyCode::Backspace if state.settings.is_text_field() => {
    remove_before(&mut state.settings.input, &mut state.settings.cursor);
}
KeyCode::Delete if state.settings.is_text_field() => {
    remove_at(&mut state.settings.input, &mut state.settings.cursor);
}
KeyCode::Left | KeyCode::Char('h') => {
    if state.settings.cursor > 0 {
        state.settings.cursor -= 1;
    }
}
KeyCode::Right | KeyCode::Char('l') => {
    let len = state.settings.input.chars().count();
    if state.settings.cursor < len {
        state.settings.cursor += 1;
    }
}
KeyCode::Home => {
    state.settings.cursor = 0;
}
KeyCode::End => {
    state.settings.cursor = state.settings.input.chars().count();
}
```

Add helper functions at the bottom of `mod.rs` (reuse the same logic from `src/ui/views/session.rs`):

```rust
fn clamp_cursor(input: &str, cursor: &mut usize) {
    let len = input.chars().count();
    if *cursor > len {
        *cursor = len;
    }
}

fn insert_char(input: &mut String, cursor: &mut usize, c: char) {
    clamp_cursor(input, cursor);
    if let Some(pos) = input.chars().take(*cursor).map(|c| c.len_utf8()).sum::<usize>().checked_sub(0) {
        input.insert(pos, c);
    } else {
        input.push(c);
    }
    *cursor += 1;
}

fn remove_before(input: &mut String, cursor: &mut usize) {
    clamp_cursor(input, cursor);
    if *cursor == 0 {
        return;
    }
    let byte_pos = input.chars().take(*cursor).map(|c| c.len_utf8()).sum();
    let prev_byte_pos = input.chars().take(*cursor - 1).map(|c| c.len_utf8()).sum();
    input.replace_range(prev_byte_pos..byte_pos, "");
    *cursor -= 1;
}

fn remove_at(input: &mut String, cursor: &mut usize) {
    clamp_cursor(input, cursor);
    if *cursor >= input.chars().count() {
        return;
    }
    let byte_pos = input.chars().take(*cursor).map(|c| c.len_utf8()).sum();
    let next_byte_pos = input.chars().take(*cursor + 1).map(|c| c.len_utf8()).sum();
    input.replace_range(byte_pos..next_byte_pos, "");
}
```

**Step 5: Render the active text field with a caret**

Change `build_body` to return `Text<'static>` instead of `String`. Update `draw_section_page` to render the returned Text widget.

In `build_body`:

```rust
fn build_body(state: &AppState) -> Text<'static> {
    let config = match state.config.as_ref() {
        Some(c) => c,
        None => return Text::from("No configuration available. Press Esc to return."),
    };

    if state.settings.section == Section::Provider && state.settings.in_section {
        return Text::from(provider_setup::build_provider_setup_body(state, config));
    }

    let mut lines = vec![];

    let count = state.settings.field_count();
    for i in 0..count {
        let is_active = i == state.settings.active_field;
        let marker = if is_active { "> " } else { "  " };
        let label = fields::field_label(state.settings.section, i);

        if is_active && state.settings.section != Section::Data && state.settings.is_text_field() {
            let value = &state.settings.input;
            let cursor = state.settings.cursor;
            let prefix = format!("{}{}: ", marker, label);
            let before: String = value.chars().take(cursor).collect();
            let at = value.chars().nth(cursor).unwrap_or(' ');
            let after: String = value.chars().skip(cursor + 1).collect();
            lines.push(Line::from(vec![
                Span::raw(prefix),
                Span::raw(before),
                Span::styled(
                    at.to_string(),
                    Style::default().bg(Color::White).fg(Color::Black),
                ),
                Span::raw(after),
            ]));
        } else {
            let value = if is_active && state.settings.section != Section::Data {
                state.settings.input.clone()
            } else {
                fields::field_value(config, state.settings.section, i)
            };
            lines.push(Line::from(format!("{}{}: {}", marker, label, value)));
        }
    }

    Text::from(lines)
}
```

In `draw_section_page`, change:

```rust
frame.render_widget(
    Paragraph::new(build_body(state)).style(Style::default().fg(Color::White)),
    chunks[1],
);
```

to:

```rust
frame.render_widget(
    Paragraph::new(build_body(state)).style(Style::default().fg(Color::White)),
    chunks[1],
);
```

(Paragraph::new accepts Text, so this stays the same.)

Make sure `Span` and `Color` are imported in `mod.rs` (they already are).

**Step 6: Update the footer for the Profile section**

For Profile, show:

```
←/→: move caret | Type: edit | Enter: save | Esc: back
```

In `build_footer`, add a special case for `Section::Profile`.

**Step 7: Run the Profile screen and verify**

Run: `cargo run`, open Settings → Profile.
Expected: only `Age` is shown. Typing shows a block cursor. Left/Right/Home/End move the cursor. Only numbers 1-120 are accepted on save.

**Step 8: Commit**

```bash
git add src/ui/views/settings/fields.rs src/ui/views/settings/mod.rs
git commit -m "feat(settings): profile age input with caret and remove CEFR field"
```

---

### Task 5: Update tests and verify compilation

**Files:**
- Modify: `tests/config_test.rs` (if needed for removed CEFR usage)

**Step 1: Check if tests reference the removed UI fields**

Run: `cargo test`

If `config_test.rs` still uses `self_assessed_cefr` in tests that verify roundtrip/config, those tests do NOT need to change because the field still exists in `UserProfile` — it is only hidden from the settings UI.

**Step 2: Run tests**

Run: `cargo test`
Expected: all tests pass.

**Step 3: Commit**

```bash
git add .
git commit -m "test(settings): verify settings UI fixes pass all tests"
```

---

## Execution Handoff

**Plan complete and saved to `docs/plans/2026-07-18-settings-ui-fixes.md`. Two execution options:**

**1. Subagent-Driven (this session)** — I dispatch fresh subagent per task, review between tasks, fast iteration.

**2. Parallel Session (separate)** — Open new session with `superpowers:executing-plans`, batch execution with checkpoints.

**Which approach?**
