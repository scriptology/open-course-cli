use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use unicode_normalization::UnicodeNormalization;

use crate::app::{AppState, View};
use crate::core::session::{AnalysisResult, MentorSession};
use crate::db::curriculum::Topic;
use crate::error::Result;
use crate::ui::labels::{ReportLabels, get_report_labels, native_language_code};
use crate::ui::views::{docs, session};

#[derive(Debug, Clone)]
pub struct ReportState {
    pub analysis: AnalysisResult,
    pub session: MentorSession,
    pub weak_topics: Vec<Topic>,
    pub scroll_offset: u16,
    pub max_scroll_offset: u16,
    pub target_topic_id: Option<String>,
}

impl Default for ReportState {
    fn default() -> Self {
        Self {
            analysis: AnalysisResult {
                session_score: None,
                sentences: Vec::new(),
                evaluated_topics: Vec::new(),
                new_topics: Vec::new(),
            },
            session: MentorSession {
                id: String::new(),
                exercises: Vec::new(),
                answers: std::collections::HashMap::new(),
                current_exercise_index: 0,
            },
            weak_topics: Vec::new(),
            scroll_offset: 0,
            max_scroll_offset: 0,
            target_topic_id: None,
        }
    }
}

impl ReportState {
    pub fn new() -> Self {
        Self::default()
    }
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let labels = get_report_labels(native_language_code(state.config.as_ref()));

    let chunks: [Rect; 2] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    let (paragraph, max_offset) = {
        let lines = build_report_lines(&state.report, labels);
        let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true });
        let line_count = paragraph.line_count(chunks[0].width);
        let max_offset = line_count.saturating_sub(chunks[0].height as usize) as u16;
        (paragraph, max_offset)
    };

    state.report.scroll_offset = state.report.scroll_offset.min(max_offset);
    state.report.max_scroll_offset = max_offset;

    frame.render_widget(paragraph.scroll((state.report.scroll_offset, 0)), chunks[0]);

    frame.render_widget(
        Paragraph::new("↑/↓: scroll | n: new topic | r: repeat | d: docs | Esc: dashboard")
            .style(Style::default().fg(Color::DarkGray)),
        chunks[1],
    );
}

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => {
            state.view = View::Dashboard;
        }
        KeyCode::Char('n') => {
            state.view = View::Session;
            session::start_new_topic_session(state).await?;
        }
        KeyCode::Char('r') => {
            if let Some(topic_id) = state.report.target_topic_id.clone() {
                state.view = View::Session;
                session::start_review_topic_session(state, topic_id).await?;
            }
        }
        KeyCode::Char('d') => {
            if let Some(topic_id) = state.report.target_topic_id.clone() {
                docs::load(state).await?;
                if let Some(topic) = state.docs.topics.iter().find(|t| t.id == topic_id).cloned() {
                    if let Some(index) = state.docs.topics.iter().position(|t| t.id == topic_id) {
                        state.docs.list_state.select(Some(index));
                    }
                    docs::start_viewing(state, topic);
                    state.view = View::Docs;
                }
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            state.report.scroll_offset =
                (state.report.scroll_offset + 1).min(state.report.max_scroll_offset);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.report.scroll_offset = state.report.scroll_offset.saturating_sub(1);
        }
        _ => {}
    }
    Ok(())
}

