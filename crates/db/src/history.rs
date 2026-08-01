use std::sync::Arc;

use arrow_array::builder::{ListBuilder, StringBuilder};
use arrow_array::{Float64Array, ListArray, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::query::ExecutableQuery;

use open_course_core::error::Result;

pub use open_course_core::history::*;

pub const TABLE_NAME: &str = "session_history";

pub(crate) fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
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
        Field::new("updated_at", DataType::Utf8, true),
    ]))
}

#[derive(Clone)]
pub struct HistoryTable {
    table: lancedb::Table,
}

impl HistoryTable {
    pub async fn open(connection: &Connection) -> Result<Self> {
        if let Ok(table) = connection.open_table(TABLE_NAME).execute().await {
            if needs_migration(&table).await? {
                let batches = read_raw_batches(&table).await?;
                let migrated = migrate_batches(batches)?;
                connection
                    .drop_table(TABLE_NAME, &[])
                    .await
                    .map_err(crate::error::DbError::from)?;
                let table = connection
                    .create_empty_table(TABLE_NAME, schema())
                    .execute()
                    .await
                    .map_err(crate::error::DbError::from)?;
                add_batches(&table, migrated).await?;
                return Ok(Self { table });
            }
            return Ok(Self { table });
        }

        let table = connection
            .create_empty_table(TABLE_NAME, schema())
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(Self { table })
    }

    pub async fn read_all(&self) -> Result<Vec<SessionSummary>> {
        let records = self
            .table
            .query()
            .execute()
            .await
            .map_err(crate::error::DbError::from)?
            .try_collect::<Vec<_>>()
            .await
            .map_err(crate::error::DbError::from)?;
        let mut summaries = Vec::new();
        for record in records {
            summaries.extend(history_from_record_batch(&record)?);
        }
        Ok(summaries)
    }

    pub async fn read_last(&self, n: usize) -> Result<Vec<SessionSummary>> {
        let mut all = self.read_all().await?;
        all.sort_by(|a, b| a.date.cmp(&b.date));
        Ok(all.into_iter().rev().take(n).collect())
    }

    /// Appends a summary, stamping `updated_at` with the current time.
    pub async fn append(&self, summary: &SessionSummary) -> Result<()> {
        let mut summary = summary.clone();
        summary.updated_at = Some(crate::util::now_rfc3339());
        self.append_with_timestamps(&summary).await
    }

    /// Appends a summary exactly as given — used when applying synced
    /// changes whose timestamps must be preserved.
    pub async fn append_with_timestamps(&self, summary: &SessionSummary) -> Result<()> {
        let mut all = self.read_all().await?;
        all.push(summary.clone());
        let total = all.len();
        if total > MAX_HISTORY_ENTRIES {
            all = all.into_iter().skip(total - MAX_HISTORY_ENTRIES).collect();
        }
        self.table
            .delete("id IS NOT NULL")
            .await
            .map_err(crate::error::DbError::from)?;
        let batch = history_to_record_batch(&all)?;
        self.table
            .add(vec![batch])
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(())
    }

    pub async fn reset(&self) -> Result<()> {
        self.table
            .delete("id IS NOT NULL")
            .await
            .map_err(crate::error::DbError::from)?;
        Ok(())
    }
}

async fn needs_migration(table: &lancedb::Table) -> Result<bool> {
    let records: Vec<RecordBatch> = table
        .query()
        .execute()
        .await
        .map_err(crate::error::DbError::from)?
        .try_collect()
        .await
        .map_err(crate::error::DbError::from)?;
    if records.is_empty() {
        return Ok(false);
    }
    Ok(!records[0]
        .schema()
        .fields()
        .iter()
        .any(|f| f.name() == "new_topic_ids"))
}

async fn read_raw_batches(table: &lancedb::Table) -> Result<Vec<RecordBatch>> {
    let records: Vec<RecordBatch> = table
        .query()
        .execute()
        .await
        .map_err(crate::error::DbError::from)?
        .try_collect()
        .await
        .map_err(crate::error::DbError::from)?;
    Ok(records)
}

fn migrate_batches(batches: Vec<RecordBatch>) -> Result<Vec<RecordBatch>> {
    batches.into_iter().map(add_new_topic_ids_column).collect()
}

async fn add_batches(table: &lancedb::Table, batches: Vec<RecordBatch>) -> Result<()> {
    for batch in batches {
        table
            .add(vec![batch])
            .execute()
            .await
            .map_err(crate::error::DbError::from)?;
    }
    Ok(())
}

