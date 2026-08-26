use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;
use reqwest::Client;
use serde_json::json;

use open_course_core::error::{AppError, Result};

#[derive(Debug, Clone)]
pub enum StreamChunk {
    Content(String),
    Reasoning(String),
}

pub type LlmStream = Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>;

struct SseStream {
    bytes_stream: Pin<Box<dyn Stream<Item = std::result::Result<Bytes, reqwest::Error>> + Send>>,
    buffer: Vec<u8>,
    finished: bool,
    parser: fn(&str) -> Option<StreamChunk>,
}

impl Stream for SseStream {
    type Item = Result<StreamChunk>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            if let Some(frame) = extract_frame(&mut this.buffer) {
                if let Some(content) = (this.parser)(&frame) {
                    return Poll::Ready(Some(Ok(content)));
                }
                continue;
            }
            if this.finished {
                return Poll::Ready(None);
            }
            match Pin::new(&mut this.bytes_stream).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    this.finished = true;
                    continue;
                }
                Poll::Ready(Some(item)) => match item {
                    Ok(bytes) => {
                        this.buffer.extend_from_slice(&bytes);
                        continue;
                    }
                    Err(e) => return Poll::Ready(Some(Err(classify_stream_error(e)))),
                },
            }
        }
    }
}

fn extract_frame(buffer: &mut Vec<u8>) -> Option<String> {
    if let Some(pos) = buffer.windows(2).position(|w| w == b"\n\n") {
        let frame_bytes = buffer[..pos].to_vec();
        buffer.drain(..pos + 2);
        String::from_utf8(frame_bytes).ok()
    } else {
        None
    }
}

fn classify_stream_error<E: std::fmt::Display>(e: E) -> AppError {
    let msg = e.to_string();
    if msg.contains("Inference is temporarily unavailable")
        || msg.contains("failover_exhausted")
        || msg.contains("temporarily unavailable")
        || msg.contains("server_error")
    {
        AppError::ProviderUnavailable(msg)
    } else {
        AppError::Llm(msg)
    }
}

fn parse_sse_content(
    frame: &str,
    extract: impl Fn(&serde_json::Value) -> Option<StreamChunk>,
) -> Option<StreamChunk> {
    for line in frame.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            let data = data.trim();
            if data == "[DONE]" {
                return None;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(data)
                && let Some(chunk) = extract(&value)
            {
                return Some(chunk);
            }
        }
    }
    None
}

fn parse_openai_frame(frame: &str) -> Option<StreamChunk> {
    parse_sse_content(frame, |value| {
        if let Some(content) = value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("reasoning_content"))
            .and_then(|c| c.as_str())
        {
            return Some(StreamChunk::Reasoning(content.to_string()));
        }
        if let Some(content) = value
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first())
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_str())
        {
            return Some(StreamChunk::Content(content.to_string()));
        }
        None
    })
}

fn parse_anthropic_frame(frame: &str) -> Option<StreamChunk> {
    parse_sse_content(frame, |value| {
        if let Some(text) = value
            .get("delta")
            .and_then(|d| d.get("thinking"))
            .and_then(|t| t.as_str())
        {
            return Some(StreamChunk::Reasoning(text.to_string()));
        }
        if let Some(text) = value
            .get("delta")
            .and_then(|d| d.get("text"))
            .and_then(|t| t.as_str())
        {
            return Some(StreamChunk::Content(text.to_string()));
        }
        None
    })
}

fn build_openai_request_body(
    model: &str,
    system: Option<&str>,
    prompt: &str,
    reasoning_effort: Option<&str>,
    enable_thinking: Option<bool>,
    max_tokens: u32,
    openai_native: bool,
) -> serde_json::Value {
    let mut messages = Vec::new();
    if let Some(system) = system {
        messages.push(json!({ "role": "system", "content": system }));
    }
    messages.push(json!({ "role": "user", "content": prompt }));
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
    });
    // OpenAI's gpt-5/o-series models reject `max_tokens` and require
    // `max_completion_tokens`; OpenAI-compatible gateways expect
    // `max_tokens`.
    if openai_native {
        body["max_completion_tokens"] = json!(max_tokens);
    } else {
        body["max_tokens"] = json!(max_tokens);
    }
    if let Some(effort) = reasoning_effort {
        body["reasoning_effort"] = json!(effort);
    }
    // OpenAI rejects the Qwen/Aliyun `enable_thinking` extension with a 400.
    if !openai_native && let Some(enable_thinking) = enable_thinking {
        body["enable_thinking"] = json!(enable_thinking);
    }
    body
}

