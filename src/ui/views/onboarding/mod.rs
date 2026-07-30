mod handlers;
mod state;
mod steps;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use std::sync::Arc;

use crate::app::{AppState, LlmResult, View};
use crate::config::profile::UserProfile;
use crate::config::provider::ProviderConfig;
use crate::config::{OpenCourseConfig, write_config};
use crate::error::{AppError, Result};
use crate::llm::provider::ProviderMeta;
use crate::ui::colors;
use crate::ui::labels::{get_common_labels, get_onboarding_labels, native_language_code};
use crate::ui::views::model_check;
use crate::ui::widgets::Logo;
use crate::ui::widgets::build_footer;
use crate::ui::widgets::build_footer_wrapped;
use crate::ui::widgets::model_picker;

pub use handlers::handle_key;
pub use state::{OnboardingMode, OnboardingState};
pub use steps::Step;

fn display_input(input: &str, step: Step) -> String {
    if step == Step::ApiKey {
        "*".repeat(input.chars().count())
    } else {
        input.to_string()
    }
}

pub fn draw(frame: &mut ratatui::Frame, area: ratatui::layout::Rect, state: &mut AppState) {
    let lang = native_language_code(state.config.as_ref());
    let labels = get_onboarding_labels(lang);
    let common = get_common_labels(lang);
    let step = state.onboarding.current_step();
    let width = area.width as usize;
    let footer_text = match step {
        Step::Provider => build_footer_wrapped(
            &[
                ("↑/↓", common.select_provider),
                ("Enter", common.next),
                ("Esc", common.quit),
                ("?", common.help),
            ],
            width,
        ),
        Step::Cefr => build_footer_wrapped(
            &[
                ("↑/↓", common.select_level),
                ("Enter", common.next),
                ("Esc", common.quit),
                ("?", common.help),
            ],
            width,
        ),
        Step::BatchSize => build_footer_wrapped(
            &[
                ("↑/↓", common.select_batch_size),
                ("Enter", common.next),
                ("Esc", common.quit),
                ("?", common.help),
            ],
            width,
        ),
        Step::Model if state.onboarding.model_picker.loading => labels.loading_models.replace(
            "{}",
            &build_footer_wrapped(&[("Esc", common.quit), ("?", common.help)], width),
        ),
        Step::Model if state.onboarding.model_picker.error.is_some() => build_footer_wrapped(
            &[
                ("r", common.retry),
                ("m", common.manual),
                ("Esc", common.quit),
                ("?", common.help),
            ],
            width,
        ),
        Step::Model if state.onboarding.model_picker.manual => build_footer_wrapped(
            &[
                (common.type_hint, common.model_id),
                ("Enter", common.next),
                ("Esc", common.quit),
            ],
            width,
        ),
        Step::Model if !state.onboarding.model_picker.models.is_empty() => build_footer_wrapped(
            &[
                ("↑/↓", common.select_model),
                ("Enter", common.next),
                ("Esc", common.quit),
                ("?", common.help),
            ],
            width,
        ),
        Step::BaseUrl if !steps::shows_base_url_step(state.onboarding.provider) => {
            build_footer_wrapped(&[("Enter", common.next), ("Esc", common.quit)], width)
        }
        _ => build_footer_wrapped(
            &[
                ("Tab/Enter", common.next),
                ("Shift+Tab", common.prev),
                ("Esc", common.quit),
            ],
            width,
        ),
    };
    let mut footer_lines: Vec<Line> = footer_text.lines().map(Line::from).collect();
    if !state.onboarding.error.is_empty() {
        footer_lines.push(
            Line::from(state.onboarding.error.clone()).style(Style::default().fg(Color::Red)),
        );
    }
    let footer_height = footer_lines.len() as u16;

    let accent = colors::BLUE;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .split(area);

    // Header: logo + subtitle + global hint. The padding keeps the logo off
    // the terminal edge and gives it the same margins as on the dashboard.
    let header_block = Block::default().padding(Padding::new(1, 1, 1, 0));
    let header_inner = header_block.inner(chunks[0]);
    frame.render_widget(header_block, chunks[0]);

    let header_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(2)])
        .split(header_inner);

    frame.render_widget(
        Logo::new(ratatui::layout::Alignment::Left),
        header_chunks[0],
    );

    let subtitle = Text::from(vec![
        Line::from(Span::styled(
            labels.setup_subtitle,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            build_footer(&[
                ("Tab/Enter", common.continue_label),
                ("Shift+Tab", common.prev),
                ("Esc", common.quit),
                ("↑/↓", labels.select_lists),
            ]),
            Style::default().fg(Color::DarkGray),
        )),
    ]);
    frame.render_widget(Paragraph::new(subtitle), header_chunks[1]);

    // Step card with border.
    let visible_steps: Vec<Step> = state
        .onboarding
        .steps
        .iter()
        .filter(|s| state.onboarding.is_step_visible(**s))
        .copied()
        .collect();
    let current_position = visible_steps
        .iter()
        .position(|s| *s == step)
        .map(|i| i + 1)
        .unwrap_or(1);
    let progress = labels
        .step_progress
        .replacen("{}", &current_position.to_string(), 1)
        .replacen("{}", &visible_steps.len().to_string(), 1);
    let title = format!("{} — {}", step.label(lang), progress);
    let card_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .title(Span::styled(
            title,
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let card_inner = card_block.inner(chunks[1]);
    frame.render_widget(card_block, chunks[1]);

    if steps::is_text_step(step) {
        let inner_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(card_inner);

        let input_paragraph = crate::ui::widgets::text_input::input_paragraph(
            &display_input(&state.onboarding.input, step),
            None,
            accent,
        );
        frame.render_widget(input_paragraph, inner_chunks[0]);

        let help_text = steps::step_help_text(step, state);
        frame.render_widget(
            Paragraph::new(help_text).style(Style::default().fg(Color::DarkGray)),
            inner_chunks[1],
        );
    } else {
        match step {
            Step::Provider | Step::Cefr | Step::BatchSize => {
                let text = steps::step_selector_text(step, state);
                frame.render_widget(
                    Paragraph::new(text).style(Style::default().fg(Color::White)),
                    card_inner,
                );
            }
            Step::Model => render_model_step(frame, card_inner, state),
            _ => {}
        }
    }

    frame.render_widget(
        Paragraph::new(footer_lines).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn render_model_step(
    frame: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &mut AppState,
) {
    let labels = get_onboarding_labels(native_language_code(state.config.as_ref()));
    if state.onboarding.model_picker.loading {
        frame.render_widget(
            Paragraph::new(labels.fetching_models).style(Style::default().fg(colors::YELLOW)),
            area,
        );
        return;
    }

    if let Some(err) = &state.onboarding.model_picker.error {
        frame.render_widget(
            Paragraph::new(labels.failed_load_models.replace("{}", err))
                .style(Style::default().fg(Color::Red)),
            area,
        );
        return;
    }

    if state.onboarding.model_picker.manual {
        frame.render_widget(
            Paragraph::new(labels.enter_model_manually).style(Style::default().fg(Color::White)),
            area,
        );
        return;
    }

    if state.onboarding.model_picker.models.is_empty() {
        frame.render_widget(
            Paragraph::new(labels.no_models_found).style(Style::default().fg(Color::White)),
            area,
        );
        return;
    }

    let selected = state.onboarding.model_picker.selected;
    let total = state.onboarding.model_picker.models.len();
    let info = labels
        .model_picker_info
        .replacen("{}", &(selected + 1).to_string(), 1)
        .replacen("{}", &total.to_string(), 1);
    model_picker::draw_model_list(frame, area, &state.onboarding.model_picker, Some(&info));
}

async fn advance_onboarding(state: &mut AppState) -> Result<()> {
    let lang = native_language_code(state.config.as_ref());
    if let Err(e) = state.onboarding.apply_input(lang) {
        state.onboarding.error = e.to_string();
        return Ok(());
    }
    if state.onboarding.current_step() == Step::Model {
        let config = build_config_from_onboarding(&state.onboarding);
        model_check::start(state, config, View::Onboarding);
        return Ok(());
    }
    if state.onboarding.active == state.onboarding.steps.len() - 1 {
        finish_onboarding(state).await?;
    } else {
        state.onboarding.go_forward();
        if state.onboarding.current_step() == Step::Model {
            spawn_model_fetch(state);
        }
    }
    Ok(())
}

pub(crate) async fn finish_onboarding(state: &mut AppState) -> Result<()> {
    match state.onboarding.mode {
        OnboardingMode::Initial => {
            let config = build_config_from_onboarding(&state.onboarding);
            let pair_id = config.active_pair.clone();
            write_config(&config, &state.data_dir)?;
            state.config = Some(config);

            let db_path = crate::config::pair_db_path(&state.data_dir, &pair_id);
            let db = crate::db::Database::connect(&db_path).await?;
            state.db = Arc::new(db);

            state.view = View::Curriculum;
        }
        OnboardingMode::AddPair => {
            let profile = build_profile_from_onboarding(&state.onboarding);
            let config = state
                .config
                .as_mut()
                .ok_or_else(|| AppError::Config("No config available".to_string()))?;
            let new_id = config.add_pair(profile)?.to_string();
            write_config(config, &state.data_dir)?;
            crate::app::switch_pair(state, &new_id).await?;
        }
    }
    Ok(())
}

fn build_profile_from_onboarding(onboarding: &OnboardingState) -> UserProfile {
    UserProfile {
        native_language: onboarding.native_language.clone(),
        target_language: onboarding.target_language.clone(),
        age: if onboarding.age.is_empty() {
            None
        } else {
            onboarding.age.parse().ok()
        },
        self_assessed_cefr: Some(onboarding.cefr.clone()),
    }
}

fn build_config_from_onboarding(onboarding: &OnboardingState) -> OpenCourseConfig {
    let profile = build_profile_from_onboarding(onboarding);

    let provider_config = ProviderConfig::ApiKey {
        api_key: if onboarding.api_key.is_empty() {
            None
        } else {
            Some(onboarding.api_key.clone())
        },
        model: onboarding.model.clone(),
        base_url: if steps::shows_base_url_step(onboarding.provider)
            && !onboarding.base_url.is_empty()
        {
            Some(onboarding.base_url.clone())
        } else {
            None
        },
        endpoint: None,
        reasoning_effort: None,
    };

    let mut config = OpenCourseConfig::new(onboarding.provider, provider_config, profile);
    config.preferences.batch_size = onboarding.batch_size;
    config
}

fn spawn_model_fetch(state: &mut AppState) {
    let provider_id = state.onboarding.provider;
    let meta = ProviderMeta::for_provider(provider_id);
    let api_key = state.onboarding.api_key.clone();
    let base_url =
        if steps::shows_base_url_step(provider_id) && !state.onboarding.base_url.is_empty() {
            Some(state.onboarding.base_url.clone())
        } else {
            meta.default_base_url.map(|s| s.to_string())
        };

    state.onboarding.model_picker.reset();

    let api_key = meta.resolve_api_key(if api_key.is_empty() {
        None
    } else {
        Some(api_key.as_str())
    });
    model_picker::spawn_load(
        &mut state.onboarding.model_picker,
        state.llm_tx.clone(),
        provider_id,
        api_key,
        base_url,
        LlmResult::OnboardingModels,
    );
}
