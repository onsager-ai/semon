//! Transaction-local adapter for the existing native event transition owner.
//! Identity classes are exact, qualified native equivalence classes: a digest
//! or an arbitrary source string may never be used directly as a class key.

use super::*;
use crate::events::Kind;
use crate::events::native_event::NativeEvent;
use crate::events::native_producer::ToolStore;
use std::borrow::Borrow;
use std::cell::RefCell;

fn seq(value: usize) -> io::Result<i64> {
    i64::try_from(value).map_err(io::Error::other)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Text {
    original: [u8; 32],
    class: u64,
}
impl Text {
    // Only the exact native comparison owner may create production text.
    pub(super) fn qualified(original: [u8; 32], class: u64) -> Self {
        Self { original, class }
    }
    pub(crate) fn original(&self) -> [u8; 32] {
        self.original
    }
    pub(crate) fn identity(&self) -> u64 {
        self.class
    }
}
impl Borrow<u64> for Text {
    fn borrow(&self) -> &u64 {
        &self.class
    }
}

pub(super) struct Events<'a> {
    db: &'a Connection,
    original: [u8; 32],
    work: RefCell<RepairWork>,
}
impl<'a> Events<'a> {
    pub(super) fn new(db: &'a Connection, original: [u8; 32]) -> io::Result<Self> {
        let store = Self {
            db,
            original,
            work: RefCell::new(RepairWork::default()),
        };
        store.account(1, 40)?;
        db.execute(
            "INSERT OR IGNORE INTO native_original_event_state VALUES(?1,0)",
            params![original.as_slice()],
        )
        .map_err(io::Error::other)?;
        Ok(store)
    }
    pub(super) fn work(&self) -> RepairWork {
        self.work.borrow().clone()
    }
    fn account(&self, rows: usize, bytes: usize) -> io::Result<()> {
        let mut work = self.work.borrow_mut();
        work.metadata_rows += rows;
        work.metadata_bytes += bytes;
        if work.metadata_rows > ROWS || work.metadata_bytes > BYTES {
            return Err(io::Error::other(
                "native event transition requires a resumed step",
            ));
        }
        Ok(())
    }
    fn exact_sequence(&self, sql: &str, id: &u64, floor: usize) -> io::Result<Option<usize>> {
        self.account(1, 24)?;
        let sequence: Option<i64> = self
            .db
            .query_row(
                sql,
                params![self.original.as_slice(), seq_u64(*id)?, seq(floor)?],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        sequence
            .map(usize::try_from)
            .transpose()
            .map_err(io::Error::other)
    }
    fn qualified_event(&self, event: &NativeEvent<Text>) -> io::Result<()> {
        if event
            .text_refs()
            .into_iter()
            .flatten()
            .any(|text| text.original != self.original)
        {
            return Err(io::Error::other(
                "native text identity belongs to a different Original",
            ));
        }
        Ok(())
    }
}
impl ToolStore for Events<'_> {
    type Error = io::Error;
    type Identity = u64;
    type Text = Text;
    fn event_count(&self) -> io::Result<usize> {
        self.account(1, 8)?;
        let count: i64 = self
            .db
            .query_row(
                "SELECT event_count FROM native_original_event_state WHERE original=?1",
                params![self.original.as_slice()],
                |row| row.get(0),
            )
            .map_err(io::Error::other)?;
        usize::try_from(count).map_err(io::Error::other)
    }
    fn event(&self, sequence: usize) -> io::Result<Option<NativeEvent<Text>>> {
        let row:Option<(Option<String>,i64)>=self.db.query_row("SELECT CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM native_original_events WHERE original=?1 AND sequence=?2",params![self.original.as_slice(),seq(sequence)?],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(io::Error::other)?;
        self.account(
            1,
            row.as_ref()
                .map_or(0, |row| usize::try_from(row.1).unwrap_or(BYTES + 1)),
        )?;
        row.map(|row| {
            serde_json::from_str(
                row.0
                    .as_deref()
                    .ok_or_else(|| io::Error::other("native event exceeds metadata residency"))?,
            )
            .map_err(io::Error::other)
        })
        .transpose()
    }
    fn replace_event(&mut self, sequence: usize, event: NativeEvent<Text>) -> io::Result<()> {
        self.qualified_event(&event)?;
        let text = bounded_json(&event, STATE_BYTES).map_err(io::Error::other)?;
        self.account(2, text.len() + 32)?;
        self.db.execute("UPDATE native_original_events SET is_tool=?3,identity=?4,code_mode=?5,code_mode_open=?6,metadata=?7 WHERE original=?1 AND sequence=?2",params![self.original.as_slice(),seq(sequence)?,event.k==Kind::Tool,event.id.as_ref().map(|id|seq_u64(id.class)).transpose()?,event.code_mode,event.code_mode_open,text]).map_err(io::Error::other)?;
        // At most one pending identity owns this exact event. A replacement
        // changes only its open flag, never identity or pending attribution.
        if let Some(id) = event.id {
            self.db.execute("UPDATE native_original_pending SET code_mode_open=?4 WHERE original=?1 AND identity=?2 AND sequence=?3",params![self.original.as_slice(),seq_u64(id.class)?,seq(sequence)?,event.k==Kind::Tool && event.code_mode && event.code_mode_open]).map_err(io::Error::other)?;
        }
        Ok(())
    }
    fn append_event(&mut self, event: NativeEvent<Text>) -> io::Result<usize> {
        self.qualified_event(&event)?;
        let sequence = self.event_count()?;
        let next = sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("native event sequence overflow"))?;
        let text = bounded_json(&event, STATE_BYTES).map_err(io::Error::other)?;
        self.account(2, text.len() + 40)?;
        self.db
            .execute(
                "INSERT INTO native_original_events VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    self.original.as_slice(),
                    seq(sequence)?,
                    event.k == Kind::Tool,
                    event.id.as_ref().map(|id| seq_u64(id.class)).transpose()?,
                    event.code_mode,
                    event.code_mode_open,
                    text
                ],
            )
            .map_err(io::Error::other)?;
        self.db
            .execute(
                "UPDATE native_original_event_state SET event_count=?2 WHERE original=?1",
                params![self.original.as_slice(), seq(next)?],
            )
            .map_err(io::Error::other)?;
        Ok(sequence)
    }
    fn pending(&self, id: &u64) -> io::Result<Option<usize>> {
        self.exact_sequence("SELECT sequence FROM native_original_pending WHERE original=?1 AND identity=?2 AND sequence>=?3",id,0)
    }
    fn take_pending(&mut self, id: &u64) -> io::Result<Option<usize>> {
        let sequence = self.pending(id)?;
        if sequence.is_some() {
            self.account(1, 24)?;
            self.db
                .execute(
                    "DELETE FROM native_original_pending WHERE original=?1 AND identity=?2",
                    params![self.original.as_slice(), seq_u64(*id)?],
                )
                .map_err(io::Error::other)?;
        }
        Ok(sequence)
    }
    fn set_pending(&mut self, id: &u64, sequence: usize) -> io::Result<()> {
        let event = self
            .event(sequence)?
            .ok_or_else(|| io::Error::other("native pending event absent"))?;
        self.account(1, 32)?;
        self.db.execute("INSERT INTO native_original_pending VALUES(?1,?2,?3,?4) ON CONFLICT(original,identity) DO UPDATE SET sequence=excluded.sequence,code_mode_open=excluded.code_mode_open",params![self.original.as_slice(),seq_u64(*id)?,seq(sequence)?,event.k==Kind::Tool && event.code_mode && event.code_mode_open]).map_err(io::Error::other)?;
        Ok(())
    }
    fn add_tool_id(&mut self, id: &u64, sequence: usize) -> io::Result<()> {
        let event = self
            .event(sequence)?
            .ok_or_else(|| io::Error::other("native tool occurrence absent"))?;
        self.account(1, 32)?;
        self.db
            .execute(
                "INSERT INTO native_original_tool_ids VALUES(?1,?2,?3,?4)",
                params![
                    self.original.as_slice(),
                    seq_u64(*id)?,
                    seq(sequence)?,
                    !event.code_mode
                ],
            )
            .map_err(io::Error::other)?;
        Ok(())
    }
    fn latest_tool(&self, id: &u64, plain: bool, floor: usize) -> io::Result<Option<usize>> {
        let sql = if plain {
            "SELECT sequence FROM native_original_tool_ids WHERE original=?1 AND identity=?2 AND plain=1 AND sequence>=?3 ORDER BY sequence DESC LIMIT 1"
        } else {
            "SELECT sequence FROM native_original_tool_ids WHERE original=?1 AND identity=?2 AND sequence>=?3 ORDER BY sequence DESC LIMIT 1"
        };
        self.exact_sequence(sql, id, floor)
    }
    fn recent_tool(&self, id: &u64, floor: usize) -> io::Result<Option<usize>> {
        self.exact_sequence("SELECT sequence FROM native_original_events WHERE original=?1 AND identity=?2 AND is_tool=1 AND sequence>=?3 ORDER BY sequence DESC LIMIT 1",id,floor)
    }
    fn unique_open_code_mode(&self) -> io::Result<Option<usize>> {
        let mut query=self.db.prepare("SELECT sequence FROM native_original_pending WHERE original=?1 AND code_mode_open=1 ORDER BY identity LIMIT 2").map_err(io::Error::other)?;
        let mut rows = query
            .query(params![self.original.as_slice()])
            .map_err(io::Error::other)?;
        let first = rows
            .next()
            .map_err(io::Error::other)?
            .map(|row| row.get::<_, i64>(0))
            .transpose()
            .map_err(io::Error::other)?;
        let ambiguous = rows.next().map_err(io::Error::other)?.is_some();
        self.account(usize::from(first.is_some()) + usize::from(ambiguous), 16)?;
        if ambiguous {
            Ok(None)
        } else {
            first
                .map(usize::try_from)
                .transpose()
                .map_err(io::Error::other)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::native_event::NativeReply;
    use crate::events::native_producer;

    #[test]
    fn standalone_private_schema_supports_point_native_transitions() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        let original = [7; 32];
        let mut events = Events::new(&db, original).unwrap();
        let id = Text::qualified(original, 1);
        native_producer::tool_event(
            &mut events,
            NativeEvent {
                k: Kind::Tool,
                id: Some(id.clone()),
                code_mode: true,
                code_mode_open: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(events.unique_open_code_mode().unwrap(), Some(0));
        assert_eq!(events.latest_tool(&1, true, 0).unwrap(), None);
        assert_eq!(events.latest_tool(&1, false, 0).unwrap(), Some(0));
        native_producer::resolve(&mut events, &1, |_| NativeReply::<Text> {
            o: 9,
            e: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(events.pending(&1).unwrap(), None);
        assert_eq!(events.unique_open_code_mode().unwrap(), None);
        let event = events.event(0).unwrap().unwrap();
        assert!(!event.code_mode_open);
        assert!(event.r.unwrap().e);
        assert!(events.work().metadata_rows <= ROWS);
    }
}
