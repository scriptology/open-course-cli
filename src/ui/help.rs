use crate::app::{AppState, View};
use crate::config::provider::ProviderId;
use crate::ui::labels::{ReportLabels, get_docs_labels, get_report_labels, native_language_code};
use crate::ui::views::curriculum::CurriculumSortBy;
use crate::ui::views::onboarding::Step as OnboardingStep;
use crate::ui::views::session::Mode as SessionMode;
use crate::ui::views::settings::{ProviderSetupStep, Section};

pub struct HelpEntry {
    pub key: &'static str,
    pub action: String,
}

pub struct HelpGroup {
    pub title: &'static str,
    pub entries: Vec<HelpEntry>,
}

fn entry(key: &'static str, action: impl Into<String>) -> HelpEntry {
    HelpEntry {
        key,
        action: action.into(),
    }
}

fn group(title: &'static str, entries: Vec<HelpEntry>) -> HelpGroup {
    HelpGroup { title, entries }
}

fn mouse_entries(state: &AppState) -> Vec<HelpEntry> {
    if state.mouse_capture {
        vec![
            entry("wheel", "scroll"),
            entry("m", "switch to text selection"),
        ]
    } else {
        vec![
            entry("mouse", "select text"),
            entry("m", "switch to wheel scroll"),
        ]
    }
}

pub fn groups_for(state: &AppState) -> Vec<HelpGroup> {
    let labels = get_report_labels(native_language_code(state.config.as_ref()));
    match state.view {
        View::Dashboard => dashboard_groups(state, labels),
        View::Curriculum => curriculum_groups(state, labels),
        View::Docs => docs_groups(state),
        View::Session => session_groups(state, labels),
        View::Pairs => pairs_groups(labels),
        View::Report => report_groups(state),
        View::ModelCheck => model_check_groups(state),
        View::Settings => settings_groups(state),
        View::Onboarding => onboarding_groups(state),
        View::UpdateAvailable => update_groups(),
        View::Quitting => Vec::new(),
    }
}

fn dashboard_groups(state: &AppState, labels: ReportLabels) -> Vec<HelpGroup> {
    let mut nav = Vec::new();
    let mut actions = Vec::new();
    if state.dashboard.weak_visible_len() > 0 {
        nav.push(entry("↑↓", labels.select_topic));
        actions.push(entry("Enter", labels.start_label));
    }
    nav.push(entry("Esc", "clear topic selection"));
    actions.push(entry("n", labels.start_next_label));
    actions.push(entry("d", labels.docs));
    actions.push(entry("c", labels.curriculum));
    actions.push(entry("p", labels.pairs));
    actions.push(entry("s", labels.settings));
    vec![
        group("Navigation", nav),
        group("Actions", actions),
        group("Exit", vec![entry("q", labels.quit)]),
    ]
}

fn curriculum_groups(state: &AppState, labels: ReportLabels) -> Vec<HelpGroup> {
    if state.curriculum.pending_reset {
        return vec![group(
            "Actions",
            vec![entry("y", "confirm reset"), entry("n / Esc", "cancel")],
        )];
    }
    if state.curriculum.pending_delete.is_some() {
        return vec![group(
            "Actions",
            vec![entry("y", "confirm delete"), entry("n / Esc", "cancel")],
        )];
    }
    if state.curriculum.topics.is_empty() {
        return vec![
            group("Actions", vec![entry("g / Enter", labels.generate_label)]),
            group("Exit", vec![entry("Esc", labels.back)]),
        ];
    }
    let sort_label = match state.curriculum.sort_by {
        CurriculumSortBy::Progression => labels.sort_progression,
        CurriculumSortBy::Score => labels.sort_score,
    };
    vec![
        group("Navigation", vec![entry("↑↓ / wheel", labels.navigate)]),
        group(
            "Actions",
            vec![
                entry("Enter", labels.docs),
                entry("s", format!("{} ({})", labels.sort, sort_label)),
                entry("a", labels.add_topics_label),
                entry("x", labels.delete_label),
                entry("r", labels.reset_label),
            ],
        ),
        group("Exit", vec![entry("Esc", labels.back)]),
    ]
}

