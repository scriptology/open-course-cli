use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Widget};

use crate::app::{AppState, LlmResult, View};
use crate::ui::colors;
use crate::ui::labels::{get_common_labels, get_report_labels, native_language_code};
use crate::ui::views::curriculum;
use crate::ui::views::utils::{
    screen_chunks, select_next_wrapping, select_previous_wrapping, wrapped_input_text,
};
use crate::ui::widgets::{Card, build_footer_wrapped};
use open_course_core::error::{AppError, Result};
use open_course_core::session::{MentorSession, NextSessionTopic, WarmupItem, WarmupKind};
use open_course_db::curriculum::Topic;
use open_course_llm::pipeline::log_debug_event;

#[derive(Debug, Clone, Default)]
pub enum Mode {
    #[default]
    TopicSelection,
    WarmUp,
    Practicing,
}

#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub mode: Mode,
    pub input: String,
    pub cursor: usize,
    pub topics: Vec<Topic>,
    pub list_state: ListState,
    pub mentor_session: Option<MentorSession>,
    pub loading: bool,
    pub loading_title: Option<String>,
    pub pending_new_topic: bool,
    pub target_topic_id: Option<String>,
    pub learning_item_ids: Vec<String>,
    pub lemma_ids: Vec<String>,
    /// Warm-up cards shown before the exercises; empty when the session has
    /// no teachable forced vocabulary.
    pub warmup_items: Vec<WarmupItem>,
    pub warmup_index: usize,
    /// Whether the current warm-up card's translation is visible.
    pub warmup_revealed: bool,
}

