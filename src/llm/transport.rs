use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};

use crate::app::LlmResult;
use crate::error::{AppError, Result};
use crate::llm::client::LlmClient;
use crate::llm::debug_log::log_debug_event;
use crate::llm::streaming::StreamChunk;

pub(crate) const LLM_TIMEOUT_SECONDS: u64 = 60;
pub(crate) const LLM_CURRICULUM_TIMEOUT_SECONDS: u64 = 300;

#[derive(Debug, Clone)]
pub(crate) struct LlmResponse {
    pub raw: String,
    pub content_chars: usize,
    pub reasoning_chars: usize,
}

impl LlmResponse {
    pub fn empty() -> Self {
        Self {
            raw: String::new(),
            content_chars: 0,
            reasoning_chars: 0,
        }
    }

    pub fn from_text(text: String) -> Self {
        let chars = text.chars().count();
        Self {
            raw: text,
            content_chars: chars,
            reasoning_chars: 0,
        }
    }
}

pub(crate) fn send_status(label: &str, stream_tx: Option<&mpsc::Sender<LlmResult>>) {
    if let Some(tx) = stream_tx {
        let _ = tx.try_send(LlmResult::StreamChunk(label.to_string()));
    }
}

pub(crate) fn send_stream_status(tx: &mpsc::Sender<LlmResult>, level: Option<&str>, status: &str) {
    if let Some(level) = level {
        let _ = tx.try_send(LlmResult::CurriculumStreamChunk {
            level: level.to_string(),
            status: status.to_string(),
        });
    } else {
        let _ = tx.try_send(LlmResult::StreamChunk(status.to_string()));
    }
}

pub(crate) async fn with_timeout_secs<T, F>(future: F, secs: u64) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    timeout(Duration::from_secs(secs), future)
        .await
        .map_err(|_| AppError::Llm(format!("LLM request timed out after {secs}s")))?
}

/// Non-streaming fallback used when streaming is unavailable or fails.
async fn prompt_fallback(
    client: &dyn LlmClient,
    prompt: &str,
    system: &str,
    max_tokens: u32,
) -> Result<LlmResponse> {
    let text = with_timeout_secs(
        client.prompt(prompt, Some(system), max_tokens),
        LLM_TIMEOUT_SECONDS,
    )
    .await?;
    Ok(LlmResponse::from_text(text))
}

pub(crate) async fn stream_or_prompt(
    client: &dyn LlmClient,
    prompt: &str,
    system: &str,
    stream_tx: Option<&mpsc::Sender<LlmResult>>,
    status_label: &str,
    level: Option<&str>,
    max_tokens: u32,
) -> Result<LlmResponse> {
    let Some(tx) = stream_tx else {
        return prompt_fallback(client, prompt, system, max_tokens).await;
    };

    send_stream_status(tx, level, &format!("{status_label} (starting...)"));
    let mut stream = match client.stream_prompt(prompt, Some(system), max_tokens).await {
        Ok(stream) => stream,
        Err(e) => {
            log_debug_event(
                "stream",
                &format!("Streaming failed ({e}), falling back to non-streaming prompt"),
                None,
            );
            return prompt_fallback(client, prompt, system, max_tokens).await;
        }
    };
    let mut raw = String::new();
    let mut reasoning_text = String::new();
    let mut content_chars = 0usize;
    let mut reasoning_chars = 0usize;
    let idle_timeout = Duration::from_secs(45);
    let overall_timeout = Duration::from_secs(300);
    let start = Instant::now();

    loop {
        if start.elapsed() > overall_timeout {
            return Err(AppError::Llm(format!(
                "{status_label} stream exceeded overall timeout of 300s"
            )));
        }
        let chunk = match timeout(idle_timeout, stream.next()).await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => {
                return Err(AppError::Llm(format!(
                    "{status_label} stream idle timeout after 45s"
                )));
            }
        };
        match chunk {
            Ok(StreamChunk::Content(text)) => {
                content_chars += text.chars().count();
                raw.push_str(&text);
                let status = format!("{status_label} (writing {content_chars} chars)");
                send_stream_status(tx, level, &status);
            }
            Ok(StreamChunk::Reasoning(text)) => {
                reasoning_chars += text.chars().count();
                reasoning_text.push_str(&text);
                let status = format!("{status_label} (thinking {reasoning_chars} chars)");
                send_stream_status(tx, level, &status);
            }
            Err(e) => {
                log_debug_event(
                    "stream",
                    &format!("Streaming chunk failed ({e}), falling back to non-streaming prompt"),
                    None,
                );
                return prompt_fallback(client, prompt, system, max_tokens).await;
            }
        }
    }
    if raw.trim().is_empty() && !reasoning_text.trim().is_empty() {
        log_debug_event(
            "stream",
            &format!(
                "{status_label} stream returned no content chunks; using reasoning text as fallback ({reasoning_chars} chars)"
            ),
            None,
        );
        return Ok(LlmResponse {
            raw: reasoning_text,
            content_chars: reasoning_chars,
            reasoning_chars,
        });
    }
    Ok(LlmResponse {
        raw,
        content_chars,
        reasoning_chars,
    })
}
