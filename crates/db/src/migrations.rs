//! Versioned schema migrations for a pair's tables.
//!
//! The current version is stored in the metadata table under
//! `schema_version` (absent means 1). `migrate` runs every migration newer
//! than the stored version. Older ad-hoc migrations (progress drop/recreate,
//! history `new_topic_ids`) still live in the table `open` functions, run
//! after this registry, and are idempotent no-ops on migrated tables.

use std::future::Future;
use std::pin::Pin;

use arrow_array::RecordBatch;
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::query::ExecutableQuery;

use crate::metadata::MetadataTable;
use open_course_core::error::Result;

/// The schema version this build migrates to.
pub const CURRENT_SCHEMA_VERSION: i32 = 5;

type MigrationFn = fn(&Connection) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

/// Versioned migrations, applied in order when the stored schema version is
/// older than the entry's version.
const MIGRATIONS: &[(i32, MigrationFn)] = &[
    (2, |c| Box::pin(migrate_v2_timestamps(c))),
    (3, |c| Box::pin(migrate_v3_session_uuids(c))),
    (4, |c| Box::pin(migrate_v4_outbox(c))),
    (5, |c| Box::pin(migrate_v5_vocabulary(c))),
];

/// Brings the database at `connection` up to `CURRENT_SCHEMA_VERSION`.
pub async fn migrate(connection: &Connection, metadata: &MetadataTable) -> Result<()> {
    let version = metadata.schema_version().await?;
    for (migration_version, migration) in MIGRATIONS {
        if version < *migration_version {
            migration(connection).await?;
            metadata.set_schema_version(*migration_version).await?;
        }
    }
    Ok(())
}

async fn table_exists(connection: &Connection, name: &str) -> bool {
    connection
        .table_names()
        .execute()
        .await
        .unwrap_or_default()
        .contains(&name.to_string())
}

async fn read_batches(table: &lancedb::Table) -> Result<Vec<RecordBatch>> {
    Ok(table
        .query()
        .execute()
        .await
        .map_err(crate::error::DbError::from)?
        .try_collect()
        .await
        .map_err(crate::error::DbError::from)?)
}

/// v2: `updated_at` / `deleted_at` columns on curriculum, progress,
/// learning_items (plus `updated_at` on session_history). Existing rows get
/// NULL timestamps — "unknown", treated as the oldest by sync.
async fn migrate_v2_timestamps(connection: &Connection) -> Result<()> {
    if table_exists(connection, crate::curriculum::TABLE_NAME).await {
        let old = connection
            .open_table(crate::curriculum::TABLE_NAME)
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        let mut curriculum = open_course_core::curriculum::Curriculum {
            version: 1,
            topics: Vec::new(),
            target_language: String::new(),
            native_language: String::new(),
        };
        for batch in read_batches(&old).await? {
            let parsed = crate::curriculum::topics_from_record_batch(&batch)?;
            if !parsed.topics.is_empty() {
                curriculum.version = parsed.version;
                curriculum.target_language = parsed.target_language;
                curriculum.native_language = parsed.native_language;
            }
            curriculum.topics.extend(parsed.topics);
        }
        connection
            .drop_table(crate::curriculum::TABLE_NAME, &[])
            .await
            .map_err(crate::error::DbError::from)?;
        let new_table = connection
            .create_empty_table(crate::curriculum::TABLE_NAME, crate::curriculum::schema())
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        for topic in &curriculum.topics {
            let batch = crate::curriculum::topic_to_record_batch(topic)?;
            new_table
                .add(vec![batch])
                .execute()
                .await
                .map_err(crate::error::DbError::from)?;
        }
    }

    if table_exists(connection, crate::progress::TABLE_NAME).await {
        let old = connection
            .open_table(crate::progress::TABLE_NAME)
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        let mut topics = Vec::new();
        for batch in read_batches(&old).await? {
            topics.extend(crate::progress::progress_from_record_batch(&batch)?.topics);
        }
        connection
            .drop_table(crate::progress::TABLE_NAME, &[])
            .await
            .map_err(crate::error::DbError::from)?;
        let new_table = connection
            .create_empty_table(crate::progress::TABLE_NAME, crate::progress::schema())
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        for topic in &topics {
            let batch = crate::progress::progress_topic_to_record_batch(topic)?;
            new_table
                .add(vec![batch])
                .execute()
                .await
                .map_err(crate::error::DbError::from)?;
        }
    }

    if table_exists(connection, crate::learning_items::TABLE_NAME).await {
        let old = connection
            .open_table(crate::learning_items::TABLE_NAME)
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        let mut items = Vec::new();
        for batch in read_batches(&old).await? {
            items.extend(crate::learning_items::learning_items_from_record_batch(
                &batch,
            )?);
        }
        connection
            .drop_table(crate::learning_items::TABLE_NAME, &[])
            .await
            .map_err(crate::error::DbError::from)?;
        let new_table = connection
            .create_empty_table(
                crate::learning_items::TABLE_NAME,
                crate::learning_items::schema(),
            )
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        for item in &items {
            let batch = crate::learning_items::learning_item_to_record_batch(item)?;
            new_table
                .add(vec![batch])
                .execute()
                .await
                .map_err(crate::error::DbError::from)?;
        }
    }

    if table_exists(connection, crate::history::TABLE_NAME).await {
        rewrite_history(connection, |summaries| summaries).await?;
    }

    Ok(())
}

