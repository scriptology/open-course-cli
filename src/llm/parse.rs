use serde::{Deserialize, Serialize};
use serde_json::from_str;

use crate::core::session::{AnalysisResult, Exercise, SentenceAnalysis};
use crate::db::curriculum::Topic;
use crate::error::{AppError, Result};
use crate::llm::transport::LlmResponse;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Exercises {
    pub exercises: Vec<Exercise>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LevelCurriculum {
    pub topics: Vec<Topic>,
}

pub(crate) fn parse_exercises(
    cleaned: &str,
    content_chars: usize,
    reasoning_chars: usize,
) -> Result<Vec<Exercise>> {
    if cleaned.trim().is_empty() {
        return Err(AppError::Llm(format!(
            "empty response (content {content_chars} chars, reasoning {reasoning_chars} chars)"
        )));
    }

    if let Ok(wrapper) = from_str::<Exercises>(cleaned) {
        if wrapper.exercises.is_empty() {
            return Err(AppError::Llm("parsed JSON contains no exercises".to_string()));
        }
        return Ok(wrapper.exercises);
    }
    if let Ok(vec) = from_str::<Vec<Exercise>>(cleaned) {
        if vec.is_empty() {
            return Err(AppError::Llm("parsed JSON array is empty".to_string()));
        }
        return Ok(vec);
    }

    Err(AppError::Llm("JSON does not match expected exercise schema".to_string()))
}

pub(crate) fn exercise_parse_errors(cleaned: &str) -> String {
    let wrapper_err = from_str::<Exercises>(cleaned)
        .err()
        .map(|e| format!("as {{exercises}}: {e}"))
        .unwrap_or_default();
    let vec_err = from_str::<Vec<Exercise>>(cleaned)
        .err()
        .map(|e| format!("as array: {e}"))
        .unwrap_or_default();
    format!("{wrapper_err}; {vec_err}")
}

pub(crate) fn parse_analysis(
    cleaned: &str,
    expected_sentence_count: usize,
    content_chars: usize,
    reasoning_chars: usize,
) -> Result<AnalysisResult> {
    if cleaned.trim().is_empty() {
        return Err(AppError::Llm(format!(
            "empty response (content {content_chars} chars, reasoning {reasoning_chars} chars)"
        )));
    }

    let analysis: AnalysisResult = if let Ok(analysis) = from_str::<AnalysisResult>(cleaned) {
        analysis
    } else if let Ok(sentences) = from_str::<Vec<SentenceAnalysis>>(cleaned) {
        AnalysisResult {
            session_score: None,
            sentences,
            evaluated_topics: vec![],
            new_topics: vec![],
            new_learning_items: vec![],
        }
    } else if let Ok(value) = from_str::<serde_json::Value>(cleaned)
        && let Some(sentences_value) = value.get("sentences")
        && let Ok(sentences) =
            serde_json::from_value::<Vec<SentenceAnalysis>>(sentences_value.clone())
    {
        AnalysisResult {
            session_score: None,
            sentences,
            evaluated_topics: vec![],
            new_topics: vec![],
            new_learning_items: vec![],
        }
    } else {
        return Err(AppError::Llm(
            "JSON does not match expected analysis schema".to_string(),
        ));
    };

    validate_analysis_sentences(analysis, expected_sentence_count, content_chars, reasoning_chars)
}

pub(crate) fn validate_analysis_sentences(
    mut analysis: AnalysisResult,
    expected_sentence_count: usize,
    content_chars: usize,
    reasoning_chars: usize,
) -> Result<AnalysisResult> {
    if analysis.sentences.is_empty() {
        return Err(AppError::Llm(format!(
            "analysis has no sentences (content {content_chars} chars, reasoning {reasoning_chars} chars)"
        )));
    }
    if analysis.sentences.len() != expected_sentence_count {
        return Err(AppError::Llm(format!(
            "expected {expected_sentence_count} sentences, got {actual}",
            actual = analysis.sentences.len()
        )));
    }
    // Fill missing sentence numbers if the model skipped them.
    for (i, sentence) in analysis.sentences.iter_mut().enumerate() {
        if sentence.sentence_number <= 0 {
            sentence.sentence_number = (i + 1) as i32;
        }
    }
    Ok(analysis)
}

pub(crate) fn analysis_parse_errors(cleaned: &str, expected_sentence_count: usize) -> String {
    let top_err = from_str::<AnalysisResult>(cleaned)
        .err()
        .map(|e| format!("top-level: {e}"))
        .unwrap_or_default();
    let arr_err = from_str::<Vec<SentenceAnalysis>>(cleaned)
        .err()
        .map(|e| format!("as array: {e}"))
        .unwrap_or_default();
    format!(
        "expected {expected_sentence_count} sentences; top-level: {top_err}; array: {arr_err}"
    )
}

pub(crate) fn parse_curriculum_level(
    cleaned: &str,
    level: &str,
    content_chars: usize,
    reasoning_chars: usize,
) -> Result<Vec<Topic>> {
    if cleaned.trim().is_empty() {
        return Err(AppError::Llm(format!(
            "empty response (content {content_chars} chars, reasoning {reasoning_chars} chars)"
        )));
    }

    let level_curriculum: LevelCurriculum = match from_str::<LevelCurriculum>(cleaned) {
        Ok(v) => v,
        Err(parse_err) => {
            let repaired = sanitize_curriculum_ids(cleaned);
            from_str::<LevelCurriculum>(&repaired).map_err(|_| {
                AppError::Llm(format!(
                    "Failed to parse {level} curriculum response: {parse_err}"
                ))
            })?
        }
    };

    if level_curriculum.topics.is_empty() {
        return Err(AppError::Llm(format!(
            "Level {level} curriculum returned no topics"
        )));
    }

    Ok(level_curriculum.topics)
}

pub(crate) fn curriculum_parse_errors(cleaned: &str, level: &str) -> String {
    from_str::<LevelCurriculum>(cleaned)
        .err()
        .map(|e| format!("{level} curriculum parse: {e}"))
        .unwrap_or_default()
}

pub(crate) fn build_parse_error(
    kind: &str,
    response: &LlmResponse,
    cleaned: &str,
    parse_errors: &str,
    dump_path: Option<&str>,
) -> AppError {
    let raw_preview = if response.raw.len() > 500 {
        format!(
            "{}...[truncated, total {} chars]",
            &response.raw[..500],
            response.raw.len()
        )
    } else {
        response.raw.clone()
    };
    let cleaned_preview = if cleaned.len() > 500 {
        format!("{}...[truncated, total {} chars]", &cleaned[..500], cleaned.len())
    } else {
        cleaned.to_string()
    };
    let dump_hint = dump_path
        .map(|p| format!("\nFull dump written to: {p}"))
        .unwrap_or_default();
    AppError::Llm(format!(
        "Failed to generate {kind} after all retries.\nRaw ({raw_len} chars, content {content} chars, reasoning {reasoning} chars): {raw_preview}\nCleaned: {cleaned_preview}\nParse errors: {parse_errors}{dump_hint}",
        raw_len = response.raw.len(),
        content = response.content_chars,
        reasoning = response.reasoning_chars,
    ))
}

pub(crate) fn clean_json_response(raw: &str) -> String {
    // Replace raw newlines with spaces so models that emit multi-line strings
    // without escaping do not break JSON parsing.
    let trimmed = raw.trim().replace('\r', "").replace('\n', " ");
    let start = [trimmed.find('{'), trimmed.find('[')]
        .into_iter()
        .flatten()
        .min();
    if let Some(start) = start {
        let bytes = trimmed.as_bytes();
        let open = bytes[start];
        let close = if open == b'{' { b'}' } else { b']' };
        let mut depth = 1;
        let mut in_string = false;
        let mut escape = false;
        for i in (start + 1)..bytes.len() {
            let c = bytes[i];
            if in_string {
                if escape {
                    escape = false;
                } else if c == b'\\' {
                    escape = true;
                } else if c == b'"' {
                    in_string = false;
                }
            } else {
                if c == b'"' {
                    in_string = true;
                } else if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        return trimmed[start..=i].to_string();
                    }
                }
            }
        }
    }
    trimmed.to_string()
}

