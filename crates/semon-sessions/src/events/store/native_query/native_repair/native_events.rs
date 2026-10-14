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
    pub(super) fn read_only(db: &'a Connection, original: [u8; 32]) -> Self {
        Self {
            db,
            original,
            work: RefCell::new(RepairWork::default()),
        }
    }
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
    fn position(&self, extra: bool, sequence: usize, event: &NativeEvent<Text>) -> io::Result<()> {
        let changed = self.db.execute(
            "INSERT INTO native_original_positions VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(original,extra,sequence) DO UPDATE SET offset=excluded.offset,block=excluded.block,time=excluded.time,child=excluded.child,parent=excluded.parent WHERE offset IS NOT excluded.offset OR block IS NOT excluded.block OR time IS NOT excluded.time OR child IS NOT excluded.child OR parent IS NOT excluded.parent",
            params![self.original.as_slice(),extra,seq(sequence)?,event.o.to_be_bytes().as_slice(),event.b,event.t,matches!(event.k, Kind::Operation | Kind::Tool) && event.parent.is_some(),event.parent.map(seq).transpose()?],
        ).map_err(io::Error::other)?;
        // Reply/closure-only replacements do not rewrite immutable position
        // facts. The SQL statement remains measured actual VM work in tests.
        self.account(changed, if changed == 0 { 0 } else { 64 })
    }
    fn channel_count(&self, channel: &'static str) -> io::Result<usize> {
        self.account(1, 8)?;
        let count: Option<i64> = self
            .db
            .query_row(
                "SELECT count FROM native_original_channel_state WHERE original=?1 AND channel=?2",
                params![self.original.as_slice(), channel],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        usize::try_from(count.unwrap_or(0)).map_err(io::Error::other)
    }
    fn advance_channel(&self, channel: &'static str, sequence: usize) -> io::Result<()> {
        self.account(1, 40)?;
        let next = sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("native channel sequence overflow"))?;
        self.db.execute("INSERT INTO native_original_channel_state VALUES(?1,?2,?3) ON CONFLICT(original,channel) DO UPDATE SET count=excluded.count",params![self.original.as_slice(),channel,seq(next)?]).map_err(io::Error::other)?;
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
        self.position(false, sequence, &event)?;
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
        self.position(false, sequence, &event)?;
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

impl crate::events::native_harness::ExtraStore for Events<'_> {
    fn append_extra(&mut self, event: NativeEvent<Text>) -> io::Result<()> {
        self.qualified_event(&event)?;
        let sequence = self.channel_count("extras")?;
        let text = bounded_json(&event, STATE_BYTES).map_err(io::Error::other)?;
        self.position(true, sequence, &event)?;
        self.account(1, text.len() + 40)?;
        self.db
            .execute(
                "INSERT INTO native_original_extras VALUES(?1,?2,?3)",
                params![self.original.as_slice(), seq(sequence)?, text],
            )
            .map_err(io::Error::other)?;
        self.advance_channel("extras", sequence)
    }
}
impl crate::events::native_producer::SignalStore for Events<'_> {
    fn last_signal(
        &self,
        kind: Option<crate::events::SignalKind>,
    ) -> io::Result<Option<(usize, crate::events::Signal)>> {
        let sql = if kind.is_some() {
            "SELECT sequence,CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM native_original_signals WHERE original=?1 AND kind=?2 ORDER BY sequence DESC LIMIT 1"
        } else {
            "SELECT sequence,CASE WHEN octet_length(metadata)<=65536 THEN metadata END,octet_length(metadata) FROM native_original_signals WHERE original=?1 AND annotates=0 ORDER BY sequence DESC LIMIT 1"
        };
        let row: Option<(i64, Option<String>, i64)> = if let Some(kind) = kind {
            self.db
                .query_row(
                    sql,
                    params![
                        self.original.as_slice(),
                        json(&kind).map_err(io::Error::other)?
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
        } else {
            self.db
                .query_row(sql, params![self.original.as_slice()], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .optional()
        }
        .map_err(io::Error::other)?;
        self.account(
            1,
            row.as_ref()
                .map_or(0, |row| usize::try_from(row.2).unwrap_or(BYTES + 1)),
        )?;
        row.map(|(sequence, text, _)| {
            Ok((
                usize::try_from(sequence).map_err(io::Error::other)?,
                serde_json::from_str(
                    text.as_deref()
                        .ok_or_else(|| io::Error::other("native signal exceeds residency"))?,
                )
                .map_err(io::Error::other)?,
            ))
        })
        .transpose()
    }
    fn append_signal(&mut self, signal: crate::events::Signal) -> io::Result<()> {
        let sequence = self.channel_count("signals")?;
        let text = bounded_json(&signal, STATE_BYTES).map_err(io::Error::other)?;
        self.account(1, text.len() + 40)?;
        self.db
            .execute(
                "INSERT INTO native_original_signals VALUES(?1,?2,?3,?4,?5)",
                params![
                    self.original.as_slice(),
                    seq(sequence)?,
                    json(&signal.k).map_err(io::Error::other)?,
                    signal.k.annotates(),
                    text
                ],
            )
            .map_err(io::Error::other)?;
        self.advance_channel("signals", sequence)
    }
    fn replace_signal(&mut self, sequence: usize, signal: crate::events::Signal) -> io::Result<()> {
        let text = bounded_json(&signal, STATE_BYTES).map_err(io::Error::other)?;
        self.account(1, text.len() + 40)?;
        if self.db.execute("UPDATE native_original_signals SET kind=?3,annotates=?4,metadata=?5 WHERE original=?1 AND sequence=?2",params![self.original.as_slice(),seq(sequence)?,json(&signal.k).map_err(io::Error::other)?,signal.k.annotates(),text]).map_err(io::Error::other)?!=1 { return Err(io::Error::other("native signal address changed")); }
        Ok(())
    }
}

impl Events<'_> {
    pub(super) fn initialize_copilot(&mut self) -> io::Result<()> {
        self.account(1, 32)?;
        self.db
            .execute(
                "INSERT INTO native_original_copilot_started VALUES(?1)",
                params![self.original.as_slice()],
            )
            .map_err(io::Error::other)?;
        Ok(())
    }
}
impl crate::events::native_harness::CopilotIdentityStore for Events<'_> {
    fn copilot_started(&self) -> io::Result<bool> {
        self.account(1, 1)?;
        self.db
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_original_copilot_started WHERE original=?1)",
                params![self.original.as_slice()],
                |row| row.get(0),
            )
            .map_err(io::Error::other)
    }
    fn copilot_record_seen(&mut self, identity: &Text) -> io::Result<bool> {
        if identity.original != self.original {
            return Err(io::Error::other(
                "native Copilot record identity belongs to another Original",
            ));
        }
        self.account(1, 40)?;
        Ok(self
            .db
            .execute(
                "INSERT OR IGNORE INTO native_original_copilot_seen VALUES(?1,?2)",
                params![self.original.as_slice(), seq_u64(identity.class)?],
            )
            .map_err(io::Error::other)?
            == 0)
    }
}

