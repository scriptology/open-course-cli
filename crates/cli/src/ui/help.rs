use crate::app::{AppState, View};
use crate::ui::labels::{
    CommonLabels, ReportLabels, get_common_labels, get_docs_labels, get_report_labels,
    native_language_code,
};
use crate::ui::views::curriculum::CurriculumSortBy;
use crate::ui::views::onboarding::Step as OnboardingStep;
use crate::ui::views::session::Mode as SessionMode;
use crate::ui::views::settings::{ProviderSetupStep, Section};
use open_course_config::provider::ProviderId;

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

pub fn groups_for(state: &AppState) -> Vec<HelpGroup> {
    let lang = native_language_code(state.config.as_ref());
    let labels = get_report_labels(lang);
    let common = get_common_labels(lang);
    match state.view {
        View::Dashboard => dashboard_groups(state, labels, common),
        View::Curriculum => curriculum_groups(state, labels, common),
        View::Docs => docs_groups(state, labels, common),
        View::Session => session_groups(state, labels, common),
        View::Pairs => pairs_groups(labels, common),
        View::Report => report_groups(common),
        View::ModelCheck => model_check_groups(state, common),
        View::Settings => settings_groups(state, common),
        View::Onboarding => onboarding_groups(state, common),
        View::UpdateAvailable => update_groups(common),
        View::Quitting => Vec::new(),
    }
}

fn dashboard_groups(
    state: &AppState,
    labels: ReportLabels,
    common: CommonLabels,
) -> Vec<HelpGroup> {
    let mut nav = Vec::new();
    let mut actions = Vec::new();
    if state.dashboard.weak_visible_len() > 0 {
        nav.push(entry("↑↓", labels.select_topic));
        actions.push(entry("Enter", labels.start_label));
    }
    nav.push(entry("Esc", common.clear_topic_selection));
    actions.push(entry("n", labels.start_next_label));
    actions.push(entry("c", labels.curriculum));
    actions.push(entry("p", labels.pairs));
    actions.push(entry("s", labels.settings));
    vec![
        group(common.group_navigation, nav),
        group(common.group_actions, actions),
        group(common.group_exit, vec![entry("q", labels.quit)]),
    ]
}

fn curriculum_groups(
    state: &AppState,
    labels: ReportLabels,
    common: CommonLabels,
) -> Vec<HelpGroup> {
    if state.curriculum.pending_reset {
        return vec![group(
            common.group_actions,
            vec![
                entry("y", format!("{} {}", common.confirm, common.reset)),
                entry("n / Esc", common.cancel),
            ],
        )];
    }
    if state.curriculum.pending_delete.is_some() {
        return vec![group(
            common.group_actions,
            vec![
                entry("y", format!("{} {}", common.confirm, common.delete)),
                entry("n / Esc", common.cancel),
            ],
        )];
    }
    if state.curriculum.topics.is_empty() {
        return vec![
            group(
                common.group_actions,
                vec![entry("g / Enter", labels.generate_label)],
            ),
            group(common.group_exit, vec![entry("Esc", labels.back)]),
        ];
    }
    let sort_label = match state.curriculum.sort_by {
        CurriculumSortBy::Progression => labels.sort_progression,
        CurriculumSortBy::Score => labels.sort_score,
    };
    let nav = vec![entry("↑↓", labels.navigate)];
    vec![
        group(common.group_navigation, nav),
        group(
            common.group_actions,
            vec![
                entry("Enter", common.start_practice),
                entry("d", labels.docs),
                entry("s", format!("{} ({})", labels.sort, sort_label)),
                entry("a", labels.add_topics_label),
                entry("x", labels.delete_label),
                entry("r", labels.reset_label),
            ],
        ),
        group(common.group_exit, vec![entry("Esc", labels.back)]),
    ]
}

fn docs_groups(
    state: &AppState,
    report_labels: ReportLabels,
    common: CommonLabels,
) -> Vec<HelpGroup> {
    let labels = get_docs_labels(native_language_code(state.config.as_ref()));
    if state.docs.viewing_topic.is_some() {
        let nav = vec![entry("↑/↓", report_labels.wheel_scroll)];
        return vec![
            group(common.group_navigation, nav),
            group(
                common.group_actions,
                vec![
                    entry("e", labels.regenerate),
                    entry("n", common.start_practice),
                ],
            ),
            group(common.group_exit, vec![entry("Esc", common.all_topics)]),
        ];
    }
    let nav = vec![
        entry("↑/↓", report_labels.navigate),
        entry("s", labels.sort),
    ];
    vec![
        group(common.group_navigation, nav),
        group(
            common.group_actions,
            vec![
                entry("Enter", common.view),
                entry("n", common.start_practice),
            ],
        ),
        group(common.group_exit, vec![entry("Esc", common.back)]),
    ]
}