static CURRICULUM_ID_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r#""id"\s*:\s*"([^"]*)""#).unwrap());

/// Repair malformed topic ids inside a curriculum JSON string.
/// Some models return ids containing brackets, semicolons, etc., which break
/// JSON parsing. This replaces every id value with a kebab-case string
/// containing only lowercase letters, digits, and hyphens.
pub(crate) fn sanitize_curriculum_ids(raw: &str) -> String {
    CURRICULUM_ID_RE
        .replace_all(raw, |caps: &regex::Captures| {
            let value = &caps[1];
            let sanitized: String = value
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
            format!(r#""id": "{}""#, sanitized)
        })
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::session::SemanticVerdict;

    // --- clean_json_response ---

    #[test]
    fn clean_json_unwraps_markdown_fence() {
        let raw = "```json\n{\"a\": 1}\n```";
        assert_eq!(clean_json_response(raw), "{\"a\": 1}");
    }

    #[test]
    fn clean_json_keeps_bare_json() {
        let raw = "{\"a\": 1}";
        assert_eq!(clean_json_response(raw), "{\"a\": 1}");
    }

    #[test]
    fn clean_json_extracts_bare_array() {
        let raw = "[1, {\"a\": 2}]";
        assert_eq!(clean_json_response(raw), "[1, {\"a\": 2}]");
    }

    #[test]
    fn clean_json_handles_nested_braces_and_trailing_garbage() {
        let raw = "{\"a\": {\"b\": [1, {\"c\": 2}]}} trailing text";
        assert_eq!(clean_json_response(raw), "{\"a\": {\"b\": [1, {\"c\": 2}]}}");
    }

    #[test]
    fn clean_json_ignores_braces_inside_strings() {
        let raw = "{\"text\": \"use {curly} braces\"} extra";
        assert_eq!(clean_json_response(raw), "{\"text\": \"use {curly} braces\"}");
    }

    #[test]
    fn clean_json_handles_escaped_quotes_in_strings() {
        let raw = "{\"text\": \"he said \\\"hi\\\"\"} extra";
        assert_eq!(clean_json_response(raw), "{\"text\": \"he said \\\"hi\\\"\"}");
    }

    #[test]
    fn clean_json_replaces_raw_newlines_with_spaces() {
        let raw = "{\"text\":\n\"multi\nline\"}";
        assert_eq!(clean_json_response(raw), "{\"text\": \"multi line\"}");
    }

    #[test]
    fn clean_json_returns_trimmed_input_for_unclosed_json() {
        let raw = "  {\"a\": 1  ";
        assert_eq!(clean_json_response(raw), "{\"a\": 1");
    }

    // --- sanitize_curriculum_ids ---

    #[test]
    fn sanitize_ids_lowercases_and_replaces_special_chars() {
        let raw = r#"{"id": "My Topic! (B1)", "name": "x"}"#;
        assert_eq!(
            sanitize_curriculum_ids(raw),
            r#"{"id": "my-topic---b1-", "name": "x"}"#
        );
    }

    #[test]
    fn sanitize_ids_replaces_spaces_and_slashes() {
        let raw = r#"{"id": "Hello World/a_b"}"#;
        assert_eq!(sanitize_curriculum_ids(raw), r#"{"id": "hello-world-a-b"}"#);
    }

    #[test]
    fn sanitize_ids_keeps_valid_ids_untouched() {
        let raw = r#"{"id": "already-valid-123"}"#;
        assert_eq!(sanitize_curriculum_ids(raw), r#"{"id": "already-valid-123"}"#);
    }

    #[test]
    fn sanitize_ids_handles_empty_id() {
        let raw = r#"{"id": ""}"#;
        assert_eq!(sanitize_curriculum_ids(raw), r#"{"id": ""}"#);
    }

    #[test]
    fn sanitize_ids_fixes_every_id_in_document() {
        let raw = r#"[{"id": "A B"}, {"id": "C D"}]"#;
        assert_eq!(sanitize_curriculum_ids(raw), r#"[{"id": "a-b"}, {"id": "c-d"}]"#);
    }

    // --- parse_exercises ---

    fn exercise_json(id: &str) -> String {
        format!(
            r#"{{"id": "{id}", "targetSentence": "Hola", "expectedTranslation": "Hello", "targetTopicIds": ["t1"], "sideTopicIds": []}}"#
        )
    }

    #[test]
    fn parse_exercises_accepts_wrapper_object() {
        let cleaned = format!(r#"{{"exercises": [{}]}}"#, exercise_json("ex1"));
        let result = parse_exercises(&cleaned, 0, 0).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "ex1");
    }

    #[test]
    fn parse_exercises_accepts_bare_array() {
        let cleaned = format!("[{}]", exercise_json("ex1"));
        let result = parse_exercises(&cleaned, 0, 0).unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn parse_exercises_rejects_sentences_object() {
        let cleaned = r#"{"sentences": []}"#;
        let err = parse_exercises(cleaned, 0, 0).unwrap_err();
        assert_eq!(
            err.to_string(),
            "LLM error: JSON does not match expected exercise schema"
        );
    }

    #[test]
    fn parse_exercises_empty_response_error_text() {
        let err = parse_exercises("", 3, 7).unwrap_err();
        assert_eq!(
            err.to_string(),
            "LLM error: empty response (content 3 chars, reasoning 7 chars)"
        );
    }

    #[test]
    fn parse_exercises_rejects_empty_wrapper() {
        let err = parse_exercises(r#"{"exercises": []}"#, 0, 0).unwrap_err();
        assert_eq!(err.to_string(), "LLM error: parsed JSON contains no exercises");
    }

    #[test]
    fn parse_exercises_rejects_empty_array() {
        let err = parse_exercises("[]", 0, 0).unwrap_err();
        assert_eq!(err.to_string(), "LLM error: parsed JSON array is empty");
    }

    // --- parse_analysis ---

    fn sentence_json(number: i32) -> String {
        format!(
            r#"{{"sentenceNumber": {number}, "errors": [], "perSentenceFeedback": []}}"#
        )
    }

    #[test]
    fn parse_analysis_accepts_full_wrapper_object() {
        let cleaned = format!(
            r#"{{"sessionScore": 0.9, "sentences": [{}], "evaluatedTopics": [], "newTopics": []}}"#,
            sentence_json(1)
        );
        let result = parse_analysis(&cleaned, 1, 0, 0).unwrap();
        assert_eq!(result.sentences.len(), 1);
        assert_eq!(result.session_score, Some(0.9));
    }

    #[test]
    fn parse_analysis_accepts_sentences_only_object() {
        let cleaned = format!(r#"{{"sentences": [{}]}}"#, sentence_json(1));
        let result = parse_analysis(&cleaned, 1, 0, 0).unwrap();
        assert_eq!(result.sentences.len(), 1);
        assert_eq!(result.session_score, None);
    }

    #[test]
    fn parse_analysis_accepts_bare_sentence_array() {
        let cleaned = format!("[{}]", sentence_json(1));
        let result = parse_analysis(&cleaned, 1, 0, 0).unwrap();
        assert_eq!(result.sentences.len(), 1);
        assert_eq!(result.session_score, None);
        assert!(result.evaluated_topics.is_empty());
    }

    #[test]
    fn parse_analysis_empty_response_error_text() {
        let err = parse_analysis("   ", 2, 10, 20).unwrap_err();
        assert_eq!(
            err.to_string(),
            "LLM error: empty response (content 10 chars, reasoning 20 chars)"
        );
    }

    #[test]
    fn parse_analysis_rejects_unrelated_json() {
        let err = parse_analysis("42", 1, 0, 0).unwrap_err();
        assert_eq!(
            err.to_string(),
            "LLM error: JSON does not match expected analysis schema"
        );
    }

    // --- validate_analysis_sentences ---

    fn sentence(number: i32) -> SentenceAnalysis {
        SentenceAnalysis {
            sentence_number: number,
            student_translation: String::new(),
            expected_translation: String::new(),
            acceptable_translations: vec![],
            semantic_verdict: SemanticVerdict::Correct,
            errors: vec![],
            per_sentence_feedback: vec![],
        }
    }

    fn analysis_with(numbers: &[i32]) -> AnalysisResult {
        AnalysisResult {
            session_score: None,
            sentences: numbers.iter().map(|&n| sentence(n)).collect(),
            evaluated_topics: vec![],
            new_topics: vec![],
            new_learning_items: vec![],
        }
    }

    #[test]
    fn validate_renumbers_missing_sentence_numbers() {
        let analysis = analysis_with(&[0, -2, 5]);
        let result = validate_analysis_sentences(analysis, 3, 0, 0).unwrap();
        let numbers: Vec<i32> = result.sentences.iter().map(|s| s.sentence_number).collect();
        assert_eq!(numbers, vec![1, 2, 5]);
    }

    #[test]
    fn validate_rejects_sentence_count_mismatch() {
        let analysis = analysis_with(&[1]);
        let err = validate_analysis_sentences(analysis, 2, 0, 0).unwrap_err();
        assert_eq!(err.to_string(), "LLM error: expected 2 sentences, got 1");
    }

    #[test]
    fn validate_rejects_empty_sentences() {
        let analysis = analysis_with(&[]);
        let err = validate_analysis_sentences(analysis, 0, 4, 6).unwrap_err();
        assert_eq!(
            err.to_string(),
            "LLM error: analysis has no sentences (content 4 chars, reasoning 6 chars)"
        );
    }
}
