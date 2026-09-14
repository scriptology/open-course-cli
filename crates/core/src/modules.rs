//! Situational modules: a user describes a situation in free text ("prepare
//! for a doctor's visit", "sailing regatta") and the LLM turns it into a
//! `Module` of practice `ModuleUnit`s ("Navigation & Departments", ...).
//!
//! Units are NOT curriculum grammar topics: they live in their own id
//! namespace (`unit_<uuid>`) so they can never collide with grammar topic
//! slugs, and their progress is stored in the same generic `progress` rows
//! (`ProgressTopic` with `topic_id = unit id`) so EMA scoring, decay and
//! sync of the `progress` entity are reused without a new protocol.
//!
//! Module progress is a computed aggregate of its units' progress — never
//! stored — so last-writer-wins sync cannot desynchronize it.

use serde::{Deserialize, Serialize};

use crate::progress::ProgressTopic;
use crate::session::{COMPLETED_THRESHOLD, MASTERY_THRESHOLD};

/// Id prefix of situational modules.
pub const MODULE_ID_PREFIX: &str = "mod_";
/// Id prefix of module units. Any id with this prefix names a unit, never a
/// curriculum topic — curriculum auto-creation paths must skip such ids.
pub const UNIT_ID_PREFIX: &str = "unit_";

pub fn new_module_id() -> String {
    format!("{MODULE_ID_PREFIX}{}", uuid::Uuid::now_v7())
}

pub fn new_unit_id() -> String {
    format!("{UNIT_ID_PREFIX}{}", uuid::Uuid::now_v7())
}

/// Whether `id` belongs to the unit namespace (and therefore is not a
/// curriculum topic id).
pub fn is_unit_id(id: &str) -> bool {
    id.starts_with(UNIT_ID_PREFIX)
}

const DEFAULT_MODULE_STATUS: &str = "active";
const DEFAULT_MODULE_VISIBILITY: &str = "private";

fn default_module_status() -> String {
    DEFAULT_MODULE_STATUS.to_string()
}

fn default_module_visibility() -> String {
    DEFAULT_MODULE_VISIBILITY.to_string()
}

fn default_version() -> i32 {
    1
}

/// A situational module generated from the user's free-text description.
/// Persisted as the payload of the sync entity `module`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Module {
    /// `"mod_<uuid>"`.
    pub id: String,
    pub title: String,
    pub description: String,
    /// The user's original free-text description the module was generated
    /// from.
    #[serde(default)]
    pub source_prompt: String,
    /// Lifecycle status; currently always "active".
    #[serde(default = "default_module_status")]
    pub status: String,
    /// "private" for now; reserved for a future public module catalog.
    #[serde(default = "default_module_visibility")]
    pub visibility: String,
    pub native_lang: String,
    pub target_lang: String,
    #[serde(default = "default_version")]
    pub version: i32,
    /// RFC3339 timestamp of the last local or synced modification; `None`
    /// means "unknown" and sorts as the oldest.
    #[serde(default)]
    pub updated_at: Option<String>,
    /// RFC3339 tombstone marker; `Some` rows are hidden from reads and kept
    /// only so sync can propagate the deletion.
    #[serde(default)]
    pub deleted_at: Option<String>,
}

/// One practice unit of a module. Sessions are generated per unit (as they
/// are per grammar topic) and unit scores feed the module's computed
/// progress. Persisted as the payload of the sync entity `moduleUnit`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ModuleUnit {
    /// `"unit_<uuid>"`.
    pub id: String,
    pub module_id: String,
    pub title: String,
    pub description: String,
    pub order: i32,
    /// Existing curriculum grammar topics the LLM linked to this unit at
    /// generation time; sessions on the unit practice them as side topics.
    #[serde(default)]
    pub grammar_topic_ids: Vec<String>,
    /// The unit's domain glossary: terminology its sessions weave into
    /// exercises. Lives inside the unit payload — no separate table or sync
    /// entity — so generate/refine rewrite it atomically with the unit.
    /// Additive: units created before glossaries parse with an empty list.
    #[serde(default)]
    pub vocabulary: Vec<ModuleTerm>,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// RFC3339 tombstone marker; `Some` rows are hidden from reads.
    #[serde(default)]
    pub deleted_at: Option<String>,
}

/// One glossary entry of a module unit: a domain term (headword or phrase)
/// on the target language with its native-language translation. Maps to the
/// global vocabulary via the usual lemma id scheme (`Lemma::slug_id`) —
/// mastery of the word is global, module membership is tracked through
/// `vocabulary::Lemma::module_refs`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct ModuleTerm {
    /// Headword or phrase on the target language ("tack", "port side").
    pub lemma: String,
    /// Translation on the student's native language.
    pub translation: String,
    /// Universal Dependencies POS tag ("NOUN", "VERB", ...), if known.
    #[serde(default)]
    pub pos: Option<String>,
    /// Approximate CEFR level ("A1"–"C2"), if known.
    #[serde(default)]
    pub cefr: Option<String>,
}

