use lancedb::connect;

use crate::curriculum::{CurriculumTable, TABLE_NAME as CURRICULUM_TABLE};
use crate::history::HistoryTable;
use crate::learning_items::LearningItemsTable;
use crate::metadata::MetadataTable;
use crate::outbox::OutboxTable;
use crate::progress::ProgressTable;
use crate::reviews::ReviewsTable;
use open_course_core::error::Result;

pub mod apply;
pub mod curriculum;
pub mod error;
pub mod history;
pub mod learning_items;
pub mod metadata;
pub mod migrations;
pub mod outbox;
pub mod progress;
pub mod reviews;
pub mod util;

#[derive(Clone)]
pub struct Database {
    curriculum: CurriculumTable,
    progress: ProgressTable,
    history: HistoryTable,
    reviews: ReviewsTable,
    learning_items: LearningItemsTable,
    metadata: MetadataTable,
    outbox: OutboxTable,
}

impl Database {
    pub async fn connect(path: &std::path::Path) -> Result<Self> {
        let uri = path.to_string_lossy().to_string();
        let connection = connect(&uri)
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        let metadata = MetadataTable::open(&connection).await?;
        // Versioned registry migrations run before the tables are opened;
        // the tables' own ad-hoc migrations in `open` remain as idempotent
        // no-ops on already-migrated tables.
        migrations::migrate(&connection, &metadata).await?;
        let curriculum = CurriculumTable::open(&connection).await?;
        let progress = ProgressTable::open_with_metadata(&connection, metadata.clone()).await?;
        let history = HistoryTable::open(&connection).await?;
        let reviews = ReviewsTable::open(&connection).await?;
        let learning_items = LearningItemsTable::open(&connection).await?;
        let outbox = OutboxTable::open(&connection).await?;
        Ok(Self {
            curriculum,
            progress,
            history,
            reviews,
            learning_items,
            metadata,
            outbox,
        })
    }

    pub async fn recreate_curriculum_table(path: &std::path::Path) -> Result<()> {
        let uri = path.to_string_lossy().to_string();
        let connection = connect(&uri)
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        let _ = connection.drop_table(CURRICULUM_TABLE, &[]).await;
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

    pub fn learning_items(&self) -> LearningItemsTable {
        self.learning_items.clone()
    }

    pub fn metadata(&self) -> MetadataTable {
        self.metadata.clone()
    }

    pub fn outbox(&self) -> OutboxTable {
        self.outbox.clone()
    }
}