fn session_groups(state: &AppState, labels: ReportLabels, common: CommonLabels) -> Vec<HelpGroup> {
    if state.session.loading {
        return vec![group(common.group_exit, vec![entry("Esc", labels.cancel)])];
    }
    match state.session.mode {
        SessionMode::TopicSelection => vec![
            group(common.group_navigation, vec![entry("↑↓", labels.navigate)]),
            group(
                common.group_actions,
                vec![entry("Enter", labels.start_session)],
            ),
            group(common.group_exit, vec![entry("Esc", labels.back)]),
        ],
        SessionMode::Practicing => vec![
            group(
                common.group_actions,
                vec![
                    entry("(type)", common.write_your_answer),
                    entry("Enter", labels.submit),
                ],
            ),
            group(common.group_exit, vec![entry("Esc", labels.back)]),
        ],
    }
}

fn pairs_groups(labels: ReportLabels, common: CommonLabels) -> Vec<HelpGroup> {
    vec![
        group(common.group_navigation, vec![entry("↑/↓", labels.navigate)]),
        group(
            common.group_actions,
            vec![entry("Enter", labels.switch), entry("a", labels.add_pair)],
        ),
        group(common.group_exit, vec![entry("Esc", labels.back)]),
    ]
}

fn report_groups(common: CommonLabels) -> Vec<HelpGroup> {
    // The report is printed to the main screen: it has no scrolling and no
    // mouse modes, only the navigation keys below.
    vec![
        group(
            common.group_actions,
            vec![
                entry("n", common.new_topic),
                entry("r", common.repeat),
                entry("d", common.docs),
            ],
        ),
        group(common.group_exit, vec![entry("Esc", common.dashboard)]),
    ]
}

fn model_check_groups(state: &AppState, common: CommonLabels) -> Vec<HelpGroup> {
    if state.model_check.running {
        return vec![group(common.group_exit, vec![entry("Esc", common.cancel)])];
    }
    vec![
        group(
            common.group_actions,
            vec![
                entry("Enter / c", common.continue_label),
                entry("r", common.retry),
                entry("s", common.skip),
            ],
        ),
        group(
            common.group_exit,
            vec![entry("Esc / b", common.back_to_model_list)],
        ),
    ]
}

fn settings_groups(state: &AppState, common: CommonLabels) -> Vec<HelpGroup> {
    if state.settings.pending_reset.is_some() {
        return vec![group(
            common.group_actions,
            vec![
                entry("y", format!("{} {}", common.confirm, common.reset)),
                entry("any other key", common.cancel),
            ],
        )];
    }
    if !state.settings.in_section {
        return vec![
            group(common.group_navigation, vec![entry("↑/↓", common.navigate)]),
            group(common.group_actions, vec![entry("Enter", common.open)]),
            group(common.group_exit, vec![entry("Esc", common.back)]),
        ];
    }
    if state.settings.section == Section::Provider {
        return provider_setup_groups(state, common);
    }
    match state.settings.section {
        Section::Data => vec![
            group(common.group_navigation, vec![entry("↑/↓", common.action)]),
            group(common.group_actions, vec![entry("Enter", common.reset)]),
            group(common.group_exit, vec![entry("Esc", common.back)]),
        ],
        Section::Account => vec![
            group(common.group_navigation, vec![entry("↑/↓", common.action)]),
            group(common.group_actions, vec![entry("Enter", common.select)]),
            group(common.group_exit, vec![entry("Esc", common.back)]),
        ],
        Section::Session => vec![
            group(common.group_navigation, vec![entry("↑/↓", common.select)]),
            group(common.group_exit, vec![entry("Esc", common.back)]),
        ],
        Section::Profile => vec![
            group(
                common.group_navigation,
                vec![entry("←/→", common.move_caret)],
            ),
            group(
                common.group_actions,
                vec![entry("(type)", common.edit), entry("Enter", common.save)],
            ),
            group(common.group_exit, vec![entry("Esc", common.back)]),
        ],
        Section::Provider => unreachable!("handled above"),
    }
}

