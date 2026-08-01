use crate::ui::labels::{SettingsLabels, get_settings_labels, native_language_code};
use open_course_config::OpenCourseConfig;
use open_course_config::write_config;
use open_course_core::error::{AppError, Result};

use super::data::ResetAction;
use super::{Section, SettingsState};

/// Declarative description of an editable field in the Profile/Session
/// sections: how to display it, how to load it into the input box and how to
/// validate/store the edited value.
struct FieldDef {
    section: Section,
    index: usize,
    label: fn(&SettingsLabels) -> &'static str,
    /// Whether typing goes into the input box (hint mode toggles instead).
    text_input: bool,
    /// Current value for display, with "(none)" placeholders.
    display: fn(&OpenCourseConfig, &SettingsLabels) -> String,
    /// Current value for the input box.
    load: fn(&OpenCourseConfig) -> String,
    /// Validates and stores the trimmed input value.
    apply: fn(&mut OpenCourseConfig, &str, &SettingsLabels) -> Result<()>,
}

static FIELDS: &[FieldDef] = &[
    FieldDef {
        section: Section::Profile,
        index: 0,
        label: |labels| labels.age_label,
        text_input: true,
        display: |config: &OpenCourseConfig, labels: &SettingsLabels| {
            config
                .active_profile()
                .age
                .map(|a| a.to_string())
                .unwrap_or_else(|| labels.none_placeholder.to_string())
        },
        load: |config: &OpenCourseConfig| {
            config
                .active_profile()
                .age
                .map(|a| a.to_string())
                .unwrap_or_default()
        },
        apply: |config: &mut OpenCourseConfig,
                value: &str,
                labels: &SettingsLabels|
         -> Result<()> {
            config.active_profile_mut().age = if value.is_empty() {
                None
            } else {
                match value.parse::<u32>() {
                    Ok(age) if (14..=100).contains(&age) => Some(age),
                    _ => {
                        return Err(AppError::Config(labels.err_invalid_age.to_string()));
                    }
                }
            };
            Ok(())
        },
    },
    FieldDef {
        section: Section::Session,
        index: 0,
        label: |labels| labels.batch_size_label,
        text_input: false,
        display: |config: &OpenCourseConfig, labels: &SettingsLabels| {
            let size = config.preferences.batch_size;
            let suffix = if size == 3 {
                labels.recommended_suffix
            } else {
                ""
            };
            format!("{}{}", size, suffix)
        },
        load: |config: &OpenCourseConfig| config.preferences.batch_size.to_string(),
        apply: |config: &mut OpenCourseConfig,
                value: &str,
                labels: &SettingsLabels|
         -> Result<()> {
            let size = value.parse::<u32>().map_err(|_| {
                AppError::Config(labels.err_invalid_batch.replace("{value}", value))
            })?;
            if !(2..=5).contains(&size) {
                return Err(AppError::Config(labels.err_batch_range.to_string()));
            }
            config.preferences.batch_size = size;
            Ok(())
        },
    },
];

fn find_field(section: Section, index: usize) -> Option<&'static FieldDef> {
    FIELDS
        .iter()
        .find(|f| f.section == section && f.index == index)
}

pub(super) fn field_count(section: Section) -> usize {
    match section {
        Section::Provider => 4,
        Section::Data => ResetAction::all().len(),
        // The Account section is stateful; `SettingsState::field_count`
        // handles it.
        Section::Account => 0,
        _ => FIELDS.iter().filter(|f| f.section == section).count(),
    }
}

pub(super) fn field_label(section: Section, field: usize, labels: &SettingsLabels) -> &'static str {
    match section {
        Section::Provider | Section::Account => "",
        Section::Data => ResetAction::from_field(field)
            .map(|a| super::data::action_field_label(&a, labels))
            .unwrap_or(""),
        _ => find_field(section, field)
            .map(|f| (f.label)(labels))
            .unwrap_or(""),
    }
}

pub(super) fn field_value(
    config: &OpenCourseConfig,
    section: Section,
    field: usize,
    labels: &SettingsLabels,
) -> String {
    match section {
        Section::Provider | Section::Account => String::new(),
        Section::Data => ResetAction::from_field(field)
            .map(|a| super::data::action_description(&a, labels).to_string())
            .unwrap_or_default(),
        _ => find_field(section, field)
            .map(|f| (f.display)(config, labels))
            .unwrap_or_default(),
    }
}

impl SettingsState {
    pub(super) fn field_count(&self) -> usize {
        if self.section == Section::Account {
            return super::account::action_count(&self.account);
        }
        field_count(self.section)
    }

    pub(super) fn next_field(&mut self) {
        let count = self.field_count();
        self.active_field = (self.active_field + 1) % count;
    }

    pub(super) fn prev_field(&mut self) {
        let count = self.field_count();
        self.active_field = (self.active_field + count - 1) % count;
    }

    pub(super) fn is_text_field(&self) -> bool {
        match self.section {
            Section::Data | Section::Account => false,
            Section::Provider => true,
            _ => find_field(self.section, self.active_field)
                .map(|f| f.text_input)
                .unwrap_or(true),
        }
    }

    pub(super) fn load_input(&mut self, config: &OpenCourseConfig) {
        self.input = match self.section {
            Section::Provider | Section::Data | Section::Account => String::new(),
            _ => find_field(self.section, self.active_field)
                .map(|f| (f.load)(config))
                .unwrap_or_default(),
        };
        self.cursor = self.input.chars().count();
        if self.section == Section::Session {
            let size = config.preferences.batch_size;
            self.session_batch_idx = (size.saturating_sub(2)).min(3) as usize;
        }
    }

    pub(super) fn apply_input(
        &mut self,
        config: &mut OpenCourseConfig,
        labels: &SettingsLabels,
    ) -> Result<()> {
        let value = self.input.trim().to_string();
        match self.section {
            Section::Provider | Section::Data | Section::Account => Ok(()),
            _ => match find_field(self.section, self.active_field) {
                Some(f) => (f.apply)(config, &value, labels),
                None => Ok(()),
            },
        }
    }

    pub(super) fn save(
        &mut self,
        config: &mut OpenCourseConfig,
        data_dir: &std::path::Path,
    ) -> Result<()> {
        let labels = get_settings_labels(native_language_code(Some(config)));
        self.apply_input(config, &labels)?;
        write_config(config, data_dir)?;
        Ok(())
    }
}
