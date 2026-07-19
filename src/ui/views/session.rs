use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Widget};

use crate::app::{AppState, LlmResult, View};
use crate::core::session::{
    AnalysisResult, Exercise, MentorSession, NextSessionTopic, pick_next_session_topic,
    select_side_topics,
};
use crate::db::curriculum::Topic;
use crate::db::learning_items::{LearningItem, LearningItemsTable};
use crate::error::{AppError, Result};
use crate::llm::factory::create_llm_model;
use crate::llm::pipeline::{
    finalize_analysis_with_new_topics, generate_analysis, generate_exercises, log_debug_event,
};
use crate::llm::prompts::{build_batch_analysis_prompt, build_exercise_prompt};
use crate::ui::colors;
use crate::ui::labels::{get_report_labels, native_language_code};
use crate::ui::views::curriculum;
use crate::ui::views::utils::{
    screen_chunks, select_next_wrapping, select_previous_wrapping, wrapped_input_text,
};
use crate::ui::widgets::{Card, build_footer};

#[derive(Debug, Clone, Default)]
pub enum Mode {
    #[default]
    TopicSelection,
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
}

impl SessionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn load(&mut self, db: &crate::db::Database) -> Result<()> {
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

    if state.session.loading {
        let loading_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
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
            Paragraph::new(build_footer(&[("Esc", labels.cancel), ("?", "help")]))
                .style(Style::default().fg(Color::DarkGray)),
            loading_chunks[1],
        );
        return;
    }

    let chunks = screen_chunks(area);

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
                    .map(|topic| ListItem::new(format!("{} [{}]", topic.name, topic.difficulty)))
                    .collect()
            };

            let list = List::new(items).highlight_symbol("> ").highlight_style(
                Style::default()
                    .fg(colors::BLUE)
                    .add_modifier(Modifier::BOLD),
            );

            frame.render_stateful_widget(list, chunks[1], &mut state.session.list_state);

            frame.render_widget(
                Paragraph::new(build_footer(&[
                    ("↑↓", labels.navigate),
                    ("Enter", labels.start_session),
                    ("Esc", labels.back),
                    ("?", "help"),
                ]))
                .style(Style::default().fg(Color::DarkGray)),
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
                Paragraph::new(build_footer(&[
                    ("Enter", labels.submit),
                    ("Esc", labels.back),
                ]))
                .style(Style::default().fg(Color::DarkGray)),
                chunks[2],
            );
        }
    }
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match state.session.mode {
        Mode::TopicSelection => handle_topic_selection(state, code).await,
        Mode::Practicing => handle_practicing(state, code).await,
    }
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
    let progress = state.db.progress().read_all().await?;
    match pick_next_session_topic(&state.session.topics, &progress, chrono::Utc::now()) {
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
    let profile = config.active_profile().clone();

    let target_topic = state
        .session
        .topics
        .iter()
        .find(|t| t.id == target_topic_id)
        .cloned()
        .ok_or_else(|| AppError::NotFound(format!("Topic {target_topic_id} not found")))?;
    let all_topics = state.session.topics.clone();
    if all_topics.is_empty() {
        return Err(AppError::Config(
            "No topics available. Generate a curriculum first.".to_string(),
        ));
    }
    let progress = state.db.progress().read_all().await.unwrap_or_default();
    let side_topics = select_side_topics(
        &all_topics,
        std::slice::from_ref(&target_topic),
        3,
        &progress,
        chrono::Utc::now(),
    );

    let candidate_topics: Vec<Topic> = std::iter::once(&target_topic)
        .chain(side_topics.iter())
        .cloned()
        .collect();

    let learning_items: Vec<LearningItem> = state
        .db
        .learning_items()
        .read_all()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|li| li.target_lang == profile.target_language)
        .collect();
    let forced_learning_items = LearningItemsTable::weakest(&learning_items, 3);
    state.session.learning_item_ids = forced_learning_items
        .iter()
        .map(|li| li.id.clone())
        .collect();

    let history = state.db.history().read_all().await.unwrap_or_default();
    let success_rate = crate::core::session::recent_success_rate(&history, 5);

    let prompt = build_exercise_prompt(
        &profile,
        &[target_topic],
        &side_topics,
        &candidate_topics,
        &forced_learning_items,
        config.preferences.batch_size,
        success_rate,
    );

    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    state.session.loading = true;
    state.session.loading_title =
        loading_title.or_else(|| Some(labels.loading_exercises.to_string()));
    state.session.pending_new_topic = false;
    state.session.target_topic_id = Some(target_topic_id.to_string());

    log_debug_event(
        "session",
        &format!("start_exercises_for_topic {target_topic_id}\n{prompt}"),
        Some(state.data_dir.as_path()),
    );

    let data_dir = state.data_dir.clone();
    let tx = state.llm_tx.clone();
    tokio::spawn(async move {
        let result: Result<Vec<Exercise>> = async {
            let model = create_llm_model(&config)?;
            generate_exercises(model.as_ref(), &prompt, Some(&tx), Some(data_dir.as_path())).await
        }
        .await;
        let _ = tx.send(LlmResult::Exercises(result)).await;
    });

    Ok(())
}