impl SessionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn load(&mut self, db: &open_course_db::Database) -> Result<()> {
        if !self.topics.is_empty() {
            return Ok(());
        }
        let curriculum = db.curriculum().read_all().await?;
        self.topics = curriculum.topics;
        self.list_state.select(Some(0));
        Ok(())
    }
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    let common = get_common_labels(native_language_code(state.config.as_ref()));

    if state.session.loading {
        let footer_text = build_footer_wrapped(
            &[("Esc", labels.cancel), ("?", common.help)],
            area.width as usize,
        );
        let footer_height = footer_text.lines().count() as u16;
        let loading_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(footer_height)])
            .split(area);

        let spinner_symbol = state.spinner.symbol();
        let loading_message = state.stream_status.as_deref().unwrap_or(
            state
                .session
                .loading_title
                .as_deref()
                .unwrap_or(labels.loading),
        );
        let loading_text = Line::from(vec![
            Span::styled(spinner_symbol, Style::default().fg(colors::YELLOW)),
            Span::raw(" "),
            Span::raw(loading_message),
        ]);
        frame.render_widget(
            Paragraph::new(loading_text).style(Style::default().fg(Color::White)),
            loading_chunks[0],
        );

        frame.render_widget(
            Paragraph::new(footer_text).style(Style::default().fg(Color::DarkGray)),
            loading_chunks[1],
        );
        return;
    }

    let width = area.width as usize;
    let footer_text = match state.session.mode {
        Mode::TopicSelection => build_footer_wrapped(
            &[
                ("↑↓", labels.navigate),
                ("Enter", labels.start_session),
                ("Esc", labels.back),
                ("?", common.help),
            ],
            width,
        ),
        Mode::WarmUp => {
            let enter_action = if state.session.warmup_revealed {
                common.next
            } else {
                labels.show_translation
            };
            build_footer_wrapped(
                &[
                    ("Enter", enter_action),
                    ("s", labels.skip_warmup),
                    ("Esc", labels.back),
                ],
                width,
            )
        }
        Mode::Practicing => {
            build_footer_wrapped(&[("Enter", labels.submit), ("Esc", labels.back)], width)
        }
    };
    let chunks = screen_chunks(area, footer_text.lines().count() as u16);

    match state.session.mode {
        Mode::TopicSelection => {
            frame.render_widget(
                Card::new(format!(
                    "{} - {}",
                    labels.session_report, labels.select_topic
                ))
                .line(labels.choose_topic),
                chunks[0],
            );

            let items: Vec<ListItem> = if state.session.topics.is_empty() {
                vec![ListItem::new(labels.no_topics)]
            } else {
                state
                    .session
                    .topics
                    .iter()
                    .map(|topic| {
                        let difficulty = match topic.difficulty.as_str() {
                            "beginner" => labels.difficulty_beginner,
                            "intermediate" => labels.difficulty_intermediate,
                            "advanced" => labels.difficulty_advanced,
                            _ => topic.difficulty.as_str(),
                        };
                        ListItem::new(format!("{} [{}]", topic.name, difficulty))
                    })
                    .collect()
            };

            let list = List::new(items).highlight_symbol("> ").highlight_style(
                Style::default()
                    .fg(colors::GREEN)
                    .add_modifier(Modifier::BOLD),
            );

            frame.render_stateful_widget(list, chunks[1], &mut state.session.list_state);

            frame.render_widget(
                Paragraph::new(footer_text.clone()).style(Style::default().fg(Color::DarkGray)),
                chunks[2],
            );
        }
        Mode::WarmUp => {
            let total = state.session.warmup_items.len();
            let idx = state.session.warmup_index.min(total.saturating_sub(1));
            let title = format!("{} {}/{}", labels.warmup_title, idx + 1, total);

            let mut card = Card::new(title);
            if let Some(item) = state.session.warmup_items.get(idx) {
                let mut word_spans = vec![Span::styled(
                    item.lemma.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                )];
                let new_badge = (item.kind == WarmupKind::New).then_some("NEW");
                let badges: Vec<&str> =
                    [new_badge, item.pos.as_deref(), item.cefr_level.as_deref()]
                        .into_iter()
                        .flatten()
                        .collect();
                if !badges.is_empty() {
                    word_spans.push(Span::styled(
                        format!("  {}", badges.join(" · ")),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
                card = card.line(Line::from(word_spans));

                if state.session.warmup_revealed {
                    card = card.line(Line::from(item.translation.clone()));
                    if let Some(example) = item.example.as_ref() {
                        card = card.line(Line::from(Span::styled(
                            example.clone(),
                            Style::default().fg(colors::YELLOW),
                        )));
                    }
                } else {
                    card = card.line(Line::from(Span::styled(
                        format!("Enter: {}", labels.show_translation),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
            frame.render_widget(card, chunks[0]);

            frame.render_widget(
                Paragraph::new(footer_text.clone()).style(Style::default().fg(Color::DarkGray)),
                chunks[2],
            );
        }
        Mode::Practicing => {
            let title;
            let prompt = if let Some(session) = state.session.mentor_session.as_ref() {
                let total = session.exercises.len();
                let idx = session.current_exercise_index + 1;
                title = format!("{} {}/{}", labels.translate, idx, total);
                if let Some(exercise) = session.exercises.get(session.current_exercise_index) {
                    exercise.target_sentence.clone()
                } else {
                    labels.no_exercise.to_string()
                }
            } else {
                title = labels.translate.to_string();
                labels.no_exercise.to_string()
            };

            frame.render_widget(Card::new(title).line(prompt), chunks[0]);

            let input = &state.session.input;
            let cursor = state.session.cursor;
            let input_block = Block::default()
                .title(labels.your_answer)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
            let input_inner = input_block.inner(chunks[1]);
            input_block.render(chunks[1], frame.buffer_mut());
            let input_text = wrapped_input_text(input, cursor, input_inner.width as usize);
            frame.render_widget(Paragraph::new(input_text), input_inner);

            frame.render_widget(
                Paragraph::new(footer_text.clone()).style(Style::default().fg(Color::DarkGray)),
                chunks[2],
            );
        }
    }
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match state.session.mode {
        Mode::TopicSelection => handle_topic_selection(state, code).await,
        Mode::WarmUp => handle_warmup(state, code).await,
        Mode::Practicing => handle_practicing(state, code).await,
    }
}

/// Warm-up phase: forward-only flashcards. Enter (or Space) reveals the
/// translation, then advances to the next card; after the last card the
/// session moves on to the exercises. `s` skips the rest of the warm-up.
async fn handle_warmup(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => {
            if state.session.loading {
                state.cancelled = true;
            }
            reset_session(&mut state.session);
            state.view = View::Dashboard;
        }
        KeyCode::Char('s') => {
            state.session.mode = Mode::Practicing;
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            if !state.session.warmup_revealed {
                state.session.warmup_revealed = true;
            } else {
                state.session.warmup_index += 1;
                state.session.warmup_revealed = false;
                if state.session.warmup_index >= state.session.warmup_items.len() {
                    state.session.mode = Mode::Practicing;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

async fn handle_topic_selection(state: &mut AppState, code: KeyCode) -> Result<()> {
    let len = state.session.topics.len();
    match code {
        KeyCode::Esc => {
            if state.session.loading {
                state.cancelled = true;
            }
            reset_session(&mut state.session);
            state.view = View::Dashboard;
        }
        KeyCode::Char('j') | KeyCode::Down if !state.session.topics.is_empty() => {
            select_next_wrapping(&mut state.session.list_state, len);
        }
        KeyCode::Char('k') | KeyCode::Up if !state.session.topics.is_empty() => {
            select_previous_wrapping(&mut state.session.list_state, len);
        }
        KeyCode::Enter if !state.session.topics.is_empty() => {
            start_exercises(state).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn handle_practicing(state: &mut AppState, code: KeyCode) -> Result<()> {
    let input = &mut state.session.input;
    let cursor = &mut state.session.cursor;
    clamp_cursor(input, cursor);
    match code {
        KeyCode::Esc => {
            if state.session.loading {
                state.cancelled = true;
            }
            reset_session(&mut state.session);
            state.view = View::Dashboard;
        }
        KeyCode::Char(c) => {
            insert_char(input, cursor, c);
        }
        KeyCode::Backspace => {
            remove_before(input, cursor);
        }
        KeyCode::Delete => {
            remove_at(input, cursor);
        }
        KeyCode::Left => {
            move_left(input, cursor);
        }
        KeyCode::Right => {
            move_right(input, cursor);
        }
        KeyCode::Home => {
            *cursor = 0;
        }
        KeyCode::End => {
            *cursor = input.chars().count();
        }
        KeyCode::Enter => {
            submit_answer(state).await?;
        }
        _ => {}
    }
    Ok(())
}

fn clamp_cursor(input: &str, cursor: &mut usize) {
    let len = input.chars().count();
    if *cursor > len {
        *cursor = len;
    }
}

fn insert_char(input: &mut String, cursor: &mut usize, c: char) {
    clamp_cursor(input, cursor);
    let byte_pos = input
        .char_indices()
        .nth(*cursor)
        .map(|(i, _)| i)
        .unwrap_or(input.len());
    input.insert(byte_pos, c);
    *cursor += 1;
}

fn remove_before(input: &mut String, cursor: &mut usize) {
    clamp_cursor(input, cursor);
    if *cursor == 0 {
        return;
    }
    let start = input
        .char_indices()
        .nth(*cursor - 1)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end = input
        .char_indices()
        .nth(*cursor)
        .map(|(i, _)| i)
        .unwrap_or(input.len());
    input.replace_range(start..end, "");
    *cursor -= 1;
}

fn remove_at(input: &mut String, cursor: &mut usize) {
    clamp_cursor(input, cursor);
    if *cursor >= input.chars().count() {
        return;
    }
    let start = input
        .char_indices()
        .nth(*cursor)
        .map(|(i, _)| i)
        .unwrap_or(input.len());
    let end = input
        .char_indices()
        .nth(*cursor + 1)
        .map(|(i, _)| i)
        .unwrap_or(input.len());
    input.replace_range(start..end, "");
}

fn move_left(_input: &str, cursor: &mut usize) {
    if *cursor > 0 {
        *cursor -= 1;
    }
}

fn move_right(input: &str, cursor: &mut usize) {
    let len = input.chars().count();
    if *cursor < len {
        *cursor += 1;
    }
}

pub(crate) async fn start_exercises(state: &mut AppState) -> Result<()> {
    let selected = state.session.list_state.selected().unwrap_or(0);
    let target_topic = state
        .session
        .topics
        .get(selected)
        .cloned()
        .ok_or_else(|| AppError::NotFound("Selected topic not found".to_string()))?;
    start_exercises_for_topic(state, &target_topic.id, None).await
}

pub async fn start_new_topic_session(state: &mut AppState) -> Result<()> {
    state.session.load(&state.db).await?;
    if state.session.topics.is_empty() {
        state.view = View::Curriculum;
        return Ok(());
    }
    match open_course_service::session::next_session_topic(&state.db, &state.session.topics).await?
    {
        NextSessionTopic::Review(topic) => {
            let title = review_title(state, &topic.name);
            start_exercises_for_topic(state, &topic.id, Some(title)).await
        }
        NextSessionTopic::New(topic) => {
            let title = new_topic_title(state, &topic.name);
            start_exercises_for_topic(state, &topic.id, Some(title)).await
        }
        NextSessionTopic::ExtendCurriculum => {
            curriculum::extend_curriculum(state, 5).await?;
            state.session.loading = true;
            state.session.pending_new_topic = true;
            Ok(())
        }
    }
}

fn review_title(state: &AppState, topic_name: &str) -> String {
    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    format!("{}: {}", labels.review_session_label, topic_name)
}

fn new_topic_title(state: &AppState, topic_name: &str) -> String {
    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    format!("{}: {}", labels.new_topic_session_label, topic_name)
}

pub async fn start_review_topic_session(state: &mut AppState, topic_id: String) -> Result<()> {
    state.session.load(&state.db).await?;
    let title = state
        .session
        .topics
        .iter()
        .find(|t| t.id == topic_id)
        .map(|t| review_title(state, &t.name));
    start_exercises_for_topic(state, &topic_id, title).await
}

pub(crate) async fn start_exercises_for_topic(
    state: &mut AppState,
    target_topic_id: &str,
    loading_title: Option<String>,
) -> Result<()> {
    let config = state
        .config
        .clone()
        .ok_or_else(|| AppError::Config("No provider configured".to_string()))?;

    let preparation = open_course_service::session::prepare_exercises(
        &state.db,
        &config,
        &state.session.topics,
        target_topic_id,
    )
    .await?;

    state.session.learning_item_ids = preparation.forced_learning_item_ids;
    state.session.lemma_ids = preparation.forced_lemma_ids;

    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    state.session.loading = true;
    state.session.loading_title =
        loading_title.or_else(|| Some(labels.loading_exercises.to_string()));
    state.session.pending_new_topic = false;
    state.session.target_topic_id = Some(target_topic_id.to_string());

    log_debug_event(
        "session",
        &format!(
            "start_exercises_for_topic {target_topic_id}\n{}",
            preparation.prompt
        ),
        Some(state.data_dir.as_path()),
    );

    let data_dir = state.data_dir.clone();
    let tx = state.llm_tx.clone();
    let prompt = preparation.prompt;
    let forced_lemmas = preparation.forced_lemmas;
    let forced_forms = preparation.forced_forms;
    let existing_lemmas = preparation.existing_lemmas;
    tokio::spawn(async move {
        let result = open_course_service::session::generate_session_exercises(
            &config,
            &prompt,
            &forced_lemmas,
            &forced_forms,
            &existing_lemmas,
            &tx,
            Some(data_dir.as_path()),
        )
        .await;
        let _ = tx.send(LlmResult::Exercises(result)).await;
    });

    Ok(())
}

pub async fn maybe_start_pending_new_topic(state: &mut AppState) -> Result<()> {
    if !state.session.pending_new_topic {
        return Ok(());
    }
    state.session.pending_new_topic = false;
    if let Some(topic_id) =
        open_course_service::session::pick_untouched_topic(&state.db, &state.session.topics).await?
    {
        start_exercises_for_topic(state, &topic_id, None).await?;
    } else {
        state.error = Some("No new topic available after curriculum generation".to_string());
    }
    Ok(())
}

async fn submit_answer(state: &mut AppState) -> Result<()> {
    let answer = state.session.input.clone();
    let mut session = state
        .session
        .mentor_session
        .take()
        .ok_or_else(|| AppError::Config("No active session".to_string()))?;
    let idx = session.current_exercise_index;
    session.record_answer(idx, answer);
    session.advance_exercise();

    if session.is_complete() {
        state.session.mentor_session = Some(session);
        finish_session(state).await?;
    } else {
        state.session.mentor_session = Some(session);
        state.session.input.clear();
        state.session.cursor = 0;
    }

    Ok(())
}

async fn finish_session(state: &mut AppState) -> Result<()> {
    let config = state
        .config
        .clone()
        .ok_or_else(|| AppError::Config("No provider configured".to_string()))?;
    let topics = state.session.topics.clone();

    let session = state
        .session
        .mentor_session
        .as_ref()
        .ok_or_else(|| AppError::Config("No active session".to_string()))?
        .clone();

    let preparation = open_course_service::session::prepare_analysis(&config, &session, &topics);

    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    state.session.loading = true;
    state.session.loading_title = Some(labels.loading_analysis.to_string());

    let data_dir = state.data_dir.clone();
    let tx = state.llm_tx.clone();
    tokio::spawn(async move {
        let result = open_course_service::session::run_session_analysis(
            &config,
            &topics,
            preparation,
            &tx,
            Some(data_dir.as_path()),
        )
        .await;
        let _ = tx.send(LlmResult::Analysis(result)).await;
    });

    Ok(())
}

pub(crate) fn reset_session(session: &mut SessionState) {
    session.mode = Mode::TopicSelection;
    session.input.clear();
    session.cursor = 0;
    session.mentor_session = None;
    session.list_state.select(Some(0));
    session.loading = false;
    session.loading_title = None;
    session.pending_new_topic = false;
    session.target_topic_id = None;
    session.warmup_items.clear();
    session.warmup_index = 0;
    session.warmup_revealed = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulate_cursor_not_reset_after_clear() {
        // Reproduces the reported bug: input was cleared (e.g. on new exercise
        // load) but cursor stayed at the previous length. Typing a character and
        // pressing backspace must delete only that character, not the whole line.
        let mut input = String::new();
        let mut cursor = 5; // stale cursor from a previous, longer answer
        insert_char(&mut input, &mut cursor, 'h');
        assert_eq!(input, "h");
        remove_before(&mut input, &mut cursor);
        assert_eq!(input, "");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn remove_before_does_not_clear_all_when_cursor_out_of_bounds() {
        let mut input = "hello".to_string();
        let mut cursor = 10;
        remove_before(&mut input, &mut cursor);
        // Cursor is clamped to end and the last character is removed, never
        // the whole line.
        assert_eq!(input, "hell");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn remove_at_does_nothing_when_cursor_out_of_bounds() {
        let mut input = "hello".to_string();
        let mut cursor = 10;
        remove_at(&mut input, &mut cursor);
        assert_eq!(input, "hello");
    }

    #[test]
    fn insert_char_with_out_of_bounds_cursor_appends() {
        let mut input = "hi".to_string();
        let mut cursor = 10;
        insert_char(&mut input, &mut cursor, 'x');
        assert_eq!(input, "hix");
    }

    #[test]
    fn ascii_insert_and_delete() {
        let mut input = String::new();
        let mut cursor = 0;
        insert_char(&mut input, &mut cursor, 'a');
        insert_char(&mut input, &mut cursor, 'b');
        insert_char(&mut input, &mut cursor, 'c');
        assert_eq!(input, "abc");
        assert_eq!(cursor, 3);

        remove_before(&mut input, &mut cursor);
        assert_eq!(input, "ab");
        assert_eq!(cursor, 2);

        move_left(&input, &mut cursor);
        assert_eq!(cursor, 1);
        remove_at(&mut input, &mut cursor);
        assert_eq!(input, "a");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn multibyte_insert_and_delete() {
        let mut input = "привет".to_string();
        let mut cursor = 3; // after "при"
        insert_char(&mut input, &mut cursor, 'b');
        assert_eq!(input, "приbвет");
        assert_eq!(cursor, 4);

        remove_before(&mut input, &mut cursor);
        assert_eq!(input, "привет");
        assert_eq!(cursor, 3);

        remove_at(&mut input, &mut cursor);
        assert_eq!(input, "приет");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn move_and_home_end() {
        let input = "abc".to_string();
        let mut cursor = 0;
        move_right(&input, &mut cursor);
        assert_eq!(cursor, 1);
        move_left(&input, &mut cursor);
        assert_eq!(cursor, 0);
        cursor = input.chars().count();
        assert_eq!(cursor, 3);
    }
}
