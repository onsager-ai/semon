//! Opt-in producer measurements. Identifiers are opaque diagnostics, never
//! source authority, and contain no cache paths or native content.
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, time::Instant};

/// Connection-local pager events, separate from kernel byte accounting. In WAL
/// mode writes count WAL pages; spills count overflow writes during a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SqliteCache {
    spill: u64,
    write: u64,
    hit: u64,
    miss: u64,
    // Bytes, not pages. This overlaps TEMP pager CACHE_WRITE and includes
    // finalized external sorts, but does not count statement journals.
    temp_spill_bytes: Option<u64>,
}

impl SqliteCache {
    fn read(connection: &rusqlite::Connection) -> Option<Self> {
        fn counter(connection: &rusqlite::Connection, operation: i32) -> Option<u64> {
            let mut current: rusqlite::ffi::sqlite3_int64 = 0;
            let mut highwater: rusqlite::ffi::sqlite3_int64 = 0;
            // SAFETY: the borrowed live Connection owns this handle throughout
            // the call. Store samples its writer on its owning thread, through
            // the Transaction while active. SQLite only reads fixed status
            // counters and writes these initialized, correctly typed outputs;
            // reset=0 changes no counters. No handle or pointer escapes.
            let result = unsafe {
                rusqlite::ffi::sqlite3_db_status64(
                    connection.handle(),
                    operation,
                    &mut current,
                    &mut highwater,
                    0,
                )
            };
            (result == rusqlite::ffi::SQLITE_OK)
                .then(|| u64::try_from(current).ok())
                .flatten()
        }
        Some(Self {
            spill: counter(connection, rusqlite::ffi::SQLITE_DBSTATUS_CACHE_SPILL)?,
            write: counter(connection, rusqlite::ffi::SQLITE_DBSTATUS_CACHE_WRITE)?,
            hit: counter(connection, rusqlite::ffi::SQLITE_DBSTATUS_CACHE_HIT)?,
            miss: counter(connection, rusqlite::ffi::SQLITE_DBSTATUS_CACHE_MISS)?,
            temp_spill_bytes: counter(connection, rusqlite::ffi::SQLITE_DBSTATUS_TEMPBUF_SPILL),
        })
    }

    fn delta(self, before: Self) -> Option<Self> {
        Some(Self {
            spill: self.spill.checked_sub(before.spill)?,
            write: self.write.checked_sub(before.write)?,
            hit: self.hit.checked_sub(before.hit)?,
            miss: self.miss.checked_sub(before.miss)?,
            temp_spill_bytes: self
                .temp_spill_bytes
                .zip(before.temp_spill_bytes)
                .and_then(|(after, before)| after.checked_sub(before)),
        })
    }
}

#[derive(Clone, Copy)]
struct CacheStage {
    phase: &'static str,
    before: Option<SqliteCache>,
    after: Option<SqliteCache>,
    delta: Option<SqliteCache>,
}

/// Calling-thread kernel accounting, not physical-device I/O. Counter resets
/// remain unavailable deltas; WAL lengths alone do not prove checkpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ThreadWrites {
    wchar: u64,
    syscw: u64,
    write_bytes: u64,
    cancelled_write_bytes: u64,
}