fn docs_groups(state: &AppState) -> Vec<HelpGroup> {
    let labels = get_docs_labels(native_language_code(state.config.as_ref()));
    if state.docs.viewing_topic.is_some() {
        let mut nav = vec![entry("↑/↓", "scroll")];
        nav.extend(mouse_entries(state));
        return vec![
            group("Navigation", nav),
            group(
                "Actions",
                vec![entry("e", labels.regenerate), entry("p", labels.practice)],
            ),
            group("Exit", vec![entry("Esc", "back to list")]),
        ];
    }
    vec![
        group(
            "Navigation",
            vec![entry("↑/↓ / wheel", "navigate"), entry("s", labels.sort)],
        ),
        group(
            "Actions",
            vec![entry("Enter", "view"), entry("p", labels.practice)],
        ),
        group("Exit", vec![entry("Esc", "back")]),
    ]
}

fn session_groups(state: &AppState, labels: ReportLabels) -> Vec<HelpGroup> {
    if state.session.loading {
        return vec![group("Exit", vec![entry("Esc", labels.cancel)])];
    }
    match state.session.mode {
        SessionMode::TopicSelection => vec![
            group("Navigation", vec![entry("↑↓", labels.navigate)]),
            group("Actions", vec![entry("Enter", labels.start_session)]),
            group("Exit", vec![entry("Esc", labels.back)]),
        ],
        SessionMode::Practicing => vec![
            group(
                "Actions",
                vec![
                    entry("(type)", "write your answer"),
                    entry("Enter", labels.submit),
                ],
            ),
            group("Exit", vec![entry("Esc", labels.back)]),
        ],
    }
}

fn pairs_groups(labels: ReportLabels) -> Vec<HelpGroup> {
    vec![
        group("Navigation", vec![entry("↑/↓", labels.navigate)]),
        group(
            "Actions",
            vec![entry("Enter", labels.switch), entry("a", labels.add_pair)],
        ),
        group("Exit", vec![entry("Esc", labels.back)]),
    ]
}

fn report_groups(state: &AppState) -> Vec<HelpGroup> {
    let mut nav = vec![entry("↑/↓", "scroll")];
    nav.extend(mouse_entries(state));
    vec![
        group("Navigation", nav),
        group(
            "Actions",
            vec![
                entry("n", "new topic"),
                entry("r", "repeat"),
                entry("d", "docs"),
            ],
        ),
        group("Exit", vec![entry("Esc", "dashboard")]),
    ]
}

fn model_check_groups(state: &AppState) -> Vec<HelpGroup> {
    if state.model_check.running {
        return vec![group("Exit", vec![entry("Esc", "cancel")])];
    }
    vec![
        group(
            "Actions",
            vec![
                entry("Enter / c", "continue"),
                entry("r", "retry"),
                entry("s", "skip"),
            ],
        ),
        group("Exit", vec![entry("Esc / b", "back to model list")]),
    ]
}

fn settings_groups(state: &AppState) -> Vec<HelpGroup> {
    if state.settings.pending_reset.is_some() {
        return vec![group(
            "Actions",
            vec![
                entry("y", "confirm reset"),
                entry("any other key", "cancel"),
            ],
        )];
    }
    if !state.settings.in_section {
        return vec![
            group("Navigation", vec![entry("↑/↓", "navigate")]),
            group("Actions", vec![entry("Enter", "open")]),
            group("Exit", vec![entry("Esc", "back")]),
        ];
    }
    if state.settings.section == Section::Provider {
        return provider_setup_groups(state);
    }
    match state.settings.section {
        Section::Data => vec![
            group("Navigation", vec![entry("↑/↓", "action")]),
            group("Actions", vec![entry("Enter", "reset")]),
            group("Exit", vec![entry("Esc", "back")]),
        ],
        Section::Session => vec![
            group("Navigation", vec![entry("↑/↓", "select")]),
            group("Exit", vec![entry("Esc", "back")]),
        ],
        Section::Profile => vec![
            group("Navigation", vec![entry("←/→", "move caret")]),
            group(
                "Actions",
                vec![entry("(type)", "edit"), entry("Enter", "save")],
            ),
            group("Exit", vec![entry("Esc", "back")]),
        ],
        Section::Provider => unreachable!("handled above"),
    }
}

