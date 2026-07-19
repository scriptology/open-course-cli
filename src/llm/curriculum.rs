use std::path::Path;

use futures_util::future::join_all;
use tokio::sync::mpsc;

use crate::app::LlmResult;
use crate::config::profile::UserProfile;
use crate::db::curriculum::{
    CEFR_LEVELS, CURRICULUM_DOMAIN_DESCRIPTIONS, Curriculum, Topic, cefr_to_difficulty,
    target_level_topic_count,
};
use crate::error::{AppError, Result};
use crate::llm::client::{LlmClient, extract_typed};
use crate::llm::parse::{LevelCurriculum, curriculum_parse_errors, parse_curriculum_level};
use crate::llm::prompts::{build_curriculum_gap_prompt, build_curriculum_level_prompt};
use crate::llm::retry::{RetryConfig, generate_with_retries};
use crate::llm::transport::{LLM_CURRICULUM_TIMEOUT_SECONDS, send_status};

const MAX_TOKENS_CURRICULUM: u32 = 16384;

pub async fn generate_curriculum(
    client: &dyn LlmClient,
    profile: &UserProfile,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Curriculum> {
    let start_index = current_cefr_index(profile.self_assessed_cefr.as_deref());
    let levels: Vec<&str> = CEFR_LEVELS.iter().skip(start_index).copied().collect();

    send_status("Generating curriculum plan...", stream_tx);

    let level_futures: Vec<_> = levels
        .iter()
        .map(|&level| {
            let level_owned = level.to_string();
            async move {
                let previous = previous_cefr_level(&level_owned);
                let count = target_level_topic_count(&level_owned);
                let prompt = build_curriculum_level_prompt(profile, &level_owned, previous, count);
                let mut topics =
                    generate_curriculum_batch(client, &prompt, &level_owned, stream_tx, data_dir)
                        .await?;
                normalize_level_topics(&mut topics, profile, &level_owned);
                topics =
                    fill_level_gaps(client, profile, &level_owned, topics, stream_tx, data_dir)
                        .await?;
                normalize_level_topics(&mut topics, profile, &level_owned);
                if let Some(tx) = stream_tx {
                    let _ = tx
                        .send(LlmResult::CurriculumStreamChunk {
                            level: level_owned.clone(),
                            status: format!("{level_owned} complete"),
                        })
                        .await;
                }
                Ok::<Vec<Topic>, AppError>(topics)
            }
        })
        .collect();

    let mut all_topics = Vec::new();
    for result in join_all(level_futures).await {
        all_topics.extend(result?);
    }

    send_status("Curriculum generation complete", stream_tx);

    Ok(Curriculum {
        version: 1,
        target_language: profile.target_language.clone(),
        native_language: profile.native_language.clone(),
        topics: all_topics,
    })
}

fn previous_cefr_level(level: &str) -> Option<&'static str> {
    let idx = CEFR_LEVELS
        .iter()
        .position(|&l| l == level.to_uppercase())?;
    idx.checked_sub(1).map(|i| CEFR_LEVELS[i])
}

fn current_cefr_index(cefr: Option<&str>) -> usize {
    let cefr = cefr
        .map(|c| c.to_uppercase())
        .unwrap_or_else(|| "A1".to_string());
    CEFR_LEVELS.iter().position(|&l| l == cefr).unwrap_or(0)
}

fn normalize_level_topics(topics: &mut [Topic], profile: &UserProfile, level: &str) {
    for topic in topics.iter_mut() {
        topic.id = topic
            .id
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        if topic.id.is_empty() {
            topic.id = "topic".to_string();
        }
        if topic.target_lang.is_empty() {
            topic.target_lang = profile.target_language.clone();
        }
        if topic.native_lang.is_empty() {
            topic.native_lang = profile.native_language.clone();
        }
        if topic.level.is_none() {
            topic.level = Some(level.to_string());
        }
        if topic.difficulty.is_empty() {
            topic.difficulty = cefr_to_difficulty(level).to_string();
        }
    }
}

