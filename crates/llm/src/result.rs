//! Result messages sent from background LLM tasks back to the app event loop.

use open_course_core::curriculum::{Curriculum, Topic};
use open_course_core::error::Result;
use open_course_core::session::{AnalysisResult, GeneratedSession};

use crate::diagnostics::CheckResult;
use crate::model_listing::ModelInfo;

#[derive(Debug)]
pub enum LlmResult {
    Exercises(Result<GeneratedSession>),
    Analysis(Result<AnalysisResult>),
    Curriculum(Result<Curriculum>),
    CurriculumExtension(Result<Vec<Topic>>),
    TopicReview(Result<String>),
    Models(Result<Vec<ModelInfo>>),
    OnboardingModels(Result<Vec<ModelInfo>>),
    SimpleText(Result<String>),
    StreamChunk(String),
    CurriculumStreamChunk { level: String, status: String },
    DiagnosticUpdate(CheckResult),
    DiagnosticsDone,
    UpdateCheck(Option<String>),
}
