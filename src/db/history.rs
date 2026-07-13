use std::sync::Arc;

use arrow_array::builder::{ListBuilder, StringBuilder};
use arrow_array::{Float64Array, ListArray, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures_util::stream::TryStreamExt;
use lancedb::Connection;
use lancedb::query::ExecutableQuery;

use crate::error::Result;

pub const TABLE_NAME: &str = "session_history";

pub const MAX_HISTORY_ENTRIES: usize = 500;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub date: String,
    pub target_topic_ids: Vec<String>,
    pub side_topic_ids: Vec<String>,
    pub new_topic_ids: Vec<String>,
    pub avg_target_score: f64,
    pub target_delta: f64,
}

fn schema() -> Arc<Schema> {
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
                connection.drop_table(TABLE_NAME, &[]).await?;
                let table = connection
                    .create_empty_table(TABLE_NAME, schema())
                    .execute()
                    .await?;
                add_batches(&table, migrated).await?;
                return Ok(Self { table });
            }
            return Ok(Self { table });
        }

        let table = connection
            .create_empty_table(TABLE_NAME, schema())
            .execute()
            .await?;
        Ok(Self { table })
    }

    pub async fn read_all(&self) -> Result<Vec<SessionSummary>> {
        let records = self
            .table
            .query()
            .execute()
            .await?
            .try_collect::<Vec<_>>()
            .await?;
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

    pub async fn append(&self, summary: &SessionSummary) -> Result<()> {
        let mut all = self.read_all().await?;
        all.push(summary.clone());
        let total = all.len();
        if total > MAX_HISTORY_ENTRIES {
            all = all.into_iter().skip(total - MAX_HISTORY_ENTRIES).collect();
        }
        self.table.delete("id IS NOT NULL").await?;
        let batch = history_to_record_batch(&all)?;
        self.table.add(vec![batch]).execute().await?;
        Ok(())
    }

    pub async fn reset(&self) -> Result<()> {
        self.table.delete("id IS NOT NULL").await?;
        Ok(())
    }
}

async fn needs_migration(table: &lancedb::Table) -> Result<bool> {
    let records: Vec<RecordBatch> = table.query().execute().await?.try_collect().await?;
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
    let records: Vec<RecordBatch> = table.query().execute().await?.try_collect().await?;
    Ok(records)
}

fn migrate_batches(batches: Vec<RecordBatch>) -> Result<Vec<RecordBatch>> {
    batches.into_iter().map(add_new_topic_ids_column).collect()
}

async fn add_batches(table: &lancedb::Table, batches: Vec<RecordBatch>) -> Result<()> {
    for batch in batches {
        table.add(vec![batch]).execute().await?;
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
    Ok(RecordBatch::try_new(schema, new_columns)?)
}

fn history_to_record_batch(history: &[SessionSummary]) -> Result<RecordBatch> {
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
        ],
    )?;
    Ok(batch)
}

fn history_from_record_batch(batch: &RecordBatch) -> Result<Vec<SessionSummary>> {
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
        .unwrap()
        .as_any()
        .downcast_ref::<ListArray>()
        .unwrap();
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

    let mut summaries = Vec::with_capacity(n);
    for i in 0..n {
        let target_list = target_col.value(i);
        let side_list = side_col.value(i);
        let new_topics_list = new_topics_col.value(i);
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
        let new_topic_ids = new_topics_list
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap()
            .iter()
            .filter_map(|s| s.map(|s| s.to_string()))
            .collect();

        summaries.push(SessionSummary {
            id: id_col.value(i).to_string(),
            date: date_col.value(i).to_string(),
            target_topic_ids,
            side_topic_ids,
            new_topic_ids,
            avg_target_score: avg_col.value(i),
            target_delta: delta_col.value(i),
        });
    }

    Ok(summaries)
}
