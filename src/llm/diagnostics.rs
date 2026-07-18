use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::time::{Instant, timeout};

use crate::config::profile::UserProfile;
use crate::core::session::{AnalysisResult, Exercise, GrammarErrorType};
use crate::db::curriculum::Topic;
use crate::error::Result;
use crate::llm::client::{LlmClient, DEFAULT_MAX_TOKENS};
use crate::llm::pipeline::{
    generate_analysis, generate_exercises, generate_topic_review, is_valid_topic_review,
};
use crate::llm::prompts::{
    build_batch_analysis_prompt, build_exercise_prompt, build_topic_review_prompt,
};
use crate::llm::streaming::{LlmStream, StreamChunk};

#[derive(Debug, Clone, Default)]
pub struct StreamMetrics {
    content_chars: Arc<AtomicUsize>,
    reasoning_chars: Arc<AtomicUsize>,
}

impl StreamMetrics {
    fn add_content(&self, n: usize) {
        self.content_chars.fetch_add(n, Ordering::Relaxed);
    }

    fn add_reasoning(&self, n: usize) {
        self.reasoning_chars.fetch_add(n, Ordering::Relaxed);
    }

    pub fn reasoning_ratio(&self) -> Option<f32> {
        let content = self.content_chars.load(Ordering::Relaxed);
        let reasoning = self.reasoning_chars.load(Ordering::Relaxed);
        let total = content.saturating_add(reasoning);
        if total == 0 {
            None
        } else {
            Some(reasoning as f32 / total as f32)
        }
    }
}

pub struct DiagnosticLlmClient {
    inner: Arc<dyn LlmClient>,
    metrics: StreamMetrics,
}

impl DiagnosticLlmClient {
    pub fn new(inner: Box<dyn LlmClient>) -> Self {
        Self::from_arc(Arc::from(inner))
    }

    pub fn from_arc(inner: Arc<dyn LlmClient>) -> Self {
        Self {
            inner,
            metrics: StreamMetrics::default(),
        }
    }

    pub fn metrics(&self) -> &StreamMetrics {
        &self.metrics
    }

    pub fn inner(&self) -> &dyn LlmClient {
        &*self.inner
    }
}

#[async_trait]
impl LlmClient for DiagnosticLlmClient {
    async fn prompt(
        &self,
        prompt: &str,
        system: Option<&str>,
        max_tokens: u32,
    ) -> Result<String> {
        let text = self.inner.prompt(prompt, system, max_tokens).await?;
        self.metrics.add_content(text.chars().count());
        Ok(text)
    }

