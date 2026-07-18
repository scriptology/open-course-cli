use crate::config::OpenCourseConfig;
use crate::config::write_config;
use crate::error::{AppError, Result};

use super::{Section, SettingsState};
use super::data::ResetAction;

/// Declarative description of an editable field in the Profile/Session
/// sections: how to display it, how to load it into the input box and how to
/// validate/store the edited value.
struct FieldDef {
    section: Section,
    index: usize,
    label: &'static str,
    /// Whether typing goes into the input box (hint mode toggles instead).
    text_input: bool,
    /// Current value for display, with "(none)" placeholders.
    display: fn(&OpenCourseConfig) -> String,
    /// Current value for the input box.
    load: fn(&OpenCourseConfig) -> String,
    /// Validates and stores the trimmed input value.
    apply: fn(&mut OpenCourseConfig, &str) -> Result<()>,
}

static FIELDS: &[FieldDef] = &[
    FieldDef {
        section: Section::Profile,
        index: 0,
        label: "Age",
        text_input: true,
        display: |config: &OpenCourseConfig| {
            config
                .active_profile()
                .age
                .map(|a| a.to_string())
                .unwrap_or_else(|| "(none)".to_string())
        },
        load: |config: &OpenCourseConfig| {
            config
                .active_profile()
                .age
                .map(|a| a.to_string())
                .unwrap_or_default()
        },
        apply: |config: &mut OpenCourseConfig, value: &str| -> Result<()> {
            config.active_profile_mut().age = if value.is_empty() {
                None
            } else {
                match value.parse::<u32>() {
                    Ok(age) if (1..=120).contains(&age) => Some(age),
                    _ => {
                        return Err(AppError::Config(format!(
                            "Age must be a number between 1 and 120: {value}"
                        )));
                    }
                }
            };
            Ok(())
        },
    },
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
        _ => FIELDS.iter().filter(|f| f.section == section).count(),
    }
}

pub(super) fn field_label(section: Section, field: usize) -> &'static str {
    match section {
        Section::Provider => "",
        Section::Data => ResetAction::from_field(field)
            .map(|a| a.field_label())
            .unwrap_or(""),
        _ => find_field(section, field).map(|f| f.label).unwrap_or(""),
    }
}

pub(super) fn field_value(config: &OpenCourseConfig, section: Section, field: usize) -> String {
    match section {
        Section::Provider => String::new(),
        Section::Data => ResetAction::from_field(field)
            .map(|a| a.description().to_string())
            .unwrap_or_default(),
        _ => find_field(section, field)
            .map(|f| (f.display)(config))
            .unwrap_or_default(),
    }
}

impl SettingsState {
    pub(super) fn field_count(&self) -> usize {
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
            Section::Data => false,
            Section::Provider => true,
            _ => find_field(self.section, self.active_field)
                .map(|f| f.text_input)
                .unwrap_or(true),
        }
    }

    pub(super) fn load_input(&mut self, config: &OpenCourseConfig) {
        self.input = match self.section {
            Section::Provider | Section::Data => String::new(),
            _ => find_field(self.section, self.active_field)
                .map(|f| (f.load)(config))
                .unwrap_or_default(),
        };
        self.cursor = self.input.chars().count();
    }

    pub(super) fn apply_input(&mut self, config: &mut OpenCourseConfig) -> Result<()> {
        let value = self.input.trim().to_string();
        match self.section {
            Section::Provider | Section::Data => Ok(()),
            _ => match find_field(self.section, self.active_field) {
                Some(f) => (f.apply)(config, &value),
                None => Ok(()),
            },
        }
    }

    pub(super) fn save(
        &mut self,
        config: &mut OpenCourseConfig,
        data_dir: &std::path::Path,
    ) -> Result<()> {
        self.apply_input(config)?;
        write_config(config, data_dir)?;
        Ok(())
    }
}
