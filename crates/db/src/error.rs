//! Storage-layer error: converts LanceDB/Arrow failures into the shared
//! `AppError` without `open-course-core` depending on them.

use open_course_core::error::AppError;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("Database error: {0}")]
    Lance(#[from] lancedb::Error),

    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
}

impl From<DbError> for AppError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::Lance(e) => AppError::Db(e.to_string()),
            DbError::Arrow(e) => AppError::Arrow(e.to_string()),
        }
    }
}
