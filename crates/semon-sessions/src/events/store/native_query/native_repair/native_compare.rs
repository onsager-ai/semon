//! Durable exact native-byte equality for two independently qualified Originals.
//! This owns no copy/alias/relationship semantics. Compare remains the decoder;
//! the caller supplies the complete current relationship-membership fence.
use super::*;

pub(in super::super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_original_comparisons (
 left_original BLOB NOT NULL, left_class INTEGER NOT NULL,
 right_original BLOB NOT NULL, right_class INTEGER NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL,
 PRIMARY KEY(left_original,left_class,right_original,right_class)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS native_original_comparison_right
 ON native_original_comparisons(right_original,right_class,left_original,left_class);
";
const VERSION: u32 = 1;
const DECODED: usize = 32 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct ComparisonSource<'a, 'r> {
    pub(crate) witness: &'a SourceWitness,
    pub(crate) repair: &'a Repair,
    pub(crate) text: &'a OriginalText,
    pub(crate) records: &'a crate::query_source::Records<'r>,
}

#[derive(Debug)]
pub(crate) struct TextComparisonStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) comparison: Option<Comparison>,
    /// Explicit pair/representative SQLite statements, excluding caller fences.
    pub(crate) comparison_sql_vm_steps: i32,
    pub(crate) serialized_cursor_bytes: usize,
    pub(crate) fence_checks: usize,
    pub(crate) records_finish_calls: usize,
    /// Owned fragment payloads returned by Records, including any anchors.
    /// Records' independently measured proof/provider I/O is a separate cost.
    pub(crate) native_fragment_bytes: usize,
    /// Compare may hold two decoded chunks plus one bounded replay chunk.
    pub(crate) decoded_buffer_bound: usize,
}