fn build_report_lines(report: &ReportState, labels: ReportLabels) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![Span::styled(
        labels.per_exercise_results,
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Rgb(0, 122, 255)),
    )]));

    for (i, sentence) in report.analysis.sentences.iter().enumerate() {
        let has_errors = !sentence.errors.is_empty();

        let mut student_line = vec![
            Span::raw(format!("{}. ", i + 1)),
            Span::styled(
                format!("{}: ", labels.your_translation),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];

        if !has_errors {
            student_line.push(Span::styled("✓ ", Style::default().fg(Color::Green)));
        }

        student_line.extend(student_translation_spans(
            &sentence.student_translation,
            &sentence.expected_translation,
            has_errors,
        ));
        lines.push(Line::from(student_line));

        if has_errors {
            let mut correct_line = vec![
                Span::raw("   "),
                Span::styled(
                    format!("{}: ", labels.correct_answer),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ];
            correct_line.extend(correct_answer_spans(
                &sentence.expected_translation,
                &sentence.student_translation,
            ));
            lines.push(Line::from(correct_line));
        }

        for error in &sentence.errors {
            lines.push(Line::from(vec![
                Span::raw("   ↪ "),
                Span::styled(
                    error.explanation.clone(),
                    Style::default()
                        .add_modifier(Modifier::ITALIC)
                        .fg(Color::Yellow),
                ),
            ]));
        }

        for comment in &sentence.per_sentence_feedback {
            lines.push(Line::from(vec![
                Span::raw("   ↪ "),
                Span::styled(comment.comment.clone(), Style::default().fg(Color::Yellow)),
            ]));
        }
        lines.push(Line::from(""));
    }

    let new_topic_ids: std::collections::HashSet<&str> = report
        .analysis
        .new_topics
        .iter()
        .map(|t| t.id.as_str())
        .collect();

    let changed_topics: Vec<_> = report
        .analysis
        .evaluated_topics
        .iter()
        .filter(|topic| {
            topic
                .previous_score
                .map(|prev| (topic.score - prev).abs() > 0.5)
                .unwrap_or(true)
        })
        .collect();

    let changed_topic_ids: std::collections::HashSet<_> = changed_topics
        .iter()
        .map(|t| t.topic_id.as_str())
        .collect();

    let extra_new_topics: Vec<_> = report
        .analysis
        .new_topics
        .iter()
        .filter(|t| !changed_topic_ids.contains(t.id.as_str()))
        .collect();

    if !changed_topics.is_empty() || !extra_new_topics.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            labels.topic_scores,
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(0, 122, 255)),
        )]));
        for topic in changed_topics {
            let is_new = new_topic_ids.contains(topic.topic_id.as_str());
            let mut spans = vec![
                Span::raw("• "),
                Span::raw(topic.topic_id.clone()),
                Span::raw(": "),
            ];

            let prev = topic.previous_score.unwrap_or(0.0);
            let delta = topic.score - prev;
            let score_color = if delta > 0.0 {
                Color::Green
            } else if delta < 0.0 {
                Color::Red
            } else {
                Color::White
            };
            spans.push(Span::styled(
                format!("{:.0}", topic.score),
                Style::default().fg(score_color),
            ));
            if topic.previous_score.is_some() && delta.abs() > 0.5 {
                let sign = if delta > 0.0 { "+" } else { "-" };
                spans.push(Span::raw(format!(" ({}{:.0})", sign, delta.abs())));
            }
            if is_new {
                spans.push(Span::styled(
                    format!(" ({})", labels.new_topic_label),
                    Style::default().fg(Color::Yellow),
                ));
            }

            lines.push(Line::from(spans));
        }
        for topic in extra_new_topics {
            lines.push(Line::from(vec![
                Span::raw("• "),
                Span::raw(topic.name.clone()),
                Span::raw(": "),
                Span::styled("0", Style::default().fg(Color::White)),
                Span::styled(
                    format!(" ({})", labels.new_topic_label),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
        }
    }

    if !report.weak_topics.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            labels.weak_topics,
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Rgb(0, 122, 255)),
        )]));
        for topic in &report.weak_topics {
            lines.push(Line::from(vec![
                Span::raw("• "),
                Span::raw(topic.name.clone()),
            ]));
        }
    }

    lines
}

fn normalize_word(word: &str) -> String {
    word.nfkd()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn is_word_in_text(word: &str, text: &str) -> bool {
    let norm = normalize_word(word);
    if norm.is_empty() {
        return false;
    }
    text.split_whitespace()
        .map(normalize_word)
        .any(|w| w == norm)
}

fn student_translation_spans(text: &str, expected: &str, has_errors: bool) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for token in text.split_whitespace() {
        let is_correct = is_word_in_text(token, expected);
        let style = if has_errors {
            if is_correct {
                Style::default()
            } else {
                Style::default().fg(Color::Red)
            }
        } else {
            Style::default().fg(Color::Green)
        };
        spans.push(Span::styled(token.to_string(), style));
        spans.push(Span::raw(" "));
    }
    if !spans.is_empty() {
        spans.pop();
    }
    spans
}

fn correct_answer_spans(text: &str, student: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for token in text.split_whitespace() {
        let is_added = !is_word_in_text(token, student);
        let style = if is_added {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        spans.push(Span::styled(token.to_string(), style));
        spans.push(Span::raw(" "));
    }
    if !spans.is_empty() {
        spans.pop();
    }
    spans
}
