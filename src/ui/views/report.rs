use std::io::Write;

use ratatui::crossterm::event::KeyCode;
use ratatui::crossterm::style::{
    Attribute, Color as CrosstermColor, ContentStyle, Print, PrintStyledContent, ResetColor,
    StyledContent,
};
use ratatui::crossterm::{ExecutableCommand, QueueableCommand};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_normalization::UnicodeNormalization;

use crate::app::{AppState, View};
use crate::core::session::{AnalysisResult, MentorSession, SemanticVerdict};
use crate::db::curriculum::Topic;
use crate::error::Result;
use crate::ui::colors;
use crate::ui::labels::{ReportLabels, get_common_labels, get_report_labels, native_language_code};
use crate::ui::views::{docs, session};
use crate::ui::widgets::build_footer;

#[derive(Debug, Clone)]
pub struct ReportState {
    pub analysis: AnalysisResult,
    pub session: MentorSession,
    pub weak_topics: Vec<Topic>,
    pub target_topic_id: Option<String>,
    pub target_topic_name: Option<String>,
}

impl Default for ReportState {
    fn default() -> Self {
        Self {
            analysis: AnalysisResult {
                session_score: None,
                sentences: Vec::new(),
                evaluated_topics: Vec::new(),
                new_topics: Vec::new(),
                new_learning_items: Vec::new(),
            },
            session: MentorSession {
                id: String::new(),
                exercises: Vec::new(),
                answers: std::collections::HashMap::new(),
                current_exercise_index: 0,
            },
            weak_topics: Vec::new(),
            target_topic_id: None,
            target_topic_name: None,
        }
    }
}

impl ReportState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the report shown after a session analysis.
    pub fn from_analysis(
        analysis: AnalysisResult,
        session: MentorSession,
        weak_topics: Vec<Topic>,
        target_topic_id: Option<String>,
        target_topic_name: Option<String>,
    ) -> Self {
        Self {
            analysis,
            session,
            weak_topics,
            target_topic_id,
            target_topic_name,
        }
    }
}

/// Prints the full report onto the main screen (the app leaves the alternate
/// screen while the report is shown) so the terminal's native scrollback and
/// native text selection just work. There is intentionally no custom
/// scrolling and no pinned footer on this page: the command line is printed
/// last and scrolls away with the content.
pub fn print(state: &AppState) -> Result<()> {
    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    let common = get_common_labels(native_language_code(state.config.as_ref()));
    let mut stdout = std::io::stdout();
    stdout.queue(Print("\r\n"))?;
    for line in build_report_lines(&state.report, labels) {
        print_line(&mut stdout, &line, None)?;
    }
    let footer = Line::from(build_footer(&[
        ("n", common.new_topic),
        ("r", common.repeat),
        ("d", labels.docs),
        ("Esc", common.dashboard),
    ]));
    print_line(&mut stdout, &footer, Some(Color::DarkGray))?;
    stdout.execute(ResetColor)?;
    stdout.flush()?;
    Ok(())
}

fn print_line(stdout: &mut impl Write, line: &Line, fallback_fg: Option<Color>) -> Result<()> {
    for span in &line.spans {
        let mut style = span.style;
        if style.fg.is_none() {
            style.fg = fallback_fg;
        }
        stdout.queue(PrintStyledContent(StyledContent::new(
            to_content_style(style),
            span.content.as_ref(),
        )))?;
    }
    stdout.queue(Print("\r\n"))?;
    Ok(())
}

fn to_content_style(style: Style) -> ContentStyle {
    let mut converted = ContentStyle::new();
    converted.foreground_color = style.fg.map(to_crossterm_color);
    if style.add_modifier.contains(Modifier::BOLD) {
        converted.attributes.set(Attribute::Bold);
    }
    if style.add_modifier.contains(Modifier::ITALIC) {
        converted.attributes.set(Attribute::Italic);
    }
    converted
}

