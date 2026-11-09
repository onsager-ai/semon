//! Parquet reading and writing shared by the store and compaction.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Array, Int64Array, StringArray};
use arrow::datatypes::SchemaRef;
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::{ArrowWriter, ProjectionMask};
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::WriterProperties;

use crate::error::{Error, Result};
use crate::schema::Table;

const ZSTD_LEVEL: i32 = 3;
const ROW_GROUP_ROWS: usize = 128 * 1024;

pub(crate) struct Written {
    pub rows: u64,
    pub bytes: u64,
    /// Decoded size of the rows, as the caller reported it.
    pub memory: u64,
}

/// A Parquet file being written. `finish` fsyncs it.
pub(crate) struct OutFile {
    writer: ArrowWriter<File>,
    sync: File,
    rows: u64,
    memory: u64,
}

impl OutFile {
    pub(crate) fn create(path: &Path, schema: SchemaRef) -> Result<Self> {
        let properties = WriterProperties::builder()
            .set_compression(Compression::ZSTD(ZstdLevel::try_new(ZSTD_LEVEL)?))
            .set_max_row_group_row_count(Some(ROW_GROUP_ROWS))
            .build();
        let file = File::create(path)?;
        let sync = file.try_clone()?;
        let writer = ArrowWriter::try_new(file, schema, Some(properties))?;
        Ok(Self {
            writer,
            sync,
            rows: 0,
            memory: 0,
        })
    }

    /// Appends `batch` as one row group. `memory` is its decoded size, which the
    /// caller knows better than a sliced batch can say.
    pub(crate) fn write_group(&mut self, batch: &RecordBatch, memory: u64) -> Result<()> {
        self.rows += batch.num_rows() as u64;
        self.memory += memory;
        self.writer.write(batch)?;
        self.writer.flush()?;
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<Written> {
        self.writer.close()?;
        self.sync.sync_all()?;
        Ok(Written {
            rows: self.rows,
            bytes: self.sync.metadata()?.len(),
            memory: self.memory,
        })
    }
}

pub(crate) fn write_file(path: &Path, batch: &RecordBatch) -> Result<Written> {
    let mut out = OutFile::create(path, batch.schema())?;
    out.write_group(batch, batch.get_array_memory_size() as u64)?;
    out.finish()
}

/// Reads every batch of a file, or only the named top-level columns.
pub(crate) fn read_batches(path: &Path, columns: Option<&[&str]>) -> Result<Vec<RecordBatch>> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?;
    let builder = match columns {
        Some(names) => {
            let mut indices = Vec::with_capacity(names.len());
            for name in names {
                indices.push(builder.schema().index_of(name)?);
            }
            let mask = ProjectionMask::roots(builder.parquet_schema(), indices);
            builder.with_projection(mask)
        }
        None => builder,
    };
    let mut batches = Vec::new();
    for batch in builder.build()? {
        batches.push(batch?);
    }
    Ok(batches)
}

/// Adds the `_gen` column to a batch of a table's declared columns.
pub(crate) fn with_generation(
    table: &Table,
    batch: &RecordBatch,
    generation: i64,
) -> Result<RecordBatch> {
    let declared = table.arrow_schema();
    if batch.schema().fields() != declared.fields() {
        return Err(Error::invalid(format!(
            "batch for {} does not match its declared schema",
            table.name
        )));
    }
    let mut columns = batch.columns().to_vec();
    columns.push(Arc::new(Int64Array::from(vec![
        generation;
        batch.num_rows()
    ])));
    Ok(RecordBatch::try_new(
        Arc::new(table.file_schema()),
        columns,
    )?)
}

pub(crate) fn string_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<StringArray>())
        .ok_or_else(|| Error::invalid(format!("column {name} is missing or not text")))
}

pub(crate) fn int_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<Int64Array>())
        .ok_or_else(|| Error::invalid(format!("column {name} is missing or not an integer")))
}
