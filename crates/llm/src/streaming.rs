use std::pin::Pin;

use futures_util::{Stream, StreamExt};
use rig::completion::CompletionError;
use rig::streaming::{StreamedAssistantContent, StreamingCompletionResponse};

use open_course_core::error::Result;

#[derive(Debug, Clone)]
pub enum StreamChunk {
    Content(String),
    Reasoning(String),
}

pub type LlmStream = Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>;

/// Adapt a rig streaming completion into our chunk stream.
///
/// Dropping the returned stream drops rig's abort handle along with it,
/// which aborts the underlying HTTP request.
pub fn into_llm_stream(response: StreamingCompletionResponse) -> LlmStream {
    Box::pin(response.filter_map(|item| async move { map_stream_item(item) }))
}

/// Map one rig stream item to our chunk type. Text deltas and reasoning
/// deltas are forwarded; everything else is dropped: completed reasoning
/// blocks (they restate deltas already forwarded), tool calls (unused on
/// our streaming paths), terminal records, and unknown provider items.
fn map_stream_item(
    item: std::result::Result<StreamedAssistantContent, CompletionError>,
) -> Option<Result<StreamChunk>> {
    match item {
        Ok(StreamedAssistantContent::Text(text)) => Some(Ok(StreamChunk::Content(text.text))),
        Ok(StreamedAssistantContent::ReasoningDelta { reasoning, .. }) => {
            Some(Ok(StreamChunk::Reasoning(reasoning)))
        }
        Ok(_) => None,
        Err(e) => Some(Err(crate::client::classify_llm_error(e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_course_core::error::AppError;
    use rig::streaming::UnknownPayload;

    #[test]
    fn text_delta_maps_to_content() {
        let item = Ok(StreamedAssistantContent::text("hello"));
        match map_stream_item(item) {
            Some(Ok(StreamChunk::Content(text))) => assert_eq!(text, "hello"),
            other => panic!("expected content chunk, got {other:?}"),
        }
    }

    #[test]
    fn reasoning_delta_maps_to_reasoning() {
        let item = Ok(StreamedAssistantContent::ReasoningDelta {
            id: "r1".to_string(),
            provider_id: None,
            reasoning: "thinking".to_string(),
        });
        match map_stream_item(item) {
            Some(Ok(StreamChunk::Reasoning(text))) => assert_eq!(text, "thinking"),
            other => panic!("expected reasoning chunk, got {other:?}"),
        }
    }

    #[test]
    fn unknown_provider_items_are_skipped() {
        let item = Ok(StreamedAssistantContent::Unknown(UnknownPayload::new(
            serde_json::json!({ "type": "web_search_call" }),
        )));
        assert!(map_stream_item(item).is_none());
    }

    #[test]
    fn stream_errors_are_classified() {
        let item = Err(CompletionError::ProviderError(
            "server_error: provider overloaded".to_string(),
        ));
        match map_stream_item(item) {
            Some(Err(AppError::ProviderUnavailable(_))) => {}
            other => panic!("expected ProviderUnavailable, got {other:?}"),
        }

        let item = Err(CompletionError::ResponseError("bad json".to_string()));
        match map_stream_item(item) {
            Some(Err(AppError::Llm(msg))) => assert!(msg.contains("bad json")),
            other => panic!("expected AppError::Llm, got {other:?}"),
        }
    }
}