fn to_crossterm_color(color: Color) -> CrosstermColor {
    match color {
        Color::Reset => CrosstermColor::Reset,
        Color::Black => CrosstermColor::Black,
        Color::Red => CrosstermColor::DarkRed,
        Color::Green => CrosstermColor::DarkGreen,
        Color::Yellow => CrosstermColor::DarkYellow,
        Color::Blue => CrosstermColor::DarkBlue,
        Color::Magenta => CrosstermColor::DarkMagenta,
        Color::Cyan => CrosstermColor::DarkCyan,
        Color::Gray => CrosstermColor::Grey,
        Color::DarkGray => CrosstermColor::DarkGrey,
        Color::LightRed => CrosstermColor::Red,
        Color::LightGreen => CrosstermColor::Green,
        Color::LightYellow => CrosstermColor::Yellow,
        Color::LightBlue => CrosstermColor::Blue,
        Color::LightMagenta => CrosstermColor::Magenta,
        Color::LightCyan => CrosstermColor::Cyan,
        Color::White => CrosstermColor::White,
        Color::Rgb(r, g, b) => CrosstermColor::Rgb { r, g, b },
        Color::Indexed(i) => CrosstermColor::AnsiValue(i),
    }
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
                    state.docs.return_to = Some(View::Report);
                    state.view = View::Docs;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn build_report_lines(report: &ReportState, labels: ReportLabels) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(name) = &report.target_topic_name {
        lines.push(Line::from(vec![Span::styled(
            format!("{}: {}", labels.topic_label, name),
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(colors::GREEN),
        )]));
        lines.push(Line::from(""));
    }

    lines.push(Line::from(vec![Span::styled(
        labels.per_exercise_results,
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(colors::BLUE),
    )]));

    for (i, sentence) in report.analysis.sentences.iter().enumerate() {
        let exercise = report
            .session
            .exercises
            .get(sentence.sentence_number.saturating_sub(1) as usize)
            .or_else(|| report.session.exercises.get(i));
        if let Some(exercise) = exercise {
            lines.push(Line::from(vec![
                Span::raw(format!("{}. ", i + 1)),
                Span::styled(
                    format!("{}: ", labels.task),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(exercise.target_sentence.clone()),
            ]));
        }

        let mut student_line = vec![
            Span::raw("   "),
            Span::styled(
                format!("{}: ", labels.your_translation),
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];

        let (status_symbol, status_style) = match sentence.semantic_verdict {
            SemanticVerdict::Correct => ("✓ ", Style::default().fg(colors::GREEN)),
            SemanticVerdict::Acceptable => ("~ ", Style::default().fg(colors::YELLOW)),
            SemanticVerdict::NeedsCorrection => ("✗ ", Style::default().fg(Color::Red)),
        };
        student_line.push(Span::styled(status_symbol, status_style));

        student_line.extend(student_translation_spans(
            &sentence.student_translation,
            &sentence.expected_translation,
            !sentence.errors.is_empty(),
        ));
        lines.push(Line::from(student_line));

        if sentence.semantic_verdict == SemanticVerdict::NeedsCorrection {
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
        } else if !sentence.acceptable_translations.is_empty() {
            let alts = sentence.acceptable_translations.join("; ");
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(labels.also_acceptable, Style::default().fg(Color::DarkGray)),
                Span::raw(alts),
            ]));
        }

        for error in &sentence.errors {
            lines.push(Line::from(vec![
                Span::raw("   ↪ "),
                Span::styled(
                    error.explanation.clone(),
                    Style::default()
                        .add_modifier(Modifier::ITALIC)
                        .fg(colors::YELLOW),
                ),
            ]));
        }

        for comment in &sentence.per_sentence_feedback {
            lines.push(Line::from(vec![
                Span::raw("   ↪ "),
                Span::styled(comment.comment.clone(), Style::default().fg(colors::YELLOW)),
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

    let changed_topic_ids: std::collections::HashSet<_> =
        changed_topics.iter().map(|t| t.topic_id.as_str()).collect();

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
                .fg(colors::BLUE),
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
                colors::GREEN
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
                    Style::default().fg(colors::YELLOW),
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
                    Style::default().fg(colors::YELLOW),
                ),
            ]));
        }
    }

    if !report.analysis.new_learning_items.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            labels.new_learning_items_label,
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(colors::BLUE),
        )]));
        for item in &report.analysis.new_learning_items {
            lines.push(Line::from(vec![
                Span::raw("• "),
                Span::raw(item.name.clone()),
            ]));
        }
    }

    if !report.weak_topics.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            labels.weak_topics,
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(colors::BLUE),
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
            Style::default().fg(colors::GREEN)
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
                .fg(colors::GREEN)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_header_shows_topic_name() {
        let report = ReportState {
            target_topic_name: Some("Preterito".to_string()),
            ..Default::default()
        };
        let lines = build_report_lines(&report, get_report_labels("ru"));
        let header: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(header, "Тема: Preterito");
    }

    #[test]
    fn ratatui_colors_convert_to_crossterm() {
        assert_eq!(
            to_crossterm_color(Color::Rgb(0x34, 0xda, 0xb7)),
            CrosstermColor::Rgb {
                r: 0x34,
                g: 0xda,
                b: 0xb7
            }
        );
        assert_eq!(
            to_crossterm_color(Color::DarkGray),
            CrosstermColor::DarkGrey
        );
        assert_eq!(to_crossterm_color(Color::Red), CrosstermColor::DarkRed);
    }

    #[test]
    fn bold_and_italic_modifiers_convert_to_attributes() {
        let style = Style::default()
            .fg(colors::GREEN)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::ITALIC);
        let converted = to_content_style(style);
        assert!(converted.foreground_color.is_some());
        assert!(converted.attributes.has(Attribute::Bold));
        assert!(converted.attributes.has(Attribute::Italic));
    }
}