/// LLM output of module generation before entity ids are assigned (see
/// `llm::prompts::build_module_generation_prompt` and
/// `llm::parse::parse_module`), or of a refine pass over an existing module
/// (see `llm::prompts::build_module_refine_prompt` and
/// `llm::parse::parse_module_refine`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModuleDraft {
    pub title: String,
    pub description: String,
    pub units: Vec<ModuleUnitDraft>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModuleUnitDraft {
    pub title: String,
    pub description: String,
    pub grammar_topic_ids: Vec<String>,
    /// The unit's domain glossary (see `ModuleUnit::vocabulary`).
    pub vocabulary: Vec<ModuleTerm>,
    /// Id of the existing unit this draft updates, echoed back by the LLM
    /// during a refine pass so the unit's progress survives; `None` for a
    /// brand-new unit (and always for initial generation). `materialize`
    /// ignores it — refine callers must apply updates themselves.
    pub existing_id: Option<String>,
}

impl ModuleDraft {
    /// Assigns entity ids and language metadata, producing the persistable
    /// `Module` and its `ModuleUnit`s (ordered as in the draft).
    pub fn materialize(
        &self,
        source_prompt: &str,
        native_lang: &str,
        target_lang: &str,
        now: &str,
    ) -> (Module, Vec<ModuleUnit>) {
        let module = Module {
            id: new_module_id(),
            title: self.title.clone(),
            description: self.description.clone(),
            source_prompt: source_prompt.to_string(),
            status: DEFAULT_MODULE_STATUS.to_string(),
            visibility: DEFAULT_MODULE_VISIBILITY.to_string(),
            native_lang: native_lang.to_string(),
            target_lang: target_lang.to_string(),
            version: 1,
            updated_at: Some(now.to_string()),
            deleted_at: None,
        };
        let units = self
            .units
            .iter()
            .enumerate()
            .map(|(i, unit)| ModuleUnit {
                id: new_unit_id(),
                module_id: module.id.clone(),
                title: unit.title.clone(),
                description: unit.description.clone(),
                order: i as i32,
                grammar_topic_ids: unit.grammar_topic_ids.clone(),
                vocabulary: unit.vocabulary.clone(),
                updated_at: Some(now.to_string()),
                deleted_at: None,
            })
            .collect();
        (module, units)
    }
}

/// Computed aggregate of a module's per-unit progress. Mirrors the
/// dashboard's level progress (`dashboard::get_progress_by_level`): a unit
/// counts as completed at `COMPLETED_THRESHOLD`, as in progress once
/// practiced, and the headline percent is the average unit score. `weak`
/// additionally flags practiced units below `MASTERY_THRESHOLD`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModuleProgress {
    pub total: usize,
    pub completed: usize,
    pub in_progress: usize,
    pub not_started: usize,
    /// Practiced units with a score below `MASTERY_THRESHOLD` (a subset of
    /// `in_progress`).
    pub weak: usize,
    /// Average score across all units, rounded.
    pub percent: f64,
}

