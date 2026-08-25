use serde::{Deserialize, Serialize};
use serde_json::from_str;

use crate::curriculum::Topic;
use crate::error::{AppError, Result};
use crate::llm::response::LlmResponse;
use crate::session::{AnalysisResult, Exercise, SentenceAnalysis};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Exercises {
    pub exercises: Vec<Exercise>,
    /// Warm-up cards for the session's forced vocabulary, one per forced
    /// lemma. Optional: older prompts and bare-array responses have none.
    #[serde(default)]
    pub warmup: Vec<RawWarmupItem>,
    /// Content vocabulary the model used across the generated exercises,
    /// requested independently of `warmup`/forced vocabulary so genuinely
    /// new words (not yet in the learner's vocabulary table) can be
    /// previewed too (see `vocabulary::new_word_items`). Optional: older
    /// prompts and bare-array responses have none.
    #[serde(default)]
    pub vocabulary: Vec<RawVocabularyItem>,
    /// Cloze (word-bank) items, one per word of `vocabulary`, for words
    /// without positive learning progress (see `vocabulary::cloze_items`).
    /// Optional: older prompts and bare-array responses have none.
    #[serde(default)]
    pub cloze: Vec<RawClozeItem>,
}

/// Warm-up entry as returned by the LLM, before it is matched against the
/// session's forced lemmas. Every field is optional so a partial entry never
/// breaks parsing of the whole response.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RawWarmupItem {
    #[serde(default)]
    pub lemma: String,
    #[serde(default)]
    pub pos: Option<String>,
    #[serde(default)]
    pub cefr_level: Option<String>,
    #[serde(default)]
    pub translation: Option<String>,
    #[serde(default)]
    pub example: Option<String>,
}

/// Content-vocabulary entry as returned by the LLM for the exercises it just
/// generated, before it is diffed against the learner's existing vocabulary
/// (see `vocabulary::new_word_items`). Every field is optional so a partial
/// entry never breaks parsing of the whole response.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RawVocabularyItem {
    #[serde(default)]
    pub lemma: String,
    /// The exact inflected form as it appears in the sentence — checked
    /// against the generated text instead of `lemma` (which, for an
    /// inflected verb like "como" from "comer", usually never appears
    /// verbatim), so a chatty LLM can't invent vocabulary that isn't there.
    #[serde(default)]
    pub surface: String,
    #[serde(default)]
    pub pos: Option<String>,
    #[serde(default)]
    pub cefr_level: Option<String>,
    #[serde(default)]
    pub translation: Option<String>,
}

/// Cloze (fill-in-the-blank with a word bank) entry as returned by the LLM,
/// before it is validated and matched against the learner's vocabulary (see
/// `vocabulary::cloze_items`). Every field is optional so a partial entry
/// never breaks parsing of the whole response. The sentence is returned
/// complete — the pipeline blanks the answer out itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RawClozeItem {
    #[serde(default)]
    pub lemma: String,
    #[serde(default)]
    pub sentence: String,
    #[serde(default)]
    pub answer: String,
    #[serde(default)]
    pub distractors: Vec<String>,
    #[serde(default)]
    pub translation: Option<String>,
    #[serde(default)]
    pub pos: Option<String>,
    #[serde(default)]
    pub cefr_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LevelCurriculum {
    pub topics: Vec<Topic>,
}

pub fn parse_exercises(
    cleaned: &str,
    content_chars: usize,
    reasoning_chars: usize,
) -> Result<Vec<Exercise>> {
    parse_session_exercises(cleaned, content_chars, reasoning_chars).map(|w| w.exercises)
}

/// Same as `parse_exercises`, but keeps the whole wrapper object so the
/// caller can use the optional `warmup` array alongside the exercises.
pub fn parse_session_exercises(
    cleaned: &str,
    content_chars: usize,
    reasoning_chars: usize,
) -> Result<Exercises> {
    if cleaned.trim().is_empty() {
        return Err(AppError::Llm(format!(
            "empty response (content {content_chars} chars, reasoning {reasoning_chars} chars)"
        )));
    }

    if let Ok(wrapper) = from_str::<Exercises>(cleaned) {
        if wrapper.exercises.is_empty() {
            return Err(AppError::Llm(
                "parsed JSON contains no exercises".to_string(),
            ));
        }
        return Ok(wrapper);
    }
    if let Ok(vec) = from_str::<Vec<Exercise>>(cleaned) {
        if vec.is_empty() {
            return Err(AppError::Llm("parsed JSON array is empty".to_string()));
        }
        return Ok(Exercises {
            exercises: vec,
            warmup: vec![],
            vocabulary: vec![],
            cloze: vec![],
        });
    }

    Err(AppError::Llm(
        "JSON does not match expected exercise schema".to_string(),
    ))
}

