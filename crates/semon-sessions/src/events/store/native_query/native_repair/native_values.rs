//! One decoded reader and ordering decision for whole fields and producer views.
//! Digests select candidates; only retained decoded bytes establish order/equality.
use super::native_views::{QualifiedTextView, ReadCursor, ViewMeter};
use super::*;
use crate::json_string::native_identity::{Stream, decoded_order};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) enum Value {
    Whole {
        record: u64,
        field: OriginalField,
        receipt: Receipt,
    },
    View(QualifiedTextView),
}
impl Value {
    pub(super) fn record(&self) -> u64 {
        match self {
            Self::Whole { record, .. } => *record,
            Self::View(view) => view.record(),
        }
    }
    pub(super) fn receipt(&self) -> Receipt {
        match self {
            Self::Whole { receipt, .. } => *receipt,
            Self::View(view) => view.receipt(),
        }
    }
    pub(super) fn legacy_ordinal(&self) -> io::Result<i64> {
        match self {
            Self::Whole { field, .. } => seq_u64(field.ordinal),
            Self::View(_) => Ok(-1),
        }
    }
    pub(super) fn view_id(&self) -> Option<u64> {
        match self {
            Self::Whole { .. } => None,
            Self::View(view) => Some(view.id()),
        }
    }
    fn bounds(&self) -> io::Result<Option<(u64, u64)>> {
        match self {
            Self::Whole { record, field, .. } => Ok(Some((
                record
                    .checked_add(field.span.start)
                    .ok_or_else(|| io::Error::other("native text start overflow"))?,
                record
                    .checked_add(field.span.end)
                    .ok_or_else(|| io::Error::other("native text end overflow"))?,
            ))),
            Self::View(_) => Ok(None),
        }
    }
    pub(super) fn current(
        &self,
        db: &Connection,
        repair: &Repair,
        work: &mut RepairWork,
        meter: &mut ValueMeter,
    ) -> io::Result<bool> {
        match self {
            Self::Whole {
                record,
                field,
                receipt,
            } => {
                let (current, proof, vm) =
                    native_classes::qualified_field_at(db, repair, *record, field.ordinal, work)?;
                meter.vm += vm;
                Ok(serde_json::to_vec(&current).map_err(io::Error::other)?
                    == serde_json::to_vec(field).map_err(io::Error::other)?
                    && proof == *receipt)
            }
            Self::View(view) => {
                let mut view_meter = ViewMeter::default();
                let result = native_views::view_current(db, repair, view, work, &mut view_meter);
                meter.vm += view_meter.vm;
                result
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Position {
    Whole(Stream),
    View(ReadCursor),
}
impl Position {
    fn begin(value: &Value) -> io::Result<Self> {
        Ok(match value.bounds()? {
            Some((start, end)) => Self::Whole(Stream::new(start, end)?),
            None => Self::View(ReadCursor::default()),
        })
    }
    fn complete(&self, value: &Value) -> bool {
        match (self, value) {
            (Self::Whole(stream), Value::Whole { .. }) => stream.complete(),
            (Self::View(position), Value::View(view)) => position.complete_for(view),
            _ => false,
        }
    }
    fn valid(&self, value: &Value) -> io::Result<bool> {
        Ok(match (self, value) {
            (Self::Whole(stream), Value::Whole { .. }) => value
                .bounds()?
                .is_some_and(|(start, end)| stream.matches_bounds(start, end)),
            (Self::View(position), Value::View(view)) => position.valid_for(view),
            _ => false,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NativeTextOrder {
    Less,
    Equal,
    Greater,
}
impl From<std::cmp::Ordering> for NativeTextOrder {
    fn from(value: std::cmp::Ordering) -> Self {
        match value {
            std::cmp::Ordering::Less => Self::Less,
            std::cmp::Ordering::Equal => Self::Equal,
            std::cmp::Ordering::Greater => Self::Greater,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pair {
    left: Position,
    right: Position,
    pub(super) compared: u64,
    pub(super) result: Option<NativeTextOrder>,
}
#[derive(Default)]
pub(super) struct ValueMeter {
    pub(super) vm: i32,
    pub(super) decoded: usize,
    pub(super) peak_decoded: usize,
    pub(super) peak_native: usize,
}
struct Chunk {
    bytes: Vec<u8>,
    position: Position,
    complete: bool,
}

fn chunk(
    db: &Connection,
    repair: &Repair,
    value: &Value,
    position: &Position,
    records: &crate::query_source::Records<'_>,
    native_allowance: usize,
    row_allowance: usize,
    maximum: usize,
    work: &mut RepairWork,
    meter: &mut ValueMeter,
) -> io::Result<Chunk> {
    if !position.valid(value)? {
        return Err(io::Error::other(
            "native text comparison cursor changed endpoints",
        ));
    }
    match (value, position) {
        (Value::Whole { .. }, Position::Whole(stream)) => {
            let mut stream = stream.clone();
            let (_, end) = value
                .bounds()?
                .ok_or_else(|| io::Error::other("whole text lacks bounds"))?;
            let mut native = None;
            if stream.at < end {
                if native_allowance < 12 {
                    return Ok(Chunk {
                        bytes: Vec::new(),
                        position: Position::Whole(stream),
                        complete: false,
                    });
                }
                let pin = repair
                    .cursor
                    .pin
                    .as_ref()
                    .ok_or_else(|| io::Error::other("native text lacks Original pin"))?;
                let take = native_allowance
                    .min(64 * 1024)
                    .min(usize::try_from(end - stream.at).unwrap_or(usize::MAX));
                let part =
                    records.read_range(&repair.source, Some(&pin.generation), stream.at, take)?;
                work.native_bytes += part.bytes.len();
                work.range_requests += 1;
                meter.peak_native = meter.peak_native.max(part.bytes.len());
                if part.offset != stream.at
                    || part.bytes.is_empty()
                    || part.bytes.len() > take
                    || part.generation != pin.generation
                    || part.length != pin.length
                {
                    return Err(io::Error::other("native text provider changed"));
                }
                native = Some(part);
            }
            let decoded = stream.step(
                native.as_ref().map_or(&[][..], |part| part.bytes.as_ref()),
                maximum,
            )?;
            meter.decoded += decoded.bytes.len();
            Ok(Chunk {
                bytes: decoded.bytes,
                position: Position::Whole(stream),
                complete: decoded.complete,
            })
        }
        (Value::View(view), Position::View(position)) => {
            let mut view_meter = ViewMeter::default();
            let result = native_views::read_chunk(
                db,
                repair,
                view,
                position,
                records,
                native_allowance,
                row_allowance,
                maximum,
                0,
                work,
                &mut view_meter,
                &mut |_, _, _, _, _, _| Ok(()),
            );
            meter.vm += view_meter.vm;
            meter.decoded += view_meter.decoded;
            meter.peak_native = meter.peak_native.max(view_meter.peak_native);
            let chunk = result?;
            Ok(Chunk {
                bytes: chunk.bytes,
                position: Position::View(chunk.cursor),
                complete: chunk.complete,
            })
        }
        _ => Err(io::Error::other("native text reader kind changed")),
    }
}
impl Pair {
    pub(super) fn new(left: &Value, right: &Value) -> io::Result<Self> {
        Ok(Self {
            left: Position::begin(left)?,
            right: Position::begin(right)?,
            compared: 0,
            result: None,
        })
    }
    pub(super) fn matches(&self, left: &Value, right: &Value) -> io::Result<bool> {
        Ok(self.left.valid(left)?
            && self.right.valid(right)?
            && self.compared
                <= left
                    .receipt()
                    .decoded_bytes
                    .min(right.receipt().decoded_bytes)
            && (self.result != Some(NativeTextOrder::Equal)
                || self.left.complete(left)
                    && self.right.complete(right)
                    && self.compared == left.receipt().decoded_bytes
                    && self.compared == right.receipt().decoded_bytes))
    }
    /// Reserve four equal fragments: two initial reads and up to two exact
    /// common-prefix replays. No text enters the durable continuation. Empty
    /// transformed gaps preserve their source progress even with zero output.
    pub(super) fn step(
        &mut self,
        db: &Connection,
        left_repair: &Repair,
        left: &Value,
        right_repair: &Repair,
        right: &Value,
        records: &crate::query_source::Records<'_>,
        native_allowance: usize,
        row_allowance: usize,
        work: &mut RepairWork,
        meter: &mut ValueMeter,
    ) -> io::Result<bool> {
        if self.result.is_some() || !self.matches(left, right)? {
            return Err(io::Error::other(
                "native text comparison is complete or changed",
            ));
        }
        let fragment = native_allowance.saturating_sub(2) / 4;
        if fragment < 72 || row_allowance < 4 {
            return Ok(false);
        }
        let rows = row_allowance / 4;
        let l = chunk(
            db,
            left_repair,
            left,
            &self.left,
            records,
            fragment,
            rows,
            32 * 1024,
            work,
            meter,
        )?;
        let r = chunk(
            db,
            right_repair,
            right,
            &self.right,
            records,
            fragment,
            rows,
            32 * 1024,
            work,
            meter,
        )?;
        meter.peak_decoded = meter.peak_decoded.max(l.bytes.len() + r.bytes.len());
        if let Some(result) = decoded_order(&l.bytes, l.complete, &r.bytes, r.complete) {
            self.compared = self
                .compared
                .checked_add(l.bytes.len().min(r.bytes.len()) as u64)
                .ok_or_else(|| io::Error::other("native text comparison address overflow"))?;
            self.result = Some(result.into());
            self.left = l.position;
            self.right = r.position;
            return Ok(true);
        }
        let common = l.bytes.len().min(r.bytes.len());
        let retained = l.bytes.len() + r.bytes.len();
        let next_left = if l.bytes.len() == common {
            l.position
        } else if common == 0 {
            self.left.clone()
        } else {
            let replay = chunk(
                db,
                left_repair,
                left,
                &self.left,
                records,
                fragment,
                rows,
                common,
                work,
                meter,
            )?;
            meter.peak_decoded = meter.peak_decoded.max(retained + replay.bytes.len());
            replay.position
        };
        let next_right = if r.bytes.len() == common {
            r.position
        } else if common == 0 {
            self.right.clone()
        } else {
            let replay = chunk(
                db,
                right_repair,
                right,
                &self.right,
                records,
                fragment,
                rows,
                common,
                work,
                meter,
            )?;
            meter.peak_decoded = meter.peak_decoded.max(retained + replay.bytes.len());
            replay.position
        };
        self.left = next_left;
        self.right = next_right;
        self.compared = self
            .compared
            .checked_add(common as u64)
            .ok_or_else(|| io::Error::other("native text comparison address overflow"))?;
        Ok(true)
    }
}