impl ThreadWrites {
    fn delta(self, before: Self) -> Option<Self> {
        Some(Self {
            wchar: self.wchar.checked_sub(before.wchar)?,
            syscw: self.syscw.checked_sub(before.syscw)?,
            write_bytes: self.write_bytes.checked_sub(before.write_bytes)?,
            cancelled_write_bytes: self
                .cancelled_write_bytes
                .checked_sub(before.cancelled_write_bytes)?,
        })
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse_thread_writes(bytes: &[u8]) -> Option<ThreadWrites> {
    if bytes.len() > 4096 || !bytes.ends_with(b"\n") {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut fields = [None; 4];
    for line in text.lines() {
        let (name, value) = line.split_once(':')?;
        let index = match name {
            "wchar" => 0,
            "syscw" => 1,
            "write_bytes" => 2,
            "cancelled_write_bytes" => 3,
            _ => continue,
        };
        if fields[index].is_some() {
            return None;
        }
        fields[index] = Some(value.trim().parse().ok()?);
    }
    Some(ThreadWrites {
        wchar: fields[0]?,
        syscw: fields[1]?,
        write_bytes: fields[2]?,
        cancelled_write_bytes: fields[3]?,
    })
}

fn thread_writes() -> Option<ThreadWrites> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let file = std::fs::File::open("/proc/thread-self/io").ok()?;
        let mut bytes = Vec::with_capacity(4097);
        file.take(4097).read_to_end(&mut bytes).ok()?;
        parse_thread_writes(&bytes)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

struct WriteSample {
    thread: Option<ThreadWrites>,
    cache: Option<SqliteCache>,
    wal: Option<u64>,
    sample_us: u128,
}

impl WriteSample {
    fn read(wal: &Path, connection: &rusqlite::Connection) -> Self {
        let started = Instant::now();
        let thread = thread_writes();
        let cache = SqliteCache::read(connection);
        let wal = std::fs::metadata(wal).ok().map(|value| value.len());
        Self {
            thread,
            cache,
            wal,
            sample_us: started.elapsed().as_micros(),
        }
    }
}

struct WriteState {
    producer: String,
    group: &'static str,
    wal: std::path::PathBuf,
    before: WriteSample,
    initial_sample_us: u128,
    // Catalog has eight fixed intervals; Index has three. Discard these on a
    // failed publication: CACHE_WRITE is undefined after a write I/O error.
    cache_stages: [Option<CacheStage>; 8],
    cache_stage_count: usize,
}

#[cfg(test)]
type ObservedCacheStages = Vec<(&'static str, CacheStage)>;

#[cfg(test)]
std::thread_local! {
    static OBSERVED_CACHE_STAGES: std::cell::RefCell<Option<ObservedCacheStages>> = const { std::cell::RefCell::new(None) };
}

impl WriteState {
    fn emit_committed(&mut self) {
        if self.cache_stage_count > self.cache_stages.len() {
            return;
        }
        for stage in self.cache_stages.iter_mut().filter_map(Option::take) {
            #[cfg(test)]
            OBSERVED_CACHE_STAGES.with(|observations| {
                if let Some(observations) = &mut *observations.borrow_mut() {
                    observations.push((self.group, stage));
                }
            });
            eprintln!(
                "semon sessions native: producer={} sqlite_activity={} stage={} publication=committed counter_bits=64 cache_before={:?} cache_after={:?} cache_delta={:?}",
                self.producer, self.group, stage.phase, stage.before, stage.after, stage.delta
            );
        }
    }
}

fn wal_change(after: Option<u64>, before: Option<u64>) -> Option<i128> {
    after
        .zip(before)
        .map(|(after, before)| i128::from(after) - i128::from(before))
}

/// Fixed boundary snapshots only. Disabled observation creates no paths,
/// identifiers, snapshots or clocks, and never switches to process I/O scope.
pub(crate) struct WriteActivity(Option<WriteState>);

impl WriteActivity {
    pub(crate) fn new(
        enabled: bool,
        index: &Path,
        group: &'static str,
        connection: &rusqlite::Connection,
    ) -> Self {
        Self(enabled.then(|| {
            let mut wal = index.as_os_str().to_os_string();
            wal.push("-wal");
            let wal = std::path::PathBuf::from(wal);
            let before = WriteSample::read(&wal, connection);
            let initial_sample_us = before.sample_us;
            WriteState {
                producer: id(index),
                group,
                wal,
                before,
                initial_sample_us,
                cache_stages: [None; 8],
                cache_stage_count: 0,
            }
        }))
    }

    pub(crate) fn stage(&mut self, phase: &'static str, connection: &rusqlite::Connection) {
        let Some(state) = &mut self.0 else {
            return;
        };
        let after = WriteSample::read(&state.wal, connection);
        let delta = after
            .thread
            .zip(state.before.thread)
            .and_then(|(after, before)| after.delta(before));
        let wal_change = wal_change(after.wal, state.before.wal);
        let sampling_us = state.initial_sample_us.saturating_add(after.sample_us);
        let cache_delta = after
            .cache
            .zip(state.before.cache)
            .and_then(|(after, before)| after.delta(before));
        if let Some(stage) = state.cache_stages.get_mut(state.cache_stage_count) {
            *stage = Some(CacheStage {
                phase,
                before: state.before.cache,
                after: after.cache,
                delta: cache_delta,
            });
        }
        state.cache_stage_count = state.cache_stage_count.saturating_add(1);
        // The previous interval's diagnostic stderr is part of kernel counters.
        // Sampling cost is reported once per sample, outside production leaves.
        eprintln!(
            "semon sessions native: producer={} write_activity={} stage={phase} io_scope=calling_thread io_before={:?} io_after={:?} io_delta={delta:?} wal_before_bytes={:?} wal_after_bytes={:?} wal_change_bytes={wal_change:?} sampling_us={sampling_us}",
            state.producer,
            state.group,
            state.before.thread,
            after.thread,
            state.before.wal,
            after.wal
        );
        state.initial_sample_us = 0;
        state.before = after;
        if phase == "commit" {
            state.emit_committed();
        }
    }

    /// Flush a nested sample only after its owning transaction commits. Consume
    /// the observer without sampling the unrelated tail of that transaction.
    pub(crate) fn committed(mut self) {
        if let Some(mut state) = self.0.take() {
            state.emit_committed();
        }
    }
}

pub(crate) fn enabled() -> bool {
    std::env::var("SEMON_BUILD_TIMING").is_ok_and(|value| value == "1")
}

pub(crate) fn id(index: &Path) -> String {
    format!("{:x}", Sha256::digest(index.as_os_str().as_encoded_bytes()))
}

/// Bounded operation aggregates: `(microseconds, workload units)`. Call sites
/// count source observations, rows, postings or scope groups, never physical I/O.
/// Disabled measurements retain no entries and start no clocks.
#[derive(Default)]
pub(crate) struct Measurements {
    enabled: bool,
    values: BTreeMap<&'static str, (u128, usize)>,
}

impl Measurements {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Self::default()
        }
    }

    pub(crate) fn start(&self) -> Option<Instant> {
        self.enabled.then(Instant::now)
    }

    pub(crate) fn record(&mut self, name: &'static str, started: Option<Instant>, units: usize) {
        if let Some(started) = started {
            let entry = self.values.entry(name).or_default();
            entry.0 = entry.0.saturating_add(started.elapsed().as_micros());
            entry.1 = entry.1.saturating_add(units);
        }
    }

    pub(crate) fn count(&mut self, name: &'static str, units: usize) {
        if self.enabled {
            let entry = self.values.entry(name).or_default();
            entry.1 = entry.1.saturating_add(units);
        }
    }

    pub(crate) fn emit(&self, producer: &str, group: &'static str) {
        if self.enabled {
            eprintln!(
                "semon sessions native: producer={producer} breakdown={group} us_units={:?}",
                self.values
            );
        }
    }
}

pub(crate) struct Phase(Option<(String, &'static str, Instant)>);

impl Phase {
    pub(crate) fn new(cache: &Path, name: &'static str) -> Self {
        Self(enabled().then(|| {
            (
                id(&crate::events::EventCache::path(cache)),
                name,
                Instant::now(),
            )
        }))
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        if let Some((id, name, started)) = &self.0 {
            eprintln!(
                "semon sessions native: producer={id} phase={name} us={}",
                started.elapsed().as_micros()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wal_connection(name: &str) -> (std::path::PathBuf, rusqlite::Connection) {
        let root = std::env::temp_dir().join(format!(
            "semon-native-timing-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let connection = rusqlite::Connection::open(root.join("index.sqlite3")).unwrap();
        let journal: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "wal");
        (root, connection)
    }

    #[test]
    fn finalized_external_sort_reports_temp_bytes_without_resetting_or_crossing_connections() {
        let (root, left) = wal_connection("temp-sort");
        let right = rusqlite::Connection::open(root.join("index.sqlite3")).unwrap();
        let before = SqliteCache::read(&left).unwrap();
        let idle = SqliteCache::read(&right).unwrap();
        // About 8MiB of unindexed sort keys exceed the pinned bundled default
        // sorter memory threshold. Do not change cache, TEMP or thread policy.
        {
            let mut statement = left.prepare("WITH RECURSIVE seq(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM seq WHERE n<8192) SELECT printf('%08d',8192-n)||hex(zeroblob(512)) AS value FROM seq ORDER BY value").unwrap();
            let mut rows = statement.query([]).unwrap();
            let mut count = 0;
            while let Some(row) = rows.next().unwrap() {
                let text: String = row.get(0).unwrap();
                assert_eq!(text.len(), 1032);
                assert!(text.starts_with(&format!("{count:08}")));
                assert!(text[8..].bytes().all(|byte| byte == b'0'));
                count += 1;
            }
            assert_eq!(count, 8192);
        } // SorterClose transfers task spill bytes to the connection counter.
        let after = SqliteCache::read(&left).unwrap();
        assert!(after.delta(before).unwrap().temp_spill_bytes.unwrap() > 0);
        assert_eq!(SqliteCache::read(&left), Some(after));
        assert_eq!(SqliteCache::read(&right), Some(idle));
        drop(right);
        drop(left);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nested_wal_observation_emits_only_committed_sample_without_transaction_tail() {
        let (root, mut connection) = wal_connection("nested-commit");
        connection
            .execute_batch("CREATE TABLE activity(value INTEGER PRIMARY KEY)")
            .unwrap();
        for fail in [true, false] {
            OBSERVED_CACHE_STAGES.with(|observations| {
                *observations.borrow_mut() = Some(Vec::new());
            });
            let mut sampled_end = None;
            let result = (|| -> rusqlite::Result<()> {
                let tx = connection.transaction()?;
                let mut sample = WriteActivity::new(
                    true,
                    &root.join("index.sqlite3"),
                    "catalog_metadata_sample",
                    &tx,
                );
                for (value, phase) in [
                    (1, "overview"),
                    (2, "catalog_rows_changed"),
                    (3, "source_retirement"),
                    (4, "sources_and_dependencies"),
                ] {
                    tx.execute("INSERT INTO activity VALUES(?1)", [value])?;
                    sample.stage(phase, &tx);
                }
                sampled_end = SqliteCache::read(&tx);
                assert!(
                    OBSERVED_CACHE_STAGES.with(|stages| stages
                        .borrow()
                        .as_ref()
                        .unwrap()
                        .is_empty())
                );
                // Later rows and search work must not be mislabeled as part of
                // the first-row sample when its buffered reports are flushed.
                for value in 5..=64 {
                    tx.execute("INSERT INTO activity VALUES(?1)", [value])?;
                }
                if fail {
                    tx.execute("INSERT INTO activity VALUES(1)", [])?;
                }
                tx.commit()?;
                sample.committed();
                Ok(())
            })();
            let observations = OBSERVED_CACHE_STAGES
                .with(|observations| observations.borrow_mut().take().unwrap());
            let count: i64 = connection
                .query_row("SELECT COUNT(*) FROM activity", [], |row| row.get(0))
                .unwrap();
            if fail {
                assert!(result.is_err());
                assert_eq!(count, 0, "failed transaction did not roll back");
                assert!(observations.is_empty(), "rollback reported committed cache");
            } else {
                result.unwrap();
                assert_eq!(count, 64);
                assert_eq!(observations.len(), 4);
                assert!(
                    observations
                        .iter()
                        .all(|(group, _)| *group == "catalog_metadata_sample")
                );
                assert_eq!(observations.last().unwrap().1.after, sampled_end);
                assert_ne!(sampled_end, SqliteCache::read(&connection));
            }
        }
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sqlite_cache_snapshots_preserve_counters_and_connection_scope() {
        let left = rusqlite::Connection::open_in_memory().unwrap();
        let right = rusqlite::Connection::open_in_memory().unwrap();
        let left_before = SqliteCache::read(&left).unwrap();
        let right_before = SqliteCache::read(&right).unwrap();
        left.execute_batch(
            "CREATE TABLE activity(value TEXT); INSERT INTO activity VALUES('first'),('second');",
        )
        .unwrap();
        let left_after = SqliteCache::read(&left).unwrap();
        assert!(left_after.hit > left_before.hit);
        assert_eq!(SqliteCache::read(&left), Some(left_after));
        assert_eq!(SqliteCache::read(&right), Some(right_before));
    }

    #[test]
    fn thread_write_parser_requires_a_complete_bounded_counter_set() {
        let sample = b"rchar: 5\nwchar: 8\nsyscr: 2\nsyscw: 3\nread_bytes: 0\nwrite_bytes: 4096\ncancelled_write_bytes: 0\n";
        let writes = parse_thread_writes(sample).unwrap();
        assert_eq!(
            writes,
            ThreadWrites {
                wchar: 8,
                syscw: 3,
                write_bytes: 4096,
                cancelled_write_bytes: 0
            }
        );
        assert!(parse_thread_writes(&sample[..sample.len() - 1]).is_none());
        assert!(parse_thread_writes(b"wchar: 8\nsyscw: 3\nwrite_bytes: 4096\n").is_none());
        assert!(
            parse_thread_writes(
                b"wchar: invalid\nsyscw: 3\nwrite_bytes: 4096\ncancelled_write_bytes: 0\n"
            )
            .is_none()
        );
        assert!(
            parse_thread_writes(
                b"wchar: 8\nwchar: 8\nsyscw: 3\nwrite_bytes: 4096\ncancelled_write_bytes: 0\n"
            )
            .is_none()
        );
        assert!(parse_thread_writes(&[b'\n'; 4097]).is_none());
    }

    #[test]
    fn write_deltas_preserve_reset_unavailability_and_wal_shrink() {
        let before = ThreadWrites {
            wchar: 8,
            syscw: 3,
            write_bytes: 4096,
            cancelled_write_bytes: 0,
        };
        let after = ThreadWrites {
            wchar: 10,
            syscw: 4,
            write_bytes: 8192,
            cancelled_write_bytes: 0,
        };
        assert_eq!(
            after.delta(before),
            Some(ThreadWrites {
                wchar: 2,
                syscw: 1,
                write_bytes: 4096,
                cancelled_write_bytes: 0
            })
        );
        assert!(before.delta(after).is_none());
        assert_eq!(wal_change(Some(2), Some(8)), Some(-6));
        assert_eq!(wal_change(Some(8), Some(2)), Some(6));
        assert_eq!(wal_change(None, Some(2)), None);
    }

    #[test]
    fn disabled_diagnostics_retain_no_observation_or_measurements() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let mut activity =
            WriteActivity::new(false, Path::new("/unused"), "index_initial", &connection);
        activity.stage("rows", &connection);
        assert!(activity.0.is_none());
        let mut measurements = Measurements::default();
        assert!(measurements.start().is_none());
        measurements.record("chunks_prepare", None, 1);
        measurements.count("chunks_page_bytes", 1);
        assert!(measurements.values.is_empty());
    }
}