/// Aggregates the progress rows of `units` (unit progress lives in
/// `ProgressTopic` rows whose `topic_id` is the unit id). Tombstoned units
/// are excluded.
pub fn get_module_progress(units: &[ModuleUnit], progress: &[ProgressTopic]) -> ModuleProgress {
    let active_units: Vec<&ModuleUnit> = units.iter().filter(|u| u.deleted_at.is_none()).collect();
    let total = active_units.len();
    let mut completed = 0;
    let mut in_progress = 0;
    let mut not_started = 0;
    let mut weak = 0;
    let mut total_score = 0.0;

    for unit in active_units {
        match progress.iter().find(|p| p.topic_id == unit.id) {
            None => not_started += 1,
            Some(pt) => {
                total_score += pt.score;
                if pt.score >= COMPLETED_THRESHOLD {
                    completed += 1;
                } else if pt.last_practiced.is_some() {
                    in_progress += 1;
                    if pt.score < MASTERY_THRESHOLD {
                        weak += 1;
                    }
                } else {
                    not_started += 1;
                }
            }
        }
    }

    let percent = if total > 0 {
        (total_score / total as f64).round()
    } else {
        0.0
    };

    ModuleProgress {
        total,
        completed,
        in_progress,
        not_started,
        weak,
        percent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: &str) -> ModuleUnit {
        ModuleUnit {
            id: id.to_string(),
            ..Default::default()
        }
    }

    fn practiced(topic_id: &str, score: f64) -> ProgressTopic {
        ProgressTopic {
            topic_id: topic_id.to_string(),
            score,
            mastery: score,
            practice_count: 1,
            last_practiced: Some("2026-01-01T00:00:00Z".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn generated_ids_carry_their_namespace_prefix() {
        assert!(new_module_id().starts_with(MODULE_ID_PREFIX));
        assert!(new_unit_id().starts_with(UNIT_ID_PREFIX));
        assert!(is_unit_id("unit_123"));
        assert!(!is_unit_id("mod_123"));
        assert!(!is_unit_id("grammar-topic-slug"));
    }

    #[test]
    fn materialize_assigns_ids_order_and_metadata() {
        let draft = ModuleDraft {
            title: "At the doctor".to_string(),
            description: "Medical visits".to_string(),
            units: vec![
                ModuleUnitDraft {
                    title: "Making an appointment".to_string(),
                    description: String::new(),
                    grammar_topic_ids: vec!["t1".to_string()],
                    vocabulary: vec![ModuleTerm {
                        lemma: "cita previa".to_string(),
                        translation: "appointment".to_string(),
                        pos: Some("NOUN".to_string()),
                        cefr: Some("A2".to_string()),
                    }],
                    existing_id: None,
                },
                ModuleUnitDraft {
                    title: "Describing symptoms".to_string(),
                    description: String::new(),
                    grammar_topic_ids: vec![],
                    vocabulary: vec![],
                    existing_id: None,
                },
            ],
        };
        let (module, units) = draft.materialize("doctor visit", "ru", "es", "2026-01-01T00:00:00Z");
        assert!(module.id.starts_with(MODULE_ID_PREFIX));
        assert_eq!(module.source_prompt, "doctor visit");
        assert_eq!(module.status, "active");
        assert_eq!(module.visibility, "private");
        assert_eq!(module.native_lang, "ru");
        assert_eq!(module.target_lang, "es");
        assert_eq!(units.len(), 2);
        assert!(units.iter().all(|u| u.id.starts_with(UNIT_ID_PREFIX)));
        assert!(units.iter().all(|u| u.module_id == module.id));
        assert_eq!(units[0].order, 0);
        assert_eq!(units[1].order, 1);
        assert_eq!(units[0].grammar_topic_ids, ["t1"]);
        // The glossary is carried over from the draft.
        assert_eq!(units[0].vocabulary.len(), 1);
        assert_eq!(units[0].vocabulary[0].lemma, "cita previa");
        assert_eq!(units[0].vocabulary[0].translation, "appointment");
        assert!(units[1].vocabulary.is_empty());
    }

    #[test]
    fn unit_without_vocabulary_still_parses() {
        // Units persisted before glossaries existed carry no `vocabulary`
        // key at all; the payload must stay valid.
        let legacy = r#"{
            "id": "unit_1",
            "module_id": "mod_1",
            "title": "Prices",
            "description": "Asking about costs",
            "order": 0
        }"#;
        let unit: ModuleUnit = serde_json::from_str(legacy).unwrap();
        assert!(unit.vocabulary.is_empty());
    }

    #[test]
    fn module_term_tolerates_minimal_json() {
        // pos/cefr are optional; the LLM may omit them.
        let term: ModuleTerm =
            serde_json::from_str(r#"{"lemma": "tack", "translation": "галс"}"#).unwrap();
        assert_eq!(term.lemma, "tack");
        assert_eq!(term.pos, None);
        assert_eq!(term.cefr, None);
    }

    #[test]
    fn module_progress_buckets_units_by_threshold() {
        let units = vec![
            unit("unit_1"),
            unit("unit_2"),
            unit("unit_3"),
            unit("unit_4"),
        ];
        let progress = vec![
            practiced("unit_1", 85.0), // completed
            practiced("unit_2", 60.0), // in progress
            practiced("unit_3", 30.0), // weak
                                       // unit_4 has no progress row: not started
        ];
        let aggregate = get_module_progress(&units, &progress);
        assert_eq!(aggregate.total, 4);
        assert_eq!(aggregate.completed, 1);
        assert_eq!(aggregate.in_progress, 2);
        assert_eq!(aggregate.not_started, 1);
        assert_eq!(aggregate.weak, 1);
        // (85 + 60 + 30) / 4 units = 43.75 → 44 (unpracticed units score 0).
        assert_eq!(aggregate.percent, 44.0);
    }

    #[test]
    fn module_progress_counts_unpracticed_rows_as_not_started() {
        let units = vec![unit("unit_1")];
        let progress = vec![ProgressTopic {
            topic_id: "unit_1".to_string(),
            score: 40.0,
            last_practiced: None,
            ..Default::default()
        }];
        let aggregate = get_module_progress(&units, &progress);
        assert_eq!(aggregate.not_started, 1);
        assert_eq!(aggregate.in_progress, 0);
        // The stored score still counts toward the average.
        assert_eq!(aggregate.percent, 40.0);
    }

    #[test]
    fn module_progress_skips_tombstoned_units() {
        let mut deleted = unit("unit_1");
        deleted.deleted_at = Some("2026-01-02T00:00:00Z".to_string());
        let units = vec![deleted, unit("unit_2")];
        let aggregate = get_module_progress(&units, &[]);
        assert_eq!(aggregate.total, 1);
        assert_eq!(aggregate.not_started, 1);
        assert_eq!(aggregate.percent, 0.0);
    }

    #[test]
    fn module_progress_empty_module_is_zero() {
        let aggregate = get_module_progress(&[], &[]);
        assert_eq!(aggregate, ModuleProgress::default());
    }
}
