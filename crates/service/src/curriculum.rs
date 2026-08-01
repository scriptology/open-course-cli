//! Curriculum operations: generation, extension, persistence, and deletion of
//! topics. The caller (CLI adapter) owns the UI state and the task spawning.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use open_course_config::OpenCourseConfig;
use open_course_core::error::Result;
use open_course_db::Database;
use open_course_db::curriculum::{Curriculum, Topic, difficulty_to_cefr};
use open_course_llm::client::{DEFAULT_MAX_TOKENS, extract_typed};
use open_course_llm::factory::create_llm_model;
use open_course_llm::pipeline::generate_curriculum as generate_curriculum_llm;
use open_course_llm::prompts::build_curriculum_extension_prompt;
use open_course_llm::result::LlmResult;

/// Generates a full curriculum for the configured language pair and fills in
/// any missing topic fields (languages, version, level, order).
pub async fn generate_curriculum(
    config: &OpenCourseConfig,
    tx: &mpsc::Sender<LlmResult>,
    data_dir: Option<&Path>,
) -> Result<Curriculum> {
    let profile = config.active_profile();
    let target_language = profile.target_language.clone();
    let native_language = profile.native_language.clone();

    let model = create_llm_model(config)?;
    let mut curriculum =
        generate_curriculum_llm(model.as_ref(), profile, Some(tx), data_dir).await?;
    normalize_generated_topics(&mut curriculum.topics, &target_language, &native_language);
    for (index, topic) in curriculum.topics.iter_mut().enumerate() {
        if topic.order.is_none() {
            topic.order = Some(topic.cefr_numeric() * 1000 + index as i32);
        }
    }
    Ok(curriculum)
}

/// Generates `count` additional topics continuing the existing curriculum and
/// fills in any missing topic fields.
pub async fn extend_curriculum(
    db: &Database,
    config: &OpenCourseConfig,
    count: usize,
) -> Result<Vec<Topic>> {
    let profile = config.active_profile();
    let target_language = profile.target_language.clone();
    let native_language = profile.native_language.clone();

    let curriculum = db.curriculum().read_all().await?;
    let progress = db.progress().read_all().await?;

    let prompt =
        build_curriculum_extension_prompt(profile, &curriculum.topics, &progress.topics, count);

    let base_order = curriculum
        .topics
        .iter()
        .filter_map(|t| t.order)
        .max()
        .unwrap_or(0);

    let model = create_llm_model(config)?;
    let mut extension =
        extract_typed::<CurriculumExtension>(model.as_ref(), &prompt, DEFAULT_MAX_TOKENS).await?;
    normalize_generated_topics(&mut extension.topics, &target_language, &native_language);
    for (index, topic) in extension.topics.iter_mut().enumerate() {
        if topic.order.is_none() {
            topic.order = Some(base_order + 1 + index as i32);
        }
    }
    Ok(extension.topics)
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
struct CurriculumExtension {
    topics: Vec<Topic>,
}

/// Fills fields the LLM may have left out: languages, version, and CEFR level
/// derived from the difficulty.
fn normalize_generated_topics(topics: &mut [Topic], target_language: &str, native_language: &str) {
    for topic in topics {
        if topic.target_lang.is_empty() {
            topic.target_lang = target_language.to_string();
        }
        if topic.native_lang.is_empty() {
            topic.native_lang = native_language.to_string();
        }
        if topic.version == 0 {
            topic.version = 1;
        }
        if topic.level.is_none() {
            topic.level = difficulty_to_cefr(&topic.difficulty);
        }
    }
}

/// Upserts topics into the curriculum, replacing it wholesale when
/// `replace_all` is set. Writes the corresponding outbox entries.
pub async fn persist_topics(db: &Database, topics: &[Topic], replace_all: bool) -> Result<()> {
    use open_course_db::outbox::{ENTITY_TOPIC, OP_TOMBSTONE_RESET, OP_UPSERT};

    let table = db.curriculum();
    if replace_all {
        table.delete_all().await?;
        let payload = crate::reset_payload(&chrono::Utc::now().to_rfc3339());
        crate::outbox_append(db, OP_TOMBSTONE_RESET, ENTITY_TOPIC, "*", &payload).await;
    }
    for topic in topics {
        // Stamp once so the outbox payload matches the stored row.
        let mut stamped = topic.clone();
        stamped.updated_at = Some(chrono::Utc::now().to_rfc3339());
        table.upsert_with_timestamps(&stamped).await?;
        if let Ok(payload) = serde_json::to_string(&stamped) {
            crate::outbox_append(db, OP_UPSERT, ENTITY_TOPIC, &stamped.id, &payload).await;
        }
    }
    Ok(())
}

/// Removes a topic from the curriculum along with its progress and reviews.
pub async fn delete_topic(db: &Database, topic_id: &str) -> Result<()> {
    use open_course_db::outbox::{ENTITY_PROGRESS, ENTITY_TOPIC, OP_DELETE};

    db.curriculum().delete_by_topic_id(topic_id).await?;
    db.progress().delete_by_topic_id(topic_id).await?;
    db.reviews().remove_by_topic_id(topic_id).await?;
    crate::outbox_append(db, OP_DELETE, ENTITY_TOPIC, topic_id, "").await;
    crate::outbox_append(db, OP_DELETE, ENTITY_PROGRESS, topic_id, "").await;
    Ok(())
}

/// Reads the curriculum (after the idempotent cleanup) together with the
/// per-topic scores.
pub async fn load_curriculum(db: &Database) -> Result<(Vec<Topic>, HashMap<String, f64>)> {
    open_course_db::curriculum::cleanup_topics(db).await?;
    let curriculum = db.curriculum().read_all().await?;
    let progress = db.progress().read_all().await?;
    let progress_map: HashMap<String, f64> = progress
        .topics
        .iter()
        .map(|t| (t.topic_id.clone(), t.score))
        .collect();
    Ok((curriculum.topics, progress_map))
}

/// Clears the curriculum, progress, and reviews tables ahead of a full
/// regeneration.
pub async fn reset_curriculum_data(db: &Database) -> Result<()> {
    db.curriculum().reset().await?;
    db.progress().reset().await?;
    db.reviews().reset().await?;
    Ok(())
}
