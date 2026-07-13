use lancedb::connect;

use crate::db::curriculum::{CurriculumTable, TABLE_NAME};
use crate::db::history::HistoryTable;
use crate::db::progress::ProgressTable;
use crate::db::reviews::ReviewsTable;
use crate::error::Result;

pub mod curriculum;
pub mod history;
pub mod metadata;
pub mod progress;
pub mod reviews;
pub mod util;

#[derive(Clone)]
pub struct Database {
    curriculum: CurriculumTable,
    progress: ProgressTable,
    history: HistoryTable,
    reviews: ReviewsTable,
}

impl Database {
    pub async fn connect(path: &std::path::Path) -> Result<Self> {
        let uri = path.to_string_lossy().to_string();
        let connection = connect(&uri).execute().await?;
        let curriculum = CurriculumTable::open(&connection).await?;
        let progress = ProgressTable::open(&connection).await?;
        let history = HistoryTable::open(&connection).await?;
        let reviews = ReviewsTable::open(&connection).await?;
        Ok(Self {
            curriculum,
            progress,
            history,
            reviews,
        })
    }

    pub async fn recreate_curriculum_table(path: &std::path::Path) -> Result<()> {
        let uri = path.to_string_lossy().to_string();
        let connection = connect(&uri).execute().await?;
        let _ = connection.drop_table(TABLE_NAME, &[]).await;
        Ok(())
    }

    pub fn curriculum(&self) -> CurriculumTable {
        self.curriculum.clone()
    }

    pub fn progress(&self) -> ProgressTable {
        self.progress.clone()
    }

    pub fn history(&self) -> HistoryTable {
        self.history.clone()
    }

    pub fn reviews(&self) -> ReviewsTable {
        self.reviews.clone()
    }
}