#[derive(Default)]
struct Meter {
    vm: i32,
    cursor: usize,
    fences: usize,
    finishes: usize,
    fragments: usize,
}
impl Meter {
    fn statement(&mut self, query: &rusqlite::Statement<'_>) {
        self.vm += query.get_status(rusqlite::StatementStatus::VmStep);
    }
    fn result(
        &self,
        outcome: Outcome,
        work: RepairWork,
        comparison: Option<Comparison>,
    ) -> TextComparisonStep {
        TextComparisonStep {
            outcome,
            work,
            comparison,
            comparison_sql_vm_steps: self.vm,
            serialized_cursor_bytes: self.cursor,
            fence_checks: self.fences,
            records_finish_calls: self.finishes,
            native_fragment_bytes: self.fragments,
            decoded_buffer_bound: 3 * DECODED,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    original: [u8; 32],
    class: u64,
    record: u64,
    field: OriginalField,
    receipt: Receipt,
}
impl Binding {
    fn from(representative: &OriginalTextRepresentative) -> Self {
        Self {
            original: representative.original,
            class: representative.class,
            record: representative.record,
            field: representative.field.clone(),
            receipt: representative.receipt,
        }
    }
    fn bounds(&self) -> io::Result<(u64, u64)> {
        let start = self
            .record
            .checked_add(self.field.span.start)
            .ok_or_else(|| io::Error::other("native pair descriptor offset overflow"))?;
        let end = self
            .record
            .checked_add(self.field.span.end)
            .ok_or_else(|| io::Error::other("native pair descriptor offset overflow"))?;
        Ok((start, end))
    }
    fn same(&self, other: &Self) -> io::Result<bool> {
        Ok(serde_json::to_vec(self).map_err(io::Error::other)?
            == serde_json::to_vec(other).map_err(io::Error::other)?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Mode {
    Native,
    SameClass,
    UnequalLength,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u32,
    left: Binding,
    right: Binding,
    mode: Mode,
    compare: Compare,
}
impl Cursor {
    fn mode(left: &Binding, right: &Binding) -> Mode {
        if left.original == right.original && left.class == right.class {
            Mode::SameClass
        } else if left.receipt.decoded_bytes != right.receipt.decoded_bytes {
            Mode::UnequalLength
        } else {
            Mode::Native
        }
    }
    fn new(left: Binding, right: Binding) -> io::Result<Self> {
        let mode = Self::mode(&left, &right);
        let mut compare = Compare::new(left.bounds()?, right.bounds()?)?;
        compare.result = match mode {
            Mode::Native => None,
            Mode::SameClass => Some(Comparison::Equal),
            Mode::UnequalLength => Some(Comparison::Different),
        };
        Ok(Self {
            version: VERSION,
            left,
            right,
            mode,
            compare,
        })
    }
    fn valid(&self, left: &Binding, right: &Binding) -> io::Result<bool> {
        let (ls, le) = left.bounds()?;
        let (rs, re) = right.bounds()?;
        Ok(self.version == VERSION
            && self.left.same(left)?
            && self.right.same(right)?
            && self.mode == Self::mode(left, right)
            && self.compare.left.matches_bounds(ls, le)
            && self.compare.right.matches_bounds(rs, re)
            && self.compare.compared <= left.receipt.decoded_bytes.min(right.receipt.decoded_bytes)
            && match self.mode {
                Mode::SameClass => self.compare.result == Some(Comparison::Equal),
                Mode::UnequalLength => self.compare.result == Some(Comparison::Different),
                Mode::Native => {
                    self.compare.result != Some(Comparison::Equal)
                        || (self.compare.left.complete()
                            && self.compare.right.complete()
                            && self.compare.compared == left.receipt.decoded_bytes)
                }
            })
    }
}

fn pair_current(
    db: &Connection,
    job: &Job,
    left: ComparisonSource<'_, '_>,
    right: ComparisonSource<'_, '_>,
    closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    meter: &mut Meter,
) -> io::Result<bool> {
    meter.fences += 1;
    if !job_current(db, job).map_err(io::Error::other)? {
        return Ok(false);
    }
    for source in [left, right] {
        meter.fences += 1;
        if !check_source(db, source.witness).map_err(io::Error::other)? {
            return Ok(false);
        }
        meter.fences += 1;
        if !repair_current(db, source.repair).map_err(io::Error::other)? {
            return Ok(false);
        }
        meter.fences += 1;
        if !admitted_original(db, job, source.witness).map_err(io::Error::other)? {
            return Ok(false);
        }
    }
    meter.fences += 1;
    closure(db, job.id).map_err(io::Error::other)
}

fn representative(
    db: &Connection,
    source: ComparisonSource<'_, '_>,
    work: &mut RepairWork,
    meter: &mut Meter,
) -> io::Result<Binding> {
    let (value, vm) = native_classes::representative_at(db, source.repair, source.text, work)?;
    meter.vm += vm;
    if value.source != source.repair.source {
        return Err(io::Error::other(
            "native pair representative source changed",
        ));
    }
    Ok(Binding::from(&value))
}

fn fragment(
    source: ComparisonSource<'_, '_>,
    at: u64,
    end: u64,
    maximum: usize,
    work: &mut RepairWork,
) -> io::Result<Option<crate::SessionSourceRange>> {
    if at > end || end > source.repair.source.offset {
        return Err(io::Error::other("native pair escaped Original descriptor"));
    }
    if at == end {
        return Ok(None);
    }
    // Usually touch one proof block. Permit a complete native scalar across
    // its boundary so an escape/surrogate prefix can never stall forever.
    let boundary = crate::source_chunks::SIZE - at as usize % crate::source_chunks::SIZE;
    let take = maximum.min(boundary.max(12));
    let take = usize::try_from((end - at).min(take as u64)).map_err(io::Error::other)?;
    let pin = source
        .repair
        .cursor
        .pin
        .as_ref()
        .ok_or_else(|| io::Error::other("native pair lacks Original pin"))?;
    let part = source
        .records
        .read_range(&source.repair.source, Some(&pin.generation), at, take)?;
    work.range_requests += 1;
    work.native_bytes += part.bytes.len();
    if part.offset != at
        || part.bytes.is_empty()
        || part.bytes.len() > take
        || part.generation != pin.generation
        || part.length != pin.length
    {
        return Err(io::Error::other("native pair Original provider changed"));
    }
    Ok(Some(part))
}

fn anchor(
    source: ComparisonSource<'_, '_>,
    binding: &Binding,
    work: &mut RepairWork,
) -> io::Result<()> {
    // Representatives can belong to earlier records in a completed Original.
    // Anchor that exact record rather than the repair cursor's current EOF.
    let end = binding
        .record
        .checked_add(1)
        .ok_or_else(|| io::Error::other("native pair anchor overflow"))?;
    if fragment(source, binding.record, end, 1, work)?.is_none() {
        return Err(io::Error::other("native pair anchor unavailable"));
    }
    Ok(())
}

impl NativeStore {
    pub(crate) fn original_compare_step(
        &mut self,
        job: &Job,
        left: ComparisonSource<'_, '_>,
        right: ComparisonSource<'_, '_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<TextComparisonStep> {
        let mut work = RepairWork::default();
        let mut meter = Meter::default();
        if cancelled() {
            return Ok(meter.result(Outcome::Deferred, work, None));
        }
        // Admission binds one CatalogSource per absolute (job, path). Two
        // root-relative references to that path cannot authorize two Original
        // namespaces under that single admitted witness.
        if left.repair.original != right.repair.original
            && left.witness.source.path == right.witness.source.path
        {
            return Ok(meter.result(Outcome::Conflict, work, None));
        }
        for source in [left, right] {
            meter.fences += 1;
            if reference(source.records, source.witness)? != source.repair.source
                || source.text.original() != source.repair.original
            {
                return Ok(meter.result(Outcome::Conflict, work, None));
            }
        }
        meter.fences += 1;
        if !self.runtime_current(&job.spec)? {
            return Ok(meter.result(Outcome::Conflict, work, None));
        }
        let (left_binding, right_binding, revision, mut cursor) = {
            let tx = self
                .connection
                .unchecked_transaction()
                .map_err(io::Error::other)?;
            if !pair_current(&tx, job, left, right, closure, &mut meter)? {
                return Ok(meter.result(Outcome::Conflict, work, None));
            }
            let l = representative(&tx, left, &mut work, &mut meter)?;
            let r = representative(&tx, right, &mut work, &mut meter)?;
            let mut query = tx.prepare("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_comparisons WHERE left_original=?1 AND left_class=?2 AND right_original=?3 AND right_class=?4").map_err(io::Error::other)?;
            let old: Option<(i64, Option<String>, i64)> = query
                .query_row(
                    params![
                        l.original.as_slice(),
                        seq_u64(l.class)?,
                        r.original.as_slice(),
                        seq_u64(r.class)?
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(io::Error::other)?;
            meter.statement(&query);
            let (revision, cursor) = if let Some((revision, text, size)) = old {
                let size = usize::try_from(size).map_err(io::Error::other)?;
                work.metadata_rows += 1;
                work.metadata_bytes += size + 16;
                meter.cursor = size;
                let cursor: Cursor = serde_json::from_str(
                    text.as_deref()
                        .ok_or_else(|| io::Error::other("native pair cursor exceeds residency"))?,
                )
                .map_err(io::Error::other)?;
                if !cursor.valid(&l, &r)? {
                    return Err(io::Error::other("native pair cursor descriptor changed"));
                }
                (Some(revision), cursor)
            } else {
                (None, Cursor::new(l.clone(), r.clone())?)
            };
            (l, r, revision, cursor)
        };
        let cached = cursor.compare.result.is_some();
        let mut l_part = None;
        let mut r_part = None;
        if !cached {
            l_part = fragment(
                left,
                cursor.compare.left.at,
                left_binding.bounds()?.1,
                64 * 1024,
                &mut work,
            )?;
            r_part = fragment(
                right,
                cursor.compare.right.at,
                right_binding.bounds()?.1,
                64 * 1024,
                &mut work,
            )?;
            cursor.compare.step(
                l_part.as_ref().map_or(&[][..], |p| p.bytes.as_ref()),
                r_part.as_ref().map_or(&[][..], |p| p.bytes.as_ref()),
                DECODED,
            )?;
        }
        // A side that actually read bytes is already independently touched.
        // Only cached/empty sides need an anchor; never add two extra bytes to
        // two full fragments merely because decoding reached their ends.
        meter.fragments = l_part.as_ref().map_or(0, |p| p.bytes.len())
            + r_part.as_ref().map_or(0, |p| p.bytes.len());
        if l_part.is_none() {
            anchor(left, &left_binding, &mut work)?;
            meter.fragments = meter
                .fragments
                .max(r_part.as_ref().map_or(0, |p| p.bytes.len()) + 1);
        }
        if r_part.is_none() {
            anchor(right, &right_binding, &mut work)?;
            meter.fragments = meter
                .fragments
                .max(l_part.as_ref().map_or(0, |p| p.bytes.len()) + 1);
        }
        if work.native_bytes > crate::SESSION_SOURCE_CHUNK_MAX {
            return Err(io::Error::other(
                "native pair exceeds total native fragment budget",
            ));
        }
        drop(l_part);
        drop(r_part);
        meter.finishes += 1;
        left.records.finish()?;
        meter.finishes += 1;
        right.records.finish()?;
        if cancelled() {
            return Ok(meter.result(Outcome::Deferred, work, None));
        }
        meter.fences += 1;
        if !self.runtime_current(&job.spec)? {
            return Ok(meter.result(Outcome::Conflict, work, None));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !pair_current(&tx, job, left, right, closure, &mut meter)? {
            return Ok(meter.result(Outcome::Conflict, work, None));
        }
        let current_left = representative(&tx, left, &mut work, &mut meter)?;
        let current_right = representative(&tx, right, &mut work, &mut meter)?;
        if !cursor.valid(&current_left, &current_right)? {
            return Ok(meter.result(Outcome::Conflict, work, None));
        }
        let mut query = tx.prepare("SELECT revision FROM native_original_comparisons WHERE left_original=?1 AND left_class=?2 AND right_original=?3 AND right_class=?4").map_err(io::Error::other)?;
        let current: Option<i64> = query
            .query_row(
                params![
                    left_binding.original.as_slice(),
                    seq_u64(left_binding.class)?,
                    right_binding.original.as_slice(),
                    seq_u64(right_binding.class)?
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        meter.statement(&query);
        work.metadata_rows += usize::from(current.is_some());
        work.metadata_bytes += usize::from(current.is_some()) * 8;
        drop(query);
        if current != revision {
            return Ok(meter.result(Outcome::Conflict, work, None));
        }
        if !cached || revision.is_none() {
            let state = bounded_json(&cursor, STATE_BYTES).map_err(io::Error::other)?;
            meter.cursor = state.len();
            work.metadata_rows += 1;
            work.metadata_bytes += state.len() + 80;
            let mut save = tx.prepare("INSERT INTO native_original_comparisons VALUES(?1,?2,?3,?4,0,?5) ON CONFLICT(left_original,left_class,right_original,right_class) DO UPDATE SET cursor=excluded.cursor,revision=revision+1").map_err(io::Error::other)?;
            save.execute(params![
                left_binding.original.as_slice(),
                seq_u64(left_binding.class)?,
                right_binding.original.as_slice(),
                seq_u64(right_binding.class)?,
                state
            ])
            .map_err(io::Error::other)?;
            meter.statement(&save);
        }
        meter.fences += 1;
        if !closure(&tx, job.id).map_err(io::Error::other)? {
            return Ok(meter.result(Outcome::Conflict, work, None));
        }
        if work.metadata_rows > ROWS || work.metadata_bytes > BYTES {
            return Err(io::Error::other("native pair exceeds metadata work"));
        }
        if cancelled() {
            return Ok(meter.result(Outcome::Deferred, work, None));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(meter.result(
            if cursor.compare.result.is_some() {
                Outcome::Written
            } else {
                Outcome::Deferred
            },
            work,
            cursor.compare.result,
        ))
    }
}

/// Called inside NativeRetirement's already fenced transaction, before classes
/// or lexical evidence disappear. Both endpoint indexes are bounded point-prefix
/// seeks; a committed deletion batch is its own durable restart checkpoint.
pub(super) fn retire(db: &Connection, obsolete: &[u8], work: &mut RepairWork) -> io::Result<bool> {
    let mut schema = db
        .prepare("SELECT EXISTS(SELECT 1 FROM pragma_table_info('native_original_comparisons'))")
        .map_err(io::Error::other)?;
    let exists: bool = schema
        .query_row([], |row| row.get(0))
        .map_err(io::Error::other)?;
    work.cleanup_vm_steps += schema.get_status(rusqlite::StatementStatus::VmStep);
    work.metadata_rows += 1;
    work.metadata_bytes += 8;
    if !exists {
        return Ok(false);
    }
    let remaining = ROWS.saturating_sub(work.metadata_rows) / 2;
    let count = remaining.min(30);
    if count == 0 {
        return Err(io::Error::other(
            "native pair retirement lacks bounded row allowance",
        ));
    }
    let mut keys = Vec::new();
    for (sql, left) in [
        (
            "SELECT left_class,right_original,right_class FROM native_original_comparisons WHERE left_original=?1 ORDER BY left_class,right_original,right_class LIMIT ?2",
            true,
        ),
        (
            "SELECT right_class,left_original,left_class FROM native_original_comparisons INDEXED BY native_original_comparison_right WHERE right_original=?1 ORDER BY right_class,left_original,left_class LIMIT ?2",
            false,
        ),
    ] {
        let mut query = db.prepare(sql).map_err(io::Error::other)?;
        let mut rows = query
            .query(params![obsolete, count as i64])
            .map_err(io::Error::other)?;
        while let Some(row) = rows.next().map_err(io::Error::other)? {
            let other = row
                .get_ref(1)
                .map_err(io::Error::other)?
                .as_blob()
                .map_err(io::Error::other)?;
            if other.len() != 32 {
                return Err(io::Error::other(
                    "native pair retirement endpoint disagrees",
                ));
            }
            let first: i64 = row.get(0).map_err(io::Error::other)?;
            let last: i64 = row.get(2).map_err(io::Error::other)?;
            keys.push((left, first, other.to_vec(), last));
            work.metadata_rows += 1;
            work.metadata_bytes += 48 + 80;
        }
        drop(rows);
        work.cleanup_vm_steps += query.get_status(rusqlite::StatementStatus::VmStep);
        if !keys.is_empty() {
            break;
        }
    }
    work.cleanup_key_bytes = work
        .cleanup_key_bytes
        .max(keys.len() * (std::mem::size_of::<(bool, i64, Vec<u8>, i64)>() + 32));
    let mut remove = db.prepare("DELETE FROM native_original_comparisons WHERE left_original=?1 AND left_class=?2 AND right_original=?3 AND right_class=?4").map_err(io::Error::other)?;
    for (left, first, other, last) in &keys {
        let (lo, lc, ro, rc) = if *left {
            (obsolete, *first, other.as_slice(), *last)
        } else {
            (other.as_slice(), *last, obsolete, *first)
        };
        remove
            .execute(params![lo, lc, ro, rc])
            .map_err(io::Error::other)?;
        work.metadata_rows += 1;
    }
    work.cleanup_vm_steps += remove.get_status(rusqlite::StatementStatus::VmStep);
    if work.metadata_rows > ROWS || work.metadata_bytes > BYTES {
        return Err(io::Error::other(
            "native pair retirement exceeds metadata work",
        ));
    }
    Ok(!keys.is_empty())
}

pub(super) fn has_rows(db: &Connection, obsolete: &[u8]) -> io::Result<bool> {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('native_original_comparisons'))",
            [],
            |row| row.get(0),
        )
        .map_err(io::Error::other)?;
    if !exists {
        return Ok(false);
    }
    db.query_row("SELECT EXISTS(SELECT 1 FROM native_original_comparisons WHERE left_original=?1 UNION ALL SELECT 1 FROM native_original_comparisons WHERE right_original=?1)",params![obsolete],|row|row.get(0)).map_err(io::Error::other)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::cell::Cell;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(in super::super) struct Fixture {
        root: PathBuf,
        pub(in super::super) options: crate::Options,
        pub(in super::super) store: NativeStore,
        pub(in super::super) job: Job,
        pub(in super::super) witnesses: Vec<SourceWitness>,
        rows: Vec<CatalogRow>,
        pub(in super::super) sources: Arc<crate::query_source::Sources>,
        pub(in super::super) bytes: Vec<Vec<u8>>,
        pub(in super::super) repairs: Vec<Repair>,
        texts: Vec<Option<OriginalText>>,
    }
    impl Fixture {
        pub(in super::super) fn new(encoded: &[String]) -> Self {
            Self::with_overlapping_roots(encoded, false)
        }
        fn with_overlapping_roots(encoded: &[String], overlap: bool) -> Self {
            let root = std::env::temp_dir().join(format!(
                "semon-original-compare-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let mut options = crate::Options {
                claude_home: root.join("claude"),
                claude_json: root.join("claude.json"),
                codex_home: root.join("codex"),
                copilot_home: root.join("copilot"),
                cache: root.join("cache"),
                ..crate::Options::default()
            };
            if overlap {
                options.codex_home = options.claude_home.join("projects");
            }
            let (mut writer, _) = SqliteStore::attempt_scoped(
                &super::super::super::super::super::EventCache::path(&options.cache),
                None,
                true,
            )
            .unwrap_or_else(|error| panic!("comparison fixture store must open: {error}"));
            let mut rows = Vec::new();
            let mut proofs = Vec::new();
            let mut bytes = Vec::new();
            for (i, encoded) in encoded.iter().enumerate() {
                let native = format!("{{\"s\":{encoded}}}\n").into_bytes();
                let id = format!("owner-{i}");
                let path = options.claude_home.join(format!("projects/p/{id}.jsonl"));
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, &native).unwrap();
                let captured = crate::source_chunks::capture(
                    &mut crate::sealed::LogFile::open(&path).unwrap(),
                    native.len() as u64,
                    0,
                )
                .unwrap();
                let ledger = Ledger {
                    stat: Stat::of(&path).unwrap(),
                    offset: native.len() as u64,
                    prefix: captured.prefix,
                    tail: captured.tail,
                };
                let chunks = Arc::new(captured.chunks);
                let source = ledger.source(&path, &id);
                let mut index = FileIndex {
                    source_chunks: Some(chunks.clone()),
                    ..Default::default()
                };
                if let Ok(value) = serde_json::from_slice::<Value>(&native) {
                    super::super::super::super::super::claude(&mut index, &value, 0);
                } else {
                    super::super::super::super::super::gap(&mut index, 0);
                }
                assert_eq!(
                    writer
                        .write_one(path.to_str().unwrap(), None, None, &ledger, &index)
                        .unwrap(),
                    Outcome::Written
                );
                let row:CatalogRow = serde_json::from_value(serde_json::json!({"key":id,"name":id,"harness":"claude","kind":"session","native_ids":[id],"parent":null,"parent_source":null,"repo":null,"branch":null,"model":null,"effort":null,"start":null,"last":null,"tokens":[0.0,0.0,0.0],"cost":crate::pricing::Cost::default(),"sources":[source]})).unwrap();
                writer.connection.execute("INSERT INTO session_catalog(session_key,last_ms,harness,metadata) VALUES(?1,0,'claude',?2)",params![row.key,json(&row).unwrap()]).unwrap();
                proofs.push((source, Some(chunks)));
                rows.push(row);
                bytes.push(native);
            }
            writer.connection.execute_batch(SCHEMA).unwrap();
            writer
                .connection
                .execute_batch(
                    "CREATE TABLE comparison_required_owners(key TEXT PRIMARY KEY) WITHOUT ROWID;",
                )
                .unwrap();
            for row in &rows {
                writer
                    .connection
                    .execute(
                        "INSERT INTO comparison_required_owners VALUES(?1)",
                        [&row.key],
                    )
                    .unwrap();
            }
            let sources = Arc::new(
                crate::query_source::Sources::capture(&options, proofs.into_iter()).unwrap(),
            );
            let mut store = NativeStore {
                connection: writer.connection,
                options: options.clone(),
            };
            let witnesses: Vec<_> = rows
                .iter()
                .map(|row| store.source_witness(&row.sources[0]).unwrap())
                .collect();
            let owners: Vec<_> = rows
                .iter()
                .map(|row| store.owner_witness(row).unwrap())
                .collect();
            let spec = JobSpec {
                view_key: "original-comparison-fixture".into(),
                version: 1,
                catalog_generation: None,
                runtime: crate::native_evidence::query_runtime_generation(&options).unwrap(),
            };
            let job = store.begin(&spec).unwrap();
            assert_eq!(
                store
                    .admit(&job, &witnesses, &owners, "all-owner-closure")
                    .unwrap(),
                Outcome::Written
            );
            let job = store.reload(&spec.view_key).unwrap().unwrap();
            let mut fixture = Self {
                root,
                options,
                store,
                job,
                witnesses,
                rows,
                sources,
                bytes,
                repairs: Vec::new(),
                texts: Vec::new(),
            };
            for i in 0..encoded.len() {
                fixture.prepare_original(i);
            }
            fixture
        }
        fn prepare_original(&mut self, i: usize) {
            let mut repair = self
                .store
                .begin_original_repair(
                    &self.job,
                    &self.witnesses[i],
                    &self.sources.reader(),
                    &|| false,
                )
                .unwrap()
                .unwrap();
            let mut attempts = 0;
            while repair.cursor.phase != Phase::FoldRecord {
                let step = self
                    .store
                    .original_lex_step(
                        &self.job,
                        &self.witnesses[i],
                        &repair,
                        &self.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                assert!(step.work.metadata_rows <= ROWS);
                repair = self
                    .store
                    .reload_original_repair(&self.job, &self.witnesses[i], &self.sources.reader())
                    .unwrap()
                    .unwrap();
                attempts += 1;
                assert!(attempts < 1024);
            }
            loop {
                let step = self
                    .store
                    .original_fields_step(
                        &self.job,
                        &self.witnesses[i],
                        &repair,
                        &self.sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                repair = self
                    .store
                    .reload_original_repair(&self.job, &self.witnesses[i], &self.sources.reader())
                    .unwrap()
                    .unwrap();
                attempts += 1;
                assert!(attempts < 2048);
                if step.outcome == Outcome::Written {
                    break;
                }
            }
            let text = if let Ok(Some(root)) =
                self.store
                    .original_root(&self.job, &self.witnesses[i], &repair)
            {
                let node = loop {
                    let step = self
                        .store
                        .original_member_step(
                            &self.job,
                            &self.witnesses[i],
                            &repair,
                            root.address,
                            "s",
                            &self.sources.reader(),
                            &|| false,
                        )
                        .unwrap();
                    if step.complete {
                        break step.node.unwrap();
                    }
                    attempts += 1;
                    assert!(attempts < 4096);
                };
                loop {
                    let step = self
                        .store
                        .original_identity_step(
                            &self.job,
                            &self.witnesses[i],
                            &repair,
                            node.ordinal,
                            &self.sources.reader(),
                            &|| false,
                        )
                        .unwrap();
                    if let Some(text) = step.text {
                        break Some(text);
                    }
                    attempts += 1;
                    assert!(attempts < 8192);
                }
            } else {
                None
            };
            self.repairs.push(repair);
            self.texts.push(text);
        }
        fn closure(db: &Connection, job: i64) -> rusqlite::Result<bool> {
            db.query_row("SELECT NOT EXISTS(SELECT 1 FROM comparison_required_owners r LEFT JOIN native_query_owners w ON w.job=?1 AND w.session_key=r.key WHERE w.session_key IS NULL)",[job],|row|row.get(0))
        }
        fn once(
            &mut self,
            left: usize,
            right: usize,
            cancel: &dyn Fn() -> bool,
        ) -> io::Result<TextComparisonStep> {
            let lrecords = self.sources.reader();
            let rrecords = self.sources.reader();
            self.store.original_compare_step(
                &self.job,
                ComparisonSource {
                    witness: &self.witnesses[left],
                    repair: &self.repairs[left],
                    text: self.texts[left].as_ref().unwrap(),
                    records: &lrecords,
                },
                ComparisonSource {
                    witness: &self.witnesses[right],
                    repair: &self.repairs[right],
                    text: self.texts[right].as_ref().unwrap(),
                    records: &rrecords,
                },
                &Self::closure,
                cancel,
            )
        }
        fn provided(
            &mut self,
            left: &dyn crate::SessionSourceReader,
            right: &dyn crate::SessionSourceReader,
        ) -> io::Result<TextComparisonStep> {
            let lrecords = self
                .sources
                .reader_with(Some((left, crate::SessionSourceReadMode::ProviderOnly)));
            let rrecords = self
                .sources
                .reader_with(Some((right, crate::SessionSourceReadMode::ProviderOnly)));
            self.store.original_compare_step(
                &self.job,
                ComparisonSource {
                    witness: &self.witnesses[0],
                    repair: &self.repairs[0],
                    text: self.texts[0].as_ref().unwrap(),
                    records: &lrecords,
                },
                ComparisonSource {
                    witness: &self.witnesses[1],
                    repair: &self.repairs[1],
                    text: self.texts[1].as_ref().unwrap(),
                    records: &rrecords,
                },
                &Self::closure,
                &|| false,
            )
        }

        fn complete(&mut self, left: usize, right: usize) -> Comparison {
            let reads = super::super::super::super::super::FILE_READS.with(Cell::get);
            for _ in 0..1024 {
                let step = self.once(left, right, &|| false).unwrap();
                assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                assert!(step.comparison_sql_vm_steps < 1024);
                assert!(step.serialized_cursor_bytes < 4096);
                assert!(step.fence_checks <= 32);
                assert_eq!(step.records_finish_calls, 2);
                assert!(step.native_fragment_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                assert_eq!(step.decoded_buffer_bound, 96 * 1024);
                self.store = NativeStore::open(&self.options).unwrap();
                if let Some(result) = step.comparison {
                    assert_eq!(
                        super::super::super::super::super::FILE_READS.with(Cell::get),
                        reads
                    );
                    return result;
                }
                assert_eq!(step.outcome, Outcome::Deferred);
            }
            panic!("bounded native comparison did not make eventual progress")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn quoted(value: &str) -> String {
        serde_json::to_string(value).unwrap()
    }

    pub(in super::super) struct Provider {
        source: crate::SessionSourceRef,
        bytes: Arc<[u8]>,
        calls: AtomicUsize,
        read_bytes: AtomicUsize,
        change_generation_after: Option<usize>,
        on_read: Box<dyn Fn(usize) + Send + Sync>,
    }
    impl Provider {
        pub(in super::super) fn observed(&self) -> (usize, usize) {
            (
                self.calls.load(Ordering::SeqCst),
                self.read_bytes.load(Ordering::SeqCst),
            )
        }
        pub(in super::super) fn new(
            source: crate::SessionSourceRef,
            bytes: &[u8],
            change_generation_after: Option<usize>,
            on_read: impl Fn(usize) + Send + Sync + 'static,
        ) -> Self {
            Self {
                source,
                bytes: Arc::from(bytes),
                calls: AtomicUsize::new(0),
                read_bytes: AtomicUsize::new(0),
                change_generation_after,
                on_read: Box::new(on_read),
            }
        }
    }
    impl crate::SessionSourceReader for Provider {
        fn read_range(
            &self,
            source: &crate::SessionSourceRef,
            expected: Option<&str>,
            offset: u64,
            maximum: usize,
        ) -> io::Result<crate::SessionSourceRange> {
            if source != &self.source {
                return Err(io::Error::other("test provider source changed"));
            }
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            (self.on_read)(call);
            let generation = if self.change_generation_after.is_some_and(|at| call > at) {
                "provider-2"
            } else {
                "provider-1"
            };
            if expected.is_some_and(|pin| pin != generation) {
                return Err(io::Error::other("test provider generation changed"));
            }
            let start = usize::try_from(offset).map_err(io::Error::other)?;
            if start >= self.bytes.len() || maximum == 0 {
                return Err(io::Error::other("test provider range unavailable"));
            }
            let end = (start + maximum).min(self.bytes.len());
            self.read_bytes.fetch_add(end - start, Ordering::SeqCst);
            Ok(crate::SessionSourceRange {
                generation: generation.into(),
                length: self.bytes.len() as u64,
                offset,
                bytes: Arc::from(&self.bytes[start..end]),
                cached: false,
            })
        }
    }

    #[test]
    fn original_pair_rejects_two_root_relative_originals_for_one_admitted_path_before_io() {
        let mut fixture = Fixture::with_overlapping_roots(&[quoted("same"), quoted("same")], true);
        assert_eq!(
            fixture.options.codex_home,
            fixture.options.claude_home.join("projects")
        );
        assert!(
            fixture.witnesses[0]
                .source
                .path
                .starts_with(&fixture.options.claude_home)
        );
        assert!(
            fixture.witnesses[0]
                .source
                .path
                .starts_with(&fixture.options.codex_home)
        );
        // Both legitimate cases retain the ordinary native equality result.
        assert_eq!(fixture.complete(0, 0), Comparison::Equal);
        assert_eq!(fixture.complete(0, 1), Comparison::Equal);
        assert_ne!(
            fixture.witnesses[0].source.path,
            fixture.witnesses[1].source.path
        );
        let left_sources = Arc::clone(&fixture.sources);
        let source = fixture.witnesses[0].source.clone();
        let captured = crate::source_chunks::capture(
            &mut crate::sealed::LogFile::open(&source.path).unwrap(),
            source.offset,
            0,
        )
        .unwrap();
        // A separately captured request resolves the same native file through
        // the other configured overlapping root. No reference/namespace is
        // fabricated: capture and the regular producer qualify both forms.
        let mut alternate = fixture.options.clone();
        alternate.claude_home = fixture.root.join("unselected-claude-root");
        fixture.sources = Arc::new(
            crate::query_source::Sources::capture(
                &alternate,
                std::iter::once((source, Some(Arc::new(captured.chunks)))),
            )
            .unwrap(),
        );
        fixture.prepare_original(0);
        let left_records = left_sources.reader();
        let right_records = fixture.sources.reader();
        let left_ref = reference(&left_records, &fixture.witnesses[0]).unwrap();
        let right_ref = reference(&right_records, &fixture.witnesses[0]).unwrap();
        assert_eq!(left_ref.root, "claude");
        assert_eq!(right_ref.root, "codex");
        assert_eq!(left_ref.path, "projects/p/owner-0.jsonl");
        assert_eq!(right_ref.path, "p/owner-0.jsonl");
        assert_ne!(fixture.repairs[0].original, fixture.repairs[2].original);
        for index in [0, 2] {
            assert!(repair_current(&fixture.store.connection, &fixture.repairs[index]).unwrap());
            assert!(
                admitted_original(
                    &fixture.store.connection,
                    &fixture.job,
                    &fixture.witnesses[0]
                )
                .unwrap()
            );
        }
        let left_provider = Provider::new(left_ref, &fixture.bytes[0], None, |_| {});
        let right_provider = Provider::new(right_ref, &fixture.bytes[0], None, |_| {});
        let left_records = left_sources.reader_with(Some((
            &left_provider,
            crate::SessionSourceReadMode::ProviderOnly,
        )));
        let right_records = fixture.sources.reader_with(Some((
            &right_provider,
            crate::SessionSourceReadMode::ProviderOnly,
        )));
        let before: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_original_comparisons",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let step = fixture
            .store
            .original_compare_step(
                &fixture.job,
                ComparisonSource {
                    witness: &fixture.witnesses[0],
                    repair: &fixture.repairs[0],
                    text: fixture.texts[0].as_ref().unwrap(),
                    records: &left_records,
                },
                ComparisonSource {
                    witness: &fixture.witnesses[0],
                    repair: &fixture.repairs[2],
                    text: fixture.texts[2].as_ref().unwrap(),
                    records: &right_records,
                },
                &Fixture::closure,
                &|| false,
            )
            .unwrap();
        assert_eq!(step.outcome, Outcome::Conflict);
        assert!(step.comparison.is_none());
        assert_eq!(step.work.native_bytes, 0);
        assert_eq!(step.work.range_requests, 0);
        assert_eq!(step.records_finish_calls, 0);
        assert_eq!(left_provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(right_provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(left_provider.read_bytes.load(Ordering::SeqCst), 0);
        assert_eq!(right_provider.read_bytes.load(Ordering::SeqCst), 0);
        let after: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_original_comparisons",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after, before, "refusal installed pair metadata");
    }

    #[test]
    fn original_pair_matches_independent_native_strings_across_namespaces_and_escapes() {
        let cases = [
            (quoted(""), quoted("")),
            (quoted("native"), "\"\\u006eative\"".into()),
            (
                quoted("é𝄞😀"),
                "\"\\u00e9\\ud834\\udd1e\\ud83d\\ude00\"".into(),
            ),
            (quoted("same"), quoted("diff")),
            (quoted("short"), quoted("longer")),
        ];
        for (l, r) in cases {
            let left: String = serde_json::from_str(&l).unwrap();
            let right: String = serde_json::from_str(&r).unwrap();
            let mut fixture = Fixture::new(&[l, r, quoted("third related owner")]);
            assert_eq!(fixture.texts[0].as_ref().unwrap().identity(), 1);
            assert_eq!(fixture.texts[1].as_ref().unwrap().identity(), 1);
            assert_ne!(
                fixture.texts[0].as_ref().unwrap().original(),
                fixture.texts[1].as_ref().unwrap().original()
            );
            assert_eq!(
                fixture.complete(0, 1),
                if left == right {
                    Comparison::Equal
                } else {
                    Comparison::Different
                }
            );
            let cached = fixture.once(0, 1, &|| false).unwrap();
            assert_eq!(cached.work.native_bytes, 2);
            assert_eq!(cached.work.range_requests, 2);
            assert_eq!(
                cached.comparison,
                Some(if left == right {
                    Comparison::Equal
                } else {
                    Comparison::Different
                })
            );
            assert_eq!(fixture.complete(0, 0), Comparison::Equal);
        }
    }

    #[test]
    fn original_pair_crosses_native_and_proof_boundaries_and_refuses_digest_collision_equality() {
        let long = format!("{}tail", "aé𝄞".repeat(40_000));
        let escaped = format!("\"{}tail\"", "\\u0061\\u00e9\\ud834\\udd1e".repeat(40_000));
        let mut fixture = Fixture::new(&[
            quoted(&long),
            escaped,
            quoted(&format!("{}fail", "aé𝄞".repeat(40_000))),
        ]);
        assert_eq!(fixture.complete(0, 1), Comparison::Equal);
        // Force both field receipts to the same digest and decoded length. It
        // may nominate the pair, but must not decide decoded native equality.
        let (digest,len):(Vec<u8>,i64)=fixture.store.connection.query_row("SELECT digest,decoded_bytes FROM native_original_fields WHERE original=?1 AND ordinal=(SELECT ordinal FROM native_original_classes WHERE original=?1 AND class=1)",params![fixture.repairs[0].original.as_slice()],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
        fixture.store.connection.execute("UPDATE native_original_fields SET digest=?2,decoded_bytes=?3 WHERE original=?1 AND ordinal=(SELECT ordinal FROM native_original_classes WHERE original=?1 AND class=1)",params![fixture.repairs[2].original.as_slice(),digest,len]).unwrap();
        assert_eq!(fixture.complete(0, 2), Comparison::Different);
        for (i, bytes) in fixture.bytes.iter().enumerate() {
            assert_eq!(fs::read(&fixture.witnesses[i].source.path).unwrap(), *bytes);
        }
    }

    #[test]
    fn original_pair_cancellation_cas_and_cached_span_validation_preserve_committed_progress() {
        let text = quoted(&"x".repeat(200_000));
        let mut fixture = Fixture::new(&[text.clone(), text]);
        let calls = Cell::new(0);
        let cancel = || {
            calls.set(calls.get() + 1);
            calls.get() == 3
        };
        assert_eq!(
            fixture.once(0, 1, &cancel).unwrap().outcome,
            Outcome::Deferred
        );
        let count: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_original_comparisons",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        let committed = fixture.once(0, 1, &|| false).unwrap();
        assert_eq!(committed.outcome, Outcome::Deferred);
        let state: String = fixture
            .store
            .connection
            .query_row(
                "SELECT cursor FROM native_original_comparisons",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fixture.once(0, 1, &|| true).unwrap().work.native_bytes, 0);
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let unchanged: String = fixture
            .store
            .connection
            .query_row(
                "SELECT cursor FROM native_original_comparisons",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unchanged, state);
        assert_eq!(fixture.complete(0, 1), Comparison::Equal);
        let mut cursor: Value = serde_json::from_str(
            &fixture
                .store
                .connection
                .query_row::<String, _, _>(
                    "SELECT cursor FROM native_original_comparisons",
                    [],
                    |row| row.get(0),
                )
                .unwrap(),
        )
        .unwrap();
        cursor["compare"]["left"]["end"] = cursor["compare"]["left"]["start"].clone();
        cursor["compare"]["left"]["at"] = cursor["compare"]["left"]["start"].clone();
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_original_comparisons SET cursor=?1",
                [cursor.to_string()],
            )
            .unwrap();
        assert!(
            fixture
                .once(0, 1, &|| false)
                .unwrap_err()
                .to_string()
                .contains("descriptor changed")
        );
    }

    #[test]
    fn original_pair_provider_receipts_reauthorize_both_sides_and_measure_proof_work() {
        for (l, r) in [
            (quoted(""), quoted("")),
            (quoted("same"), quoted("diff")),
            (quoted("short"), quoted("longer")),
        ] {
            let mut fixture = Fixture::new(&[l, r]);
            let expected = fixture.complete(0, 1);
            for witness in &fixture.witnesses {
                fs::remove_file(&witness.source.path).unwrap();
            }
            let left = Provider::new(
                fixture.repairs[0].source.clone(),
                &fixture.bytes[0],
                None,
                |_| {},
            );
            let right = Provider::new(
                fixture.repairs[1].source.clone(),
                &fixture.bytes[1],
                None,
                |_| {},
            );
            let reads = super::super::super::super::super::FILE_READS.with(Cell::get);
            let cached = fixture.provided(&left, &right).unwrap();
            assert_eq!(cached.comparison, Some(expected));
            assert_eq!(cached.work.native_bytes, 2);
            assert_eq!(cached.work.range_requests, 2);
            assert_eq!(cached.records_finish_calls, 2);
            assert_eq!(cached.native_fragment_bytes, 1);
            assert_eq!(left.calls.load(Ordering::SeqCst), 2);
            assert_eq!(right.calls.load(Ordering::SeqCst), 2);
            assert_eq!(
                left.read_bytes.load(Ordering::SeqCst),
                fixture.bytes[0].len() * 2
            );
            assert_eq!(
                right.read_bytes.load(Ordering::SeqCst),
                fixture.bytes[1].len() * 2
            );
            assert_eq!(
                super::super::super::super::super::FILE_READS.with(Cell::get),
                reads
            );
        }
        let text = quoted(&"x".repeat(200_000));
        let mut fixture = Fixture::new(&[text.clone(), text]);
        let left = Provider::new(
            fixture.repairs[0].source.clone(),
            &fixture.bytes[0],
            None,
            |_| {},
        );
        let right = Provider::new(
            fixture.repairs[1].source.clone(),
            &fixture.bytes[1],
            None,
            |_| {},
        );
        let step = fixture.provided(&left, &right).unwrap();
        assert_eq!(step.outcome, Outcome::Deferred);
        assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
        assert_eq!(
            (
                left.calls.load(Ordering::SeqCst),
                right.calls.load(Ordering::SeqCst)
            ),
            (2, 2)
        );
        assert_eq!(
            left.read_bytes.load(Ordering::SeqCst) + right.read_bytes.load(Ordering::SeqCst),
            4 * crate::source_chunks::SIZE
        );
        assert!(step.work.native_bytes < 4 * crate::source_chunks::SIZE);
    }

    #[test]
    fn original_pair_refuses_second_third_new_owner_source_runtime_and_receipt_changes() {
        for change in [
            "second", "third", "new", "source", "runtime", "field", "provider",
        ] {
            let mut fixture = Fixture::new(&[quoted("same"), quoted("same"), quoted("related")]);
            let cache = super::super::super::super::super::EventCache::path(&fixture.options.cache);
            let runtime = fixture.options.claude_json.clone();
            let path = fixture.witnesses[1]
                .source
                .path
                .to_string_lossy()
                .into_owned();
            let original = fixture.repairs[1].original;
            let left = Provider::new(
                fixture.repairs[0].source.clone(),
                &fixture.bytes[0],
                None,
                |_| {},
            );
            let right = Provider::new(
                fixture.repairs[1].source.clone(),
                &fixture.bytes[1],
                (change == "provider").then_some(0),
                move |call| {
                    if call != 0 {
                        return;
                    }
                    if change == "runtime" {
                        fs::write(&runtime, b"{\"changed\":true}").unwrap();
                        return;
                    }
                    let db = Connection::open(&cache).unwrap();
                    match change {
                        "second" => {
                            db.execute("UPDATE session_catalog SET metadata=metadata WHERE session_key='owner-1'",[]).unwrap();
                        }
                        "third" => {
                            db.execute("UPDATE session_catalog SET metadata=metadata WHERE session_key='owner-2'",[]).unwrap();
                        }
                        "new" => {
                            db.execute("INSERT INTO comparison_required_owners VALUES('new-related-owner')",[]).unwrap();
                        }
                        "source" => {
                            db.execute("INSERT INTO session_catalog_invalidations VALUES(?1,1,'changed',0)",[&path]).unwrap();
                        }
                        "field" => {
                            db.execute("UPDATE native_original_fields SET decoded_bytes=decoded_bytes+1 WHERE original=?1 AND is_key=0",params![original.as_slice()]).unwrap();
                        }
                        _ => {}
                    }
                },
            );
            let result = fixture.provided(&left, &right);
            if change == "provider" {
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap().outcome, Outcome::Conflict);
            }
            let count: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*) FROM native_original_comparisons",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[test]
    fn original_pair_competing_commit_is_cas_conflict_and_new_qualified_job_resumes() {
        let text = quoted(&"a".repeat(200_000));
        let mut fixture = Fixture::new(&[text.clone(), text]);
        let options = fixture.options.clone();
        let job = fixture.job.clone();
        let witnesses = fixture.witnesses.clone();
        let repairs = fixture.repairs.clone();
        let texts = fixture.texts.clone();
        let sources = fixture.sources.clone();
        let left = Provider::new(
            fixture.repairs[0].source.clone(),
            &fixture.bytes[0],
            None,
            move |call| {
                if call != 0 {
                    return;
                }
                let mut other = NativeStore::open(&options).unwrap();
                let lrecords = sources.reader();
                let rrecords = sources.reader();
                let result = other
                    .original_compare_step(
                        &job,
                        ComparisonSource {
                            witness: &witnesses[0],
                            repair: &repairs[0],
                            text: texts[0].as_ref().unwrap(),
                            records: &lrecords,
                        },
                        ComparisonSource {
                            witness: &witnesses[1],
                            repair: &repairs[1],
                            text: texts[1].as_ref().unwrap(),
                            records: &rrecords,
                        },
                        &Fixture::closure,
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(result.outcome, Outcome::Deferred);
            },
        );
        let right = Provider::new(
            fixture.repairs[1].source.clone(),
            &fixture.bytes[1],
            None,
            |_| {},
        );
        assert_eq!(
            fixture.provided(&left, &right).unwrap().outcome,
            Outcome::Conflict
        );
        let revision: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT revision FROM native_original_comparisons",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(revision, 0);
        let spec = JobSpec {
            view_key: "new-qualified-comparison-job".into(),
            ..fixture.job.spec.clone()
        };
        let job = fixture.store.begin(&spec).unwrap();
        let owners: Vec<_> = fixture
            .rows
            .iter()
            .map(|row| fixture.store.owner_witness(row).unwrap())
            .collect();
        assert_eq!(
            fixture
                .store
                .admit(&job, &fixture.witnesses, &owners, "complete-new-closure")
                .unwrap(),
            Outcome::Written
        );
        fixture.job = fixture.store.reload(&spec.view_key).unwrap().unwrap();
        assert_eq!(fixture.complete(0, 1), Comparison::Equal);
    }

    #[test]
    fn original_pair_only_obsolete_cache_retires_both_endpoints_in_called_bounded_steps() {
        let mut fixture = Fixture::new(&[quoted("left"), quoted("right")]);
        let old = format_two_namespace(&fixture.repairs[0].source).unwrap();
        let right = fixture.repairs[1].original;
        {
            let tx = fixture.store.connection.transaction().unwrap();
            for i in 0..128 {
                tx.execute("INSERT INTO native_original_comparisons VALUES(?1,?2,?3,1,0,'{unknown obsolete cursor')",params![old.as_slice(),i,right.as_slice()]).unwrap();
                tx.execute("INSERT INTO native_original_comparisons VALUES(?1,1,?2,?3,0,'{unknown obsolete cursor')",params![right.as_slice(),old.as_slice(),i]).unwrap();
            }
            tx.execute(
                "INSERT INTO native_original_comparisons VALUES(?1,1,?1,1,0,'{unrelated cache')",
                params![right.as_slice()],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let mut repair = fixture
            .store
            .begin_original_repair(
                &fixture.job,
                &fixture.witnesses[0],
                &fixture.sources.reader(),
                &|| false,
            )
            .unwrap()
            .unwrap();
        let phase = repair.cursor.phase.clone();
        let mut steps = 0;
        while native_retirement::pending(&fixture.store.connection, repair.original).unwrap() {
            let step = fixture
                .store
                .original_lex_step(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Deferred);
            assert!(step.work.metadata_rows <= ROWS);
            assert!(step.work.metadata_bytes <= BYTES);
            assert!(step.work.cleanup_vm_steps < 4096);
            assert!(step.work.cleanup_key_bytes <= STATE_BYTES);
            assert_eq!(step.work.native_bytes, 0);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            repair = fixture
                .store
                .reload_original_repair(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.sources.reader(),
                )
                .unwrap()
                .unwrap();
            assert_eq!(repair.cursor.phase, phase);
            steps += 1;
            assert!(steps < 32);
        }
        assert!(steps >= 10);
        let remaining: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_original_comparisons",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn original_pair_invalid_and_unqualified_text_never_gains_native_equality() {
        let invalid = "\"\\ud800\"".to_owned();
        assert!(serde_json::from_str::<String>(&invalid).is_err());
        let mut fixture = Fixture::new(&[invalid, quoted("valid")]);
        assert!(fixture.texts[0].is_none());
        fixture.texts[0] = Some(native_events::Text::qualified(
            fixture.repairs[0].original,
            1,
        ));
        assert!(fixture.once(0, 1, &|| false).is_err());
        let count: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_original_comparisons",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn original_pair_cached_result_refuses_provider_custody_and_configured_root_revocation() {
        struct Denied;
        impl crate::SessionSourceReader for Denied {
            fn read_range(
                &self,
                _source: &crate::SessionSourceRef,
                _generation: Option<&str>,
                _offset: u64,
                _maximum: usize,
            ) -> io::Result<crate::SessionSourceRange> {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "test custody revoked",
                ))
            }
        }
        let mut fixture = Fixture::new(&[quoted("same"), quoted("same")]);
        assert_eq!(fixture.complete(0, 1), Comparison::Equal);
        let before: String = fixture
            .store
            .connection
            .query_row(
                "SELECT cursor FROM native_original_comparisons",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let left = Provider::new(
            fixture.repairs[0].source.clone(),
            &fixture.bytes[0],
            None,
            |_| {},
        );
        assert!(fixture.provided(&left, &Denied).is_err());
        let after: String = fixture
            .store
            .connection
            .query_row(
                "SELECT cursor FROM native_original_comparisons",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after, before);
        let denied =
            crate::query_source::Sources::capture(&fixture.options, std::iter::empty()).unwrap();
        let left_records = fixture.sources.reader();
        let denied_records = denied.reader();
        assert!(
            fixture
                .store
                .original_compare_step(
                    &fixture.job,
                    ComparisonSource {
                        witness: &fixture.witnesses[0],
                        repair: &fixture.repairs[0],
                        text: fixture.texts[0].as_ref().unwrap(),
                        records: &left_records
                    },
                    ComparisonSource {
                        witness: &fixture.witnesses[1],
                        repair: &fixture.repairs[1],
                        text: fixture.texts[1].as_ref().unwrap(),
                        records: &denied_records
                    },
                    &Fixture::closure,
                    &|| false
                )
                .is_err()
        );
        let mut outside = fixture.options.clone();
        outside.claude_home = fixture.root.join("unadmitted-root");
        assert!(
            crate::query_source::Sources::capture(
                &outside,
                std::iter::once((fixture.witnesses[1].source.clone(), None))
            )
            .is_err()
        );
    }

    #[test]
    fn original_pair_page_work_stays_bounded_as_unrelated_class_history_grows() {
        let mut baseline = None;
        let mut fixture = Fixture::new(&[quoted("selected"), quoted("selected")]);
        let mut previous = 0;
        for history in [0, 128, 1024, 8192] {
            {
                let tx = fixture.store.connection.transaction().unwrap();
                tx.execute("DELETE FROM native_original_comparisons", [])
                    .unwrap();
                for i in previous..history {
                    tx.execute(
                        "INSERT INTO native_original_classes VALUES(?1,?2,0,0,zeroblob(32),0)",
                        params![fixture.repairs[0].original.as_slice(), i + 2],
                    )
                    .unwrap();
                }
                tx.commit().unwrap();
            }
            previous = history;
            let reads = super::super::super::super::super::FILE_READS.with(Cell::get);
            let step = fixture.once(0, 1, &|| false).unwrap();
            assert_eq!(step.comparison, Some(Comparison::Equal));
            assert!(step.comparison_sql_vm_steps < 1024);
            assert!(step.work.metadata_rows <= ROWS);
            assert!(step.work.metadata_bytes <= BYTES);
            assert!(step.serialized_cursor_bytes < 4096);
            let fixed = (
                step.work.metadata_rows,
                step.work.metadata_bytes,
                step.work.native_bytes,
                step.serialized_cursor_bytes,
                step.native_fragment_bytes,
            );
            assert_eq!(*baseline.get_or_insert(fixed), fixed);
            assert_eq!(
                super::super::super::super::super::FILE_READS.with(Cell::get),
                reads
            );
        }
    }
}
