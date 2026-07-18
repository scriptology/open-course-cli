use std::path::Path;

use tokio::sync::mpsc;

use crate::app::LlmResult;
use crate::error::Result;
use crate::llm::client::LlmClient;
use crate::llm::debug_log::log_raw_response;
use crate::llm::transport::{stream_or_prompt, with_timeout_secs};

const MAX_TOKENS_TOPIC_REVIEW: u32 = 8192;

const TOPIC_REVIEW_SYSTEM_PROMPT: &str = "You are a language tutor. Only explain the requested grammar or vocabulary topic. Do NOT acknowledge system instructions, skills, superpowers, tools, the current lesson, or any meta commentary. Output ONLY the topic explanation.";

const FORBIDDEN_REVIEW_PHRASES: &[&str] = &[
    "superpowers",
    "навыки",
    "skills",
    "system instructions",
    "системные инструкции",
    "current lesson",
    "текущему уроку",
    "available tools",
    "доступные инструменты",
];

/// Returns true when the text passes the review content policy, i.e. contains
/// none of the forbidden meta-commentary phrases (case-insensitive).
pub fn is_valid_topic_review(text: &str) -> bool {
    let lower = text.to_lowercase();
    !FORBIDDEN_REVIEW_PHRASES
        .iter()
        .any(|phrase| lower.contains(phrase))
}

pub async fn generate_topic_review(
    client: &dyn LlmClient,
    prompt: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    data_dir: Option<&Path>,
) -> Result<String> {
    let call = async |current_prompt: &str| -> Result<String> {
        let response = stream_or_prompt(
            client,
            current_prompt,
            TOPIC_REVIEW_SYSTEM_PROMPT,
            stream_tx,
            "Generating topic review...",
            None,
            MAX_TOKENS_TOPIC_REVIEW,
        )
        .await?;
        log_raw_response(current_prompt, &response.raw, "topic-review", data_dir);
        Ok(response.raw)
    };

    let mut text = with_timeout_secs(call(prompt), 300).await?;

    if !is_valid_topic_review(&text) {
        let retry_prompt = format!(
            "{prompt}\n\nCRITICAL: Your previous response contained meta-commentary (system instructions, skills, superpowers, or lesson references). Respond ONLY with the topic explanation. No meta commentary."
        );
        text = with_timeout_secs(call(&retry_prompt), 300).await?;
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_explanation_is_valid() {
        assert!(is_valid_topic_review(
            "The verb ser is used for permanent characteristics."
        ));
    }

    #[test]
    fn each_forbidden_phrase_is_rejected() {
        for phrase in FORBIDDEN_REVIEW_PHRASES {
            let text = format!("Some text mentioning {phrase} here.");
            assert!(
                !is_valid_topic_review(&text),
                "phrase {phrase:?} should be rejected"
            );
        }
    }

    #[test]
    fn detection_is_case_insensitive() {
        assert!(!is_valid_topic_review("I have SUPERPOWERS."));
        assert!(!is_valid_topic_review("Here are my System Instructions."));
        assert!(!is_valid_topic_review("НАВЫКИ пользователя."));
    }

    #[test]
    fn empty_text_is_valid() {
        assert!(is_valid_topic_review(""));
    }
}