/// Reads, transforms, and rewrites the whole session_history table.
async fn rewrite_history(
    connection: &Connection,
    transform: impl Fn(
        Vec<open_course_core::history::SessionSummary>,
    ) -> Vec<open_course_core::history::SessionSummary>,
) -> Result<()> {
    let old = connection
        .open_table(crate::history::TABLE_NAME)
        .execute()
        .await
        .map_err(crate::error::DbError::from)?;
    let mut summaries = Vec::new();
    for batch in read_batches(&old).await? {
        summaries.extend(crate::history::history_from_record_batch(&batch)?);
    }
    let summaries = transform(summaries);
    connection
        .drop_table(crate::history::TABLE_NAME, &[])
        .await
        .map_err(crate::error::DbError::from)?;
    let new_table = connection
        .create_empty_table(crate::history::TABLE_NAME, crate::history::schema())
        .execute()
        .await
        .map_err(crate::error::DbError::from)?;
    if !summaries.is_empty() {
        let batch = crate::history::history_to_record_batch(&summaries)?;
        new_table
            .add(vec![batch])
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
    }
    Ok(())
}

/// v3: legacy numeric session ids (timestamp-millis) become UUIDv7. Entries
/// are rewritten in chronological order (by date, then the old id), so the
/// history keeps its ordering.
async fn migrate_v3_session_uuids(connection: &Connection) -> Result<()> {
    if !table_exists(connection, crate::history::TABLE_NAME).await {
        return Ok(());
    }
    rewrite_history(connection, |mut summaries| {
        summaries.sort_by(|a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)));
        for summary in &mut summaries {
            if !summary.id.is_empty() && summary.id.chars().all(|c| c.is_ascii_digit()) {
                summary.id = uuid::Uuid::now_v7().to_string();
            }
        }
        summaries
    })
    .await
}

/// v4: the sync outbox table.
async fn migrate_v4_outbox(connection: &Connection) -> Result<()> {
    if !table_exists(connection, crate::outbox::TABLE_NAME).await {
        connection
            .create_empty_table(crate::outbox::TABLE_NAME, crate::outbox::schema())
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
    }
    Ok(())
}