impl crate::events::native_harness::CopilotStore for Events<'_> {
    fn tool_occurrence_after(&self, id: &u64, after: Option<usize>) -> io::Result<Option<usize>> {
        // Strict keyset seek, so a resumed ambiguity reset never reloads the
        // preceding occurrences and never uses a matching-subset rank.
        let floor = after.map(seq).transpose()?.unwrap_or(-1);
        self.account(1, 24)?;
        let sequence: Option<i64> = self.db.query_row(
            "SELECT sequence FROM native_original_tool_ids WHERE original=?1 AND identity=?2 AND sequence>?3 ORDER BY sequence LIMIT 1",
            params![self.original.as_slice(), seq_u64(*id)?, floor],
            |row| row.get(0),
        ).optional().map_err(io::Error::other)?;
        sequence
            .map(usize::try_from)
            .transpose()
            .map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::native_event::NativeReply;
    use crate::events::native_producer;

    #[test]
    fn copilot_seen_admission_is_durable_and_cancelled_start_does_not_poison_id() {
        use crate::events::native_harness::{Admission, CopilotIdentityStore, copilot_admit};
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        let original = [13; 32];
        let identity = Text::qualified(original, 1);
        {
            let tx = db.transaction().unwrap();
            let mut events = Events::new(&tx, original).unwrap();
            assert_eq!(
                copilot_admit(&mut events, Some(&identity), false, |_| unreachable!()).unwrap(),
                Admission::BeforeStart
            );
            assert_eq!(
                copilot_admit(&mut events, None, true, |_| unreachable!()).unwrap(),
                Admission::Gap
            );
            assert_eq!(
                copilot_admit(
                    &mut events,
                    Some(&identity),
                    true,
                    Events::initialize_copilot
                )
                .unwrap(),
                Admission::Accepted
            );
            drop(tx);
        }
        {
            let tx = db.transaction().unwrap();
            let mut events = Events::new(&tx, original).unwrap();
            assert!(!events.copilot_started().unwrap());
            assert_eq!(
                copilot_admit(
                    &mut events,
                    Some(&identity),
                    true,
                    Events::initialize_copilot
                )
                .unwrap(),
                Admission::Accepted
            );
            tx.commit().unwrap();
        }
        let mut events = Events::new(&db, original).unwrap();
        assert_eq!(
            copilot_admit(&mut events, Some(&identity), false, |_| unreachable!()).unwrap(),
            Admission::Duplicate
        );
    }

    #[test]
    fn native_turn_boundary_closes_full_64_call_window_in_bounded_substeps() {
        use crate::events::native_harness::{CLOSE_STEP, codex_boundary_step};
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        let original = [11; 32];
        let mut pending = Vec::new();
        for class in 0..native_producer::CLOSE_PAGE {
            let tx = db.transaction().unwrap();
            let mut events = Events::new(&tx, original).unwrap();
            let sequence = events.event_count().unwrap();
            native_producer::tool_event(
                &mut events,
                NativeEvent {
                    k: Kind::Tool,
                    id: Some(Text::qualified(original, class as u64)),
                    code_mode: true,
                    code_mode_open: true,
                    ..Default::default()
                },
            )
            .unwrap();
            pending.push(sequence);
            tx.commit().unwrap();
        }
        let mut after = 0;
        while after < pending.len() {
            let tx = db.transaction().unwrap();
            let mut events = Events::new(&tx, original).unwrap();
            let next = codex_boundary_step(&mut events, &pending, after).unwrap();
            assert!(next - after <= CLOSE_STEP);
            assert!(events.work().metadata_rows <= 37);
            assert!(events.work().metadata_bytes < 16384);
            tx.commit().unwrap();
            // The subrecord address survives connection replacement. Pending
            // attribution remains intact until a later native result resolves it.
            after = next;
        }
        let mut events = Events::new(&db, original).unwrap();
        for class in 0..native_producer::CLOSE_PAGE {
            let event = events.event(class).unwrap().unwrap();
            assert!(!event.code_mode_open);
            assert!(event.r.is_none());
            assert_eq!(events.pending(&(class as u64)).unwrap(), Some(class));
            // Reinstantiate accounting between point observations.
            events = Events::new(&db, original).unwrap();
        }
    }

    #[test]
    fn copilot_ambiguity_reset_is_indexed_resumable_and_matches_native_reference() {
        use crate::events::native_harness::{
            Reset, copilot_completion_target, copilot_reset_reply_step,
        };
        for count in [0usize, 1, 128, 1024, 8192] {
            let mut db = Connection::open_in_memory().unwrap();
            db.execute_batch(SCHEMA).unwrap();
            let original = [9; 32];
            let id = Text::qualified(original, 1);
            // Populate in separate bounded transactions, never one Events
            // adapter whose 64-row accounting could reset between records.
            for _index in 0..count {
                let tx = db.transaction().unwrap();
                let mut events = Events::new(&tx, original).unwrap();
                events
                    .append_event(NativeEvent {
                        k: Kind::A,
                        ..Default::default()
                    })
                    .unwrap();
                native_producer::tool_event(
                    &mut events,
                    NativeEvent {
                        k: Kind::Tool,
                        id: Some(id.clone()),
                        r: Some(NativeReply {
                            e: true,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .unwrap();
                tx.commit().unwrap();
            }
            if count == 0 {
                Events::new(&db, original).unwrap();
            }
            let vm = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let observed = vm.clone();
            db.progress_handler(
                1,
                Some(move || {
                    observed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    false
                }),
            );
            let reads = crate::events::FILE_READS.with(std::cell::Cell::get);
            let mut after = None;
            let mut reset = 0;
            loop {
                vm.store(0, std::sync::atomic::Ordering::Relaxed);
                let tx = db.transaction().unwrap();
                let mut events = Events::new(&tx, original).unwrap();
                // Cancellation before cursor seal rolls back the event update.
                let outcome = copilot_reset_reply_step(&mut events, &1, after).unwrap();
                let work = events.work();
                assert!(work.metadata_rows <= 5);
                assert!(work.metadata_bytes < 4096);
                assert!(vm.load(std::sync::atomic::Ordering::Relaxed) < 512);
                assert_eq!(crate::events::FILE_READS.with(std::cell::Cell::get), reads);
                drop(tx);
                if let Reset::Progress(sequence) = outcome {
                    let events = Events::new(&db, original).unwrap();
                    assert!(events.event(sequence).unwrap().unwrap().r.is_some());
                }
                let tx = db.transaction().unwrap();
                let mut events = Events::new(&tx, original).unwrap();
                let outcome = copilot_reset_reply_step(&mut events, &1, after).unwrap();
                match outcome {
                    Reset::Complete => {
                        tx.commit().unwrap();
                        break;
                    }
                    Reset::Progress(sequence) => {
                        assert_eq!(sequence, reset * 2 + 1);
                        after = Some(sequence);
                        reset += 1;
                        tx.commit().unwrap();
                    }
                }
            }
            db.progress_handler(0, None::<fn() -> bool>);
            // Independent legacy reference: every indexed historical request
            // with the repeated id loses its reply; non-tool rows remain intact.
            let mut rows = db.prepare("SELECT sequence,metadata FROM native_original_events WHERE original=?1 ORDER BY sequence").unwrap();
            for row in rows
                .query_map(params![original.as_slice()], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
            {
                let (sequence, metadata) = row.unwrap();
                let event: NativeEvent<Text> = serde_json::from_str(&metadata).unwrap();
                assert_eq!(
                    event.k,
                    if sequence % 2 == 0 {
                        Kind::A
                    } else {
                        Kind::Tool
                    }
                );
                assert!(event.r.is_none());
            }
            assert_eq!(reset, count);
            let events = Events::new(&db, original).unwrap();
            assert_eq!(
                copilot_completion_target(&events, &1).unwrap(),
                (count == 1).then_some(1)
            );
        }
    }

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