async fn pick_untouched_topic(state: &mut AppState) -> Result<Option<String>> {
    let progress = state.db.progress().read_all().await?;
    let touched: std::collections::HashSet<String> = progress
        .topics
        .iter()
        .filter(|p| p.last_practiced.is_some())
        .map(|p| p.topic_id.clone())
        .collect();

    Ok(state
        .session
        .topics
        .iter()
        .find(|t| !touched.contains(&t.id))
        .map(|t| t.id.clone()))
}

pub async fn maybe_start_pending_new_topic(state: &mut AppState) -> Result<()> {
    if !state.session.pending_new_topic {
        return Ok(());
    }
    state.session.pending_new_topic = false;
    if let Some(topic_id) = pick_untouched_topic(state).await? {
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
    let profile = config.active_profile().clone();
    let topics = state.session.topics.clone();

    let session = state
        .session
        .mentor_session
        .as_ref()
        .ok_or_else(|| AppError::Config("No active session".to_string()))?
        .clone();

    let pairs: Vec<(Exercise, String)> = session
        .exercises
        .iter()
        .enumerate()
        .map(|(i, ex)| {
            (
                ex.clone(),
                session.answers.get(&i).cloned().unwrap_or_default(),
            )
        })
        .collect();

    let candidate_ids: std::collections::HashSet<String> = session
        .exercises
        .iter()
        .flat_map(|ex| ex.target_topic_ids.iter().chain(ex.side_topic_ids.iter()))
        .cloned()
        .collect();
    let candidate_topics: Vec<Topic> = topics
        .iter()
        .filter(|t| candidate_ids.contains(&t.id))
        .cloned()
        .collect();

    let prompt = build_batch_analysis_prompt(&profile, &pairs, &candidate_topics);

    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    state.session.loading = true;
    state.session.loading_title = Some(labels.loading_analysis.to_string());

    let data_dir = state.data_dir.clone();
    let tx = state.llm_tx.clone();
    tokio::spawn(async move {
        let result: Result<AnalysisResult> = async {
            let model = create_llm_model(&config)?;
            let mut analysis = generate_analysis(
                model.as_ref(),
                &prompt,
                pairs.len(),
                Some(&tx),
                Some(data_dir.as_path()),
            )
            .await?;
            merge_analysis_with_pairs(&mut analysis, &pairs);
            analysis = finalize_analysis_with_new_topics(
                model.as_ref(),
                &profile,
                &topics,
                analysis,
                Some(&tx),
                Some(data_dir.as_path()),
            )
            .await?;
            Ok(analysis)
        }
        .await;
        let _ = tx.send(LlmResult::Analysis(result)).await;
    });

    Ok(())
}

fn merge_analysis_with_pairs(analysis: &mut AnalysisResult, pairs: &[(Exercise, String)]) {
    for sentence in &mut analysis.sentences {
        let idx = (sentence.sentence_number - 1) as usize;
        if let Some((exercise, answer)) = pairs.get(idx) {
            sentence.student_translation = answer.clone();
            sentence.expected_translation = exercise.expected_translation.clone();
        }
    }
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