#[allow(clippy::too_many_arguments)]
pub async fn stream_openai_compatible(
    base_url: &str,
    api_key: &str,
    model: &str,
    system: Option<&str>,
    prompt: &str,
    reasoning_effort: Option<&str>,
    enable_thinking: Option<bool>,
    max_tokens: u32,
    openai_native: bool,
) -> Result<LlmStream> {
    let client = Client::new();
    let body = build_openai_request_body(
        model,
        system,
        prompt,
        reasoning_effort,
        enable_thinking,
        max_tokens,
        openai_native,
    );
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

    let mut request = client
        .post(&url)
        .header("Accept", "text/event-stream")
        .json(&body);
    if !api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {api_key}"));
    }

    let response = request
        .send()
        .await
        .map_err(|e| AppError::Llm(format!("OpenAI-compatible stream request failed: {e}")))?;

    if !response.status().is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(classify_stream_error(text));
    }

    Ok(Box::pin(SseStream {
        bytes_stream: Box::pin(response.bytes_stream()),
        buffer: Vec::new(),
        finished: false,
        parser: parse_openai_frame,
    }))
}

fn build_anthropic_request_body(
    model: &str,
    system: Option<&str>,
    prompt: &str,
    max_tokens: u32,
    disable_thinking: bool,
) -> serde_json::Value {
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{"role": "user", "content": prompt}],
        "stream": true,
    });
    if let Some(system) = system {
        body["system"] = json!(system);
    }
    // MiniMax-M3 keeps thinking off by default on its Anthropic-compatible
    // API; the explicit `disabled` guards against a default change. M2.x
    // models accept but ignore it (thinking stays on).
    if disable_thinking {
        body["thinking"] = json!({"type": "disabled"});
    }
    body
}

#[allow(clippy::too_many_arguments)]
pub async fn stream_anthropic_messages(
    base_url: &str,
    api_key: &str,
    model: &str,
    system: Option<&str>,
    prompt: &str,
    max_tokens: u32,
    disable_thinking: bool,
) -> Result<LlmStream> {
    let client = Client::new();
    let body = build_anthropic_request_body(model, system, prompt, max_tokens, disable_thinking);
    let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));

    let response = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Llm(format!("Anthropic stream request failed: {e}")))?;

    if !response.status().is_success() {
        let text = response.text().await.unwrap_or_default();
        return Err(classify_stream_error(text));
    }

    Ok(Box::pin(SseStream {
        bytes_stream: Box::pin(response.bytes_stream()),
        buffer: Vec::new(),
        finished: false,
        parser: parse_anthropic_frame,
    }))
}

pub fn stream_from_text(text: String) -> LlmStream {
    Box::pin(futures_util::stream::once(async move {
        Ok(StreamChunk::Content(text))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_omits_enable_thinking_when_unset() {
        let body = build_openai_request_body("m", None, "hi", None, None, 100, false);
        assert!(body.get("enable_thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn request_body_includes_enable_thinking_when_set() {
        let body =
            build_openai_request_body("m", Some("sys"), "hi", Some("low"), Some(false), 100, false);
        assert_eq!(body["enable_thinking"], json!(false));
        assert_eq!(body["reasoning_effort"], json!("low"));
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["max_tokens"], json!(100));
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn openai_native_body_uses_max_completion_tokens_and_drops_enable_thinking() {
        let body = build_openai_request_body("m", None, "hi", Some("low"), Some(false), 100, true);
        assert_eq!(body["max_completion_tokens"], json!(100));
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["reasoning_effort"], json!("low"));
        assert!(body.get("enable_thinking").is_none());
    }

    #[test]
    fn anthropic_body_omits_thinking_by_default() {
        let body = build_anthropic_request_body("m", Some("sys"), "hi", 100, false);
        assert!(body.get("thinking").is_none());
        assert_eq!(body["max_tokens"], json!(100));
        assert_eq!(body["system"], json!("sys"));
    }

    #[test]
    fn anthropic_body_disables_thinking_when_requested() {
        let body = build_anthropic_request_body("m", None, "hi", 100, true);
        assert_eq!(body["thinking"], json!({"type": "disabled"}));
        assert_eq!(body["stream"], json!(true));
    }
}
