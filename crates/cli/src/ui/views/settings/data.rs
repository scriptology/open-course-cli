use crate::ui::labels::SettingsLabels;

pub use open_course_service::reset::ResetAction;

pub fn action_label(action: &ResetAction, labels: &SettingsLabels) -> &'static str {
    match action {
        ResetAction::Progress => labels.reset_progress_action,
        ResetAction::History => labels.reset_history_action,
        ResetAction::Curriculum => labels.reset_curriculum_action,
        ResetAction::Reviews => labels.reset_reviews_action,
        ResetAction::All => labels.reset_all_action,
    }
}

/// Title of the field row in the Data section.
pub fn action_field_label(action: &ResetAction, labels: &SettingsLabels) -> &'static str {
    match action {
        ResetAction::Progress => labels.reset_progress_title,
        ResetAction::History => labels.reset_history_title,
        ResetAction::Curriculum => labels.reset_curriculum_title,
        ResetAction::Reviews => labels.reset_reviews_title,
        ResetAction::All => labels.reset_all_title,
    }
}

/// Description shown as the field value in the Data section.
pub fn action_description(action: &ResetAction, labels: &SettingsLabels) -> &'static str {
    match action {
        ResetAction::Progress => labels.reset_progress_desc,
        ResetAction::History => labels.reset_history_desc,
        ResetAction::Curriculum => labels.reset_curriculum_desc,
        ResetAction::Reviews => labels.reset_reviews_desc,
        ResetAction::All => labels.reset_all_desc,
    }
}