fn provider_setup_groups(state: &AppState, common: CommonLabels) -> Vec<HelpGroup> {
    match state.settings.provider_setup_step {
        ProviderSetupStep::SelectProvider => vec![
            group(common.group_navigation, vec![entry("↑/↓", common.navigate)]),
            group(common.group_actions, vec![entry("Enter", common.select)]),
            group(common.group_exit, vec![entry("Esc", common.back)]),
        ],
        ProviderSetupStep::BaseUrl | ProviderSetupStep::Endpoint => {
            let editable = state.settings.provider_setup_provider == ProviderId::Custom;
            let actions = if editable {
                vec![entry("(type)", common.edit), entry("Enter", common.save)]
            } else {
                vec![entry("Enter", common.next)]
            };
            vec![
                group(common.group_actions, actions),
                group(common.group_exit, vec![entry("Esc", common.back)]),
            ]
        }
        ProviderSetupStep::ApiKey => vec![
            group(
                common.group_actions,
                vec![entry("(type)", common.edit), entry("Enter", common.save)],
            ),
            group(common.group_exit, vec![entry("Esc", common.back)]),
        ],
        ProviderSetupStep::Model => {
            let picker = &state.settings.model_picker;
            if picker.loading {
                vec![group(common.group_exit, vec![entry("Esc", common.back)])]
            } else if picker.error.is_some() {
                vec![
                    group(
                        common.group_actions,
                        vec![entry("Enter", common.manual), entry("r", common.retry)],
                    ),
                    group(common.group_exit, vec![entry("Esc", common.back)]),
                ]
            } else if picker.manual {
                vec![
                    group(
                        common.group_actions,
                        vec![entry("(type)", common.edit), entry("Enter", common.save)],
                    ),
                    group(common.group_exit, vec![entry("Esc", common.back)]),
                ]
            } else if picker.models.is_empty() {
                vec![
                    group(
                        common.group_actions,
                        vec![entry("Enter", common.enter_manually)],
                    ),
                    group(common.group_exit, vec![entry("Esc", common.back)]),
                ]
            } else {
                vec![
                    group(common.group_navigation, vec![entry("↑/↓", common.navigate)]),
                    group(common.group_actions, vec![entry("Enter", common.select)]),
                    group(common.group_exit, vec![entry("Esc", common.back)]),
                ]
            }
        }
    }
}

fn onboarding_groups(state: &AppState, common: CommonLabels) -> Vec<HelpGroup> {
    let step = state.onboarding.steps[state.onboarding.active];
    let picker = &state.onboarding.model_picker;
    let mut groups = match step {
        OnboardingStep::Provider => vec![
            group(
                common.group_navigation,
                vec![entry("↑/↓", common.select_provider)],
            ),
            group(common.group_actions, vec![entry("Enter", common.next)]),
        ],
        OnboardingStep::Cefr => vec![
            group(
                common.group_navigation,
                vec![entry("↑/↓", common.select_level)],
            ),
            group(common.group_actions, vec![entry("Enter", common.next)]),
        ],
        OnboardingStep::BatchSize => vec![
            group(
                common.group_navigation,
                vec![entry("↑/↓", common.select_batch_size)],
            ),
            group(common.group_actions, vec![entry("Enter", common.next)]),
        ],
        OnboardingStep::Model if picker.loading => vec![group(common.group_actions, Vec::new())],
        OnboardingStep::Model if picker.error.is_some() => vec![group(
            common.group_actions,
            vec![entry("r", common.retry), entry("m", common.enter_manually)],
        )],
        OnboardingStep::Model if picker.manual => vec![group(
            common.group_actions,
            vec![
                entry("(type)", common.model_id),
                entry("Enter", common.next),
            ],
        )],
        OnboardingStep::Model if !picker.models.is_empty() => vec![
            group(
                common.group_navigation,
                vec![entry("↑/↓", common.select_model)],
            ),
            group(common.group_actions, vec![entry("Enter", common.next)]),
        ],
        _ => vec![
            group(
                common.group_navigation,
                vec![entry("Shift+Tab", common.previous_step)],
            ),
            group(
                common.group_actions,
                vec![
                    entry("(type)", common.edit),
                    entry("Tab / Enter", common.next),
                ],
            ),
        ],
    };
    groups.push(group(common.group_exit, vec![entry("Esc", common.quit)]));
    groups
}

fn update_groups(common: CommonLabels) -> Vec<HelpGroup> {
    vec![
        group(
            common.group_actions,
            vec![entry("y", common.install_update)],
        ),
        group(
            common.group_exit,
            vec![entry("n / Esc / Enter", common.skip_continue)],
        ),
    ]
}
