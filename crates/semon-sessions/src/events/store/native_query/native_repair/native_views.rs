//! Qualified native text views. Parts are bounded pages of native spans, never
//! stored text. All transformations use the complete native producer's decoder.
use super::*;
use crate::json_string::native_identity::DecodedFingerprint;
use crate::json_string::native_js::{Quote, Unit};

pub(in super::super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_original_text_epochs (
 original BLOB NOT NULL, record_offset INTEGER NOT NULL, revision INTEGER NOT NULL,
 PRIMARY KEY(original,record_offset)
) WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS native_original_text_field_change
 AFTER UPDATE OF metadata,digest,decoded_bytes ON native_original_fields
 WHEN OLD.digest IS NOT NULL AND (NEW.metadata IS NOT OLD.metadata
 OR NEW.digest IS NOT OLD.digest OR NEW.decoded_bytes IS NOT OLD.decoded_bytes)
 BEGIN INSERT INTO native_original_text_epochs VALUES(OLD.original,OLD.record_offset,1)
 ON CONFLICT(original,record_offset) DO UPDATE SET revision=revision+1; END;
CREATE TABLE IF NOT EXISTS native_original_text_views (
 original BLOB NOT NULL, view_id INTEGER NOT NULL, revision INTEGER NOT NULL,
 request TEXT NOT NULL, cursor TEXT NOT NULL, receipt TEXT,
 PRIMARY KEY(original,view_id), UNIQUE(original,request)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_text_parts (
 original BLOB NOT NULL, view_id INTEGER NOT NULL, page INTEGER NOT NULL,
 metadata TEXT NOT NULL, PRIMARY KEY(original,view_id,page)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_view_identities (
 original BLOB NOT NULL, view_id INTEGER NOT NULL, revision INTEGER NOT NULL,
 cursor TEXT NOT NULL, PRIMARY KEY(original,view_id)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_text_boundaries (
 original BLOB NOT NULL, view_id INTEGER NOT NULL, decoded_at INTEGER NOT NULL,
 metadata TEXT NOT NULL, PRIMARY KEY(original,view_id,decoded_at)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_class_values (
 original BLOB NOT NULL, class INTEGER NOT NULL, view_id INTEGER,
 PRIMARY KEY(original,class)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_class_literals (
 original BLOB NOT NULL, class INTEGER NOT NULL, literal TEXT NOT NULL,
 revision INTEGER NOT NULL, cursor TEXT NOT NULL,
 PRIMARY KEY(original,class,literal)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS native_original_text_policy (
 original BLOB PRIMARY KEY, policy INTEGER NOT NULL, legacy_through INTEGER NOT NULL,
 backfilled_through INTEGER NOT NULL
) WITHOUT ROWID;
";
const POLICY: u32 = 1;
const PAGE_PARTS: usize = 8;
const CHUNK: usize = 32 * 1024;
// A SHA block may retain 63 source-backed scalar references. Each replay and
// the next scalar can require 72 raw bytes through the layered native decoder.
// Keep at least one new scalar's allowance after the worst pending replay.
pub(crate) const TEXT_VIEW_BUILD_NATIVE_MIN: usize = 64 * 72;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QualifiedTextView {
    original: [u8; 32],
    id: u64,
    revision: u64,
    record: u64,
    epoch: u64,
    pages: u64,
    receipt: Receipt,
}
impl QualifiedTextView {
    pub(crate) fn original(&self) -> [u8; 32] {
        self.original
    }
    pub(super) fn id(&self) -> u64 {
        self.id
    }
    pub(super) fn record(&self) -> u64 {
        self.record
    }
    pub(crate) fn receipt(&self) -> Receipt {
        self.receipt
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TextBoundary {
    original: [u8; 32],
    view: u64,
    revision: u64,
    epoch: u64,
    decoded: u64,
}
impl TextBoundary {
    pub(crate) fn decoded(&self) -> u64 {
        self.decoded
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TextScanCursor {
    view: QualifiedTextView,
    position: ReadCursor,
}
impl TextScanCursor {
    pub(crate) fn begin(view: &QualifiedTextView) -> Self {
        Self {
            view: view.clone(),
            position: ReadCursor::default(),
        }
    }
    pub(crate) fn decoded(&self) -> u64 {
        self.position.decoded
    }
}
pub(crate) struct TextScanStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) cursor: TextScanCursor,
    pub(crate) bytes: Vec<u8>,
    pub(crate) marks: Vec<TextBoundary>,
    pub(crate) complete: bool,
    pub(crate) sql_vm_steps: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum ViewRequest {
    WholeField {
        record: u64,
        ordinal: u64,
    },
    Slice {
        parent: QualifiedTextView,
        start: u64,
        end: u64,
    },
    SliceMarked {
        parent: QualifiedTextView,
        start: TextBoundary,
        end: TextBoundary,
    },
    JsonStringDecode {
        parent: QualifiedTextView,
    },
    JsLiteralDecode {
        parent: QualifiedTextView,
        quote: Quote,
    },
    /// The native producer supplies parent views one at a time. `count` and
    /// the exact membership closure are fenced for every durable append.
    Join {
        record: u64,
        producer: u64,
        count: u64,
        newline: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Decode {
    Json(u8),
    JavaScript(Quote),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Part {
    Native {
        ordinal: u64,
        start: u64,
        end: u64,
        decode: Decode,
    },
    Newline,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartPage {
    parts: Vec<Part>,
}
impl PartPage {
    fn valid(&self, source_end: u64) -> bool {
        self.parts.len() <= PAGE_PARTS
            && self.parts.iter().all(|part| match part {
                Part::Native {
                    start, end, decode, ..
                } => {
                    start <= end
                        && *end <= source_end
                        && !matches!(decode, Decode::Json(layers) if !matches!(*layers, 1 | 2))
                }
                Part::Newline => true,
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadCursor {
    page: u64,
    part: usize,
    at: Option<u64>,
    skip: u8,
    pub(super) decoded: u64,
}
impl ReadCursor {
    pub(super) fn valid_for(&self, view: &QualifiedTextView) -> bool {
        self.page <= view.pages
            && self.part <= PAGE_PARTS
            && self.skip <= 3
            && self.decoded <= view.receipt.decoded_bytes
            && (self.page != view.pages || self.part == 0 && self.at.is_none() && self.skip == 0)
    }
    pub(super) fn complete_for(&self, view: &QualifiedTextView) -> bool {
        self.valid_for(view)
            && self.page == view.pages
            && self.decoded == view.receipt.decoded_bytes
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum BuildPhase {
    Parts,
    Hash,
    Complete,
    Invalid,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Build {
    policy: u32,
    #[serde(default)]
    ready_revision: u64,
    record: u64,
    epoch: u64,
    phase: BuildPhase,
    pages: u64,
    reader: ReadCursor,
    fingerprint: DecodedFingerprint,
    pending: Vec<ReadRef>,
    parent_index: u64,
    parent_page: u64,
    // A selected contiguous run can cross many foreground chunks. Numeric
    // bounds persist until the part ends; no prefix or tail enters the cursor.
    selected: Option<Part>,
    first: Option<char>,
    last: Option<char>,
}
#[derive(Debug)]
pub(crate) struct ViewStep {
    pub(crate) outcome: Outcome,
    pub(crate) work: RepairWork,
    pub(crate) view: Option<QualifiedTextView>,
    pub(crate) complete: bool,
    pub(crate) sql_vm_steps: i32,
    pub(crate) leaf_parts: usize,
    pub(crate) decoded_bytes: usize,
    pub(crate) serialized_cursor_bytes: usize,
}
#[derive(Default)]
pub(super) struct ViewMeter {
    pub(super) vm: i32,
    pub(super) leaves: usize,
    pub(super) decoded: usize,
    pub(super) peak_native: usize,
}
impl ViewMeter {
    fn statement(&mut self, statement: &rusqlite::Statement<'_>) {
        self.vm += statement.get_status(rusqlite::StatementStatus::VmStep);
    }
}

fn execute<P: rusqlite::Params>(
    db: &Connection,
    sql: &str,
    params: P,
    meter: &mut ViewMeter,
) -> io::Result<usize> {
    let mut statement = db.prepare(sql).map_err(io::Error::other)?;
    let result = statement.execute(params).map_err(io::Error::other)?;
    meter.statement(&statement);
    Ok(result)
}

fn epoch(
    db: &Connection,
    original: &[u8; 32],
    record: u64,
    work: &mut RepairWork,
    meter: &mut ViewMeter,
) -> io::Result<u64> {
    let mut query=db.prepare("SELECT coalesce((SELECT revision FROM native_original_text_epochs WHERE original=?1 AND record_offset=?2),0)").map_err(io::Error::other)?;
    let value: i64 = query
        .query_row(params![original.as_slice(), seq_u64(record)?], |row| {
            row.get(0)
        })
        .map_err(io::Error::other)?;
    meter.statement(&query);
    work.metadata_rows += 1;
    work.metadata_bytes += 8;
    u64::try_from(value).map_err(io::Error::other)
}

fn page_at(
    db: &Connection,
    original: &[u8; 32],
    view: u64,
    page: u64,
    source_end: u64,
    work: &mut RepairWork,
    meter: &mut ViewMeter,
) -> io::Result<PartPage> {
    let mut query=db.prepare("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM native_original_text_parts WHERE original=?1 AND view_id=?2 AND page=?3").map_err(io::Error::other)?;
    let (text, size): (Option<String>, i64) = query
        .query_row(
            params![original.as_slice(), seq_u64(view)?, seq_u64(page)?],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(io::Error::other)?;
    meter.statement(&query);
    work.metadata_rows += 1;
    work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
    let value: PartPage = serde_json::from_str(
        text.as_deref()
            .ok_or_else(|| io::Error::other("native text part page exceeds residency"))?,
    )
    .map_err(io::Error::other)?;
    if !value.valid(source_end) {
        return Err(io::Error::other("native text part page escaped Original"));
    }
    meter.leaves += value.parts.len();
    Ok(value)
}

pub(super) fn view_current(
    db: &Connection,
    repair: &Repair,
    view: &QualifiedTextView,
    work: &mut RepairWork,
    meter: &mut ViewMeter,
) -> io::Result<bool> {
    if view.original != repair.original
        || epoch(db, &view.original, view.record, work, meter)? != view.epoch
    {
        return Ok(false);
    }
    let mut query=db.prepare("SELECT revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,CASE WHEN octet_length(receipt)<=65536 THEN receipt END,octet_length(cursor)+coalesce(octet_length(receipt),0) FROM native_original_text_views WHERE original=?1 AND view_id=?2").map_err(io::Error::other)?;
    let row: Option<(i64, Option<String>, Option<String>, i64)> = query
        .query_row(
            params![view.original.as_slice(), seq_u64(view.id)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(io::Error::other)?;
    meter.statement(&query);
    work.metadata_rows += 1;
    let Some((revision, state, receipt, size)) = row else {
        return Ok(false);
    };
    work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
    let state: Build = serde_json::from_str(
        state
            .as_deref()
            .ok_or_else(|| io::Error::other("native text view state exceeds residency"))?,
    )
    .map_err(io::Error::other)?;
    let receipt: Option<Receipt> = receipt
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(io::Error::other)?;
    Ok(state.policy == POLICY
        && u64::try_from(revision).ok() == Some(view.revision)
        && state.ready_revision == view.revision
        && matches!(state.phase, BuildPhase::Complete)
        && state.record == view.record
        && state.epoch == view.epoch
        && state.pages == view.pages
        && state.fingerprint.receipt() == receipt
        && receipt == Some(view.receipt))
}

pub(super) fn qualified_view_at(
    db: &Connection,
    repair: &Repair,
    id: u64,
    work: &mut RepairWork,
    meter: &mut ViewMeter,
) -> io::Result<QualifiedTextView> {
    let mut query = db.prepare("SELECT CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_text_views WHERE original=?1 AND view_id=?2").map_err(io::Error::other)?;
    let (text, size): (Option<String>, i64) = query
        .query_row(params![repair.original.as_slice(), seq_u64(id)?], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(io::Error::other)?;
    meter.statement(&query);
    work.metadata_rows += 1;
    work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
    let build: Build = serde_json::from_str(
        text.as_deref()
            .ok_or_else(|| io::Error::other("native typed view exceeds residency"))?,
    )
    .map_err(io::Error::other)?;
    let view = handle(repair.original, id, &build)
        .ok_or_else(|| io::Error::other("native typed class view is incomplete"))?;
    if !view_current(db, repair, &view, work, meter)? {
        return Err(io::Error::other("native typed class view changed"));
    }
    Ok(view)
}

/// One transformed scalar and its native width. JavaScript lookahead is bounded
/// to twelve outer scalars; the existing producer owns all escape decisions.
fn scalar(
    bytes: &[u8],
    decode: &Decode,
    final_input: bool,
) -> io::Result<Option<(Option<char>, usize)>> {
    match decode {
        Decode::Json(layers) => {
            Ok(
                crate::json_string::layered_scalar(bytes, *layers, final_input)?
                    .map(|(value, width)| (Some(value), width)),
            )
        }
        Decode::JavaScript(quote) => {
            let mut text = [0u8; 48];
            let mut filled = 0;
            let mut boundaries = [(0usize, 0usize); 12];
            let mut count = 0;
            let mut raw = 0;
            loop {
                if let Some(unit) = crate::json_string::native_js::unit(
                    std::str::from_utf8(&text[..filled]).map_err(io::Error::other)?,
                    *quote,
                    final_input && raw == bytes.len(),
                ) {
                    let (value, used) = match unit {
                        Unit::Scalar { value, consumed } => (value, consumed),
                        Unit::Template { .. } | Unit::Close { .. } => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                "native JavaScript view is not a plain literal",
                            ));
                        }
                    };
                    let width = boundaries[..count]
                        .iter()
                        .find_map(|(decoded, raw)| (*decoded == used).then_some(*raw))
                        .ok_or_else(|| io::Error::other("native literal unit boundary changed"))?;
                    return Ok(Some((value, width)));
                }
                if raw == bytes.len() && final_input {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "native JavaScript literal ends inside an escape",
                    ));
                }
                if count == 12 || raw == bytes.len() {
                    return Ok(None);
                }
                let Some((value, width)) =
                    crate::json_string::layered_scalar(&bytes[raw..], 1, final_input)?
                else {
                    return Ok(None);
                };
                let next = filled + value.len_utf8();
                value.encode_utf8(&mut text[filled..next]);
                filled = next;
                raw += width;
                boundaries[count] = (filled, raw);
                count += 1;
            }
        }
    }
}

pub(super) struct ViewChunk {
    pub(super) bytes: Vec<u8>,
    pub(super) cursor: ReadCursor,
    pub(super) complete: bool,
    tail: Vec<ReadRef>,
    last_complete: ReadCursor,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadRef {
    part: Part,
    at: u64,
    skip: u8,
    count: usize,
}

pub(super) fn read_chunk(
    db: &Connection,
    repair: &Repair,
    view: &QualifiedTextView,
    initial: &ReadCursor,
    records: &crate::query_source::Records<'_>,
    native_allowance: usize,
    row_allowance: usize,
    maximum: usize,
    block_bias: usize,
    work: &mut RepairWork,
    meter: &mut ViewMeter,
    visit: &mut dyn FnMut(
        &Part,
        u64,
        u64,
        Option<char>,
        &ReadCursor,
        &ReadCursor,
    ) -> io::Result<()>,
) -> io::Result<ViewChunk> {
    if maximum == 0 || maximum > CHUNK || initial.skip > 3 || initial.page > view.pages {
        return Err(io::Error::other("native text cursor bounds changed"));
    }
    let before_native = work.native_bytes;
    let before_rows = work.metadata_rows;
    let mut cursor = initial.clone();
    let mut last_complete = initial.clone();
    let mut output = Vec::with_capacity(maximum);
    let mut tail = Vec::new();
    let mut buffer = None::<crate::SessionSourceRange>;
    let pin = repair
        .cursor
        .pin
        .as_ref()
        .ok_or_else(|| io::Error::other("native text view lacks Original pin"))?;
    while cursor.page < view.pages
        && output.len() < maximum
        && work.metadata_rows - before_rows < row_allowance
    {
        let page = page_at(
            db,
            &view.original,
            view.id,
            cursor.page,
            repair.source.offset,
            work,
            meter,
        )?;
        if cursor.part > page.parts.len() {
            return Err(io::Error::other("native text part cursor changed"));
        }
        while cursor.part < page.parts.len() && output.len() < maximum {
            let part = &page.parts[cursor.part];
            let (value, width, at, end) = match part {
                Part::Newline => (Some('\n'), 1, 0, 1),
                Part::Native {
                    start, end, decode, ..
                } => {
                    let at = cursor.at.unwrap_or(*start);
                    if at < *start || at > *end {
                        return Err(io::Error::other("native text raw cursor changed"));
                    }
                    if at == *end {
                        if cursor.skip != 0 {
                            return Err(io::Error::other(
                                "native text completed with a split scalar",
                            ));
                        }
                        cursor.part += 1;
                        cursor.at = None;
                        continue;
                    }
                    let remaining =
                        native_allowance.saturating_sub(work.native_bytes - before_native);
                    let buffered = buffer.as_ref().is_some_and(|part| {
                        at >= part.offset && at < part.offset + part.bytes.len() as u64
                    });
                    if !buffered {
                        // Two-layer JSON and JS need a complete combined scalar.
                        if remaining < 72 {
                            meter.decoded += output.len();
                            return Ok(ViewChunk {
                                bytes: output,
                                cursor,
                                complete: false,
                                tail,
                                last_complete,
                            });
                        }
                        let boundary =
                            crate::source_chunks::SIZE - at as usize % crate::source_chunks::SIZE;
                        let take = remaining
                            .min(64 * 1024)
                            .min(boundary.max(72))
                            .min(usize::try_from(*end - at).unwrap_or(usize::MAX));
                        let part =
                            records.read_range(&repair.source, Some(&pin.generation), at, take)?;
                        work.native_bytes += part.bytes.len();
                        work.range_requests += 1;
                        meter.peak_native = meter.peak_native.max(part.bytes.len());
                        if part.offset != at
                            || part.bytes.is_empty()
                            || part.bytes.len() > take
                            || part.generation != pin.generation
                            || part.length != pin.length
                        {
                            return Err(io::Error::other("native text view provider changed"));
                        }
                        buffer = Some(part);
                    }
                    let part = buffer.as_ref().unwrap();
                    let start = usize::try_from(at - part.offset).map_err(io::Error::other)?;
                    let available =
                        usize::try_from((*end - at).min((part.bytes.len() - start) as u64))
                            .map_err(io::Error::other)?;
                    let Some((value, width)) = scalar(
                        &part.bytes[start..start + available],
                        decode,
                        at + available as u64 == *end,
                    )?
                    else {
                        buffer = None;
                        if remaining < 144 {
                            meter.decoded += output.len();
                            return Ok(ViewChunk {
                                bytes: output,
                                cursor,
                                complete: false,
                                tail,
                                last_complete,
                            });
                        }
                        continue;
                    };
                    (value, width, at, *end)
                }
            };
            let mut encoded = [0; 4];
            let bytes = if let Some(value) = value {
                value.encode_utf8(&mut encoded).as_bytes()
            } else {
                &[]
            };
            if usize::from(cursor.skip) > bytes.len()
                || !bytes.is_empty() && usize::from(cursor.skip) == bytes.len()
            {
                return Err(io::Error::other("native text scalar skip changed"));
            }
            let skip = usize::from(cursor.skip);
            let mut before = cursor.clone();
            before.at = Some(at);
            before.skip = 0;
            before.decoded = before
                .decoded
                .checked_sub(skip as u64)
                .ok_or_else(|| io::Error::other("native text scalar position changed"))?;
            let take = (maximum - output.len())
                .min(bytes.len() - skip)
                .min(64 - (output.len() + block_bias) % 64);
            if take > 0 {
                tail.push(ReadRef {
                    part: part.clone(),
                    at,
                    skip: cursor.skip,
                    count: take,
                });
            }
            output.extend_from_slice(&bytes[skip..skip + take]);
            cursor.decoded = cursor
                .decoded
                .checked_add(take as u64)
                .ok_or_else(|| io::Error::other("native text decoded address overflow"))?;
            if skip + take == bytes.len() {
                cursor.skip = 0;
                cursor.at = Some(at + width as u64);
                if at + width as u64 == end {
                    cursor.part += 1;
                    cursor.at = None;
                }
                visit(part, at, at + width as u64, value, &before, &cursor)?;
                last_complete = cursor.clone();
            } else {
                cursor.skip += take as u8;
            }
            if (output.len() + block_bias) % 64 == 0 {
                tail.clear();
            }
        }
        if cursor.part == page.parts.len() {
            cursor.page += 1;
            cursor.part = 0;
            cursor.at = None;
            buffer = None;
        }
    }
    meter.decoded += output.len();
    let complete = cursor.page == view.pages && cursor.skip == 0;
    Ok(ViewChunk {
        bytes: output,
        cursor,
        complete,
        tail,
        last_complete,
    })
}

fn append_parts(
    db: &Connection,
    original: &[u8; 32],
    id: u64,
    state: &mut Build,
    mut additions: Vec<Part>,
    work: &mut RepairWork,
    meter: &mut ViewMeter,
) -> io::Result<()> {
    if additions.len() > PAGE_PARTS + 1 {
        return Err(io::Error::other("native text append exceeds one part page"));
    }
    if state.pages > 0 && !additions.is_empty() {
        let last = state.pages - 1;
        let mut page = page_at(db, original, id, last, u64::MAX, work, meter)?;
        let count = (PAGE_PARTS - page.parts.len()).min(additions.len());
        if count > 0 {
            page.parts.extend(additions.drain(..count));
            let text = bounded_json(&page, STATE_BYTES).map_err(io::Error::other)?;
            execute(
                db,
                "UPDATE native_original_text_parts SET metadata=?4 WHERE original=?1 AND view_id=?2 AND page=?3",
                params![original.as_slice(), seq_u64(id)?, seq_u64(last)?, text],
                meter,
            )?;
            work.metadata_rows += 1;
            work.metadata_bytes += text.len();
        }
    }
    while !additions.is_empty() {
        let count = PAGE_PARTS.min(additions.len());
        let page = PartPage {
            parts: additions.drain(..count).collect(),
        };
        let text = bounded_json(&page, STATE_BYTES).map_err(io::Error::other)?;
        execute(
            db,
            "INSERT INTO native_original_text_parts VALUES(?1,?2,?3,?4)",
            params![
                original.as_slice(),
                seq_u64(id)?,
                seq_u64(state.pages)?,
                text
            ],
            meter,
        )?;
        work.metadata_rows += 1;
        work.metadata_bytes += text.len();
        state.pages += 1;
    }
    Ok(())
}

fn record_current(
    db: &Connection,
    repair: &Repair,
    record: u64,
    work: &mut RepairWork,
    meter: &mut ViewMeter,
) -> io::Result<bool> {
    let mut query=db.prepare("SELECT end_offset,status IN('lexed','folded') FROM native_original_records WHERE original=?1 AND record_offset=?2").map_err(io::Error::other)?;
    let row: Option<(i64, bool)> = query
        .query_row(
            params![repair.original.as_slice(), seq_u64(record)?],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(io::Error::other)?;
    meter.statement(&query);
    work.metadata_rows += 1;
    work.metadata_bytes += 24;
    Ok(row.is_some_and(|(end, status)| {
        end > 0 && record < end as u64 && end as u64 <= repair.source.offset && status
    }))
}

fn fences(
    db: &Connection,
    job: &Job,
    witness: &SourceWitness,
    repair: &Repair,
    record: u64,
    expected_epoch: u64,
    closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
    work: &mut RepairWork,
    meter: &mut ViewMeter,
) -> io::Result<bool> {
    Ok(job_current(db, job).map_err(io::Error::other)?
        && check_source(db, witness).map_err(io::Error::other)?
        && repair_current(db, repair).map_err(io::Error::other)?
        && admitted_original(db, job, witness).map_err(io::Error::other)?
        && epoch(db, &repair.original, record, work, meter)? == expected_epoch
        && record_current(db, repair, record, work, meter)?
        && closure(db, job.id).map_err(io::Error::other)?)
}

pub(super) fn anchor_record(
    records: &crate::query_source::Records<'_>,
    repair: &Repair,
    record: u64,
    work: &mut RepairWork,
) -> io::Result<()> {
    let pin = repair
        .cursor
        .pin
        .as_ref()
        .ok_or_else(|| io::Error::other("native text anchor lacks Original pin"))?;
    let part = records.read_range(&repair.source, Some(&pin.generation), record, 1)?;
    work.native_bytes += part.bytes.len();
    work.range_requests += 1;
    if part.offset != record
        || part.bytes.len() != 1
        || part.generation != pin.generation
        || part.length != pin.length
    {
        return Err(io::Error::other("native text anchor provider changed"));
    }
    Ok(())
}

fn request_record(request: &ViewRequest) -> u64 {
    match request {
        ViewRequest::WholeField { record, .. } | ViewRequest::Join { record, .. } => *record,
        ViewRequest::Slice { parent, .. }
        | ViewRequest::SliceMarked { parent, .. }
        | ViewRequest::JsonStringDecode { parent }
        | ViewRequest::JsLiteralDecode { parent, .. } => parent.record,
    }
}

fn boundary_at(
    db: &Connection,
    parent: &QualifiedTextView,
    mark: &TextBoundary,
    work: &mut RepairWork,
    meter: &mut ViewMeter,
) -> io::Result<ReadCursor> {
    if mark.original != parent.original
        || mark.view != parent.id
        || mark.revision != parent.revision
        || mark.epoch != parent.epoch
        || mark.decoded > parent.receipt.decoded_bytes
    {
        return Err(io::Error::other(
            "native text boundary belongs to another qualified view",
        ));
    }
    let mut query=db.prepare("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM native_original_text_boundaries WHERE original=?1 AND view_id=?2 AND decoded_at=?3").map_err(io::Error::other)?;
    let (text, size): (Option<String>, i64) = query
        .query_row(
            params![
                parent.original.as_slice(),
                seq_u64(parent.id)?,
                seq_u64(mark.decoded)?
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(io::Error::other)?;
    meter.statement(&query);
    work.metadata_rows += 1;
    work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
    let cursor: ReadCursor = serde_json::from_str(
        text.as_deref()
            .ok_or_else(|| io::Error::other("native boundary cursor exceeds residency"))?,
    )
    .map_err(io::Error::other)?;
    if cursor.skip != 0 || cursor.decoded != mark.decoded || cursor.page > parent.pages {
        return Err(io::Error::other("native text boundary cursor changed"));
    }
    Ok(cursor)
}

fn handle(original: [u8; 32], id: u64, state: &Build) -> Option<QualifiedTextView> {
    matches!(state.phase, BuildPhase::Complete)
        .then(|| {
            state
                .fingerprint
                .receipt()
                .map(|receipt| QualifiedTextView {
                    original,
                    id,
                    revision: state.ready_revision,
                    record: state.record,
                    epoch: state.epoch,
                    pages: state.pages,
                    receipt,
                })
        })
        .flatten()
}

impl NativeStore {
    /// Scan one bounded decoded chunk and qualify selected scalar boundaries.
    /// A producer may first inspect the chunk, then replay this same bounded
    /// cursor with its selected offsets. Marked slices start at those persisted
    /// boundaries instead of reparsing every preceding decoded prefix.
    pub(crate) fn original_text_scan_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        cursor: &TextScanCursor,
        requested: &[u64],
        records: &crate::query_source::Records<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        native_allowance: usize,
        metadata_allowance: usize,
    ) -> io::Result<TextScanStep> {
        let mut work = RepairWork::default();
        let mut meter = ViewMeter::default();
        let empty = |outcome, work, meter: &ViewMeter| TextScanStep {
            outcome,
            work,
            cursor: cursor.clone(),
            bytes: Vec::new(),
            marks: Vec::new(),
            complete: false,
            sql_vm_steps: meter.vm,
        };
        if requested.len() > 16
            || cursor.position.skip != 0
            || native_allowance > crate::SESSION_SOURCE_CHUNK_MAX
            || metadata_allowance > ROWS
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "native text scan budget or boundary request is invalid",
            ));
        }
        if cancelled() || native_allowance < 72 || metadata_allowance < 24 + requested.len() {
            return Ok(empty(Outcome::Deferred, work, &meter));
        }
        let view = &cursor.view;
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(empty(Outcome::Conflict, work, &meter));
        }
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        if !fences(
            &tx,
            job,
            witness,
            repair,
            view.record,
            view.epoch,
            closure,
            &mut work,
            &mut meter,
        )? || !view_current(&tx, repair, view, &mut work, &mut meter)?
        {
            return Ok(empty(Outcome::Conflict, work, &meter));
        }
        drop(tx);
        let mut positions = std::collections::BTreeMap::new();
        if requested.contains(&cursor.position.decoded) {
            positions.insert(cursor.position.decoded, cursor.position.clone());
        }
        let mut visit = |_: &Part,
                         _: u64,
                         _: u64,
                         _: Option<char>,
                         before: &ReadCursor,
                         after: &ReadCursor|
         -> io::Result<()> {
            // Zero-output native units share a decoded boundary. Keep its
            // latest qualified raw position so a later marked slice does not
            // replay a long removed continuation prefix.
            for position in [before, after] {
                if requested.contains(&position.decoded) {
                    positions.insert(position.decoded, position.clone());
                }
            }
            Ok(())
        };
        let mut chunk = read_chunk(
            &self.connection,
            repair,
            view,
            &cursor.position,
            records,
            native_allowance,
            metadata_allowance.saturating_sub(work.metadata_rows + requested.len() + 8),
            CHUNK,
            0,
            &mut work,
            &mut meter,
            &mut visit,
        )?;
        if chunk.cursor.skip != 0 {
            chunk
                .bytes
                .truncate(chunk.bytes.len() - usize::from(chunk.cursor.skip));
            chunk.cursor = chunk.last_complete;
        }
        if work.range_requests == 0 {
            anchor_record(records, repair, view.record, &mut work)?;
        }
        records.finish()?;
        if cancelled() {
            return Ok(empty(Outcome::Deferred, work, &meter));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(empty(Outcome::Conflict, work, &meter));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !fences(
            &tx,
            job,
            witness,
            repair,
            view.record,
            view.epoch,
            closure,
            &mut work,
            &mut meter,
        )? || !view_current(&tx, repair, view, &mut work, &mut meter)?
        {
            return Ok(empty(Outcome::Conflict, work, &meter));
        }
        let mut marks = Vec::new();
        for (decoded, position) in positions {
            let text = bounded_json(&position, STATE_BYTES).map_err(io::Error::other)?;
            execute(
                &tx,
                "INSERT INTO native_original_text_boundaries VALUES(?1,?2,?3,?4) ON CONFLICT(original,view_id,decoded_at) DO UPDATE SET metadata=excluded.metadata",
                params![
                    view.original.as_slice(),
                    seq_u64(view.id)?,
                    seq_u64(decoded)?,
                    text
                ],
                &mut meter,
            )?;
            work.metadata_rows += 1;
            work.metadata_bytes += text.len();
            marks.push(TextBoundary {
                original: view.original,
                view: view.id,
                revision: view.revision,
                epoch: view.epoch,
                decoded,
            });
        }
        if work.native_bytes > native_allowance
            || work.metadata_rows > metadata_allowance
            || work.metadata_bytes > BYTES
        {
            return Err(io::Error::other("native text scan exceeded bounded work"));
        }
        if cancelled() {
            return Ok(empty(Outcome::Deferred, work, &meter));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(TextScanStep {
            outcome: Outcome::Written,
            work,
            cursor: TextScanCursor {
                view: view.clone(),
                position: chunk.cursor,
            },
            bytes: chunk.bytes,
            marks,
            complete: chunk.complete,
            sql_vm_steps: meter.vm,
        })
    }

    /// Construct or resume one producer view. Join membership is supplied one
    /// parent at a time and rechecked by the same caller closure after I/O.
    /// Building requires TEXT_VIEW_BUILD_NATIVE_MIN bytes; smaller allowances
    /// defer before metadata/provider I/O. Scanning/ordering have separate minima.
    pub(crate) fn original_text_view_step(
        &mut self,
        job: &Job,
        witness: &SourceWitness,
        repair: &Repair,
        request: &ViewRequest,
        join_parent: &mut dyn FnMut(u64) -> io::Result<QualifiedTextView>,
        records: &crate::query_source::Records<'_>,
        closure: &dyn Fn(&Connection, i64) -> rusqlite::Result<bool>,
        cancelled: &dyn Fn() -> bool,
        native_allowance: usize,
        metadata_allowance: usize,
    ) -> io::Result<ViewStep> {
        let mut work = RepairWork::default();
        let mut meter = ViewMeter::default();
        let result =
            |outcome, work: RepairWork, meter: &ViewMeter, state: Option<&Build>, id| ViewStep {
                outcome,
                work,
                view: state.and_then(|state| handle(repair.original, id, state)),
                complete: state.is_some_and(|state| {
                    matches!(state.phase, BuildPhase::Complete | BuildPhase::Invalid)
                }),
                sql_vm_steps: meter.vm,
                leaf_parts: meter.leaves,
                decoded_bytes: meter.decoded,
                serialized_cursor_bytes: state
                    .map(|state| serde_json::to_vec(state).map(|v| v.len()))
                    .transpose()
                    .ok()
                    .flatten()
                    .unwrap_or(0),
            };
        if cancelled()
            || native_allowance < TEXT_VIEW_BUILD_NATIVE_MIN
            || native_allowance > crate::SESSION_SOURCE_CHUNK_MAX
            || metadata_allowance < 24
            || metadata_allowance > ROWS
        {
            return Ok(result(Outcome::Deferred, work, &meter, None, 0));
        }
        if reference(records, witness)? != repair.source || !self.runtime_current(&job.spec)? {
            return Ok(result(Outcome::Conflict, work, &meter, None, 0));
        }
        let record = request_record(request);
        let request_json = bounded_json(request, STATE_BYTES).map_err(io::Error::other)?;
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(io::Error::other)?;
        let expected_epoch = epoch(&tx, &repair.original, record, &mut work, &mut meter)?;
        if !fences(
            &tx,
            job,
            witness,
            repair,
            record,
            expected_epoch,
            closure,
            &mut work,
            &mut meter,
        )? {
            return Ok(result(Outcome::Conflict, work, &meter, None, 0));
        }
        let mut query=tx.prepare("SELECT view_id,revision,CASE WHEN octet_length(cursor)<=65536 THEN cursor END,octet_length(cursor) FROM native_original_text_views WHERE original=?1 AND request=?2").map_err(io::Error::other)?;
        let old: Option<(i64, i64, Option<String>, i64)> = query
            .query_row(params![repair.original.as_slice(), request_json], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()
            .map_err(io::Error::other)?;
        meter.statement(&query);
        drop(query);
        work.metadata_rows += 1;
        let (id, revision, mut state) = if let Some((id, revision, text, size)) = old {
            work.metadata_bytes += usize::try_from(size).map_err(io::Error::other)?;
            let state: Build =
                serde_json::from_str(text.as_deref().ok_or_else(|| {
                    io::Error::other("native text view cursor exceeds residency")
                })?)
                .map_err(io::Error::other)?;
            (
                u64::try_from(id).map_err(io::Error::other)?,
                Some(revision),
                state,
            )
        } else {
            let mut query=tx.prepare("SELECT coalesce(max(view_id),0)+1 FROM native_original_text_views WHERE original=?1").map_err(io::Error::other)?;
            let id: i64 = query
                .query_row(params![repair.original.as_slice()], |row| row.get(0))
                .map_err(io::Error::other)?;
            meter.statement(&query);
            work.metadata_rows += 1;
            work.metadata_bytes += 8;
            (
                u64::try_from(id).map_err(io::Error::other)?,
                None,
                Build {
                    policy: POLICY,
                    ready_revision: 0,
                    record,
                    epoch: expected_epoch,
                    phase: BuildPhase::Parts,
                    pages: 0,
                    reader: ReadCursor::default(),
                    fingerprint: DecodedFingerprint::default(),
                    pending: Vec::new(),
                    parent_index: 0,
                    parent_page: 0,
                    selected: None,
                    first: None,
                    last: None,
                },
            )
        };
        if state.policy != POLICY || state.record != record || state.epoch != expected_epoch {
            return Ok(result(Outcome::Conflict, work, &meter, None, id));
        }
        if revision.is_none() {
            if let ViewRequest::SliceMarked { parent, start, end } = request {
                if !view_current(&tx, repair, parent, &mut work, &mut meter)? {
                    return Ok(result(Outcome::Conflict, work, &meter, None, id));
                }
                state.reader = boundary_at(&tx, parent, start, &mut work, &mut meter)?;
                let _ = boundary_at(&tx, parent, end, &mut work, &mut meter)?;
                state.parent_index = 1;
            }
        }
        drop(tx);
        let cached = matches!(state.phase, BuildPhase::Complete | BuildPhase::Invalid);
        let mut additions = Vec::new();
        let mut selected_parent = match request {
            ViewRequest::Slice { parent, .. }
            | ViewRequest::SliceMarked { parent, .. }
            | ViewRequest::JsonStringDecode { parent }
            | ViewRequest::JsLiteralDecode { parent, .. } => Some(parent.clone()),
            _ => None,
        };
        if matches!(state.phase, BuildPhase::Parts) {
            match request {
                ViewRequest::WholeField { record, ordinal } => {
                    let (field, receipt, vm) = native_classes::qualified_field_at(
                        &self.connection,
                        repair,
                        *record,
                        *ordinal,
                        &mut work,
                    )?;
                    meter.vm += vm;
                    additions.push(Part::Native {
                        ordinal: *ordinal,
                        start: record
                            .checked_add(field.span.start)
                            .ok_or_else(|| io::Error::other("native view field start overflow"))?,
                        end: record
                            .checked_add(field.span.end)
                            .ok_or_else(|| io::Error::other("native view field end overflow"))?,
                        decode: Decode::Json(1),
                    });
                    // This already-qualified whole field shares its existing
                    // SHA receipt, without preparing the field a second time.
                    state.phase = BuildPhase::Complete;
                    state.fingerprint = DecodedFingerprint::from_receipt(receipt);
                }
                ViewRequest::Join { count, newline, .. } => {
                    if state.parent_index == *count {
                        state.phase = BuildPhase::Hash;
                        state.reader = ReadCursor::default();
                    } else {
                        let parent = join_parent(state.parent_index)?;
                        selected_parent = Some(parent.clone());
                        if parent.original != repair.original
                            || parent.record != record
                            || !view_current(
                                &self.connection,
                                repair,
                                &parent,
                                &mut work,
                                &mut meter,
                            )?
                        {
                            return Ok(result(Outcome::Conflict, work, &meter, None, id));
                        }
                        if state.parent_page == 0 && *newline && state.parent_index > 0 {
                            additions.push(Part::Newline);
                        }
                        if state.parent_page < parent.pages {
                            let page = page_at(
                                &self.connection,
                                &repair.original,
                                parent.id,
                                state.parent_page,
                                repair.source.offset,
                                &mut work,
                                &mut meter,
                            )?;
                            additions.extend(page.parts);
                            state.parent_page += 1;
                        }
                        if state.parent_page == parent.pages {
                            state.parent_index += 1;
                            state.parent_page = 0;
                        }
                    }
                }
                ViewRequest::Slice { parent, .. }
                | ViewRequest::SliceMarked { parent, .. }
                | ViewRequest::JsonStringDecode { parent }
                | ViewRequest::JsLiteralDecode { parent, .. } => {
                    if !view_current(&self.connection, repair, parent, &mut work, &mut meter)? {
                        return Ok(result(Outcome::Conflict, work, &meter, None, id));
                    }
                    if !matches!(
                        request,
                        ViewRequest::Slice { .. } | ViewRequest::SliceMarked { .. }
                    ) {
                        // Native nested JSON and code-mode literal carriers
                        // are one qualified source string. Joined prompt text
                        // is sliced directly, never reinterpreted as a carrier.
                        let page = if parent.pages == 1 {
                            Some(page_at(
                                &self.connection,
                                &repair.original,
                                parent.id,
                                0,
                                repair.source.offset,
                                &mut work,
                                &mut meter,
                            )?)
                        } else {
                            None
                        };
                        if !page.as_ref().is_some_and(|page| {
                            matches!(
                                page.parts.as_slice(),
                                [Part::Native {
                                    decode: Decode::Json(1),
                                    ..
                                }]
                            )
                        }) {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "native literal carrier is not one qualified original JSON field",
                            ));
                        }
                    }
                    let (start, end, quote, decode) = match request {
                        ViewRequest::Slice { start, end, .. } => (*start, *end, None, None),
                        ViewRequest::SliceMarked { start, end, .. } => {
                            (start.decoded, end.decoded, None, None)
                        }
                        ViewRequest::JsonStringDecode { .. } => (
                            1,
                            parent.receipt.decoded_bytes.saturating_sub(1),
                            Some('"'),
                            Some(Decode::Json(2)),
                        ),
                        ViewRequest::JsLiteralDecode { quote, .. } => (
                            1,
                            parent.receipt.decoded_bytes.saturating_sub(1),
                            Some(quote.character()),
                            Some(Decode::JavaScript(*quote)),
                        ),
                        _ => unreachable!(),
                    };
                    if start > end
                        || end > parent.receipt.decoded_bytes
                        || quote.is_some() && parent.receipt.decoded_bytes < 2
                    {
                        state.phase = BuildPhase::Invalid;
                    } else if state.reader.decoded == end && state.parent_index >= 1 {
                        if state.reader.skip != 0 {
                            state.phase = BuildPhase::Invalid;
                        } else {
                            if let Some(part) = state.selected.take() {
                                additions.push(part);
                            }
                            if let Some(quote) = quote {
                                let chunk = read_chunk(
                                    &self.connection,
                                    repair,
                                    parent,
                                    &state.reader,
                                    records,
                                    native_allowance.saturating_sub(work.native_bytes),
                                    1,
                                    1,
                                    0,
                                    &mut work,
                                    &mut meter,
                                    &mut |_, _, _, _, _, _| Ok(()),
                                )?;
                                state.reader = chunk.cursor;
                                if chunk.complete
                                    && chunk.bytes == [quote as u8]
                                    && state.first == Some(quote)
                                {
                                    state.phase = BuildPhase::Hash;
                                    state.reader = ReadCursor::default();
                                } else if chunk.complete || !chunk.bytes.is_empty() {
                                    state.phase = BuildPhase::Invalid;
                                }
                            } else {
                                state.phase = BuildPhase::Hash;
                                state.reader = ReadCursor::default();
                            }
                        }
                    } else {
                        let scanning = state.reader.decoded < start;
                        let target = if scanning { start } else { end };
                        let maximum =
                            usize::try_from((target - state.reader.decoded).min(CHUNK as u64))
                                .map_err(io::Error::other)?;
                        if maximum == 0 {
                            state.parent_index = 1;
                        } else {
                            let mut selected = state.selected.take();
                            let mut visit = |part: &Part,
                                             at: u64,
                                             end: u64,
                                             _: Option<char>,
                                             _: &ReadCursor,
                                             _: &ReadCursor|
                             -> io::Result<()> {
                                if scanning {
                                    return Ok(());
                                }
                                let next = match part {
                                    Part::Native {
                                        ordinal,
                                        decode: mode,
                                        ..
                                    } => {
                                        if decode.is_some() && !matches!(mode, Decode::Json(1)) {
                                            return Err(io::Error::other(
                                                "native decoder view requires original JSON content",
                                            ));
                                        }
                                        Part::Native {
                                            ordinal: *ordinal,
                                            start: at,
                                            end,
                                            decode: decode.clone().unwrap_or_else(|| mode.clone()),
                                        }
                                    }
                                    Part::Newline => {
                                        if decode.is_some() {
                                            return Err(io::Error::other(
                                                "native decoded literal spans multiple fields",
                                            ));
                                        }
                                        Part::Newline
                                    }
                                };
                                if let (
                                    Some(Part::Native {
                                        ordinal,
                                        start: _,
                                        end: previous,
                                        decode: old,
                                    }),
                                    Part::Native {
                                        ordinal: new,
                                        start,
                                        end,
                                        decode: mode,
                                    },
                                ) = (&mut selected, &next)
                                {
                                    if ordinal == new && old == mode && previous == start {
                                        *previous = *end;
                                        return Ok(());
                                    }
                                }
                                if let Some(part) = selected.replace(next) {
                                    additions.push(part);
                                }
                                Ok(())
                            };
                            let chunk = read_chunk(
                                &self.connection,
                                repair,
                                parent,
                                &state.reader,
                                records,
                                native_allowance.saturating_sub(work.native_bytes),
                                1,
                                maximum,
                                0,
                                &mut work,
                                &mut meter,
                                &mut visit,
                            )?;
                            state.selected = selected;
                            if scanning && state.reader.decoded == 0 {
                                state.first = chunk.bytes.first().copied().map(char::from);
                            }
                            state.reader = chunk.cursor;
                            if state.reader.decoded == start {
                                if state.reader.skip != 0 {
                                    state.phase = BuildPhase::Invalid;
                                } else {
                                    state.parent_index = 1;
                                }
                            }
                            if chunk.complete && state.reader.decoded < target {
                                state.phase = BuildPhase::Invalid;
                            }
                        }
                    }
                }
            }
        } else if matches!(state.phase, BuildPhase::Hash) {
            // Preflight reserves the worst legal pending block plus one new
            // scalar. replay_refs also rejects malformed oversized tails.
            let mut bytes = replay_refs(&state.pending, repair, records, &mut work)?;
            let prefix = bytes.len();
            let temporary = QualifiedTextView {
                original: repair.original,
                id,
                revision: state.ready_revision,
                record,
                epoch: state.epoch,
                pages: state.pages,
                receipt: Receipt {
                    decoded_bytes: 0,
                    sha256: [0; 32],
                },
            };
            let chunk = read_chunk(
                &self.connection,
                repair,
                &temporary,
                &state.reader,
                records,
                native_allowance.saturating_sub(work.native_bytes),
                metadata_allowance.saturating_sub(work.metadata_rows + 16),
                CHUNK - prefix,
                prefix,
                &mut work,
                &mut meter,
                &mut |_, _, _, _, _, _| Ok(()),
            );
            match chunk {
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                    state.phase = BuildPhase::Invalid
                }
                Err(error) => return Err(error),
                Ok(chunk) => {
                    bytes.extend_from_slice(&chunk.bytes);
                    state.reader = chunk.cursor;
                    if chunk.complete {
                        state.fingerprint.observe(&bytes, true)?;
                        state.pending.clear();
                        state.phase = BuildPhase::Complete;
                    } else {
                        let full = bytes.len() / 64 * 64;
                        state.fingerprint.observe(&bytes[..full], false)?;
                        if full > 0 {
                            state.pending = chunk.tail;
                        } else {
                            state.pending.extend(chunk.tail);
                        }
                    }
                }
            }
        }
        if work.range_requests == 0 {
            anchor_record(records, repair, record, &mut work)?;
        }
        records.finish()?;
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, &meter, None, id));
        }
        if !self.runtime_current(&job.spec)? {
            return Ok(result(Outcome::Conflict, work, &meter, None, id));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io::Error::other)?;
        if !fences(
            &tx,
            job,
            witness,
            repair,
            record,
            state.epoch,
            closure,
            &mut work,
            &mut meter,
        )? {
            return Ok(result(Outcome::Conflict, work, &meter, None, id));
        }
        if let Some(parent) = selected_parent.as_ref() {
            if !view_current(&tx, repair, parent, &mut work, &mut meter)? {
                return Ok(result(Outcome::Conflict, work, &meter, None, id));
            }
        }
        let mut query=tx.prepare("SELECT view_id,revision FROM native_original_text_views WHERE original=?1 AND request=?2").map_err(io::Error::other)?;
        let current: Option<(i64, i64)> = query
            .query_row(params![repair.original.as_slice(), request_json], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .map_err(io::Error::other)?;
        meter.statement(&query);
        drop(query);
        work.metadata_rows += 1;
        work.metadata_bytes += 16;
        if current != revision.map(|revision| (id as i64, revision)) {
            return Ok(result(Outcome::Conflict, work, &meter, None, id));
        }
        if revision.is_none() {
            let mut query=tx.prepare("SELECT EXISTS(SELECT 1 FROM native_original_text_views WHERE original=?1 AND view_id=?2)").map_err(io::Error::other)?;
            let exists = query
                .query_row(params![repair.original.as_slice(), seq_u64(id)?], |row| {
                    row.get::<_, bool>(0)
                })
                .map_err(io::Error::other)?;
            meter.statement(&query);
            work.metadata_rows += 1;
            work.metadata_bytes += 8;
            if exists {
                return Ok(result(Outcome::Conflict, work, &meter, None, id));
            }
        }
        if !cached {
            state.ready_revision = match revision {
                Some(revision) => u64::try_from(revision)
                    .map_err(io::Error::other)?
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("native view revision overflow"))?,
                None => 0,
            };
            append_parts(
                &tx,
                &repair.original,
                id,
                &mut state,
                additions,
                &mut work,
                &mut meter,
            )?;
            let text = bounded_json(&state, STATE_BYTES).map_err(io::Error::other)?;
            let receipt = state
                .fingerprint
                .receipt()
                .map(|receipt| json(&receipt))
                .transpose()
                .map_err(io::Error::other)?;
            execute(
                &tx,
                "INSERT INTO native_original_text_views VALUES(?1,?2,0,?3,?4,?5) ON CONFLICT(original,view_id) DO UPDATE SET revision=revision+1,cursor=excluded.cursor,receipt=excluded.receipt",
                params![
                    repair.original.as_slice(),
                    seq_u64(id)?,
                    request_json,
                    text,
                    receipt
                ],
                &mut meter,
            )?;
            work.metadata_rows += 1;
            work.metadata_bytes += text.len() + receipt.as_ref().map_or(0, String::len);
        }
        if work.native_bytes > native_allowance
            || work.metadata_rows > metadata_allowance
            || work.metadata_bytes > BYTES
        {
            return Err(io::Error::other(
                "native text view step exceeds bounded work",
            ));
        }
        if cancelled() {
            return Ok(result(Outcome::Deferred, work, &meter, None, id));
        }
        tx.commit().map_err(io::Error::other)?;
        Ok(result(
            if matches!(state.phase, BuildPhase::Complete | BuildPhase::Invalid) {
                Outcome::Written
            } else {
                Outcome::Deferred
            },
            work,
            &meter,
            Some(&state),
            id,
        ))
    }
}

fn replay_refs(
    refs: &[ReadRef],
    repair: &Repair,
    records: &crate::query_source::Records<'_>,
    work: &mut RepairWork,
) -> io::Result<Vec<u8>> {
    if refs.len() > 63 || refs.iter().map(|part| part.count).sum::<usize>() > 63 {
        return Err(io::Error::other(
            "native text fingerprint pending references exceed a block",
        ));
    }
    let mut result = Vec::with_capacity(64);
    let pin = repair
        .cursor
        .pin
        .as_ref()
        .ok_or_else(|| io::Error::other("native text pending hash lacks Original pin"))?;
    for reference in refs {
        if reference.count == 0 || reference.count > 4 || reference.skip > 3 {
            return Err(io::Error::other("native text pending scalar changed"));
        }
        let value = match &reference.part {
            Part::Newline => '\n',
            Part::Native {
                start, end, decode, ..
            } => {
                if reference.at < *start || reference.at >= *end || *end > repair.source.offset {
                    return Err(io::Error::other(
                        "native text pending scalar escaped Original",
                    ));
                }
                let maximum =
                    usize::try_from((*end - reference.at).min(72)).map_err(io::Error::other)?;
                let part = records.read_range(
                    &repair.source,
                    Some(&pin.generation),
                    reference.at,
                    maximum,
                )?;
                work.native_bytes += part.bytes.len();
                work.range_requests += 1;
                if part.offset != reference.at
                    || part.bytes.is_empty()
                    || part.bytes.len() > maximum
                    || part.generation != pin.generation
                    || part.length != pin.length
                {
                    return Err(io::Error::other(
                        "native text pending scalar provider changed",
                    ));
                }
                scalar(
                    &part.bytes,
                    decode,
                    reference.at + part.bytes.len() as u64 == *end,
                )?
                .and_then(|(value, _)| value)
                .ok_or_else(|| io::Error::other("native text pending hash is not a scalar"))?
            }
        };
        let mut encoded = [0; 4];
        let bytes = value.encode_utf8(&mut encoded).as_bytes();
        let start = usize::from(reference.skip);
        let end = start + reference.count;
        result.extend_from_slice(
            bytes
                .get(start..end)
                .ok_or_else(|| io::Error::other("native text pending decoded range changed"))?,
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::super::native_compare::tests::{Fixture, Provider};
    use super::*;
    use std::cell::Cell;

    fn make_fixture(text: &str) -> Fixture {
        let fixture = Fixture::new(&[serde_json::to_string(text).unwrap()]);
        fixture.store.connection.execute_batch(SCHEMA).unwrap();
        fixture
    }
    fn membership(db: &Connection, job: i64) -> rusqlite::Result<bool> {
        db.query_row("SELECT NOT EXISTS(SELECT 1 FROM comparison_required_owners r LEFT JOIN native_query_owners w ON w.job=?1 AND w.session_key=r.key WHERE w.session_key IS NULL)",[job],|row|row.get(0))
    }
    fn step(
        fixture: &mut Fixture,
        request: &ViewRequest,
        parents: &[QualifiedTextView],
        cancel: &dyn Fn() -> bool,
    ) -> ViewStep {
        let records = fixture.sources.reader();
        fixture
            .store
            .original_text_view_step(
                &fixture.job,
                &fixture.witnesses[0],
                &fixture.repairs[0],
                request,
                &mut |index| {
                    parents
                        .get(index as usize)
                        .cloned()
                        .ok_or_else(|| io::Error::other("test Join parent missing"))
                },
                &records,
                &|_, _| Ok(true),
                cancel,
                crate::SESSION_SOURCE_CHUNK_MAX,
                ROWS,
            )
            .unwrap()
    }
    fn complete(
        fixture: &mut Fixture,
        request: &ViewRequest,
        parents: &[QualifiedTextView],
    ) -> Option<QualifiedTextView> {
        let reads = super::super::super::super::super::FILE_READS.with(Cell::get);
        for _ in 0..8192 {
            let result = step(fixture, request, parents, &|| false);
            assert!(result.work.metadata_rows <= ROWS);
            assert!(result.work.metadata_bytes <= BYTES);
            assert!(result.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            assert!(result.serialized_cursor_bytes <= STATE_BYTES);
            assert!(result.sql_vm_steps < 4096);
            assert!(result.decoded_bytes <= CHUNK);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            if result.complete {
                assert_eq!(result.outcome, Outcome::Written);
                assert_eq!(
                    super::super::super::super::super::FILE_READS.with(Cell::get),
                    reads
                );
                return result.view;
            }
            assert_eq!(result.outcome, Outcome::Deferred);
        }
        panic!("qualified text view failed to make eventual progress")
    }
    fn whole(fixture: &mut Fixture) -> QualifiedTextView {
        let ordinal: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT ordinal FROM native_original_classes WHERE original=?1 AND class=1",
                params![fixture.repairs[0].original.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        complete(
            fixture,
            &ViewRequest::WholeField {
                record: 0,
                ordinal: ordinal as u64,
            },
            &[],
        )
        .unwrap()
    }
    fn rendered(fixture: &Fixture, view: &QualifiedTextView) -> String {
        let mut output = Vec::new();
        let mut cursor = ReadCursor::default();
        for _ in 0..8192 {
            let records = fixture.sources.reader();
            let mut work = RepairWork::default();
            let mut meter = ViewMeter::default();
            assert!(
                view_current(
                    &fixture.store.connection,
                    &fixture.repairs[0],
                    view,
                    &mut work,
                    &mut meter
                )
                .unwrap()
            );
            let chunk = read_chunk(
                &fixture.store.connection,
                &fixture.repairs[0],
                view,
                &cursor,
                &records,
                crate::SESSION_SOURCE_CHUNK_MAX,
                ROWS - 4,
                CHUNK,
                0,
                &mut work,
                &mut meter,
                &mut |_, _, _, _, _, _| Ok(()),
            )
            .unwrap();
            assert!(work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            assert!(work.metadata_rows <= ROWS);
            records.finish().unwrap();
            output.extend_from_slice(&chunk.bytes);
            cursor = chunk.cursor;
            if chunk.complete {
                return String::from_utf8(output).unwrap();
            }
        }
        panic!("bounded view reader failed to finish")
    }

    fn identity(
        fixture: &mut Fixture,
        view: Option<&QualifiedTextView>,
        ordinal: u64,
    ) -> OriginalText {
        for _ in 0..8192 {
            let records = fixture.sources.reader();
            let step = match view {
                Some(view) => fixture.store.original_view_identity_step(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.repairs[0],
                    view,
                    &records,
                    &|_, _| Ok(true),
                    &|| false,
                    crate::SESSION_SOURCE_CHUNK_MAX,
                    ROWS,
                ),
                None => fixture.store.original_identity_step(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.repairs[0],
                    ordinal,
                    &records,
                    &|| false,
                ),
            }
            .unwrap();
            assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            assert!(step.work.metadata_rows <= ROWS);
            assert!(step.work.metadata_bytes <= BYTES);
            assert!(step.serialized_cursor_bytes <= STATE_BYTES);
            assert!(step.class_sql_vm_steps < 4096);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            if let Some(text) = step.text {
                return text;
            }
            assert_eq!(step.outcome, Outcome::Deferred);
        }
        panic!("shared text value interner failed to make progress")
    }
    fn clear_derived_classes(fixture: &Fixture) {
        fixture.store.connection.execute_batch("DELETE FROM native_original_identities; DELETE FROM native_original_view_identities; DELETE FROM native_original_class_values; DELETE FROM native_original_classes; DELETE FROM native_original_text_policy;").unwrap();
    }
    fn joined_literal(fixture: &mut Fixture) -> OriginalText {
        let parent = whole(fixture);
        let mut parts = Vec::new();
        for (start, end) in [(0, 3), (3, 8)] {
            parts.push(
                complete(
                    fixture,
                    &ViewRequest::Slice {
                        parent: parent.clone(),
                        start,
                        end,
                    },
                    &[],
                )
                .unwrap(),
            );
        }
        let join = complete(
            fixture,
            &ViewRequest::Join {
                record: 0,
                producer: 101,
                count: 2,
                newline: false,
            },
            &parts,
        )
        .unwrap();
        assert_eq!(rendered(fixture, &join), "selected");
        clear_derived_classes(fixture);
        let text = identity(fixture, Some(&join), 0);
        assert!(matches!(
            fixture
                .store
                .original_text_representative(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.repairs[0],
                    &text,
                )
                .unwrap()
                .value,
            native_values::Value::View(_)
        ));
        text
    }
    fn literal_snapshot(fixture: &Fixture) -> Option<(i64, String)> {
        fixture
            .store
            .connection
            .query_row(
                "SELECT revision,cursor FROM native_original_class_literals LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap()
    }
    fn cache_literal(fixture: &mut Fixture, text: &OriginalText) {
        let records = fixture.sources.reader();
        let step = fixture
            .store
            .original_text_literal_step(
                &fixture.job,
                &fixture.witnesses[0],
                &fixture.repairs[0],
                text,
                "selected",
                &records,
                &membership,
                &|| false,
            )
            .unwrap();
        assert_eq!(step.matches, Some(true));
        assert!(literal_snapshot(fixture).is_some());
    }

    #[test]
    fn joined_literals_fence_related_owners_membership_and_sources_after_provider_io() {
        for cached in [false, true] {
            for change in [
                "third", "new", "source", "runtime", "epoch", "repair", "provider",
            ] {
                let encoded = serde_json::to_string("selected").unwrap();
                let mut fixture = Fixture::new(&[encoded.clone(), encoded.clone(), encoded]);
                let text = joined_literal(&mut fixture);
                if cached {
                    cache_literal(&mut fixture, &text);
                }
                let before = literal_snapshot(&fixture);
                // No local original may satisfy the retained-provider path.
                fs::remove_file(&fixture.witnesses[0].source.path).unwrap();
                let cache =
                    super::super::super::super::super::EventCache::path(&fixture.options.cache);
                let runtime = fixture.options.claude_json.clone();
                let path = fixture.witnesses[0]
                    .source
                    .path
                    .to_string_lossy()
                    .into_owned();
                let original = fixture.repairs[0].original;
                let provider = Provider::new(
                    fixture.repairs[0].source.clone(),
                    &fixture.bytes[0],
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
                            "third" => {
                                db.execute("UPDATE session_catalog SET metadata=metadata WHERE session_key='owner-2'", []).unwrap();
                            }
                            "new" => {
                                db.execute("INSERT INTO comparison_required_owners VALUES('new-related-owner')", []).unwrap();
                            }
                            "source" => {
                                db.execute("INSERT INTO session_catalog_invalidations VALUES(?1,1,'changed',0)", [&path]).unwrap();
                            }
                            "epoch" => {
                                db.execute("UPDATE native_original_fields SET decoded_bytes=decoded_bytes+1 WHERE original=?1 AND is_key=0", params![original.as_slice()]).unwrap();
                            }
                            "repair" => {
                                db.execute("UPDATE native_original_repairs SET revision=revision+1 WHERE original=?1", params![original.as_slice()]).unwrap();
                            }
                            _ => {}
                        }
                    },
                );
                let records = fixture.sources.reader_with(Some((
                    &provider,
                    crate::SessionSourceReadMode::ProviderOnly,
                )));
                let step = fixture.store.original_text_literal_step_with_budget(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.repairs[0],
                    &text,
                    "selected",
                    &records,
                    &membership,
                    &|| false,
                    72,
                    24,
                );
                if change == "provider" {
                    assert!(step.is_err());
                } else {
                    let step = step.unwrap();
                    assert_eq!(step.outcome, Outcome::Conflict, "{change}, cached={cached}");
                    assert_eq!(step.matches, None);
                    assert!(step.work.native_bytes <= 72);
                    assert!(step.work.metadata_rows <= 24);
                    assert!(step.work.metadata_bytes <= BYTES);
                }
                assert!(provider.observed().0 > 0);
                assert_eq!(literal_snapshot(&fixture), before);
            }
        }
    }

    #[test]
    fn changed_join_membership_refuses_fresh_and_cached_literals_before_io() {
        for cached in [false, true] {
            let mut fixture = make_fixture("selected");
            let text = joined_literal(&mut fixture);
            if cached {
                cache_literal(&mut fixture, &text);
            }
            let before = literal_snapshot(&fixture);
            fixture
                .store
                .connection
                .execute(
                    "INSERT INTO comparison_required_owners VALUES('new-related-owner')",
                    [],
                )
                .unwrap();
            let provider = Provider::new(
                fixture.repairs[0].source.clone(),
                &fixture.bytes[0],
                None,
                |_| {},
            );
            let records = fixture.sources.reader_with(Some((
                &provider,
                crate::SessionSourceReadMode::ProviderOnly,
            )));
            let step = fixture
                .store
                .original_text_literal_step(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.repairs[0],
                    &text,
                    "selected",
                    &records,
                    &membership,
                    &|| false,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Conflict);
            assert!(step.matches.is_none());
            assert_eq!((step.work.native_bytes, step.work.range_requests), (0, 0));
            assert_eq!(provider.observed(), (0, 0));
            assert_eq!(literal_snapshot(&fixture), before);
        }
    }

    #[test]
    fn literal_callbacks_precede_owned_rechecks_and_cached_cursor_cas() {
        for cached in [false, true] {
            for phase in [0, 1, 2] {
                for change in ["owner", "runtime", "epoch", "cas"] {
                    if change == "cas" && !cached {
                        continue;
                    }
                    let encoded = serde_json::to_string("selected").unwrap();
                    let mut fixture = Fixture::new(&[encoded.clone(), encoded.clone(), encoded]);
                    let text = joined_literal(&mut fixture);
                    if cached {
                        cache_literal(&mut fixture, &text);
                    }
                    let before = literal_snapshot(&fixture);
                    let provider = Provider::new(
                        fixture.repairs[0].source.clone(),
                        &fixture.bytes[0],
                        None,
                        |_| {},
                    );
                    let records = fixture.sources.reader_with(Some((
                        &provider,
                        crate::SessionSourceReadMode::ProviderOnly,
                    )));
                    let calls = Cell::new(0);
                    let original = fixture.repairs[0].original;
                    let runtime = fixture.options.claude_json.clone();
                    let closure = |db: &Connection, job| {
                        let call = calls.get();
                        calls.set(call + 1);
                        if call == phase {
                            match change {
                                "owner" => {
                                    db.execute("UPDATE session_catalog SET metadata=metadata WHERE session_key='owner-2'", [])?;
                                }
                                "runtime" => {
                                    fs::write(&runtime, b"{\"changed\":true}").unwrap();
                                }
                                "epoch" => {
                                    db.execute("UPDATE native_original_fields SET decoded_bytes=decoded_bytes+1 WHERE original=?1 AND is_key=0", params![original.as_slice()])?;
                                }
                                "cas" => {
                                    db.execute("UPDATE native_original_class_literals SET revision=revision+1 WHERE original=?1", params![original.as_slice()])?;
                                }
                                _ => unreachable!(),
                            }
                        }
                        membership(db, job)
                    };
                    let result = fixture.store.original_text_literal_step_with_budget(
                        &fixture.job,
                        &fixture.witnesses[0],
                        &fixture.repairs[0],
                        &text,
                        "selected",
                        &records,
                        &closure,
                        &|| false,
                        72,
                        24,
                    );
                    assert!(calls.get() > phase, "callback {phase} was not reached");
                    // A stale typed view may refuse as invalid native evidence;
                    // no phase may publish an answer or change its cached cursor.
                    if let Ok(step) = result {
                        assert_eq!(step.outcome, Outcome::Conflict, "{change} phase={phase}");
                        assert!(step.matches.is_none());
                        assert!(step.work.native_bytes <= 72);
                        assert!(step.work.metadata_rows <= 24);
                    }
                    assert_eq!(literal_snapshot(&fixture), before);
                    if phase == 0 && change != "cas" {
                        assert_eq!(provider.observed(), (0, 0));
                    }
                }
            }
        }
    }

    #[test]
    fn joined_literal_cancellation_reopen_and_cached_provider_proof_work_are_separate() {
        for unrelated in [0, 16] {
            let encoded = serde_json::to_string("selected").unwrap();
            let mut fixture = Fixture::new(&vec![encoded; unrelated + 3]);
            let text = joined_literal(&mut fixture);
            let calls = Cell::new(0);
            let cancel = || {
                calls.set(calls.get() + 1);
                calls.get() == 3
            };
            let records = fixture.sources.reader();
            let cancelled = fixture
                .store
                .original_text_literal_step_with_budget(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.repairs[0],
                    &text,
                    "selected",
                    &records,
                    &membership,
                    &cancel,
                    72,
                    24,
                )
                .unwrap();
            assert_eq!(cancelled.outcome, Outcome::Deferred);
            assert!(cancelled.matches.is_none());
            assert!(literal_snapshot(&fixture).is_none());
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            cache_literal(&mut fixture, &text);
            let before = literal_snapshot(&fixture);
            fs::remove_file(&fixture.witnesses[0].source.path).unwrap();
            let provider = Provider::new(
                fixture.repairs[0].source.clone(),
                &fixture.bytes[0],
                None,
                |_| {},
            );
            let records = fixture.sources.reader_with(Some((
                &provider,
                crate::SessionSourceReadMode::ProviderOnly,
            )));
            let cached = fixture
                .store
                .original_text_literal_step_with_budget(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.repairs[0],
                    &text,
                    "selected",
                    &records,
                    &membership,
                    &|| false,
                    72,
                    24,
                )
                .unwrap();
            assert_eq!(cached.outcome, Outcome::Written);
            assert_eq!(cached.matches, Some(true));
            assert_eq!(
                (cached.work.range_requests, cached.work.native_bytes),
                (1, 1)
            );
            assert!(cached.work.metadata_rows <= 24);
            assert!(cached.work.metadata_bytes <= BYTES);
            assert_eq!(provider.observed(), (2, fixture.bytes[0].len() * 2));
            assert_eq!(literal_snapshot(&fixture), before);
        }
    }

    fn budgeted_build(
        fixture: &mut Fixture,
        request: &ViewRequest,
        parents: &[QualifiedTextView],
        native: usize,
        rows: usize,
    ) -> ViewStep {
        let records = fixture.sources.reader();
        fixture
            .store
            .original_text_view_step(
                &fixture.job,
                &fixture.witnesses[0],
                &fixture.repairs[0],
                request,
                &mut |index| {
                    parents
                        .get(index as usize)
                        .cloned()
                        .ok_or_else(|| io::Error::other("test Join parent missing"))
                },
                &records,
                &membership,
                &|| false,
                native,
                rows,
            )
            .unwrap()
    }
    fn saved_build(fixture: &Fixture, request: &ViewRequest) -> Option<(u64, Build)> {
        let value: Option<(i64, String)> = fixture.store.connection.query_row(
            "SELECT view_id,cursor FROM native_original_text_views WHERE original=?1 AND request=?2",
            params![fixture.repairs[0].original.as_slice(), bounded_json(request, STATE_BYTES).unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().unwrap();
        value.map(|(id, text)| (id as u64, serde_json::from_str(&text).unwrap()))
    }
    fn progress_bytes(mut state: Build) -> Vec<u8> {
        // Merely incrementing the row revision cannot establish progress.
        state.ready_revision = 0;
        serde_json::to_vec(&state).unwrap()
    }

    #[test]
    fn every_undersized_builder_allowance_defers_before_metadata_or_provider_io() {
        let mut fixture = make_fixture(&"x".repeat(512));
        let parent = whole(&mut fixture);
        let request = ViewRequest::Slice {
            parent,
            start: 0,
            end: 512,
        };
        let provider = Provider::new(
            fixture.repairs[0].source.clone(),
            &fixture.bytes[0],
            None,
            |_| {},
        );
        let records = fixture.sources.reader_with(Some((
            &provider,
            crate::SessionSourceReadMode::ProviderOnly,
        )));
        for native in 0..TEXT_VIEW_BUILD_NATIVE_MIN {
            let step = fixture
                .store
                .original_text_view_step(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.repairs[0],
                    &request,
                    &mut |_| unreachable!(),
                    &records,
                    &membership,
                    &|| false,
                    native,
                    24,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Deferred);
            assert_eq!(
                (
                    step.work.native_bytes,
                    step.work.range_requests,
                    step.work.metadata_rows,
                    step.work.metadata_bytes,
                    step.sql_vm_steps
                ),
                (0, 0, 0, 0, 0)
            );
        }
        assert_eq!(provider.observed(), (0, 0));
        assert!(saved_build(&fixture, &request).is_none());
    }

    #[test]
    fn builder_minimum_advances_a_maximal_raw_reference_tail_without_plaintext_state() {
        let text = "x".repeat(512);
        let mut fixture = make_fixture(&text);
        let parent = whole(&mut fixture);
        let request = ViewRequest::Slice {
            parent,
            start: 0,
            end: 512,
        };
        let mut hash = None;
        for _ in 0..64 {
            budgeted_build(&mut fixture, &request, &[], TEXT_VIEW_BUILD_NATIVE_MIN, 24);
            let (id, state) = saved_build(&fixture, &request).unwrap();
            if matches!(state.phase, BuildPhase::Hash) {
                hash = Some((id, state));
                break;
            }
        }
        let (id, mut state) = hash.expect("slice Parts must reach Hash");
        let temporary = QualifiedTextView {
            original: fixture.repairs[0].original,
            id,
            revision: state.ready_revision,
            record: state.record,
            epoch: state.epoch,
            pages: state.pages,
            receipt: Receipt {
                decoded_bytes: 0,
                sha256: [0; 32],
            },
        };
        let records = fixture.sources.reader();
        let chunk = read_chunk(
            &fixture.store.connection,
            &fixture.repairs[0],
            &temporary,
            &state.reader,
            &records,
            crate::SESSION_SOURCE_CHUNK_MAX,
            ROWS,
            63,
            0,
            &mut RepairWork::default(),
            &mut ViewMeter::default(),
            &mut |_, _, _, _, _, _| Ok(()),
        )
        .unwrap();
        records.finish().unwrap();
        assert_eq!(chunk.bytes, vec![b'x'; 63]);
        assert_eq!(chunk.tail.len(), 63);
        state.reader = chunk.cursor;
        state.pending = chunk.tail;
        let encoded = bounded_json(&state, STATE_BYTES).unwrap();
        assert!(!encoded.contains(&"x".repeat(16)));
        fixture
            .store
            .connection
            .execute(
                "UPDATE native_original_text_views SET cursor=?3 WHERE original=?1 AND view_id=?2",
                params![
                    fixture.repairs[0].original.as_slice(),
                    seq_u64(id).unwrap(),
                    encoded
                ],
            )
            .unwrap();
        let before = progress_bytes(state);
        let reads = super::super::super::super::super::FILE_READS.with(Cell::get);
        // The old accepted 292-byte call made an anchor read and saved an
        // unchanged tail forever. It must now defer without touching that tail.
        let denied = budgeted_build(&mut fixture, &request, &[], 292, 24);
        assert_eq!(
            (denied.work.range_requests, denied.work.native_bytes),
            (0, 0)
        );
        assert_eq!(
            progress_bytes(saved_build(&fixture, &request).unwrap().1),
            before
        );
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let mut finished = None;
        for _ in 0..64 {
            let before = progress_bytes(saved_build(&fixture, &request).unwrap().1);
            let step = budgeted_build(&mut fixture, &request, &[], TEXT_VIEW_BUILD_NATIVE_MIN, 24);
            assert!(step.work.native_bytes <= TEXT_VIEW_BUILD_NATIVE_MIN);
            assert!(step.work.metadata_rows <= 24);
            assert!(step.work.metadata_bytes <= BYTES);
            assert!(step.serialized_cursor_bytes <= STATE_BYTES);
            let state = saved_build(&fixture, &request).unwrap().1;
            assert!(state.pending.len() <= 63);
            assert!(state.pending.iter().map(|part| part.count).sum::<usize>() <= 63);
            assert_ne!(progress_bytes(state), before);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            if step.complete {
                finished = step.view;
                break;
            }
            assert_eq!(step.outcome, Outcome::Deferred);
        }
        let view = finished.expect("minimum builder must finish the retained SHA tail");
        assert_eq!(rendered(&fixture, &view), text);
        assert_eq!(
            view.receipt.sha256,
            <[u8; 32]>::from(Sha256::digest(text.as_bytes()))
        );
        assert_eq!(
            super::super::super::super::super::FILE_READS.with(Cell::get),
            reads
        );
    }

    #[test]
    fn builder_minimum_resumes_zero_output_js_and_paged_join_at_metadata_minimum() {
        for native in [
            TEXT_VIEW_BUILD_NATIVE_MIN,
            TEXT_VIEW_BUILD_NATIVE_MIN + 1,
            crate::SESSION_SOURCE_CHUNK_MAX,
        ] {
            for mode in ["join", "gaps", "wide"] {
                let literal = match mode {
                    "gaps" => format!("'{}{}'", "\\\n".repeat(2500), "x".repeat(257)),
                    "wide" => format!("'{}'", "\\uD83D\\uDE00".repeat(257)),
                    _ => "abcd".into(),
                };
                let mut fixture = if mode == "wide" {
                    // Twelve JS surrogate-escape characters, each represented
                    // by a six-byte outer JSON escape: 72 raw bytes per scalar.
                    let escaped: String = literal
                        .bytes()
                        .map(|byte| format!("\\u{byte:04x}"))
                        .collect();
                    Fixture::new(&[format!("\"{escaped}\"")])
                } else {
                    make_fixture(&literal)
                };
                let parent = whole(&mut fixture);
                let parents = if mode != "join" {
                    Vec::new()
                } else {
                    vec![parent.clone(); 73]
                };
                let request = if mode != "join" {
                    ViewRequest::JsLiteralDecode {
                        parent,
                        quote: Quote::Single,
                    }
                } else {
                    ViewRequest::Join {
                        record: 0,
                        producer: 102,
                        count: 73,
                        newline: true,
                    }
                };
                let expected = match mode {
                    "gaps" => "x".repeat(257),
                    "wide" => "😀".repeat(257),
                    _ => vec!["abcd"; 73].join("\n"),
                };
                let mut gap_progress = false;
                let mut finished = None;
                for _ in 0..1024 {
                    let before = saved_build(&fixture, &request);
                    let step = budgeted_build(&mut fixture, &request, &parents, native, 24);
                    assert!(step.work.native_bytes <= native);
                    assert!(step.work.metadata_rows <= 24);
                    assert!(step.work.metadata_bytes <= BYTES);
                    assert!(step.serialized_cursor_bytes <= STATE_BYTES);
                    let (_, after) = saved_build(&fixture, &request).unwrap();
                    if let Some((_, before)) = before {
                        gap_progress |= matches!(before.phase, BuildPhase::Hash)
                            && before.reader.decoded == after.reader.decoded
                            && before.reader != after.reader;
                        assert_ne!(progress_bytes(before), progress_bytes(after));
                    }
                    fixture.store = NativeStore::open(&fixture.options).unwrap();
                    if step.complete {
                        finished = step.view;
                        break;
                    }
                    assert_eq!(step.outcome, Outcome::Deferred);
                }
                let view = finished.expect("every accepted builder boundary must finish");
                assert_eq!(rendered(&fixture, &view), expected);
                assert_eq!(
                    view.receipt.sha256,
                    <[u8; 32]>::from(Sha256::digest(expected.as_bytes()))
                );
                if mode == "gaps" && native == TEXT_VIEW_BUILD_NATIVE_MIN {
                    assert!(gap_progress);
                }
            }
        }
    }
    #[test]
    fn whole_fields_and_views_share_classes_in_both_allocation_orders() {
        for view_first in [false, true] {
            let encoded = format!(
                "{},\"t\":{}",
                serde_json::to_string("xabc").unwrap(),
                serde_json::to_string("abc").unwrap()
            );
            let mut fixture = Fixture::new(&[encoded]);
            let parent = whole(&mut fixture);
            let slice = complete(
                &mut fixture,
                &ViewRequest::Slice {
                    parent,
                    start: 1,
                    end: 4,
                },
                &[],
            )
            .unwrap();
            let ordinal:i64=fixture.store.connection.query_row("SELECT ordinal FROM native_original_fields WHERE original=?1 AND is_key=0 AND decoded_bytes=3",params![fixture.repairs[0].original.as_slice()],|row|row.get(0)).unwrap();
            clear_derived_classes(&fixture);
            let (left, right) = if view_first {
                (
                    identity(&mut fixture, Some(&slice), 0),
                    identity(&mut fixture, None, ordinal as u64),
                )
            } else {
                (
                    identity(&mut fixture, None, ordinal as u64),
                    identity(&mut fixture, Some(&slice), 0),
                )
            };
            assert_eq!(left, right);
            let (classes, legacy): (i64, i64) = fixture
                .store
                .connection
                .query_row(
                    "SELECT count(*),min(ordinal) FROM native_original_classes",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(classes, 1);
            assert_eq!(legacy, if view_first { -1 } else { ordinal });
            for literal in ["abc", "abd", "ab", ""] {
                let mut finished = false;
                for _ in 0..16 {
                    let records = fixture.sources.reader();
                    let step = fixture
                        .store
                        .original_text_literal_step_with_budget(
                            &fixture.job,
                            &fixture.witnesses[0],
                            &fixture.repairs[0],
                            &left,
                            literal,
                            &records,
                            &membership,
                            &|| false,
                            crate::SESSION_SOURCE_CHUNK_MAX,
                            ROWS,
                        )
                        .unwrap();
                    assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                    assert!(step.work.metadata_rows <= ROWS);
                    if let Some(result) = step.matches {
                        assert_eq!(result, literal == "abc");
                        finished = true;
                        break;
                    }
                }
                assert!(finished);
            }
            let records = fixture.sources.reader();
            fixture
                .store
                .connection
                .execute(
                    "DELETE FROM native_original_class_values WHERE original=?1 AND class=?2",
                    params![
                        fixture.repairs[0].original.as_slice(),
                        seq_u64(left.identity()).unwrap()
                    ],
                )
                .unwrap();
            assert!(
                fixture
                    .store
                    .original_text_representative(
                        &fixture.job,
                        &fixture.witnesses[0],
                        &fixture.repairs[0],
                        &left
                    )
                    .is_err()
            );
            assert!(
                fixture
                    .store
                    .original_text_literal_step(
                        &fixture.job,
                        &fixture.witnesses[0],
                        &fixture.repairs[0],
                        &left,
                        "abc",
                        &records,
                        &membership,
                        &|| false
                    )
                    .is_err()
            );
        }
    }
    #[test]
    fn forced_digest_collision_never_joins_a_slice_with_different_whole_text() {
        for view_first in [false, true] {
            let encoded = format!(
                "{},\"t\":{}",
                serde_json::to_string("xabc").unwrap(),
                serde_json::to_string("abd").unwrap()
            );
            let mut fixture = Fixture::new(&[encoded]);
            let ordinal:i64=fixture.store.connection.query_row("SELECT ordinal FROM native_original_fields WHERE original=?1 AND is_key=0 AND decoded_bytes=3",params![fixture.repairs[0].original.as_slice()],|row|row.get(0)).unwrap();
            let digest: [u8; 32] = Sha256::digest(b"abc").into();
            fixture
                .store
                .connection
                .execute(
                    "UPDATE native_original_fields SET digest=?3 WHERE original=?1 AND ordinal=?2",
                    params![
                        fixture.repairs[0].original.as_slice(),
                        ordinal,
                        digest.as_slice()
                    ],
                )
                .unwrap();
            let parent = whole(&mut fixture);
            let slice = complete(
                &mut fixture,
                &ViewRequest::Slice {
                    parent,
                    start: 1,
                    end: 4,
                },
                &[],
            )
            .unwrap();
            assert_eq!(rendered(&fixture, &slice), "abc");
            let target = complete(
                &mut fixture,
                &ViewRequest::WholeField {
                    record: 0,
                    ordinal: ordinal as u64,
                },
                &[],
            )
            .unwrap();
            assert_eq!(rendered(&fixture, &target), "abd");
            assert_eq!(slice.receipt, target.receipt);
            clear_derived_classes(&fixture);
            let (a, b) = if view_first {
                (
                    identity(&mut fixture, Some(&slice), 0),
                    identity(&mut fixture, None, ordinal as u64),
                )
            } else {
                (
                    identity(&mut fixture, None, ordinal as u64),
                    identity(&mut fixture, Some(&slice), 0),
                )
            };
            assert_ne!(a, b);
            let count: i64 = fixture
                .store
                .connection
                .query_row("SELECT count(*) FROM native_original_classes", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 2);
        }
    }
    #[test]
    fn typed_class_legacy_reference_backfill_is_bounded_durable_and_cancellable() {
        let mut fixture = make_fixture("legacy");
        let ordinal: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT ordinal FROM native_original_classes WHERE class=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        fixture.store.connection.execute_batch("DELETE FROM native_original_class_values; DELETE FROM native_original_text_policy;").unwrap();
        for class in 2..=33 {
            fixture.store.connection.execute("INSERT INTO native_original_classes SELECT original,?1,record_offset,ordinal,digest,decoded_bytes FROM native_original_classes WHERE class=1",[class]).unwrap();
        }
        let cancelled = fixture
            .store
            .original_identity_step(
                &fixture.job,
                &fixture.witnesses[0],
                &fixture.repairs[0],
                ordinal as u64,
                &fixture.sources.reader(),
                &|| true,
            )
            .unwrap();
        assert_eq!(cancelled.outcome, Outcome::Deferred);
        assert_eq!(cancelled.work.native_bytes, 0);
        let before: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_original_text_policy",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(before, 0);
        let mut after = 0;
        for _ in 0..9 {
            let actual = identity(&mut fixture, None, ordinal as u64);
            assert_eq!(actual.identity(), 1);
            let (through, next): (i64, i64) = fixture
                .store
                .connection
                .query_row(
                    "SELECT legacy_through,backfilled_through FROM native_original_text_policy",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(through, 33);
            assert!(next > after);
            assert!(next - after <= 4);
            after = next;
        }
        assert_eq!(after, 33);
        let refs: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_original_class_values",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(refs, 33);
    }
    #[test]
    fn concurrent_whole_allocation_refuses_a_stale_view_interner_generation() {
        let encoded = format!(
            "{},\"t\":{}",
            serde_json::to_string("abc").unwrap(),
            serde_json::to_string("def").unwrap()
        );
        let mut fixture = Fixture::new(&[encoded]);
        let parent = whole(&mut fixture);
        let view = complete(
            &mut fixture,
            &ViewRequest::Slice {
                parent,
                start: 0,
                end: 3,
            },
            &[],
        )
        .unwrap();
        let ordinal:i64=fixture.store.connection.query_row("SELECT ordinal FROM native_original_fields WHERE original=?1 AND is_key=0 AND decoded_bytes=3 ORDER BY ordinal DESC LIMIT 1",params![fixture.repairs[0].original.as_slice()],|row|row.get(0)).unwrap();
        let options = fixture.options.clone();
        let job = fixture.job.clone();
        let witness = fixture.witnesses[0].clone();
        let repair = fixture.repairs[0].clone();
        let sources = fixture.sources.clone();
        let provider = Provider::new(
            fixture.repairs[0].source.clone(),
            &fixture.bytes[0],
            None,
            move |call| {
                if call != 0 {
                    return;
                }
                let mut other = NativeStore::open(&options).unwrap();
                let step = other
                    .original_identity_step(
                        &job,
                        &witness,
                        &repair,
                        ordinal as u64,
                        &sources.reader(),
                        &|| false,
                    )
                    .unwrap();
                assert_eq!(step.outcome, Outcome::Written);
                assert_eq!(step.text.unwrap().identity(), 2);
            },
        );
        let records = fixture.sources.reader_with(Some((
            &provider,
            crate::SessionSourceReadMode::ProviderOnly,
        )));
        let step = fixture
            .store
            .original_view_identity_step(
                &fixture.job,
                &fixture.witnesses[0],
                &fixture.repairs[0],
                &view,
                &records,
                &|_, _| Ok(true),
                &|| false,
                crate::SESSION_SOURCE_CHUNK_MAX,
                ROWS,
            )
            .unwrap();
        assert_eq!(step.outcome, Outcome::Conflict);
        assert!(step.text.is_none());
        assert!(step.work.native_bytes > 0);
        let cursors: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_original_view_identities",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(cursors, 0);
        drop(records);
        let actual = identity(&mut fixture, Some(&view), 0);
        assert_eq!(actual.identity(), 1);
        let count: i64 = fixture
            .store
            .connection
            .query_row("SELECT count(*) FROM native_original_classes", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
    }
    #[test]
    fn empty_join_requires_a_real_qualified_native_record() {
        let mut fixture = make_fixture("native record");
        let step = step(
            &mut fixture,
            &ViewRequest::Join {
                record: 1,
                producer: 999,
                count: 0,
                newline: false,
            },
            &[],
            &|| false,
        );
        assert_eq!(step.outcome, Outcome::Conflict);
        assert!(step.view.is_none());
        assert_eq!(step.work.native_bytes, 0);
    }
    #[test]
    fn empty_join_binds_real_original_and_shares_empty_whole_class() {
        for view_first in [false, true] {
            let mut fixture = make_fixture("");
            let ordinal: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT ordinal FROM native_original_classes WHERE class=1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let empty = complete(
                &mut fixture,
                &ViewRequest::Join {
                    record: 0,
                    producer: 77,
                    count: 0,
                    newline: false,
                },
                &[],
            )
            .unwrap();
            clear_derived_classes(&fixture);
            let (a, b) = if view_first {
                (
                    identity(&mut fixture, Some(&empty), 0),
                    identity(&mut fixture, None, ordinal as u64),
                )
            } else {
                (
                    identity(&mut fixture, None, ordinal as u64),
                    identity(&mut fixture, Some(&empty), 0),
                )
            };
            assert_eq!(a, b);
            assert_eq!(a.original(), fixture.repairs[0].original);
            let representative = fixture
                .store
                .original_text_representative(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.repairs[0],
                    &a,
                )
                .unwrap();
            assert_eq!(representative.receipt.decoded_bytes, 0);
            assert_eq!(representative.source, fixture.repairs[0].source);
            if view_first {
                fixture.store.connection.execute("UPDATE native_original_classes SET ordinal=0 WHERE original=?1 AND class=?2",params![a.original().as_slice(),seq_u64(a.identity()).unwrap()]).unwrap();
                assert!(
                    fixture
                        .store
                        .original_text_representative(
                            &fixture.job,
                            &fixture.witnesses[0],
                            &fixture.repairs[0],
                            &a
                        )
                        .is_err()
                );
            }
        }
    }
    #[test]
    fn durable_order_upgrades_unequal_length_equality_and_qualifies_view_representatives() {
        let mut fixture = Fixture::new(&[
            serde_json::to_string("'z'").unwrap(),
            serde_json::to_string("aa").unwrap(),
        ]);
        let parent = whole(&mut fixture);
        let view = complete(
            &mut fixture,
            &ViewRequest::JsLiteralDecode {
                parent,
                quote: Quote::Single,
            },
            &[],
        )
        .unwrap();
        let left = identity(&mut fixture, Some(&view), 0);
        let right = OriginalText::qualified(fixture.repairs[1].original, 1);
        for order in [false, true, true] {
            let mut finished = false;
            for _ in 0..32 {
                let records = fixture.sources.reader();
                let a = ComparisonSource {
                    witness: &fixture.witnesses[0],
                    repair: &fixture.repairs[0],
                    text: &left,
                    records: &records,
                };
                let b = ComparisonSource {
                    witness: &fixture.witnesses[1],
                    repair: &fixture.repairs[1],
                    text: &right,
                    records: &records,
                };
                let step = if order {
                    fixture.store.original_text_order_step(
                        &fixture.job,
                        a,
                        b,
                        &|_, _| Ok(true),
                        &|| false,
                    )
                } else {
                    fixture.store.original_compare_step(
                        &fixture.job,
                        a,
                        b,
                        &|_, _| Ok(true),
                        &|| false,
                    )
                }
                .unwrap();
                assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.comparison_sql_vm_steps < 4096);
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                if step.outcome == Outcome::Written {
                    assert_eq!(step.comparison, Some(Comparison::Different));
                    if order {
                        assert_eq!(step.order, Some(NativeTextOrder::Greater));
                    }
                    finished = true;
                    break;
                }
            }
            assert!(finished);
        }
    }

    #[test]
    fn zero_output_boundaries_keep_latest_qualified_raw_position_for_late_slices() {
        let literal = format!("'{}selected'", "\\\n".repeat(100_000));
        let mut fixture = make_fixture(&literal);
        let parent = whole(&mut fixture);
        let decoded = complete(
            &mut fixture,
            &ViewRequest::JsLiteralDecode {
                parent,
                quote: Quote::Single,
            },
            &[],
        )
        .unwrap();
        let mut cursor = TextScanCursor::begin(&decoded);
        let mut marks = Vec::new();
        for _ in 0..32 {
            let records = fixture.sources.reader();
            let step = fixture
                .store
                .original_text_scan_step(
                    &fixture.job,
                    &fixture.witnesses[0],
                    &fixture.repairs[0],
                    &cursor,
                    &[0, 8],
                    &records,
                    &|_, _| Ok(true),
                    &|| false,
                    crate::SESSION_SOURCE_CHUNK_MAX,
                    ROWS,
                )
                .unwrap();
            assert!(step.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            marks.extend(step.marks);
            cursor = step.cursor;
            if step.complete {
                break;
            }
        }
        let first = marks
            .iter()
            .find(|mark| mark.decoded() == 0)
            .unwrap()
            .clone();
        let last = marks
            .iter()
            .find(|mark| mark.decoded() == 8)
            .unwrap()
            .clone();
        let request = ViewRequest::SliceMarked {
            parent: decoded,
            start: first,
            end: last,
        };
        let first = step(&mut fixture, &request, &[], &|| false);
        assert!(
            first.work.native_bytes <= 8,
            "marked JS slice replayed its removed prefix: {}",
            first.work.native_bytes
        );
        let view = complete(&mut fixture, &request, &[]).unwrap();
        assert_eq!(rendered(&fixture, &view), "selected");
    }
    #[test]
    fn ordering_remaining_budget_defers_before_io_and_restarts_long_common_prefix() {
        let prefix = "é".repeat(1024);
        let mut fixture = Fixture::new(&[
            serde_json::to_string(&format!("{prefix}a")).unwrap(),
            serde_json::to_string(&format!("{prefix}b")).unwrap(),
        ]);
        let left = OriginalText::qualified(fixture.repairs[0].original, 1);
        let right = OriginalText::qualified(fixture.repairs[1].original, 1);
        for (native, rows) in [(0, 40), (291, 40), (292, 39)] {
            let records = fixture.sources.reader();
            let a = ComparisonSource {
                witness: &fixture.witnesses[0],
                repair: &fixture.repairs[0],
                text: &left,
                records: &records,
            };
            let b = ComparisonSource {
                witness: &fixture.witnesses[1],
                repair: &fixture.repairs[1],
                text: &right,
                records: &records,
            };
            let step = fixture
                .store
                .original_text_order_step_with_budget(
                    &fixture.job,
                    a,
                    b,
                    &|_, _| Ok(true),
                    &|| false,
                    native,
                    rows,
                )
                .unwrap();
            assert_eq!(step.outcome, Outcome::Deferred);
            assert_eq!(step.work.native_bytes, 0);
            assert_eq!(step.work.metadata_rows, 0);
        }
        let mut completed = false;
        let mut deferred = 0;
        for _ in 0..512 {
            let records = fixture.sources.reader();
            let a = ComparisonSource {
                witness: &fixture.witnesses[0],
                repair: &fixture.repairs[0],
                text: &left,
                records: &records,
            };
            let b = ComparisonSource {
                witness: &fixture.witnesses[1],
                repair: &fixture.repairs[1],
                text: &right,
                records: &records,
            };
            let step = fixture
                .store
                .original_text_order_step_with_budget(
                    &fixture.job,
                    a,
                    b,
                    &|_, _| Ok(true),
                    &|| false,
                    292,
                    40,
                )
                .unwrap();
            assert!(step.work.native_bytes <= 292);
            assert!(step.work.metadata_rows <= 40);
            assert!(step.work.metadata_bytes <= BYTES);
            assert!(step.comparison_sql_vm_steps < 4096);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            if let Some(order) = step.order {
                assert_eq!(order, NativeTextOrder::Less);
                completed = true;
                break;
            }
            deferred += 1;
            assert_eq!(step.outcome, Outcome::Deferred);
        }
        assert!(completed);
        assert!(deferred > 1);
    }

    fn exact_order(
        fixture: &Fixture,
        left: &QualifiedTextView,
        right: &QualifiedTextView,
    ) -> super::super::native_values::NativeTextOrder {
        use super::super::native_values::{Pair, Value, ValueMeter};
        let l = Value::View(left.clone());
        let r = Value::View(right.clone());
        let mut pair = Pair::new(&l, &r).unwrap();
        for _ in 0..8192 {
            let records = fixture.sources.reader();
            let mut work = RepairWork::default();
            let mut meter = ValueMeter::default();
            assert!(pair.matches(&l, &r).unwrap());
            assert!(
                pair.step(
                    &fixture.store.connection,
                    &fixture.repairs[0],
                    &l,
                    &fixture.repairs[0],
                    &r,
                    &records,
                    &records,
                    crate::SESSION_SOURCE_CHUNK_MAX,
                    ROWS,
                    &mut work,
                    &mut meter
                )
                .unwrap()
            );
            records.finish().unwrap();
            assert!(work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
            assert!(work.metadata_rows <= ROWS);
            assert!(meter.peak_native <= 64 * 1024);
            assert!(meter.peak_decoded <= 3 * 32 * 1024);
            let bytes = serde_json::to_vec(&pair).unwrap();
            assert!(bytes.len() < STATE_BYTES);
            pair = serde_json::from_slice(&bytes).unwrap();
            assert!(pair.matches(&l, &r).unwrap());
            if let Some(order) = pair.result {
                return order;
            }
        }
        panic!("generic text ordering failed to make progress")
    }
    #[test]
    fn generic_view_order_matches_native_string_order_including_equal_transforms() {
        use super::super::native_values::NativeTextOrder;
        let mut fixture = make_fixture("a\0é中😀z");
        let parent = whole(&mut fixture);
        let mut views = Vec::new();
        for (start, end) in [
            (0, 0),
            (0, 1),
            (0, 2),
            (2, 4),
            (4, 7),
            (7, 11),
            (11, 12),
            (0, 12),
        ] {
            views.push(
                complete(
                    &mut fixture,
                    &ViewRequest::Slice {
                        parent: parent.clone(),
                        start,
                        end,
                    },
                    &[],
                )
                .unwrap(),
            );
        }
        for left in &views {
            for right in &views {
                let expected = rendered(&fixture, left).cmp(&rendered(&fixture, right));
                assert_eq!(
                    exact_order(&fixture, left, right),
                    NativeTextOrder::from(expected)
                );
            }
        }
        let empty = complete(
            &mut fixture,
            &ViewRequest::Join {
                record: 0,
                producer: 91,
                count: 0,
                newline: false,
            },
            &[],
        )
        .unwrap();
        assert_eq!(
            exact_order(&fixture, &empty, &views[0]),
            NativeTextOrder::Equal
        );
        let mut fixture = make_fixture(&format!("'{}abc'", "\\\n".repeat(100_000)));
        let parent = whole(&mut fixture);
        let decoded = complete(
            &mut fixture,
            &ViewRequest::JsLiteralDecode {
                parent: parent.clone(),
                quote: Quote::Single,
            },
            &[],
        )
        .unwrap();
        let length = parent.receipt.decoded_bytes;
        let plain = complete(
            &mut fixture,
            &ViewRequest::Slice {
                parent,
                start: length - 4,
                end: length - 1,
            },
            &[],
        )
        .unwrap();
        assert_eq!(
            exact_order(&fixture, &decoded, &plain),
            NativeTextOrder::Equal
        );
        assert_eq!(
            exact_order(&fixture, &plain, &decoded),
            NativeTextOrder::Equal
        );
    }

    #[test]
    fn obsolete_view_parts_and_references_retire_without_decoding_unknown_cursors() {
        let mut fixture = make_fixture("retained original");
        let current = whole(&mut fixture);
        let obsolete = format_two_namespace(&fixture.repairs[0].source).unwrap();
        {
            let tx = fixture.store.connection.transaction().unwrap();
            for id in 1..=128 {
                tx.execute("INSERT INTO native_original_text_views VALUES(?1,?2,0,?3,'{unknown view',NULL)",params![obsolete.as_slice(),id,format!("obsolete-{id}")]).unwrap();
                tx.execute(
                    "INSERT INTO native_original_text_parts VALUES(?1,?2,0,'{unknown parts')",
                    params![obsolete.as_slice(), id],
                )
                .unwrap();
                tx.execute("INSERT INTO native_original_text_boundaries VALUES(?1,?2,0,'{unknown boundary')",params![obsolete.as_slice(),id]).unwrap();
                tx.execute("INSERT INTO native_original_view_identities VALUES(?1,?2,0,'{unknown identity')",params![obsolete.as_slice(),id]).unwrap();
                tx.execute(
                    "INSERT INTO native_original_class_values VALUES(?1,?2,?2)",
                    params![obsolete.as_slice(), id],
                )
                .unwrap();
            }
            tx.execute(
                "INSERT INTO native_original_text_policy VALUES(?1,1,128,0)",
                params![obsolete.as_slice()],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO native_original_text_epochs VALUES(?1,0,1)",
                params![obsolete.as_slice()],
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
            assert_eq!(step.work.native_bytes, 0);
            assert!(step.work.cleanup_vm_steps < 4096);
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
            steps += 1;
            assert!(steps < 40);
        }
        assert!(steps > 20);
        for table in [
            "native_original_text_views",
            "native_original_text_parts",
            "native_original_text_boundaries",
            "native_original_view_identities",
            "native_original_class_values",
            "native_original_text_policy",
            "native_original_text_epochs",
        ] {
            let count: i64 = fixture
                .store
                .connection
                .query_row(
                    &format!("SELECT count(*) FROM {table} WHERE original=?1"),
                    params![obsolete.as_slice()],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0);
        }
        assert_eq!(rendered(&fixture, &current), "retained original");
    }
    #[test]
    fn epoch_retirement_precedes_evidence_across_cancellation_and_every_reopen() {
        for epochs in [1, 65] {
            let mut fixture = make_fixture("retained original");
            let current = whole(&mut fixture);
            let repair = fixture.repairs[0].clone();
            let obsolete = format_two_namespace(&repair.source).unwrap();
            {
                let tx = fixture.store.connection.transaction().unwrap();
                for record in 0..epochs {
                    tx.execute(
                        "INSERT INTO native_original_text_epochs VALUES(?1,?2,1)",
                        params![obsolete.as_slice(), record],
                    )
                    .unwrap();
                }
                tx.execute("INSERT INTO native_original_text_views VALUES(?1,1,0,'old','{unknown view',NULL)", params![obsolete.as_slice()]).unwrap();
                tx.execute(
                    "INSERT INTO native_original_classes VALUES(?1,1,0,0,zeroblob(32),4)",
                    params![obsolete.as_slice()],
                )
                .unwrap();
                tx.execute("INSERT INTO native_original_fields VALUES(?1,0,0,0,1,NULL,0,'{unknown field','{unknown fingerprint',NULL,NULL)", params![obsolete.as_slice()]).unwrap();
                tx.execute(
                    "INSERT INTO native_original_records VALUES(?1,0,1,'folded')",
                    params![obsolete.as_slice()],
                )
                .unwrap();
                tx.commit().unwrap();
            }
            native_retirement::enqueue(&fixture.store.connection, repair.original, &repair.source)
                .unwrap();
            let count = |fixture: &Fixture, table: &str| -> i64 {
                fixture
                    .store
                    .connection
                    .query_row(
                        &format!("SELECT count(*) FROM {table} WHERE original=?1"),
                        params![obsolete.as_slice()],
                        |row| row.get(0),
                    )
                    .unwrap()
            };
            let evidence = [
                "native_original_text_views",
                "native_original_classes",
                "native_original_fields",
                "native_original_records",
            ];
            let calls = Cell::new(0);
            let cancel = || {
                calls.set(calls.get() + 1);
                calls.get() == 2
            };
            let cancelled = native_retirement::advance(
                &mut fixture.store,
                &fixture.job,
                &fixture.witnesses[0],
                &repair,
                &fixture.sources.reader(),
                &cancel,
            )
            .unwrap();
            assert_eq!(cancelled.outcome, Outcome::Deferred);
            fixture.store = NativeStore::open(&fixture.options).unwrap();
            assert_eq!(count(&fixture, "native_original_text_epochs"), epochs);
            for table in evidence {
                assert_eq!(count(&fixture, table), 1);
            }
            let mut reclaimed = false;
            for _ in 0..32 {
                if !native_retirement::pending(&fixture.store.connection, repair.original).unwrap()
                {
                    break;
                }
                let step = native_retirement::advance(
                    &mut fixture.store,
                    &fixture.job,
                    &fixture.witnesses[0],
                    &repair,
                    &fixture.sources.reader(),
                    &|| false,
                )
                .unwrap();
                assert!(step.work.metadata_rows <= ROWS);
                assert!(step.work.metadata_bytes <= BYTES);
                assert!(step.work.cleanup_vm_steps < 4096);
                assert!(step.work.cleanup_key_bytes <= STATE_BYTES);
                assert_eq!((step.work.range_requests, step.work.native_bytes), (0, 0));
                fixture.store = NativeStore::open(&fixture.options).unwrap();
                let left = count(&fixture, "native_original_text_epochs");
                for table in evidence {
                    if count(&fixture, table) == 0 {
                        reclaimed = true;
                        assert_eq!(left, 0, "{table} retired before epochs");
                    }
                }
                if left > 0 {
                    for table in evidence {
                        assert_eq!(count(&fixture, table), 1);
                    }
                }
            }
            assert!(reclaimed);
            assert!(
                !native_retirement::pending(&fixture.store.connection, repair.original).unwrap()
            );
            assert_eq!(count(&fixture, "native_original_text_epochs"), 0);
            for table in evidence {
                assert_eq!(count(&fixture, table), 0);
            }
            assert_eq!(rendered(&fixture, &current), "retained original");
        }
    }
    #[test]
    fn view_cancellation_before_commit_preserves_no_partial_publication() {
        let mut fixture = make_fixture("selected");
        let ordinal: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT ordinal FROM native_original_classes WHERE original=?1 AND class=1",
                params![fixture.repairs[0].original.as_slice()],
                |row| row.get(0),
            )
            .unwrap();
        let request = ViewRequest::WholeField {
            record: 0,
            ordinal: ordinal as u64,
        };
        let calls = Cell::new(0);
        let cancelled = || {
            calls.set(calls.get() + 1);
            calls.get() == 3
        };
        assert_eq!(
            step(&mut fixture, &request, &[], &cancelled).outcome,
            Outcome::Deferred
        );
        let count: i64 = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM native_original_text_views",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        assert!(complete(&mut fixture, &request, &[]).is_some());
    }
    #[test]
    fn cached_views_reauthorize_retained_providers_and_related_membership_after_io() {
        for change in ["third", "new", "field", "parent"] {
            let encoded = serde_json::to_string("selected").unwrap();
            let mut fixture = Fixture::new(&[encoded.clone(), encoded.clone(), encoded]);
            fixture.store.connection.execute_batch(SCHEMA).unwrap();
            let view = whole(&mut fixture);
            let ordinal: i64 = fixture
                .store
                .connection
                .query_row(
                    "SELECT ordinal FROM native_original_classes WHERE original=?1 AND class=1",
                    params![fixture.repairs[0].original.as_slice()],
                    |row| row.get(0),
                )
                .unwrap();
            let request = if change == "parent" {
                let request = ViewRequest::Slice {
                    parent: view.clone(),
                    start: 0,
                    end: 8,
                };
                complete(&mut fixture, &request, &[]).unwrap();
                request
            } else {
                ViewRequest::WholeField {
                    record: 0,
                    ordinal: ordinal as u64,
                }
            };
            let parent_id = view.id;
            let cache = super::super::super::super::super::EventCache::path(&fixture.options.cache);
            let original = fixture.repairs[0].original;
            let provider = Provider::new(
                fixture.repairs[0].source.clone(),
                &fixture.bytes[0],
                None,
                move |call| {
                    if call != 0 {
                        return;
                    }
                    let db = Connection::open(&cache).unwrap();
                    match change {
                        "third" => {
                            db.execute("UPDATE session_catalog SET metadata=metadata WHERE session_key='owner-2'",[]).unwrap();
                        }
                        "new" => {
                            db.execute("INSERT INTO comparison_required_owners VALUES('new-related-owner')",[]).unwrap();
                        }
                        "field" => {
                            db.execute("UPDATE native_original_fields SET decoded_bytes=decoded_bytes+1 WHERE original=?1 AND is_key=0",params![original.as_slice()]).unwrap();
                        }
                        "parent" => {
                            db.execute("UPDATE native_original_text_views SET receipt=NULL WHERE original=?1 AND view_id=?2",params![original.as_slice(), seq_u64(parent_id).unwrap()]).unwrap();
                        }
                        _ => unreachable!(),
                    }
                },
            );
            let records = fixture.sources.reader_with(Some((
                &provider,
                crate::SessionSourceReadMode::ProviderOnly,
            )));
            let result=fixture.store.original_text_view_step(&fixture.job,&fixture.witnesses[0],&fixture.repairs[0],&request,&mut|_|Err(io::Error::other("no Join parent")),&records,&|db,job|db.query_row("SELECT NOT EXISTS(SELECT 1 FROM comparison_required_owners r LEFT JOIN native_query_owners w ON w.job=?1 AND w.session_key=r.key WHERE w.session_key IS NULL)",[job],|row|row.get(0)),&||false,crate::SESSION_SOURCE_CHUNK_MAX,ROWS).unwrap();
            assert_eq!(result.outcome, Outcome::Conflict);
            assert!(result.view.is_none());
            assert_eq!(view.receipt.decoded_bytes, 8);
        }
        let mut fixture = make_fixture("selected");
        let view = whole(&mut fixture);
        fs::remove_file(&fixture.witnesses[0].source.path).unwrap();
        let provider = Provider::new(
            fixture.repairs[0].source.clone(),
            &fixture.bytes[0],
            None,
            |_| {},
        );
        let records = fixture.sources.reader_with(Some((
            &provider,
            crate::SessionSourceReadMode::ProviderOnly,
        )));
        let scan = fixture
            .store
            .original_text_scan_step(
                &fixture.job,
                &fixture.witnesses[0],
                &fixture.repairs[0],
                &TextScanCursor::begin(&view),
                &[],
                &records,
                &|_, _| Ok(true),
                &|| false,
                crate::SESSION_SOURCE_CHUNK_MAX,
                ROWS,
            )
            .unwrap();
        assert_eq!(scan.outcome, Outcome::Written);
        assert_eq!(scan.bytes, b"selected");
        assert!(scan.complete);
        assert_eq!(provider.observed(), (2, fixture.bytes[0].len() * 2));
    }
    #[test]
    fn recorded_scalar_boundaries_avoid_replaying_a_late_slice_prefix() {
        for prefix in [0, 128 * 1024, 1024 * 1024] {
            let text = format!("{}selected", "x".repeat(prefix));
            let mut fixture = make_fixture(&text);
            let parent = whole(&mut fixture);
            let mut cursor = TextScanCursor::begin(&parent);
            let start = prefix as u64;
            let end = start + 8;
            let mut marks = Vec::new();
            for _ in 0..128 {
                let records = fixture.sources.reader();
                let scan = fixture
                    .store
                    .original_text_scan_step(
                        &fixture.job,
                        &fixture.witnesses[0],
                        &fixture.repairs[0],
                        &cursor,
                        &[start, end],
                        &records,
                        &|_, _| Ok(true),
                        &|| false,
                        crate::SESSION_SOURCE_CHUNK_MAX,
                        ROWS,
                    )
                    .unwrap();
                assert_eq!(scan.outcome, Outcome::Written);
                assert!(std::str::from_utf8(&scan.bytes).is_ok());
                assert!(scan.work.native_bytes <= crate::SESSION_SOURCE_CHUNK_MAX);
                assert!(scan.work.metadata_rows <= ROWS);
                cursor = scan.cursor;
                marks.extend(scan.marks);
                if scan.complete {
                    break;
                }
            }
            let first = marks
                .iter()
                .find(|mark| mark.decoded == start)
                .unwrap()
                .clone();
            let last = marks
                .iter()
                .find(|mark| mark.decoded == end)
                .unwrap()
                .clone();
            let request = ViewRequest::SliceMarked {
                parent,
                start: first,
                end: last,
            };
            let first = step(&mut fixture, &request, &[], &|| false);
            assert!(
                first.work.native_bytes <= 8,
                "late slice reread its prefix: {}",
                first.work.native_bytes
            );
            let view = complete(&mut fixture, &request, &[]).unwrap();
            assert_eq!(rendered(&fixture, &view), "selected");
        }
    }
    #[test]
    fn qualified_slices_match_native_decoded_ranges_and_refuse_split_unicode() {
        let text = "prefix:aé𝄞:suffix";
        let mut fixture = make_fixture(text);
        let parent = whole(&mut fixture);
        let start = text.find('a').unwrap() as u64;
        let end = start + "aé𝄞".len() as u64;
        let view = complete(
            &mut fixture,
            &ViewRequest::Slice {
                parent: parent.clone(),
                start,
                end,
            },
            &[],
        )
        .unwrap();
        assert_eq!(
            rendered(&fixture, &view),
            &text[start as usize..end as usize]
        );
        let expected: [u8; 32] = Sha256::digest("aé𝄞".as_bytes()).into();
        assert_eq!(view.receipt.sha256, expected);
        assert!(
            complete(
                &mut fixture,
                &ViewRequest::Slice {
                    parent: parent.clone(),
                    start: start + 2,
                    end
                },
                &[]
            )
            .is_none()
        );
        let empty = complete(
            &mut fixture,
            &ViewRequest::Slice {
                parent,
                start: end,
                end,
            },
            &[],
        )
        .unwrap();
        assert_eq!(rendered(&fixture, &empty), "");
    }

    #[test]
    fn qualified_native_json_and_js_decoders_match_complete_behavior() {
        let nested = "\"a\\u0062\\uD834\\uDD1E\"";
        let expected: String = serde_json::from_str(nested).unwrap();
        let mut fixture = make_fixture(nested);
        let parent = whole(&mut fixture);
        let view = complete(&mut fixture, &ViewRequest::JsonStringDecode { parent }, &[]).unwrap();
        assert_eq!(rendered(&fixture, &view), expected);
        for (literal, expected) in [
            ("'a\\x62\\u0063\\u{1f600}'", Some("abc😀")),
            ("'\\uD800\\xZZ\\u{}'", Some("�xZZu{}")),
            ("'a\\\nb'", Some("ab")),
            ("`a${x}`", None),
        ] {
            let mut fixture = make_fixture(literal);
            let parent = whole(&mut fixture);
            let quote = Quote::of(literal.chars().next().unwrap()).unwrap();
            let view = complete(
                &mut fixture,
                &ViewRequest::JsLiteralDecode { parent, quote },
                &[],
            );
            assert_eq!(
                view.as_ref().map(|view| rendered(&fixture, view)),
                expected.map(str::to_owned)
            );
        }
    }

    #[test]
    fn normalized_join_and_slice_preserve_parts_and_empty_join() {
        let mut fixture = make_fixture("abXXé");
        let parent = whole(&mut fixture);
        let left = complete(
            &mut fixture,
            &ViewRequest::Slice {
                parent: parent.clone(),
                start: 0,
                end: 2,
            },
            &[],
        )
        .unwrap();
        let right = complete(
            &mut fixture,
            &ViewRequest::Slice {
                parent,
                start: 4,
                end: 6,
            },
            &[],
        )
        .unwrap();
        let join = complete(
            &mut fixture,
            &ViewRequest::Join {
                record: 0,
                producer: 1,
                count: 2,
                newline: true,
            },
            &[left, right],
        )
        .unwrap();
        assert_eq!(rendered(&fixture, &join), "ab\né");
        let slice = complete(
            &mut fixture,
            &ViewRequest::Slice {
                parent: join,
                start: 1,
                end: 5,
            },
            &[],
        )
        .unwrap();
        assert_eq!(rendered(&fixture, &slice), "b\né");
        let empty = complete(
            &mut fixture,
            &ViewRequest::Join {
                record: 0,
                producer: 2,
                count: 0,
                newline: false,
            },
            &[],
        )
        .unwrap();
        assert_eq!(rendered(&fixture, &empty), "");
    }

    #[test]
    fn js_zero_output_gaps_resume_without_plaintext_or_whole_prefix_replay() {
        let literal = format!("'prefix{}tail'", "\\\n".repeat(100_000));
        let mut fixture = make_fixture(&literal);
        let parent = whole(&mut fixture);
        let request = ViewRequest::JsLiteralDecode {
            parent,
            quote: Quote::Single,
        };
        let first = step(&mut fixture, &request, &[], &|| false);
        assert_eq!(first.outcome, Outcome::Deferred);
        let before: Vec<(i64, String)> = fixture
            .store
            .connection
            .prepare("SELECT view_id,cursor FROM native_original_text_views ORDER BY view_id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            step(&mut fixture, &request, &[], &|| true)
                .work
                .native_bytes,
            0
        );
        fixture.store = NativeStore::open(&fixture.options).unwrap();
        let after: Vec<(i64, String)> = fixture
            .store
            .connection
            .prepare("SELECT view_id,cursor FROM native_original_text_views ORDER BY view_id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(before, after);
        let view = complete(&mut fixture, &request, &[]).unwrap();
        assert_eq!(rendered(&fixture, &view), "prefixtail");
        let expected: [u8; 32] = Sha256::digest(b"prefixtail").into();
        assert_eq!(view.receipt.sha256, expected);
        for (_, state) in after {
            assert!(!state.contains("prefix") && !state.contains("tail"));
        }
    }

    #[test]
    fn cached_view_page_work_is_fixed_and_parent_descriptor_changes_refuse() {
        let mut fixture = make_fixture("selected");
        let parent = whole(&mut fixture);
        let request = ViewRequest::Slice {
            parent,
            start: 1,
            end: 7,
        };
        let view = complete(&mut fixture, &request, &[]).unwrap();
        let mut baseline = None;
        let mut previous = 0;
        for history in [0, 128, 1024, 8192] {
            {
                let tx = fixture.store.connection.transaction().unwrap();
                for i in previous..history {
                    tx.execute(
                        "INSERT INTO native_original_text_views VALUES(?1,?2,0,?3,'{}',NULL)",
                        params![
                            fixture.repairs[0].original.as_slice(),
                            i + 100_000,
                            format!("unrelated-{i}")
                        ],
                    )
                    .unwrap();
                }
                tx.commit().unwrap();
            }
            previous = history;
            let result = step(&mut fixture, &request, &[], &|| false);
            assert_eq!(result.view, Some(view.clone()));
            assert_eq!(result.work.native_bytes, 1);
            let fixed = (
                result.work.metadata_rows,
                result.work.metadata_bytes,
                result.work.native_bytes,
            );
            assert_eq!(*baseline.get_or_insert(fixed), fixed);
        }
        fixture.store.connection.execute("UPDATE native_original_fields SET decoded_bytes=decoded_bytes+1 WHERE original=?1 AND is_key=0",params![fixture.repairs[0].original.as_slice()]).unwrap();
        assert_eq!(
            step(&mut fixture, &request, &[], &|| false).outcome,
            Outcome::Conflict
        );
    }
}