pub fn exercise_parse_errors(cleaned: &str) -> String {
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

pub fn parse_analysis(
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
            new_lemmas: vec![],
            new_forms: vec![],
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
            new_lemmas: vec![],
            new_forms: vec![],
        }
    } else {
        return Err(AppError::Llm(
            "JSON does not match expected analysis schema".to_string(),
        ));
    };

    validate_analysis_sentences(
        analysis,
        expected_sentence_count,
        content_chars,
        reasoning_chars,
    )
}

pub fn validate_analysis_sentences(
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

pub fn analysis_parse_errors(cleaned: &str, expected_sentence_count: usize) -> String {
    let top_err = from_str::<AnalysisResult>(cleaned)
        .err()
        .map(|e| format!("top-level: {e}"))
        .unwrap_or_default();
    let arr_err = from_str::<Vec<SentenceAnalysis>>(cleaned)
        .err()
        .map(|e| format!("as array: {e}"))
        .unwrap_or_default();
    format!("expected {expected_sentence_count} sentences; top-level: {top_err}; array: {arr_err}")
}

pub fn parse_curriculum_level(
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
            from_str::<LevelCurriculum>(&repaired).map_err(|retry_err| {
                AppError::Llm(format!(
                    "Failed to parse {level} curriculum response: {parse_err} (sanitized retry: {retry_err}); response excerpt ({} chars total): {}",
                    cleaned.len(),
                    error_excerpt(cleaned, &parse_err),
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

pub fn curriculum_parse_errors(cleaned: &str, level: &str) -> String {
    from_str::<LevelCurriculum>(cleaned)
        .err()
        .map(|e| format!("{level} curriculum parse: {e}"))
        .unwrap_or_default()
}

/// Bounded excerpt of `text` around the position serde_json reported in
/// `err` (1-based column; curriculum input is single-line after cleaning),
/// with an inline marker at the failure point. Char-boundary safe.
fn error_excerpt(text: &str, err: &serde_json::Error) -> String {
    const RADIUS: usize = 120;
    let mut pos = err.column().saturating_sub(1).min(text.len());
    while pos > 0 && !text.is_char_boundary(pos) {
        pos -= 1;
    }
    let start = text[..pos]
        .char_indices()
        .rev()
        .take(RADIUS)
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0);
    let end = text[pos..]
        .char_indices()
        .take(RADIUS)
        .last()
        .map(|(i, c)| pos + i + c.len_utf8())
        .unwrap_or(text.len());
    let left_ellipsis = if start > 0 { "…" } else { "" };
    let right_ellipsis = if end < text.len() { "…" } else { "" };
    format!(
        "{left_ellipsis}{} <<PARSE ERROR HERE>> {}{right_ellipsis}",
        &text[start..pos],
        &text[pos..end]
    )
}

pub fn build_parse_error(
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
        format!(
            "{}...[truncated, total {} chars]",
            &cleaned[..500],
            cleaned.len()
        )
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

pub fn clean_json_response(raw: &str) -> String {
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
pub fn sanitize_curriculum_ids(raw: &str) -> String {
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
    use crate::session::SemanticVerdict;

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
        assert_eq!(
            clean_json_response(raw),
            "{\"a\": {\"b\": [1, {\"c\": 2}]}}"
        );
    }

    #[test]
    fn clean_json_ignores_braces_inside_strings() {
        let raw = "{\"text\": \"use {curly} braces\"} extra";
        assert_eq!(
            clean_json_response(raw),
            "{\"text\": \"use {curly} braces\"}"
        );
    }

    #[test]
    fn clean_json_handles_escaped_quotes_in_strings() {
        let raw = "{\"text\": \"he said \\\"hi\\\"\"} extra";
        assert_eq!(
            clean_json_response(raw),
            "{\"text\": \"he said \\\"hi\\\"\"}"
        );
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
        assert_eq!(
            sanitize_curriculum_ids(raw),
            r#"{"id": "already-valid-123"}"#
        );
    }

    #[test]
    fn sanitize_ids_handles_empty_id() {
        let raw = r#"{"id": ""}"#;
        assert_eq!(sanitize_curriculum_ids(raw), r#"{"id": ""}"#);
    }

    #[test]
    fn sanitize_ids_fixes_every_id_in_document() {
        let raw = r#"[{"id": "A B"}, {"id": "C D"}]"#;
        assert_eq!(
            sanitize_curriculum_ids(raw),
            r#"[{"id": "a-b"}, {"id": "c-d"}]"#
        );
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
        assert_eq!(
            err.to_string(),
            "LLM error: parsed JSON contains no exercises"
        );
    }

    #[test]
    fn parse_exercises_rejects_empty_array() {
        let err = parse_exercises("[]", 0, 0).unwrap_err();
        assert_eq!(err.to_string(), "LLM error: parsed JSON array is empty");
    }

    // --- warmup parsing ---

    #[test]
    fn parse_session_exercises_defaults_warmup_when_absent() {
        let cleaned = format!(r#"{{"exercises": [{}]}}"#, exercise_json("ex1"));
        let result = parse_session_exercises(&cleaned, 0, 0).unwrap();
        assert_eq!(result.exercises.len(), 1);
        assert!(result.warmup.is_empty());
    }

    #[test]
    fn parse_session_exercises_parses_warmup() {
        let cleaned = format!(
            r#"{{"exercises": [{}], "warmup": [
                {{"lemma": "comer", "pos": "VERB", "cefrLevel": "A1", "translation": "есть", "example": "Como pan."}},
                {{"lemma": "pequeño"}}
            ]}}"#,
            exercise_json("ex1")
        );
        let result = parse_session_exercises(&cleaned, 0, 0).unwrap();
        assert_eq!(result.warmup.len(), 2);
        assert_eq!(result.warmup[0].lemma, "comer");
        assert_eq!(result.warmup[0].pos.as_deref(), Some("VERB"));
        assert_eq!(result.warmup[0].cefr_level.as_deref(), Some("A1"));
        assert_eq!(result.warmup[0].translation.as_deref(), Some("есть"));
        assert_eq!(result.warmup[0].example.as_deref(), Some("Como pan."));
        // Partial entries parse too: missing fields fall back to defaults.
        assert_eq!(result.warmup[1].lemma, "pequeño");
        assert_eq!(result.warmup[1].translation, None);
        assert_eq!(result.warmup[1].example, None);
    }

    #[test]
    fn parse_session_exercises_bare_array_has_empty_warmup() {
        let cleaned = format!("[{}]", exercise_json("ex1"));
        let result = parse_session_exercises(&cleaned, 0, 0).unwrap();
        assert_eq!(result.exercises.len(), 1);
        assert!(result.warmup.is_empty());
        assert!(result.cloze.is_empty());
    }

    // --- cloze parsing ---

    #[test]
    fn parse_session_exercises_defaults_cloze_when_absent() {
        let cleaned = format!(r#"{{"exercises": [{}]}}"#, exercise_json("ex1"));
        let result = parse_session_exercises(&cleaned, 0, 0).unwrap();
        assert_eq!(result.exercises.len(), 1);
        assert!(result.cloze.is_empty());
    }

    #[test]
    fn parse_session_exercises_parses_cloze() {
        let cleaned = format!(
            r#"{{"exercises": [{}], "cloze": [
                {{"lemma": "comer", "sentence": "Como pan cada día.", "answer": "Como", "distractors": ["Comes", "Comer"], "translation": "Я ем хлеб каждый день.", "pos": "VERB", "cefrLevel": "A1"}},
                {{"lemma": "pequeño"}}
            ]}}"#,
            exercise_json("ex1")
        );
        let result = parse_session_exercises(&cleaned, 0, 0).unwrap();
        assert_eq!(result.cloze.len(), 2);
        assert_eq!(result.cloze[0].lemma, "comer");
        assert_eq!(result.cloze[0].sentence, "Como pan cada día.");
        assert_eq!(result.cloze[0].answer, "Como");
        assert_eq!(result.cloze[0].distractors, ["Comes", "Comer"]);
        assert_eq!(
            result.cloze[0].translation.as_deref(),
            Some("Я ем хлеб каждый день.")
        );
        assert_eq!(result.cloze[0].pos.as_deref(), Some("VERB"));
        assert_eq!(result.cloze[0].cefr_level.as_deref(), Some("A1"));
        // Partial entries parse too: missing fields fall back to defaults.
        assert_eq!(result.cloze[1].lemma, "pequeño");
        assert!(result.cloze[1].sentence.is_empty());
        assert!(result.cloze[1].distractors.is_empty());
        assert_eq!(result.cloze[1].translation, None);
    }

    // --- parse_analysis ---

    fn sentence_json(number: i32) -> String {
        format!(r#"{{"sentenceNumber": {number}, "errors": [], "perSentenceFeedback": []}}"#)
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

    // --- usedVocabulary ---

    fn used_vocabulary_json() -> String {
        r#""usedVocabulary": [
            {"surface": "manzana", "lemma": "manzana", "pos": "NOUN", "feats": "Gender=Fem|Number=Sing", "side": "target", "expectedForm": true, "cefrLevel": "A1"},
            {"surface": "komo", "lemma": "comer", "pos": "VERB", "feats": "Mood=Ind|Number=Sing|Person=1|Tense=Pres", "side": "student", "spellingOk": false, "usageOk": true, "expectedForm": true}
        ]"#
        .to_string()
    }

    #[test]
    fn parse_analysis_parses_used_vocabulary() {
        let cleaned = format!(
            r#"{{"sentences": [{{"sentenceNumber": 1, "errors": [], "perSentenceFeedback": [], {}}}]}}"#,
            used_vocabulary_json()
        );
        let result = parse_analysis(&cleaned, 1, 0, 0).unwrap();
        let used = &result.sentences[0].used_vocabulary;
        assert_eq!(used.len(), 2);
        assert_eq!(used[0].side, "target");
        assert_eq!(used[0].spelling_ok, None);
        assert!(used[0].expected_form);
        assert_eq!(used[0].cefr_level.as_deref(), Some("A1"));
        assert_eq!(used[1].lemma, "comer");
        assert_eq!(used[1].spelling_ok, Some(false));
        assert_eq!(used[1].usage_ok, Some(true));
        assert_eq!(used[1].cefr_level, None);
    }

    #[test]
    fn parse_analysis_defaults_used_vocabulary_when_absent() {
        let cleaned = format!("[{}]", sentence_json(1));
        let result = parse_analysis(&cleaned, 1, 0, 0).unwrap();
        assert!(result.sentences[0].used_vocabulary.is_empty());
    }

    #[test]
    fn parse_analysis_keeps_used_vocabulary_through_value_fallback() {
        // A malformed top-level field ("sessionScore" as a string) forces the
        // `Value.get("sentences")` fallback; usedVocabulary must survive it.
        let cleaned = format!(
            r#"{{"sessionScore": "high", "sentences": [{{"sentenceNumber": 1, "errors": [], "perSentenceFeedback": [], {}}}]}}"#,
            used_vocabulary_json()
        );
        let result = parse_analysis(&cleaned, 1, 0, 0).unwrap();
        assert_eq!(result.sentences[0].used_vocabulary.len(), 2);
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
            used_vocabulary: vec![],
        }
    }

    fn analysis_with(numbers: &[i32]) -> AnalysisResult {
        AnalysisResult {
            session_score: None,
            sentences: numbers.iter().map(|&n| sentence(n)).collect(),
            evaluated_topics: vec![],
            new_topics: vec![],
            new_learning_items: vec![],
            new_lemmas: vec![],
            new_forms: vec![],
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

    // --- parse_curriculum_level / error_excerpt ---

    #[test]
    fn parse_curriculum_error_includes_excerpt_around_failure() {
        // Trailing comma makes serde_json report "key must be a string".
        let cleaned = r#"{"topics": [{"id": "My Topic!", "name": "x",}"#;
        let err = parse_curriculum_level(cleaned, "C1", cleaned.len(), 0).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Failed to parse C1 curriculum response"),
            "{msg}"
        );
        assert!(msg.contains("sanitized retry:"), "{msg}");
        assert!(msg.contains("<<PARSE ERROR HERE>>"), "{msg}");
        assert!(msg.contains(r#""name": "x","#), "{msg}");
    }

    #[test]
    fn error_excerpt_handles_multibyte_chars_and_edges() {
        // Failure right after a run of multibyte chars, near the end of input.
        let text = format!("{{\"topics\": \"{}\",}}", "п".repeat(300));
        let err = from_str::<LevelCurriculum>(&text).unwrap_err();
        let excerpt = error_excerpt(&text, &err);
        assert!(excerpt.contains("<<PARSE ERROR HERE>>"), "{excerpt}");
        assert!(excerpt.starts_with('…'), "{excerpt}");

        // Failure at the very start of a short input: no ellipses.
        let err = from_str::<LevelCurriculum>("}").unwrap_err();
        let excerpt = error_excerpt("}", &err);
        assert_eq!(excerpt, " <<PARSE ERROR HERE>> }");
    }
}
