use ratatui::crossterm::event::KeyCode;

use crate::app::{AppState, View};
use crate::config::provider::ProviderId;
use crate::db::curriculum::CEFR_LEVELS;
use crate::error::Result;
use crate::ui::widgets::model_picker::{self, ModelPickerAction, ModelPickerOptions};

use super::state::{OnboardingMode, OnboardingState};
use super::steps::{BATCH_SIZES, Step};
use super::{advance_onboarding, spawn_model_fetch};

pub async fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    let step = state.onboarding.current_step();
    match step {
        Step::Provider => handle_provider_key(state, code).await,
        Step::ApiKey => handle_text_key(state, code).await,
        Step::BaseUrl => handle_base_url_key(state, code).await,
        Step::Model => handle_model_key(state, code).await,
        Step::Cefr => handle_cefr_key(state, code).await,
        Step::BatchSize => handle_batch_size_key(state, code).await,
        _ => handle_text_key(state, code).await,
    }
}

fn handle_esc(state: &mut AppState) {
    match state.onboarding.mode {
        OnboardingMode::Initial => state.view = View::Quitting,
        OnboardingMode::AddPair => state.view = View::Pairs,
    }
}

async fn handle_provider_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    let all = ProviderId::all();
    match code {
        KeyCode::Esc => handle_esc(state),
        KeyCode::Enter | KeyCode::Char('\t') | KeyCode::Tab => {
            advance_onboarding(state).await?;
        }
        KeyCode::BackTab if state.onboarding.active > 0 => {
            state.onboarding.active -= 1;
            state.onboarding.load_input();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let idx = state.onboarding.provider_index;
            let new_idx = if idx == 0 { all.len() - 1 } else { idx - 1 };
            let new_provider = all[new_idx];
            state.onboarding.set_provider(new_provider);
            state.onboarding.input = new_provider.as_str().to_string();
            state.onboarding.error.clear();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let idx = state.onboarding.provider_index;
            let new_idx = (idx + 1) % all.len();
            let new_provider = all[new_idx];
            state.onboarding.set_provider(new_provider);
            state.onboarding.input = new_provider.as_str().to_string();
            state.onboarding.error.clear();
        }
        KeyCode::Char(c) => {
            state.onboarding.input.push(c);
            state.onboarding.input = state.onboarding.input.to_lowercase();
            let found = all
                .iter()
                .enumerate()
                .find(|(_, p)| p.as_str().starts_with(state.onboarding.input.as_str()));
            if let Some((idx, p)) = found {
                let p = *p;
                state.onboarding.provider_index = idx;
                state.onboarding.set_provider(p);
            }
            state.onboarding.error.clear();
        }
        KeyCode::Backspace => {
            state.onboarding.input.pop();
            let value = state.onboarding.input.to_lowercase();
            if let Some((idx, p)) = all
                .iter()
                .enumerate()
                .find(|(_, p)| p.as_str().starts_with(&value))
            {
                state.onboarding.provider_index = idx;
                state.onboarding.set_provider(*p);
            }
            state.onboarding.error.clear();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_base_url_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => handle_esc(state),
        KeyCode::Char('\t') | KeyCode::Tab | KeyCode::Enter => {
            advance_onboarding(state).await?;
        }
        KeyCode::BackTab | KeyCode::Up if state.onboarding.active > 0 => {
            state.onboarding.active -= 1;
            state.onboarding.load_input();
        }
        KeyCode::Char(c) => {
            state.onboarding.input.push(c);
            state.onboarding.error.clear();
        }
        KeyCode::Backspace => {
            state.onboarding.input.pop();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_model_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    let action = model_picker::handle_key(
        &mut state.onboarding.model_picker,
        &mut state.onboarding.input,
        code,
        &ModelPickerOptions::ONBOARDING,
    );
    match action {
        ModelPickerAction::Ignored | ModelPickerAction::InputPopped => {}
        ModelPickerAction::InputPushed => state.onboarding.error.clear(),
        ModelPickerAction::Back => handle_esc(state),
        ModelPickerAction::Retry => spawn_model_fetch(state),
        ModelPickerAction::EnterManual | ModelPickerAction::ExitManual => {
            state.onboarding.load_input();
        }
        ModelPickerAction::EmptyEnter | ModelPickerAction::ConfirmManual => {
            advance_onboarding(state).await?;
        }
        ModelPickerAction::Select(model_id) => {
            state.onboarding.model = model_id.clone();
            state.onboarding.input = model_id;
            advance_onboarding(state).await?;
        }
    }
    Ok(())
}

/// Shared logic for "pick one of a few constants, with a typed filter" steps
/// (CEFR level, batch size): arrows cycle the option, typing edits the input
/// and adopts the value when it parses to a valid option.
struct ChoiceField {
    options: &'static [&'static str],
    get: fn(&OnboardingState) -> String,
    set: fn(&mut OnboardingState, String),
    /// Canonicalizes the input box text in place (CEFR uppercases it).
    canonicalize: fn(&mut String),
    /// Maps the canonical input to a value when it is a valid option.
    validate: fn(&str) -> Option<String>,
    /// Which characters may be typed at all (batch size: digits only).
    char_filter: fn(char) -> bool,
}

const CEFR_FIELD: ChoiceField = ChoiceField {
    options: CEFR_LEVELS,
    get: |state: &OnboardingState| state.cefr.clone(),
    set: |state: &mut OnboardingState, value: String| state.cefr = value,
    canonicalize: |input: &mut String| *input = input.to_uppercase(),
    validate: |input: &str| {
        if CEFR_LEVELS.contains(&input) {
            Some(input.to_string())
        } else {
            None
        }
    },
    char_filter: |_| true,
};

const BATCH_SIZE_FIELD: ChoiceField = ChoiceField {
    options: BATCH_SIZES,
    get: |state: &OnboardingState| state.batch_size.to_string(),
    set: |state: &mut OnboardingState, value: String| {
        state.batch_size = value.parse().unwrap_or(3);
    },
    canonicalize: |_| {},
    validate: |input: &str| match input.parse::<u32>() {
        Ok(n) if (2..=5).contains(&n) => Some(input.to_string()),
        _ => None,
    },
    char_filter: |c: char| c.is_ascii_digit(),
};

fn cycle_choice(state: &mut AppState, field: &ChoiceField, backwards: bool) {
    let all = field.options;
    let current = (field.get)(&state.onboarding);
    let idx = all
        .iter()
        .position(|l| *l == current)
        .unwrap_or(all.len() - 1);
    let new_idx = if backwards {
        if idx == 0 { all.len() - 1 } else { idx - 1 }
    } else {
        (idx + 1) % all.len()
    };
    (field.set)(&mut state.onboarding, all[new_idx].to_string());
    state.onboarding.input = (field.get)(&state.onboarding);
    state.onboarding.error.clear();
}

async fn handle_choice_key(
    state: &mut AppState,
    code: KeyCode,
    field: &ChoiceField,
) -> Result<()> {
    match code {
        KeyCode::Esc => handle_esc(state),
        KeyCode::Enter | KeyCode::Char('\t') | KeyCode::Tab => {
            advance_onboarding(state).await?;
        }
        KeyCode::BackTab if state.onboarding.active > 0 => {
            state.onboarding.active -= 1;
            state.onboarding.load_input();
        }
        KeyCode::Up | KeyCode::Char('k') => cycle_choice(state, field, true),
        KeyCode::Down | KeyCode::Char('j') => cycle_choice(state, field, false),
        KeyCode::Char(c) if (field.char_filter)(c) => {
            state.onboarding.input.push(c);
            (field.canonicalize)(&mut state.onboarding.input);
            if let Some(value) = (field.validate)(&state.onboarding.input) {
                (field.set)(&mut state.onboarding, value);
            }
            state.onboarding.error.clear();
        }
        KeyCode::Backspace => {
            state.onboarding.input.pop();
            (field.canonicalize)(&mut state.onboarding.input);
            if let Some(value) = (field.validate)(&state.onboarding.input) {
                (field.set)(&mut state.onboarding, value);
            }
            state.onboarding.error.clear();
        }
        _ => {}
    }
    Ok(())
}

async fn handle_cefr_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    handle_choice_key(state, code, &CEFR_FIELD).await
}

async fn handle_batch_size_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    handle_choice_key(state, code, &BATCH_SIZE_FIELD).await
}

async fn handle_text_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc => handle_esc(state),
        KeyCode::Char('\t') | KeyCode::Tab | KeyCode::Enter | KeyCode::Down => {
            advance_onboarding(state).await?;
        }
        KeyCode::BackTab | KeyCode::Up if state.onboarding.active > 0 => {
            state.onboarding.active -= 1;
            state.onboarding.load_input();
        }
        KeyCode::Char(c) => {
            state.onboarding.input.push(c);
            state.onboarding.error.clear();
        }
        KeyCode::Backspace => {
            state.onboarding.input.pop();
        }
        _ => {}
    }
    Ok(())
}