/// v5: the vocabulary tables (lemmas and their inflected forms).
async fn migrate_v5_vocabulary(connection: &Connection) -> Result<()> {
    if !table_exists(connection, crate::lemmas::TABLE_NAME).await {
        connection
            .create_empty_table(crate::lemmas::TABLE_NAME, crate::lemmas::schema())
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
    }
    if !table_exists(connection, crate::forms::TABLE_NAME).await {
        connection
            .create_empty_table(crate::forms::TABLE_NAME, crate::forms::schema())
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow_array::builder::{ListBuilder, StringBuilder};
    use arrow_array::{Float64Array, Int32Array, ListArray, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use lancedb::connect;
    use tempfile::TempDir;

    use super::*;
    use crate::Database;

    /// Builds a database with pre-v2 (schema version 1) tables: no
    /// `updated_at`/`deleted_at` columns and numeric session ids.
    async fn create_legacy_db(dir: &TempDir) -> std::path::PathBuf {
        let path = dir.path().join("db");
        let connection = connect(&path.to_string_lossy()).execute().await.unwrap();

        // Legacy curriculum: one topic, no timestamp columns.
        let curriculum_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("description", DataType::Utf8, false),
            Field::new("difficulty", DataType::Utf8, false),
            Field::new("level", DataType::Utf8, true),
            Field::new("order", DataType::Int32, true),
            Field::new(
                "tags",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                false,
            ),
            Field::new("target_lang", DataType::Utf8, false),
            Field::new("native_lang", DataType::Utf8, false),
            Field::new("version", DataType::Int32, false),
        ]));
        let mut tags_builder = ListBuilder::new(StringBuilder::new());
        tags_builder.values().append_value("vocabulary");
        tags_builder.append(true);
        let batch = RecordBatch::try_new(
            curriculum_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["greetings"])),
                Arc::new(StringArray::from(vec!["Greetings"])),
                Arc::new(StringArray::from(vec!["Basic greetings"])),
                Arc::new(StringArray::from(vec!["beginner"])),
                Arc::new(StringArray::from(vec![Some("A1")])),
                Arc::new(Int32Array::from(vec![1])),
                Arc::new(tags_builder.finish()),
                Arc::new(StringArray::from(vec!["es"])),
                Arc::new(StringArray::from(vec!["ru"])),
                Arc::new(Int32Array::from(vec![1])),
            ],
        )
        .unwrap();
        let table = connection
            .create_empty_table("curriculum", curriculum_schema)
            .execute()
            .await
            .unwrap();
        table.add(vec![batch]).execute().await.unwrap();

        // Legacy progress: one entry.
        let progress_schema = Arc::new(Schema::new(vec![
            Field::new("topic_id", DataType::Utf8, false),
            Field::new("score", DataType::Float64, false),
            Field::new("mastery", DataType::Float64, false),
            Field::new("difficulty_estimate", DataType::Float64, false),
            Field::new("practice_count", DataType::Int32, false),
            Field::new("last_practiced", DataType::Utf8, true),
        ]));
        let batch = RecordBatch::try_new(
            progress_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["greetings"])),
                Arc::new(Float64Array::from(vec![75.0])),
                Arc::new(Float64Array::from(vec![75.0])),
                Arc::new(Float64Array::from(vec![0.0])),
                Arc::new(Int32Array::from(vec![2])),
                Arc::new(StringArray::from(vec![Some("2024-01-01T00:00:00Z")])),
            ],
        )
        .unwrap();
        let table = connection
            .create_empty_table("progress", progress_schema)
            .execute()
            .await
            .unwrap();
        table.add(vec![batch]).execute().await.unwrap();

        // Legacy learning items: one item.
        let items_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("description", DataType::Utf8, false),
            Field::new("level", DataType::Utf8, true),
            Field::new("target_lang", DataType::Utf8, false),
            Field::new("native_lang", DataType::Utf8, false),
            Field::new("score", DataType::Float64, false),
            Field::new("last_practiced", DataType::Utf8, true),
            Field::new("practice_count", DataType::Int32, false),
        ]));
        let batch = RecordBatch::try_new(
            items_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["es-caro-rico"])),
                Arc::new(StringArray::from(vec!["Caro vs Rico"])),
                Arc::new(StringArray::from(vec!["Confusion pair"])),
                Arc::new(StringArray::from(vec![Some("A1")])),
                Arc::new(StringArray::from(vec!["es"])),
                Arc::new(StringArray::from(vec!["ru"])),
                Arc::new(Float64Array::from(vec![30.0])),
                Arc::new(StringArray::from(vec![None::<&str>])),
                Arc::new(Int32Array::from(vec![1])),
            ],
        )
        .unwrap();
        let table = connection
            .create_empty_table("learning_items", items_schema)
            .execute()
            .await
            .unwrap();
        table.add(vec![batch]).execute().await.unwrap();

        // Legacy session history: two entries with numeric (millis) ids.
        let history_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("date", DataType::Utf8, false),
            Field::new(
                "target_topic_ids",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                false,
            ),
            Field::new(
                "side_topic_ids",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                false,
            ),
            Field::new(
                "new_topic_ids",
                DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
                false,
            ),
            Field::new("avg_target_score", DataType::Float64, false),
            Field::new("target_delta", DataType::Float64, false),
        ]));
        let ids = StringArray::from(vec!["1704067200000", "1704153600000"]);
        let dates = StringArray::from(vec!["2024-01-01T00:00:00Z", "2024-01-02T00:00:00Z"]);
        let mut list_builder = ListBuilder::new(StringBuilder::new());
        list_builder.values().append_value("greetings");
        list_builder.append(true);
        list_builder.values().append_value("greetings");
        list_builder.append(true);
        let lists: ListArray = list_builder.finish();
        let batch = RecordBatch::try_new(
            history_schema.clone(),
            vec![
                Arc::new(ids),
                Arc::new(dates),
                Arc::new(lists.clone()),
                Arc::new(lists.clone()),
                Arc::new(lists),
                Arc::new(Float64Array::from(vec![80.0, 85.0])),
                Arc::new(Float64Array::from(vec![0.0, 0.0])),
            ],
        )
        .unwrap();
        let table = connection
            .create_empty_table("session_history", history_schema)
            .execute()
            .await
            .unwrap();
        table.add(vec![batch]).execute().await.unwrap();

        path
    }

    #[tokio::test]
    async fn migrates_legacy_database_transparently() {
        let dir = TempDir::new().unwrap();
        let path = create_legacy_db(&dir).await;

        let db = Database::connect(&path).await.unwrap();

        // Data survived the migration.
        let curriculum = db.curriculum().read_all().await.unwrap();
        assert_eq!(curriculum.topics.len(), 1);
        assert_eq!(curriculum.topics[0].id, "greetings");
        assert_eq!(curriculum.topics[0].tags, vec!["vocabulary".to_string()]);
        // Timestamps are unknown (NULL) for pre-sync rows.
        assert_eq!(curriculum.topics[0].updated_at, None);
        assert_eq!(curriculum.topics[0].deleted_at, None);

        let progress = db.progress().read_all().await.unwrap();
        assert_eq!(progress.topics.len(), 1);
        assert_eq!(progress.topics[0].topic_id, "greetings");
        assert_eq!(progress.topics[0].score, 75.0);
        assert_eq!(progress.topics[0].updated_at, None);

        let items = db.learning_items().read_all().await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "es-caro-rico");
        assert_eq!(items[0].updated_at, None);

        // Session ids are UUIDs now, chronological order preserved.
        let history = db.history().read_all().await.unwrap();
        assert_eq!(history.len(), 2);
        for summary in &history {
            assert!(
                uuid::Uuid::parse_str(&summary.id).is_ok(),
                "session id should be a UUID, got {}",
                summary.id
            );
            assert_eq!(summary.updated_at, None);
        }
        assert_eq!(history[0].date, "2024-01-01T00:00:00Z");
        assert_eq!(history[1].date, "2024-01-02T00:00:00Z");
        assert!(history[0].id != history[1].id);

        // Schema version stamped, outbox created.
        assert_eq!(
            db.metadata().schema_version().await.unwrap(),
            CURRENT_SCHEMA_VERSION
        );
        assert_eq!(db.outbox().len().await.unwrap(), 0);

        // v5 vocabulary tables exist and are empty.
        assert!(db.lemmas().read_all().await.unwrap().is_empty());
        assert!(db.forms().read_all().await.unwrap().is_empty());

        // Reconnecting is a no-op (migrations are not re-applied destructively).
        let db = Database::connect(&path).await.unwrap();
        let curriculum = db.curriculum().read_all().await.unwrap();
        assert_eq!(curriculum.topics.len(), 1);
        let history = db.history().read_all().await.unwrap();
        assert_eq!(history.len(), 2);
    }

    #[tokio::test]
    async fn fresh_database_gets_current_schema() {
        let dir = TempDir::new().unwrap();
        let db = Database::connect(&dir.path().join("db")).await.unwrap();
        assert_eq!(
            db.metadata().schema_version().await.unwrap(),
            CURRENT_SCHEMA_VERSION
        );
        assert!(db.curriculum().read_all().await.unwrap().topics.is_empty());
        assert_eq!(db.outbox().len().await.unwrap(), 0);
        assert!(db.lemmas().read_all().await.unwrap().is_empty());
        assert!(db.forms().read_all().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn outbox_append_read_and_delete_through() {
        let dir = TempDir::new().unwrap();
        let db = Database::connect(&dir.path().join("db")).await.unwrap();
        let outbox = db.outbox();

        let first = outbox
            .append("upsert", "topic", "t1", "{\"id\":\"t1\"}")
            .await
            .unwrap();
        let second = outbox.append("delete", "topic", "t2", "").await.unwrap();
        assert_eq!(first.seq, 1);
        assert_eq!(second.seq, 2);
        assert_eq!(outbox.len().await.unwrap(), 2);

        let entries = outbox.read_all().await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].op, "upsert");
        assert_eq!(entries[0].entity_id, "t1");
        assert_eq!(entries[1].op, "delete");

        outbox.delete_through(first.seq).await.unwrap();
        let entries = outbox.read_all().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].seq, second.seq);

        // Sequence numbers keep increasing after deletions.
        let third = outbox
            .append("upsert", "session", "s1", "{}")
            .await
            .unwrap();
        assert_eq!(third.seq, 3);
    }

    #[tokio::test]
    async fn soft_delete_hides_rows_and_purge_removes_them() {
        let dir = TempDir::new().unwrap();
        let db = Database::connect(&dir.path().join("db")).await.unwrap();

        let topic = open_course_core::curriculum::Topic {
            id: "t1".to_string(),
            name: "Greetings".to_string(),
            target_lang: "es".to_string(),
            native_lang: "ru".to_string(),
            version: 1,
            ..Default::default()
        };
        db.curriculum().upsert(&topic).await.unwrap();
        let stored = db.curriculum().read_all().await.unwrap();
        assert_eq!(stored.topics.len(), 1);
        // Upsert stamps updated_at.
        assert!(stored.topics[0].updated_at.is_some());

        db.curriculum().delete_by_topic_id("t1").await.unwrap();
        assert!(db.curriculum().read_all().await.unwrap().topics.is_empty());

        // The tombstone is still physically there until purged.
        db.curriculum()
            .purge_deleted("2999-01-01T00:00:00Z")
            .await
            .unwrap();
        // Re-upserting after a purge works as a plain insert.
        db.curriculum().upsert(&topic).await.unwrap();
        assert_eq!(db.curriculum().read_all().await.unwrap().topics.len(), 1);

        // Progress soft-delete behaves the same way.
        let entry = open_course_core::progress::ProgressTopic::initial("t1".to_string(), 50.0);
        db.progress().upsert(&entry).await.unwrap();
        assert!(db.progress().get_by_topic_id("t1").await.unwrap().is_some());
        db.progress().delete_by_topic_id("t1").await.unwrap();
        assert!(db.progress().get_by_topic_id("t1").await.unwrap().is_none());
        assert!(db.progress().read_all().await.unwrap().topics.is_empty());

        // Learning items soft-delete behaves the same way.
        let item = open_course_core::learning_items::LearningItem {
            id: "i1".to_string(),
            name: "Caro vs Rico".to_string(),
            ..Default::default()
        };
        db.learning_items().upsert(&item).await.unwrap();
        assert_eq!(db.learning_items().read_all().await.unwrap().len(), 1);
        db.learning_items().delete_by_id("i1").await.unwrap();
        assert!(db.learning_items().read_all().await.unwrap().is_empty());
    }
}