fn add_new_topic_ids_column(batch: RecordBatch) -> Result<RecordBatch> {
    let n = batch.num_rows();
    let mut builder = ListBuilder::new(StringBuilder::new());
    for _ in 0..n {
        builder.append(true);
    }
    let new_topics_array = builder.finish();

    let new_field = Arc::new(Field::new(
        "new_topic_ids",
        DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
        false,
    ));

    let mut new_fields: Vec<Arc<Field>> = Vec::new();
    let mut new_columns: Vec<Arc<dyn arrow_array::Array>> = Vec::new();

    let mut inserted = false;
    for (i, field) in batch.schema().fields().iter().enumerate() {
        if field.name() == "avg_target_score" && !inserted {
            new_fields.push(new_field.clone());
            new_columns.push(Arc::new(new_topics_array.clone()));
            inserted = true;
        }
        new_fields.push(Arc::new(Field::clone(field)));
        new_columns.push(batch.column(i).clone());
    }

    if !inserted {
        new_fields.push(new_field);
        new_columns.push(Arc::new(new_topics_array));
    }

    let schema = Arc::new(Schema::new(new_fields));
    Ok(RecordBatch::try_new(schema, new_columns).map_err(crate::error::DbError::from)?)
}

pub(crate) fn history_to_record_batch(history: &[SessionSummary]) -> Result<RecordBatch> {
    let ids = StringArray::from_iter_values(history.iter().map(|s| s.id.as_str()));
    let dates = StringArray::from_iter_values(history.iter().map(|s| s.date.as_str()));
    let avg_scores = Float64Array::from_iter_values(history.iter().map(|s| s.avg_target_score));
    let deltas = Float64Array::from_iter_values(history.iter().map(|s| s.target_delta));

    let mut target_builder = ListBuilder::new(StringBuilder::new());
    for summary in history {
        for id in &summary.target_topic_ids {
            target_builder.values().append_value(id);
        }
        target_builder.append(true);
    }
    let target_array = target_builder.finish();

    let mut side_builder = ListBuilder::new(StringBuilder::new());
    for summary in history {
        for id in &summary.side_topic_ids {
            side_builder.values().append_value(id);
        }
        side_builder.append(true);
    }
    let side_array = side_builder.finish();

    let mut new_topics_builder = ListBuilder::new(StringBuilder::new());
    for summary in history {
        for id in &summary.new_topic_ids {
            new_topics_builder.values().append_value(id);
        }
        new_topics_builder.append(true);
    }
    let new_topics_array = new_topics_builder.finish();

    let batch = RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(ids),
            Arc::new(dates),
            Arc::new(target_array),
            Arc::new(side_array),
            Arc::new(new_topics_array),
            Arc::new(avg_scores),
            Arc::new(deltas),
            Arc::new(StringArray::from_iter(
                history.iter().map(|s| s.updated_at.as_deref()),
            )),
        ],
    )
    .map_err(crate::error::DbError::from)?;
    Ok(batch)
}

pub(crate) fn history_from_record_batch(batch: &RecordBatch) -> Result<Vec<SessionSummary>> {
    let n = batch.num_rows();
    let id_col = batch
        .column_by_name("id")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let date_col = batch
        .column_by_name("date")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    let target_col = batch
        .column_by_name("target_topic_ids")
        .unwrap()
        .as_any()
        .downcast_ref::<ListArray>()
        .unwrap();
    let side_col = batch
        .column_by_name("side_topic_ids")
        .unwrap()
        .as_any()
        .downcast_ref::<ListArray>()
        .unwrap();
    let new_topics_col = batch
        .column_by_name("new_topic_ids")
        .and_then(|c| c.as_any().downcast_ref::<ListArray>());
    let avg_col = batch
        .column_by_name("avg_target_score")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    let delta_col = batch
        .column_by_name("target_delta")
        .unwrap()
        .as_any()
        .downcast_ref::<Float64Array>()
        .unwrap();
    // Column added in schema v2 may be absent in unmigrated tables.
    let updated_col = crate::util::optional_string_column(batch, "updated_at");

    let mut summaries = Vec::with_capacity(n);
    for i in 0..n {
        let target_list = target_col.value(i);
        let side_list = side_col.value(i);
        let target_topic_ids = target_list
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .iter()
            .filter_map(|s| s.map(|s| s.to_string()))
            .collect();
        let side_topic_ids = side_list
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .iter()
            .filter_map(|s| s.map(|s| s.to_string()))
            .collect();
        // Tables predating the `new_topic_ids` column read as empty lists.
        let new_topic_ids = new_topics_col
            .map(|col| {
                col.value(i)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap()
                    .iter()
                    .filter_map(|s| s.map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        summaries.push(SessionSummary {
            id: id_col.value(i).to_string(),
            date: date_col.value(i).to_string(),
            target_topic_ids,
            side_topic_ids,
            new_topic_ids,
            avg_target_score: avg_col.value(i),
            target_delta: delta_col.value(i),
            updated_at: crate::util::optional_string_at(updated_col, i),
        });
    }

    Ok(summaries)
}