    async fn stream_prompt(
        &self,
        prompt: &str,
        system: Option<&str>,
        max_tokens: u32,
    ) -> Result<LlmStream> {
        let stream = self.inner.stream_prompt(prompt, system, max_tokens).await?;
        let metrics = self.metrics.clone();
        let wrapped = stream.map(move |chunk| match chunk {
            Ok(StreamChunk::Content(text)) => {
                metrics.add_content(text.chars().count());
                Ok(StreamChunk::Content(text))
            }
            Ok(StreamChunk::Reasoning(text)) => {
                metrics.add_reasoning(text.chars().count());
                Ok(StreamChunk::Reasoning(text))
            }
            Err(e) => Err(e),
        });
        Ok(Box::pin(wrapped))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[derive(Debug, Clone)]
pub enum CheckStatus {
    Pending,
    InProgress,
    Passed,
    Failed(String),
    Warning(String),
}

impl CheckStatus {
    pub fn is_failed(&self) -> bool {
        matches!(self, CheckStatus::Failed(_))
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            CheckStatus::Pending | CheckStatus::InProgress => None,
            CheckStatus::Passed => None,
            CheckStatus::Failed(m) => Some(m.as_str()),
            CheckStatus::Warning(m) => Some(m.as_str()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub id: &'static str,
    pub label: String,
    pub status: CheckStatus,
    pub duration_ms: u128,
    pub reasoning_ratio: Option<f32>,
}

impl CheckResult {
    pub fn verdict(&self, spinner_symbol: Option<&str>) -> String {
        match self.status {
            CheckStatus::Pending => " ".to_string(),
            CheckStatus::InProgress => spinner_symbol.unwrap_or("◌").to_string(),
            CheckStatus::Passed => "✓".to_string(),
            CheckStatus::Failed(_) => "✗".to_string(),
            CheckStatus::Warning(_) => "⚠".to_string(),
        }
    }
}

pub fn model_check_verdict(results: &[CheckResult]) -> (bool, bool) {
    let has_failed = results.iter().any(|r| r.status.is_failed());
    let has_warning = results
        .iter()
        .any(|r| matches!(r.status, CheckStatus::Warning(_)));
    (has_failed, has_warning)
}

const DIAGNOSTIC_CHECKS: &[(&'static str, &'static str)] = &[
    ("connectivity", "Connectivity"),
    ("streaming", "Streaming"),
    ("exercises", "Exercise generation"),
    ("analysis", "Answer analysis"),
    ("topic_review", "Topic review"),
];

pub async fn run_model_diagnostics<F>(
    client: Box<dyn LlmClient>,
    profile: &UserProfile,
    mut on_update: F,
) -> Vec<CheckResult>
where
    F: FnMut(CheckResult),
{
    let client: Arc<dyn LlmClient> = Arc::from(client);

    // Send all checks as pending so the UI can render them immediately.
    for (id, label) in DIAGNOSTIC_CHECKS.iter() {
        on_update(CheckResult {
            id,
            label: label.to_string(),
            status: CheckStatus::Pending,
            duration_ms: 0,
            reasoning_ratio: None,
        });
    }

    let mut results = Vec::with_capacity(DIAGNOSTIC_CHECKS.len());

    for (id, label) in DIAGNOSTIC_CHECKS.iter() {
        on_update(CheckResult {
            id,
            label: label.to_string(),
            status: CheckStatus::InProgress,
            duration_ms: 0,
            reasoning_ratio: None,
        });

        let check = match *id {
            "connectivity" => run_connectivity_check(client.clone()).await,
            "streaming" => run_streaming_check(client.clone()).await,
            "exercises" => run_exercises_check(client.clone(), profile).await,
            "analysis" => run_analysis_check(client.clone(), profile).await,
            "topic_review" => run_topic_review_check(client.clone(), profile).await,
            _ => unreachable!(),
        };

        on_update(check.clone());
        results.push(check);
    }

    results
}

const CONNECTIVITY_TIMEOUT: Duration = Duration::from_secs(15);
const STREAMING_TIMEOUT: Duration = Duration::from_secs(15);
const EXERCISES_TIMEOUT: Duration = Duration::from_secs(90);
const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(90);
const TOPIC_REVIEW_TIMEOUT: Duration = Duration::from_secs(90);

async fn run_connectivity_check(client: Arc<dyn LlmClient>) -> CheckResult {
    let wrapper = DiagnosticLlmClient::from_arc(client);
    let start = Instant::now();
    let status = match timeout(
        CONNECTIVITY_TIMEOUT,
        wrapper.prompt("Reply with exactly: OK", None, DEFAULT_MAX_TOKENS),
    )
    .await
    {
        Ok(Ok(text)) => {
            if text.trim() == "OK" {
                CheckStatus::Passed
            } else {
                CheckStatus::Failed(format!("unexpected reply: {text}"))
            }
        }
        Ok(Err(e)) => CheckStatus::Failed(e.to_string()),
        Err(_) => CheckStatus::Failed("timeout".to_string()),
    };
    CheckResult {
        id: "connectivity",
        label: "Connectivity".to_string(),
        status,
        duration_ms: start.elapsed().as_millis(),
        reasoning_ratio: None,
    }
}

async fn run_streaming_check(client: Arc<dyn LlmClient>) -> CheckResult {
    let wrapper = DiagnosticLlmClient::from_arc(client);
    let start = Instant::now();
    let status = match timeout(
        STREAMING_TIMEOUT,
        wrapper.stream_prompt("Reply with exactly: STREAM_OK", None, DEFAULT_MAX_TOKENS),
    )
    .await
    {
        Ok(Ok(mut stream)) => {
            let mut saw_content = false;
            loop {
                match timeout(Duration::from_secs(10), stream.next()).await {
                    Ok(Some(Ok(StreamChunk::Content(_)))) => saw_content = true,
                    Ok(Some(Ok(_))) => {}
                    Ok(Some(Err(e))) => break CheckStatus::Failed(format!("stream error: {e}")),
                    Ok(None) => {
                        break if saw_content {
                            CheckStatus::Passed
                        } else {
                            CheckStatus::Failed("no content chunk received".to_string())
                        };
                    }
                    Err(_) => break CheckStatus::Failed("stream idle timeout".to_string()),
                }
            }
        }
        Ok(Err(e)) => CheckStatus::Failed(e.to_string()),
        Err(_) => CheckStatus::Failed("timeout".to_string()),
    };
    CheckResult {
        id: "streaming",
        label: "Streaming".to_string(),
        status,
        duration_ms: start.elapsed().as_millis(),
        reasoning_ratio: None,
    }
}

async fn run_exercises_check(client: Arc<dyn LlmClient>, profile: &UserProfile) -> CheckResult {
    let wrapper = DiagnosticLlmClient::from_arc(client);
    let start = Instant::now();
    let topics = synthetic_topics(profile);
    let prompt = build_exercise_prompt(profile, &topics[..1], &topics[1..], &topics, &[], 1, 0.75);
    let status = match timeout(
        EXERCISES_TIMEOUT,
        generate_exercises(&wrapper, &prompt, None, None::<&Path>),
    )
    .await
    {
        Ok(Ok(exercises)) => {
            if exercises.len() >= 1 {
                CheckStatus::Passed
            } else {
                CheckStatus::Failed(format!("expected 1 exercise, got {}", exercises.len()))
            }
        }
        Ok(Err(e)) => CheckStatus::Failed(e.to_string()),
        Err(_) => CheckStatus::Failed("timeout".to_string()),
    };
    let (status, ratio) = apply_reasoning_warning(status, wrapper.metrics().reasoning_ratio());
    CheckResult {
        id: "exercises",
        label: "Exercise generation".to_string(),
        status,
        duration_ms: start.elapsed().as_millis(),
        reasoning_ratio: ratio,
    }
}

async fn run_analysis_check(client: Arc<dyn LlmClient>, profile: &UserProfile) -> CheckResult {
    let wrapper = DiagnosticLlmClient::from_arc(client);
    let start = Instant::now();
    let topics = synthetic_topics(profile);
    let exercise = synthetic_exercise(profile);
    let answer = "Mi amigo trabaja en cafe".to_string();
    let prompt = build_batch_analysis_prompt(profile, &[(exercise, answer)], &topics);
    let status = match timeout(
        ANALYSIS_TIMEOUT,
        generate_analysis(&wrapper,
            &prompt,
            1,
            None,
            None::<&Path>,
        ),
    )
    .await
    {
        Ok(Ok(analysis)) => validate_analysis(analysis),
        Ok(Err(e)) => CheckStatus::Failed(e.to_string()),
        Err(_) => CheckStatus::Failed("timeout".to_string()),
    };
    let (status, ratio) = apply_reasoning_warning(status, wrapper.metrics().reasoning_ratio());
    CheckResult {
        id: "analysis",
        label: "Answer analysis".to_string(),
        status,
        duration_ms: start.elapsed().as_millis(),
        reasoning_ratio: ratio,
    }
}

async fn run_topic_review_check(client: Arc<dyn LlmClient>, profile: &UserProfile) -> CheckResult {
    let wrapper = DiagnosticLlmClient::from_arc(client);
    let start = Instant::now();
    let topic = synthetic_topic_review(profile);
    let prompt = build_topic_review_prompt(profile, &topic);
    let status = match timeout(
        TOPIC_REVIEW_TIMEOUT,
        generate_topic_review(&wrapper, &prompt, None, None::<&Path>),
    )
    .await
    {
        Ok(Ok(text)) => {
            if is_valid_topic_review(&text) {
                CheckStatus::Passed
            } else {
                CheckStatus::Failed("output doesn't look like a topic review".to_string())
            }
        }
        Ok(Err(e)) => CheckStatus::Failed(e.to_string()),
        Err(_) => CheckStatus::Failed("timeout".to_string()),
    };
    CheckResult {
        id: "topic_review",
        label: "Topic review".to_string(),
        status,
        duration_ms: start.elapsed().as_millis(),
        reasoning_ratio: None,
    }
}

fn validate_analysis(analysis: AnalysisResult) -> CheckStatus {
    if analysis.sentences.is_empty() {
        return CheckStatus::Failed("no sentences in analysis".to_string());
    }
    let has_errors = analysis.sentences.iter().any(|s| {
        s.errors.iter().any(|e| {
            matches!(
                e.error_type,
                GrammarErrorType::Critical | GrammarErrorType::Major | GrammarErrorType::Minor
            )
        })
    });
    if has_errors {
        CheckStatus::Passed
    } else {
        CheckStatus::Failed("no errors reported for intentionally wrong answer".to_string())
    }
}

fn apply_reasoning_warning(status: CheckStatus, ratio: Option<f32>) -> (CheckStatus, Option<f32>) {
    if let CheckStatus::Passed = status
        && let Some(r) = ratio
        && r > 0.85
    {
        return (
            CheckStatus::Warning(format!("{:.0}% reasoning tokens", r * 100.0)),
            ratio,
        );
    }
    (status, ratio)
}

fn synthetic_topics(profile: &UserProfile) -> Vec<Topic> {
    vec![
        Topic {
            id: "diag-coffee".to_string(),
            name: "Coffee vocabulary".to_string(),
            description: "Words related to coffee and drinks".to_string(),
            difficulty: "beginner".to_string(),
            level: Some("A1".to_string()),
            order: Some(1),
            tags: vec![],
            target_lang: profile.target_language.clone(),
            native_lang: profile.native_language.clone(),
            version: 1,
        },
        Topic {
            id: "diag-present".to_string(),
            name: "Present tense".to_string(),
            description: "Present tense verbs".to_string(),
            difficulty: "beginner".to_string(),
            level: Some("A1".to_string()),
            order: Some(2),
            tags: vec![],
            target_lang: profile.target_language.clone(),
            native_lang: profile.native_language.clone(),
            version: 1,
        },
    ]
}

fn synthetic_exercise(_profile: &UserProfile) -> Exercise {
    Exercise {
        id: "diag-ex1".to_string(),
        target_sentence: "My friend works in a cafe".to_string(),
        expected_translation: "Mi amigo trabaja en un café".to_string(),
        acceptable_translations: vec![
            "Mi amigo labora en una cafetería".to_string(),
        ],
        target_topic_ids: vec!["diag-coffee".to_string()],
        side_topic_ids: vec!["diag-present".to_string()],
        expected_patterns: vec!["present tense".to_string()],
        hint: None,
    }
}

fn synthetic_topic_review(profile: &UserProfile) -> Topic {
    Topic {
        id: "diag-coffee".to_string(),
        name: "Coffee vocabulary".to_string(),
        description: "Words and phrases related to ordering coffee and drinks".to_string(),
        difficulty: "beginner".to_string(),
        level: Some("A1".to_string()),
        order: Some(1),
        tags: vec![],
        target_lang: profile.target_language.clone(),
        native_lang: profile.native_language.clone(),
        version: 1,
    }
}