fn provider_setup_groups(state: &AppState) -> Vec<HelpGroup> {
    match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => vec![
            group("Navigation", vec![entry("↑/↓", "navigate")]),
            group("Actions", vec![entry("Enter", "select")]),
            group("Exit", vec![entry("Esc", "back")]),
        ],
        ProviderSetupStep::BaseUrl | ProviderSetupStep::Endpoint => {
            let editable = state.settings.provider_setup_provider == ProviderId::Custom;
            let actions = if editable {
                vec![entry("(type)", "edit"), entry("Enter", "save")]
            } else {
                vec![entry("Enter", "next")]
            };
            vec![
                group("Actions", actions),
                group("Exit", vec![entry("Esc", "back")]),
            ]
        }
        ProviderSetupStep::ApiKey => vec![
            group(
                "Actions",
                vec![entry("(type)", "edit"), entry("Enter", "save")],
            ),
            group("Exit", vec![entry("Esc", "back")]),
        ],
        ProviderSetupStep::Model => {
            let picker = &state.settings.model_picker;
            if picker.loading {
                vec![group("Exit", vec![entry("Esc", "back")])]
            } else if picker.error.is_some() {
                vec![
                    group(
                        "Actions",
                        vec![entry("Enter", "manual"), entry("r", "retry")],
                    ),
                    group("Exit", vec![entry("Esc", "back")]),
                ]
            } else if picker.manual {
                vec![
                    group(
                        "Actions",
                        vec![entry("(type)", "edit"), entry("Enter", "save")],
                    ),
                    group("Exit", vec![entry("Esc", "back")]),
                ]
            } else if picker.models.is_empty() {
                vec![
                    group("Actions", vec![entry("Enter", "enter manually")]),
                    group("Exit", vec![entry("Esc", "back")]),
                ]
            } else {
                vec![
                    group("Navigation", vec![entry("↑/↓", "navigate")]),
                    group("Actions", vec![entry("Enter", "select")]),
                    group("Exit", vec![entry("Esc", "back")]),
                ]
            }
        }
    }
}

fn onboarding_groups(state: &AppState) -> Vec<HelpGroup> {
    let step = state.onboarding.steps[state.onboarding.active];
    let picker = &state.onboarding.model_picker;
    let mut groups = match step {
        OnboardingStep::Provider => vec![
            group("Navigation", vec![entry("↑/↓", "select provider")]),
            group("Actions", vec![entry("Enter", "next")]),
        ],
        OnboardingStep::Cefr => vec![
            group("Navigation", vec![entry("↑/↓", "select level")]),
            group("Actions", vec![entry("Enter", "next")]),
        ],
        OnboardingStep::BatchSize => vec![
            group("Navigation", vec![entry("↑/↓", "select batch size")]),
            group("Actions", vec![entry("Enter", "next")]),
        ],
        OnboardingStep::Model if picker.loading => vec![group("Actions", Vec::new())],
        OnboardingStep::Model if picker.error.is_some() => vec![group(
            "Actions",
            vec![entry("r", "retry"), entry("m", "enter manually")],
        )],
        OnboardingStep::Model if picker.manual => vec![group(
            "Actions",
            vec![entry("(type)", "model ID"), entry("Enter", "next")],
        )],
        OnboardingStep::Model if !picker.models.is_empty() => vec![
            group("Navigation", vec![entry("↑/↓", "select model")]),
            group("Actions", vec![entry("Enter", "next")]),
        ],
        _ => vec![
            group("Navigation", vec![entry("Shift+Tab", "previous step")]),
            group(
                "Actions",
                vec![entry("(type)", "edit"), entry("Tab / Enter", "next")],
            ),
        ],
    };
    groups.push(group("Exit", vec![entry("Esc", "quit")]));
    groups
}

fn update_groups() -> Vec<HelpGroup> {
    vec![
        group("Actions", vec![entry("y", "install update")]),
        group(
            "Exit",
            vec![entry("n / Esc / Enter", "skip, continue to app")],
        ),
    ]
}