async fn generate_curriculum_batch(
    client: &dyn LlmClient,
    prompt: &str,
    level: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Vec<Topic>> {
    let config = RetryConfig {
        system: "You are a curriculum design assistant. Return ONLY valid JSON matching the requested schema. Do not wrap in markdown, do not add explanations, do not add commentary.",
        stricter_system: "You are a curriculum design assistant. Your response must be a single valid JSON object and nothing else. No markdown code fences, no explanations, no commentary.",
        status_label: format!("{level}: generating curriculum"),
        level: Some(level),
        max_tokens: MAX_TOKENS_CURRICULUM,
        timeout_secs: LLM_CURRICULUM_TIMEOUT_SECONDS,
        log_kind: "curriculum",
        raw_log_kind: format!("curriculum-{level}"),
        error_context: format!(" for {level}"),
        extraction_empty_message: "(structured extraction returned empty topics)",
        error_kind: format!("{level} curriculum"),
    };
    generate_with_retries(
        client,
        prompt,
        config,
        |cleaned, content_chars, reasoning_chars| {
            parse_curriculum_level(cleaned, level, content_chars, reasoning_chars)
        },
        || async move {
            let level_curriculum =
                extract_typed::<LevelCurriculum>(client, prompt, MAX_TOKENS_CURRICULUM).await?;
            Ok(if level_curriculum.topics.is_empty() {
                None
            } else {
                Some(level_curriculum.topics)
            })
        },
        |cleaned| curriculum_parse_errors(cleaned, level),
        stream_tx,
        data_dir,
    )
    .await
}

fn missing_domains(topics: &[Topic], level: &str) -> Vec<&'static str> {
    use std::collections::HashSet;

    let present: HashSet<&str> = topics
        .iter()
        .filter(|t| t.level.as_deref() == Some(level))
        .filter_map(|t| crate::db::curriculum::topic_domain(t))
        .collect();

    CURRICULUM_DOMAIN_DESCRIPTIONS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !present.contains(*name))
        .collect()
}

async fn fill_level_gaps(
    client: &dyn LlmClient,
    profile: &UserProfile,
    level: &str,
    topics: Vec<Topic>,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<Vec<Topic>> {
    let gaps = missing_domains(&topics, level);
    if gaps.is_empty() {
        return Ok(topics);
    }

    let prompt = build_curriculum_gap_prompt(profile, level, &gaps);
    let gap_topics = generate_curriculum_batch(client, &prompt, level, stream_tx, data_dir).await?;

    let mut combined = topics;
    combined.extend(gap_topics);
    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(id: &str) -> Topic {
        Topic {
            id: id.to_string(),
            name: "name".to_string(),
            description: "desc".to_string(),
            difficulty: String::new(),
            level: None,
            order: None,
            tags: vec![],
            target_lang: String::new(),
            native_lang: String::new(),
            version: 1,
        }
    }

    fn profile() -> UserProfile {
        UserProfile {
            native_language: "Russian".to_string(),
            target_language: "Spanish".to_string(),
            age: None,
            self_assessed_cefr: None,
        }
    }

    // --- previous_cefr_level ---

    #[test]
    fn previous_level_of_a1_is_none() {
        assert_eq!(previous_cefr_level("A1"), None);
    }

    #[test]
    fn previous_level_returns_static_str() {
        assert_eq!(previous_cefr_level("B1"), Some("A2"));
        assert_eq!(previous_cefr_level("C2"), Some("C1"));
    }

    #[test]
    fn previous_level_is_case_insensitive() {
        assert_eq!(previous_cefr_level("b2"), Some("B1"));
    }

    #[test]
    fn previous_level_of_unknown_is_none() {
        assert_eq!(previous_cefr_level("X9"), None);
    }

    // --- normalize_level_topics ---

    #[test]
    fn normalize_converts_ids_to_kebab_case() {
        let mut topics = vec![topic("My Topic! (B1)")];
        normalize_level_topics(&mut topics, &profile(), "B1");
        assert_eq!(topics[0].id, "my-topic---b1-");
    }

    #[test]
    fn normalize_replaces_empty_id_with_placeholder() {
        let mut topics = vec![topic("")];
        normalize_level_topics(&mut topics, &profile(), "A1");
        assert_eq!(topics[0].id, "topic");
    }

    #[test]
    fn normalize_turns_invalid_chars_into_hyphens() {
        let mut topics = vec![topic("!!!")];
        normalize_level_topics(&mut topics, &profile(), "A1");
        assert_eq!(topics[0].id, "---");
    }

    #[test]
    fn normalize_fills_missing_fields() {
        let mut topics = vec![topic("ok-id")];
        normalize_level_topics(&mut topics, &profile(), "B1");
        let t = &topics[0];
        assert_eq!(t.target_lang, "Spanish");
        assert_eq!(t.native_lang, "Russian");
        assert_eq!(t.level.as_deref(), Some("B1"));
        assert_eq!(t.difficulty, "intermediate");
    }

    #[test]
    fn normalize_preserves_set_fields() {
        let mut t = topic("ok-id");
        t.target_lang = "French".to_string();
        t.level = Some("A2".to_string());
        t.difficulty = "beginner".to_string();
        let mut topics = vec![t];
        normalize_level_topics(&mut topics, &profile(), "B1");
        let t = &topics[0];
        assert_eq!(t.target_lang, "French");
        assert_eq!(t.level.as_deref(), Some("A2"));
        assert_eq!(t.difficulty, "beginner");
    }
}
