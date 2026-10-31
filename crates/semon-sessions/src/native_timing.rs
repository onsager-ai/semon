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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThreadCpuTimeNs(u128);

/// Calling-thread CPU time. Callers pass the existing timing gate so disabled
/// diagnostics never issue the clock syscall.
pub(crate) fn thread_cpu_time_ns(enabled: bool) -> Option<ThreadCpuTimeNs> {
    if !enabled {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
        // SAFETY: `clock_gettime` initializes the supplied timespec on success.
        let result =
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, value.as_mut_ptr()) };
        if result != 0 {
            return None;
        }
        // SAFETY: the successful call above initialized the timespec.
        let value = unsafe { value.assume_init() };
        let seconds = u128::try_from(value.tv_sec).ok()?;
        let nanoseconds = u128::try_from(value.tv_nsec).ok()?;
        if nanoseconds >= 1_000_000_000 {
            return None;
        }
        seconds
            .checked_mul(1_000_000_000)?
            .checked_add(nanoseconds)
            .map(ThreadCpuTimeNs)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn thread_cpu_delta_us(
    start: Option<ThreadCpuTimeNs>,
    end: Option<ThreadCpuTimeNs>,
) -> Option<u128> {
    start
        .zip(end)
        .and_then(|(start, end)| end.0.checked_sub(start.0))
        .map(|nanoseconds| nanoseconds / 1_000)
}

#[derive(Clone, Copy, Debug)]
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

/// Four fixed captures, with work clocks strictly between captures. Counter
/// intervals include sampling perturbation; sample costs are never subtracted
/// from cache/kernel counters. WAL changes describe extent, not device writes.
/// Work clocks exclude captures, but retain residual boundary bookkeeping,
/// clock-call and scheduling perturbation; sample_us only times WriteSample.
pub(crate) struct CatalogCommitBoundary(Option<CatalogCommitState>);

struct CatalogCommitState {
    producer: String,
    wal: std::path::PathBuf,
    samples: [Option<WriteSample>; 4],
    work_us: [u128; 3],
    started: Option<Instant>,
    captures: usize,
    clocks: usize,
}

#[derive(Clone, Debug)]
struct CatalogCommitInterval {
    work_us: u128,
    cache: Option<SqliteCache>,
    thread: Option<ThreadWrites>,
    wal_extent_change: Option<i128>,
}

impl CatalogCommitInterval {
    fn between(before: WriteSample, after: WriteSample, work_us: u128) -> Self {
        Self {
            work_us,
            cache: after.cache.zip(before.cache).and_then(|(a, b)| a.delta(b)),
            thread: after
                .thread
                .zip(before.thread)
                .and_then(|(a, b)| a.delta(b)),
            wal_extent_change: wal_change(after.wal, before.wal),
        }
    }
}

#[derive(Clone, Debug)]
struct CatalogCommitRecord {
    intervals: [CatalogCommitInterval; 3],
    captures: usize,
    clock_observations: usize,
    sampling_us: u128,
}

#[cfg(test)]
std::thread_local! {
    static CATALOG_TIMING_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    static CATALOG_COMMIT_RECORDS: std::cell::RefCell<Option<Vec<CatalogCommitRecord>>> = const { std::cell::RefCell::new(None) };
    static CATALOG_COMMIT_COUNTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

impl CatalogCommitBoundary {
    pub(crate) fn new(enabled: bool, index: &Path) -> Self {
        Self(enabled.then(|| {
            let mut wal = index.as_os_str().to_os_string();
            wal.push("-wal");
            CatalogCommitState {
                producer: id(index),
                wal: wal.into(),
                samples: [None; 4],
                work_us: [0; 3],
                started: None,
                captures: 0,
                clocks: 0,
            }
        }))
    }

    // Called after slots, before COMMIT, after COMMIT, then after restoration.
    // This observer does no formatting/output between boundaries. Each WriteSample
    // uses two clock observations; the three work spans use two apiece. Cache
    // status reads can short-circuit, so no inferred status-call count is given.
    pub(crate) fn capture(&mut self, connection: &rusqlite::Connection) {
        let Some(state) = &mut self.0 else { return };
        let index = state.captures;
        assert!(index < 4, "Catalog has four fixed commit boundaries");
        if let Some(started) = state.started.take() {
            state.work_us[index - 1] = started.elapsed().as_micros();
            state.clocks += 1;
        }
        state.samples[index] = Some(WriteSample::read(&state.wal, connection));
        state.clocks += 2;
        state.captures += 1;
        if index < 3 {
            state.started = Some(Instant::now());
            state.clocks += 1;
        }
        #[cfg(test)]
        CATALOG_COMMIT_COUNTS.with(|counts| counts.set((state.captures, state.clocks)));
    }

    // The caller only consumes this after Written AND timeout restoration.
    // This final aggregate is outside its own postcommit work/counter window.
    pub(crate) fn committed(self) {
        let Some(state) = self.0 else { return };
        let [Some(a), Some(b), Some(c), Some(d)] = state.samples else {
            return;
        };
        let record = CatalogCommitRecord {
            intervals: [
                CatalogCommitInterval::between(a, b, state.work_us[0]),
                CatalogCommitInterval::between(b, c, state.work_us[1]),
                CatalogCommitInterval::between(c, d, state.work_us[2]),
            ],
            captures: state.captures,
            clock_observations: state.clocks,
            sampling_us: [a, b, c, d].iter().map(|sample| sample.sample_us).sum(),
        };
        #[cfg(test)]
        CATALOG_COMMIT_RECORDS.with(|records| {
            if let Some(records) = &mut *records.borrow_mut() {
                records.push(record.clone());
            }
        });
        for (phase, interval) in ["tail", "commit", "postcommit"]
            .into_iter()
            .zip(&record.intervals)
        {
            eprintln!(
                "semon sessions native: producer={} catalog_commit_boundary={phase} publication=committed io_scope=calling_thread work_us={} cache_delta={:?} io_delta={:?} wal_extent_change_bytes={:?} captures={} clock_observations={} sampling_us={} counter_windows_include_sampling=true postcommit_includes_existing_diagnostics=true",
                state.producer,
                interval.work_us,
                interval.cache,
                interval.thread,
                interval.wal_extent_change,
                record.captures,
                record.clock_observations,
                record.sampling_us
            );
        }
    }
}

#[cfg(test)]
pub(crate) mod catalog_commit_tests {
    use super::*;

    pub(crate) struct Recording;

    impl Drop for Recording {
        fn drop(&mut self) {
            CATALOG_TIMING_OVERRIDE.with(|value| value.set(None));
            CATALOG_COMMIT_RECORDS.with(|records| *records.borrow_mut() = None);
            CATALOG_COMMIT_COUNTS.with(|counts| counts.set((0, 0)));
        }
    }

    pub(crate) fn recording(enabled: bool) -> Recording {
        CATALOG_TIMING_OVERRIDE.with(|value| assert!(value.replace(Some(enabled)).is_none()));
        CATALOG_COMMIT_RECORDS.with(|records| *records.borrow_mut() = Some(Vec::new()));
        CATALOG_COMMIT_COUNTS.with(|counts| counts.set((0, 0)));
        Recording
    }

    pub(crate) fn assert_written(enabled: bool) {
        let records =
            CATALOG_COMMIT_RECORDS.with(|records| records.borrow().as_ref().unwrap().clone());
        let counts = CATALOG_COMMIT_COUNTS.with(std::cell::Cell::get);
        if enabled {
            assert_eq!(records.len(), 1);
            assert_eq!(counts, (4, 14));
            assert_eq!(records[0].captures, 4);
            assert_eq!(records[0].clock_observations, 14);
        } else {
            assert!(records.is_empty());
            assert_eq!(counts, (0, 0));
        }
    }

    pub(crate) fn assert_uncommitted() {
        CATALOG_COMMIT_RECORDS
            .with(|records| assert!(records.borrow().as_ref().unwrap().is_empty()));
    }

    pub(crate) fn counts() -> (usize, usize) {
        CATALOG_COMMIT_COUNTS.with(std::cell::Cell::get)
    }
}

/// Numerical-only fixed boundaries for the conditional Catalog traversal test.
/// The test saves these before diagnostics and emits only committed aggregates.
#[cfg(test)]
pub(crate) struct ProbeSample {
    sample: WriteSample,
    at: Instant,
}

#[cfg(test)]
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct ProbeInterval {
    pub(crate) elapsed_us: u128,
    pub(crate) sampling_us: u128,
    cache: Option<[u64; 4]>, // hit, miss, write, spill
    temp_bytes: Option<u64>,
    thread: Option<[u64; 4]>, // wchar, syscw, write_bytes, cancelled_write_bytes
    wal_extent_change: Option<i128>,
}

#[cfg(test)]
impl ProbeSample {
    pub(crate) fn sampling_us(&self) -> u128 {
        self.sample.sample_us
    }

    pub(crate) fn read(connection: &rusqlite::Connection) -> Self {
        let mut wal = connection
            .path()
            .expect("probe uses a file-backed producer")
            .to_owned();
        wal.push_str("-wal");
        Self {
            sample: WriteSample::read(Path::new(&wal), connection),
            at: Instant::now(),
        }
    }

    pub(crate) fn interval(&self, after: &Self) -> ProbeInterval {
        let cache = after
            .sample
            .cache
            .zip(self.sample.cache)
            .and_then(|(after, before)| after.delta(before));
        let thread = after
            .sample
            .thread
            .zip(self.sample.thread)
            .and_then(|(after, before)| after.delta(before));
        ProbeInterval {
            elapsed_us: after.at.duration_since(self.at).as_micros(),
            sampling_us: self.sample.sample_us + after.sample.sample_us,
            cache: cache.map(|value| [value.hit, value.miss, value.write, value.spill]),
            temp_bytes: cache.and_then(|value| value.temp_spill_bytes),
            thread: thread.map(|value| {
                [
                    value.wchar,
                    value.syscw,
                    value.write_bytes,
                    value.cancelled_write_bytes,
                ]
            }),
            wal_extent_change: wal_change(after.sample.wal, self.sample.wal),
        }
    }
}

#[cfg(test)]
impl ProbeInterval {
    pub(crate) fn add(&mut self, other: &Self) {
        self.elapsed_us += other.elapsed_us;
        self.sampling_us += other.sampling_us;
        for (left, right) in [
            (&mut self.cache, other.cache),
            (&mut self.thread, other.thread),
        ] {
            *left = (*left)
                .zip(right)
                .map(|(left, right)| std::array::from_fn(|i| left[i] + right[i]));
        }
        self.temp_bytes = self
            .temp_bytes
            .zip(other.temp_bytes)
            .map(|(left, right)| left + right);
        self.wal_extent_change = self
            .wal_extent_change
            .zip(other.wal_extent_change)
            .map(|(left, right)| left + right);
    }

    pub(crate) fn excluding(&self, nested: &Self) -> Self {
        Self {
            elapsed_us: self.elapsed_us.checked_sub(nested.elapsed_us).unwrap(),
            // Sampling is reported as unique boundary costs separately, since
            // adjacent nested windows share the same sample.
            sampling_us: self.sampling_us,
            cache: self.cache.zip(nested.cache).and_then(|(whole, part)| {
                Some([
                    whole[0].checked_sub(part[0])?,
                    whole[1].checked_sub(part[1])?,
                    whole[2].checked_sub(part[2])?,
                    whole[3].checked_sub(part[3])?,
                ])
            }),
            temp_bytes: self
                .temp_bytes
                .zip(nested.temp_bytes)
                .and_then(|(whole, part)| whole.checked_sub(part)),
            thread: self.thread.zip(nested.thread).and_then(|(whole, part)| {
                Some([
                    whole[0].checked_sub(part[0])?,
                    whole[1].checked_sub(part[1])?,
                    whole[2].checked_sub(part[2])?,
                    whole[3].checked_sub(part[3])?,
                ])
            }),
            wal_extent_change: self
                .wal_extent_change
                .zip(nested.wal_extent_change)
                .and_then(|(whole, part)| whole.checked_sub(part)),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct SearchTraversalProbe {
    pub(crate) search: ProbeInterval,
    pub(crate) postings: ProbeInterval,
    pub(crate) counts: ProbeInterval,
    pub(crate) preparation_us: u128,
    pub(crate) sampling_us: u128,
    pub(crate) owners: usize,
    pub(crate) physical_rows: usize,
    pub(crate) logical_positions: usize,
}

#[cfg(test)]
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct CatalogTraversalProbe {
    pub(crate) search: SearchTraversalProbe,
    pub(crate) commit: ProbeInterval,
    pub(crate) changed_rows: usize,
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
    #[cfg(test)]
    if let Some(enabled) = CATALOG_TIMING_OVERRIDE.with(std::cell::Cell::get) {
        return enabled;
    }
    std::env::var("SEMON_BUILD_TIMING").is_ok_and(|value| value == "1")
}

pub(crate) fn id(index: &Path) -> String {
    format!("{:x}", Sha256::digest(index.as_os_str().as_encoded_bytes()))
}

/// Authored discovery work counters. Rows count bounded source inputs/materialized
/// results, not an estimate of SQLite's physical page or index visits.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct DiscoveryCounts {
    pub(crate) statements: usize,
    pub(crate) schema_probes: usize,
    pub(crate) rows: usize,
    pub(crate) source_fullscan_steps: usize,
    pub(crate) committed_changes: u64,
    pub(crate) factory_opens: usize,
    pub(crate) skipped: usize,
}
thread_local! {
    static DISCOVERY_COUNTS: std::cell::Cell<Option<DiscoveryCounts>> = const { std::cell::Cell::new(None) };
    #[cfg(test)]
    static DISCOVERY_RECORDS: std::cell::RefCell<Vec<(&'static str,DiscoveryCounts)>> = const { std::cell::RefCell::new(Vec::new()) };
}
pub(crate) fn discovery_count(update: impl FnOnce(&mut DiscoveryCounts)) {
    DISCOVERY_COUNTS.with(|cell| {
        if let Some(mut counts) = cell.get() {
            update(&mut counts);
            cell.set(Some(counts));
        }
    });
}
pub(crate) struct DiscoveryProbe {
    state: Option<(String, &'static str, Instant)>,
    diagnostic_emit_wall_us: Option<u128>,
}

impl DiscoveryProbe {
    pub(crate) fn new(index: &Path, phase: &'static str) -> Self {
        let state = enabled().then(|| {
            DISCOVERY_COUNTS.with(|cell| {
                assert!(cell.replace(Some(DiscoveryCounts::default())).is_none());
            });
            (id(index), phase, Instant::now())
        });
        Self {
            state,
            diagnostic_emit_wall_us: None,
        }
    }

    pub(crate) fn record_diagnostic_emit(&mut self, started: Instant) {
        if self.state.is_some() {
            self.diagnostic_emit_wall_us = Some(started.elapsed().as_micros());
        }
    }
}

impl Drop for DiscoveryProbe {
    fn drop(&mut self) {
        if let Some((producer, phase, start)) = &self.state {
            let counts = DISCOVERY_COUNTS
                .with(|cell| cell.take())
                .unwrap_or_default();
            eprintln!(
                "semon sessions native: producer={producer} discovery_detail={phase} elapsed_us={} diagnostic_emit_wall_us={:?} authored_statements={} schema_probes={} source_input_or_selected_rows={} recovery_source_fullscan_steps={} committed_changes={} changed_state={} selected_factory_opens={} witness_skipped={}",
                start.elapsed().as_micros(),
                self.diagnostic_emit_wall_us,
                counts.statements,
                counts.schema_probes,
                counts.rows,
                counts.source_fullscan_steps,
                counts.committed_changes,
                counts.committed_changes > 0,
                counts.factory_opens,
                counts.skipped
            );
            #[cfg(test)]
            DISCOVERY_RECORDS.with(|records| records.borrow_mut().push((phase, counts)));
        }
    }
}
#[cfg(test)]
pub(crate) fn take_discovery_records() -> Vec<(&'static str, DiscoveryCounts)> {
    DISCOVERY_RECORDS.with(|records| std::mem::take(&mut *records.borrow_mut()))
}

/// Bounded operation aggregates: `(microseconds, workload units)`. Call sites
/// count source observations, rows, postings or scope groups, never physical I/O.
/// Disabled measurements retain no entries and start no clocks.
#[derive(Default)]
pub(crate) struct Measurements {
    enabled: bool,
    values: BTreeMap<&'static str, (u128, usize)>,
    cpu_values: BTreeMap<&'static str, (Option<u128>, usize)>,
    #[cfg(test)]
    uncached_cleanup_reference: bool,
}

/// Static nested leaves of the existing inclusive cleanup measurement. Counts
/// use the workload half of us_units; children must not be added to parents.
pub(crate) struct CleanupLabels {
    args: &'static str,
    prepare: &'static str,
    bind_execute: &'static str,
    dispose: &'static str,
    status: &'static str,
    collector: &'static str,
    vm_valid: &'static str,
    vm_unavailable: &'static str,
    vm_steps: &'static str,
    changed_rows: &'static str,
    status_observations: &'static str,
    clock_observations: &'static str,
}

pub(crate) const CHUNKS_CLEANUP: CleanupLabels = CleanupLabels {
    args: "chunks_cleanup_args",
    prepare: "chunks_cleanup_acquire_cached",
    bind_execute: "chunks_cleanup_bind_execute",
    dispose: "chunks_cleanup_dispose",
    status: "chunks_cleanup_status",
    collector: "chunks_cleanup_collector",
    vm_valid: "chunks_cleanup_vm_valid_samples",
    vm_unavailable: "chunks_cleanup_vm_unavailable_samples",
    vm_steps: "chunks_cleanup_vm_steps",
    changed_rows: "chunks_cleanup_changed_rows",
    status_observations: "chunks_cleanup_status_observations",
    clock_observations: "chunks_cleanup_clock_observations",
};

pub(crate) const LAUNCH_CLEANUP: CleanupLabels = CleanupLabels {
    args: "launch_cleanup_args",
    prepare: "launch_cleanup_acquire_cached",
    bind_execute: "launch_cleanup_bind_execute",
    dispose: "launch_cleanup_dispose",
    status: "launch_cleanup_status",
    collector: "launch_cleanup_collector",
    vm_valid: "launch_cleanup_vm_valid_samples",
    vm_unavailable: "launch_cleanup_vm_unavailable_samples",
    vm_steps: "launch_cleanup_vm_steps",
    changed_rows: "launch_cleanup_changed_rows",
    status_observations: "launch_cleanup_status_observations",
    clock_observations: "launch_cleanup_clock_observations",
};

#[cfg(test)]
type CleanupRecords = Vec<(&'static str, BTreeMap<&'static str, (u128, usize)>)>;

#[cfg(test)]
std::thread_local! {
    static CLEANUP_RECORDS: std::cell::RefCell<Option<CleanupRecords>> = const { std::cell::RefCell::new(None) };
    static CLEANUP_STATUS_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn vm_steps(before: i32, after: i32) -> Option<usize> {
    usize::try_from(before)
        .ok()
        .zip(usize::try_from(after).ok())
        .and_then(|(before, after)| after.checked_sub(before))
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

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn record_elapsed(&mut self, name: &'static str, us: u128, units: usize) {
        let entry = self.values.entry(name).or_default();
        entry.0 = entry.0.saturating_add(us);
        entry.1 = entry.1.saturating_add(units);
    }

    /// Reuses the fixed DELETE in the existing connection-local cache. A cache
    /// miss can compile SQL, and SQLite can reprepare after schema changes.
    /// Disabled writers collect nothing; every timed leaf ends before insertion.
    pub(crate) fn cleanup(
        &mut self,
        transaction: &rusqlite::Transaction<'_>,
        prefix: &str,
        pages: impl FnOnce() -> usize,
        labels: &CleanupLabels,
    ) -> rusqlite::Result<usize> {
        let cleanup_sql = "DELETE FROM main.meta WHERE namespace=semon_namespace() AND key>=?1 AND key<?2 AND key!=?3 AND (key>=?4 OR length(key)!=?5 OR substr(key,?6) GLOB '*[^0-9]*')";
        #[cfg(test)]
        if self.uncached_cleanup_reference {
            let pages = pages();
            // Exact pre-cache acquisition and six ordered bindings, available
            // only through an explicitly scoped real-parent test publication.
            return transaction.execute(
                cleanup_sql,
                rusqlite::params![
                    prefix,
                    format!("{prefix}~"),
                    format!("{prefix}header"),
                    format!("{prefix}{pages:010}"),
                    (prefix.len() + 10) as i64,
                    (prefix.len() + 1) as i64
                ],
            );
        }
        if !self.is_enabled() {
            let pages = pages();
            let mut statement = transaction.prepare_cached(cleanup_sql)?;
            return statement.execute(rusqlite::params![
                prefix,
                format!("{prefix}~"),
                format!("{prefix}header"),
                format!("{prefix}{pages:010}"),
                (prefix.len() + 10) as i64,
                (prefix.len() + 1) as i64
            ]);
        }
        let started = Instant::now();
        let pages = pages();
        let end = format!("{prefix}~");
        let header = format!("{prefix}header");
        let obsolete = format!("{prefix}{pages:010}");
        let width = (prefix.len() + 10) as i64;
        let suffix = (prefix.len() + 1) as i64;
        let args_us = started.elapsed().as_micros();
        let started = Instant::now();
        let prepared = transaction.prepare_cached(cleanup_sql);
        let prepare_us = started.elapsed().as_micros();
        let mut statement = match prepared {
            Ok(statement) => statement,
            Err(error) => {
                let started = Instant::now();
                self.record_elapsed(labels.args, args_us, 1);
                self.record_elapsed(labels.prepare, prepare_us, 1);
                self.count(labels.vm_valid, 0);
                self.count(labels.vm_unavailable, 1);
                self.count(labels.status_observations, 0);
                self.count(labels.clock_observations, 6);
                let collector_us = started.elapsed().as_micros();
                self.record_elapsed(labels.collector, collector_us, 1);
                return Err(error);
            }
        };
        let started = Instant::now();
        let before = statement.get_status(rusqlite::StatementStatus::VmStep);
        #[cfg(test)]
        CLEANUP_STATUS_READS.with(|reads| reads.set(reads.get() + 1));
        let before_us = started.elapsed().as_micros();
        let started = Instant::now();
        let result = statement.execute(rusqlite::params![
            prefix, end, header, obsolete, width, suffix
        ]);
        let execute_us = started.elapsed().as_micros();
        let started = Instant::now();
        let after = statement.get_status(rusqlite::StatementStatus::VmStep);
        #[cfg(test)]
        CLEANUP_STATUS_READS.with(|reads| reads.set(reads.get() + 1));
        let after_us = started.elapsed().as_micros();
        let started = Instant::now();
        drop(statement);
        let dispose_us = started.elapsed().as_micros();
        let started = Instant::now();
        self.record_elapsed(labels.args, args_us, 1);
        self.record_elapsed(labels.prepare, prepare_us, 1);
        self.record_elapsed(labels.bind_execute, execute_us, 1);
        self.record_elapsed(labels.dispose, dispose_us, 1);
        self.record_elapsed(labels.status, before_us + after_us, 2);
        self.count(labels.status_observations, 2);
        self.count(labels.clock_observations, 14);
        match vm_steps(before, after) {
            Some(steps) => {
                self.count(labels.vm_valid, 1);
                self.count(labels.vm_unavailable, 0);
                self.count(labels.vm_steps, steps);
            }
            None => {
                self.count(labels.vm_valid, 0);
                self.count(labels.vm_unavailable, 1);
            }
        }
        if let Ok(changed) = &result {
            self.count(labels.changed_rows, *changed);
        }
        let collector_us = started.elapsed().as_micros();
        // This excludes its own final insertion and cannot subtract residual
        // clock/scheduling perturbation from the original inclusive cleanup.
        self.record_elapsed(labels.collector, collector_us, 1);
        result
    }

    pub(crate) fn record(&mut self, name: &'static str, started: Option<Instant>, units: usize) {
        if let Some(started) = started {
            let entry = self.values.entry(name).or_default();
            entry.0 = entry.0.saturating_add(started.elapsed().as_micros());
            entry.1 = entry.1.saturating_add(units);
        }
    }

    pub(crate) fn record_cpu(
        &mut self,
        name: &'static str,
        started_ns: Option<ThreadCpuTimeNs>,
        ended_ns: Option<ThreadCpuTimeNs>,
        units: usize,
    ) {
        if !self.enabled {
            return;
        }
        let entry = self.cpu_values.entry(name).or_insert((Some(0), 0));
        entry.0 = entry
            .0
            .zip(thread_cpu_delta_us(started_ns, ended_ns))
            .map(|(total, elapsed)| total.saturating_add(elapsed));
        entry.1 = entry.1.saturating_add(units);
    }

    pub(crate) fn count(&mut self, name: &'static str, units: usize) {
        if self.enabled {
            let entry = self.values.entry(name).or_default();
            entry.1 = entry.1.saturating_add(units);
        }
    }

    pub(crate) fn emit(&self, producer: &str, group: &'static str) {
        if self.enabled {
            #[cfg(test)]
            CLEANUP_RECORDS.with(|records| {
                if let Some(records) = &mut *records.borrow_mut() {
                    records.push((group, self.values.clone()));
                }
            });
            if self.cpu_values.is_empty() {
                eprintln!(
                    "semon sessions native: producer={producer} breakdown={group} us_units={:?}",
                    self.values
                );
            } else {
                eprintln!(
                    "semon sessions native: producer={producer} breakdown={group} us_units={:?} cpu_us_units={:?}",
                    self.values, self.cpu_values
                );
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod cleanup_tests {
    use super::*;
    use rusqlite::{Connection, Transaction, TransactionBehavior, types::Value};

    pub(crate) struct Recording;

    impl Drop for Recording {
        fn drop(&mut self) {
            CLEANUP_RECORDS.with(|records| *records.borrow_mut() = None);
            CLEANUP_STATUS_READS.with(|reads| reads.set(0));
        }
    }

    pub(crate) fn recording() -> Recording {
        reset_records();
        Recording
    }

    pub(crate) fn reset_records() {
        CLEANUP_RECORDS.with(|records| *records.borrow_mut() = Some(Vec::new()));
        CLEANUP_STATUS_READS.with(|reads| reads.set(0));
    }

    pub(crate) fn records() -> CleanupRecords {
        CLEANUP_RECORDS.with(|records| records.borrow().as_ref().unwrap().clone())
    }

    pub(crate) fn status_reads() -> usize {
        CLEANUP_STATUS_READS.with(std::cell::Cell::get)
    }

    pub(crate) fn snapshot(connection: &Connection) -> BTreeMap<String, Vec<Vec<Value>>> {
        let mut state = BTreeMap::new();
        for schema in ["main", "temp"] {
            let names = connection
                .prepare(&format!(
                    "SELECT name FROM {schema}.sqlite_schema WHERE type='table' ORDER BY name"
                ))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            // Include schema definitions as well as every main/TEMP table, so
            // trigger/index changes and rollback cannot escape state parity.
            for name in names
                .into_iter()
                .chain(std::iter::once("sqlite_schema".to_owned()))
            {
                let mut statement = connection
                    .prepare(&format!(
                        "SELECT * FROM {schema}.\"{}\"",
                        name.replace('"', "\"\"")
                    ))
                    .unwrap();
                let width = statement.column_count();
                let mut rows = statement
                    .query_map([], |row| {
                        (0..width)
                            .map(|column| row.get::<_, Value>(column))
                            .collect::<rusqlite::Result<Vec<Value>>>()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<Vec<Value>>>>()
                    .unwrap();
                rows.sort_by_key(|row| format!("{row:?}"));
                state.insert(format!("{schema}.{name}"), rows);
            }
        }
        state
    }

    // Use the same real Immediate parent transaction/commit boundary as the
    // Index publisher; failed writer or commit never emits collected children.
    pub(crate) fn publish(
        connection: &mut Connection,
        enabled: bool,
        write: impl FnOnce(&Transaction<'_>, &mut Measurements) -> rusqlite::Result<()>,
    ) -> rusqlite::Result<Measurements> {
        publish_with(connection, Measurements::new(enabled), write)
    }

    pub(crate) fn publish_uncached_reference(
        connection: &mut Connection,
        write: impl FnOnce(&Transaction<'_>, &mut Measurements) -> rusqlite::Result<()>,
    ) -> rusqlite::Result<Measurements> {
        publish_with(
            connection,
            Measurements {
                // The real parent chooses write_measured via start(). Keep
                // this test-local context through that branch so cleanup can
                // execute the old uninstrumented Connection::execute path.
                enabled: true,
                uncached_cleanup_reference: true,
                ..Measurements::default()
            },
            write,
        )
    }

    fn publish_with(
        connection: &mut Connection,
        mut measurements: Measurements,
        write: impl FnOnce(&Transaction<'_>, &mut Measurements) -> rusqlite::Result<()>,
    ) -> rusqlite::Result<Measurements> {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        write(&transaction, &mut measurements)?;
        transaction.commit()?;
        if !measurements.uncached_cleanup_reference {
            measurements.emit("synthetic-cleanup", "index_rows");
        }
        Ok(measurements)
    }

    pub(crate) fn assert_sample(
        measurements: &Measurements,
        labels: &CleanupLabels,
        changed: usize,
    ) {
        assert_eq!(measurements.values[labels.vm_valid].1, 1);
        assert_eq!(measurements.values[labels.vm_unavailable].1, 0);
        assert!(measurements.values[labels.vm_steps].1 > 0);
        assert_eq!(measurements.values[labels.changed_rows].1, changed);
        assert_eq!(measurements.values[labels.status_observations].1, 2);
        for name in [
            labels.args,
            labels.prepare,
            labels.bind_execute,
            labels.dispose,
            labels.collector,
        ] {
            assert_eq!(measurements.values[name].1, 1);
        }
    }

    pub(crate) fn assert_disabled(measurements: &Measurements) {
        assert!(measurements.start().is_none());
        assert!(measurements.values.is_empty());
    }

    pub(crate) fn assert_uncached_reference(measurements: &Measurements) {
        assert!(measurements.uncached_cleanup_reference);
        for labels in [&CHUNKS_CLEANUP, &LAUNCH_CLEANUP] {
            assert!(!measurements.values.contains_key(labels.prepare));
            assert!(!measurements.values.contains_key(labels.status));
        }
    }
}

/// Disjoint candidate-page and between-page intervals. Between-page time also
/// includes producer wrapper work; it must never be called scheduler idle time.
#[derive(Default)]
pub(crate) struct ColdDiscoveryProgress {
    pages: usize,
    examined: usize,
    candidates: usize,
    active_us: u128,
    // Unavailable if timing is disabled or any page's CPU clock sample failed.
    active_thread_cpu_us: Option<u128>,
    between_pages_us: u128,
    previous_end: Option<Instant>,
}

#[derive(Clone, Copy)]
pub(crate) struct ColdDiscoveryStart {
    wall: Instant,
    thread_cpu_ns: Option<ThreadCpuTimeNs>,
}

impl ColdDiscoveryProgress {
    pub(crate) fn start(enabled: bool) -> Option<ColdDiscoveryStart> {
        if !enabled {
            return None;
        }
        Some(ColdDiscoveryStart {
            wall: Instant::now(),
            thread_cpu_ns: thread_cpu_time_ns(true),
        })
    }

    fn record(
        &mut self,
        start: ColdDiscoveryStart,
        end: Instant,
        end_thread_cpu_ns: Option<ThreadCpuTimeNs>,
        examined: usize,
        candidates: usize,
    ) {
        if let Some(previous_end) = self.previous_end {
            self.between_pages_us += start
                .wall
                .saturating_duration_since(previous_end)
                .as_micros();
        }
        self.active_us += end.saturating_duration_since(start.wall).as_micros();
        let cpu_us = thread_cpu_delta_us(start.thread_cpu_ns, end_thread_cpu_ns);
        self.active_thread_cpu_us = if self.pages == 0 {
            cpu_us
        } else {
            self.active_thread_cpu_us
                .zip(cpu_us)
                .map(|(total, elapsed)| total.saturating_add(elapsed))
        };
        self.pages += 1;
        self.examined += examined;
        self.candidates += candidates;
        self.previous_end = Some(end);
    }

    pub(crate) fn page(
        &mut self,
        cache: &Path,
        start: Option<ColdDiscoveryStart>,
        examined: usize,
        candidates: usize,
        complete: (bool, bool),
    ) {
        let Some(start) = start else {
            return;
        };
        let end_thread_cpu_ns = thread_cpu_time_ns(true);
        let end = Instant::now();
        self.record(start, end, end_thread_cpu_ns, examined, candidates);
        let producer = id(&crate::events::EventCache::path(cache));
        eprintln!(
            "semon sessions native: producer={producer} cold_discovery_progress pages={} examined={} candidates={} active_us={} active_thread_cpu_us={:?} between_pages_us={} paths_complete={} recovery_complete={}",
            self.pages,
            self.examined,
            self.candidates,
            self.active_us,
            self.active_thread_cpu_us,
            self.between_pages_us,
            complete.0,
            complete.1,
        );
    }
}

pub(crate) struct Phase(Option<(String, &'static str, Instant, Option<ThreadCpuTimeNs>)>);

impl Phase {
    pub(crate) fn new(cache: &Path, name: &'static str) -> Self {
        Self(enabled().then(|| {
            (
                id(&crate::events::EventCache::path(cache)),
                name,
                Instant::now(),
                thread_cpu_time_ns(true),
            )
        }))
    }
}

impl Drop for Phase {
    fn drop(&mut self) {
        if let Some((id, name, started, thread_cpu_started_ns)) = &self.0 {
            let thread_cpu_ended_ns = thread_cpu_time_ns(true);
            let ended = Instant::now();
            eprintln!(
                "semon sessions native: producer={id} phase={name} us={} thread_cpu_us={:?}",
                ended.saturating_duration_since(*started).as_micros(),
                thread_cpu_delta_us(*thread_cpu_started_ns, thread_cpu_ended_ns)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cold_discovery_progress_separates_active_pages_from_continuation_gaps() {
        use std::time::Duration;

        let start = Instant::now();
        let mut progress = ColdDiscoveryProgress::default();
        progress.record(
            ColdDiscoveryStart {
                wall: start,
                thread_cpu_ns: Some(ThreadCpuTimeNs(0)),
            },
            start + Duration::from_micros(10),
            Some(ThreadCpuTimeNs(10_000)),
            64,
            64,
        );
        progress.record(
            ColdDiscoveryStart {
                wall: start + Duration::from_micros(13),
                thread_cpu_ns: Some(ThreadCpuTimeNs(13_000)),
            },
            start + Duration::from_micros(20),
            Some(ThreadCpuTimeNs(20_000)),
            512,
            0,
        );
        progress.record(
            ColdDiscoveryStart {
                wall: start + Duration::from_micros(25),
                thread_cpu_ns: Some(ThreadCpuTimeNs(25_000)),
            },
            start + Duration::from_micros(30),
            Some(ThreadCpuTimeNs(30_000)),
            60,
            60,
        );
        assert_eq!(progress.pages, 3);
        assert_eq!(progress.examined, 636);
        assert_eq!(progress.candidates, 124);
        assert_eq!(progress.active_us, 22);
        assert_eq!(progress.active_thread_cpu_us, Some(22));
        assert_eq!(progress.between_pages_us, 8);
        assert_eq!(progress.active_us + progress.between_pages_us, 30);
    }

    #[test]
    fn cleanup_vm_work_distinguishes_unavailable_from_valid_zero() {
        assert_eq!(vm_steps(0, 0), Some(0));
        assert_eq!(vm_steps(7, 19), Some(12));
        for (before, after) in [(-1, 0), (0, -1), (19, 7), (i32::MAX, i32::MIN)] {
            assert_eq!(vm_steps(before, after), None);
        }
    }

    #[test]
    fn cleanup_prepare_failure_preserves_error_without_zero_vm_work_or_record() {
        let mut connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT NOT NULL) STRICT;")
            .unwrap();
        // A valid selector reaches the actual DELETE prepare. Deny that same
        // operation for both callers, while allowing metadata introspection.
        let authorizer = |context: rusqlite::hooks::AuthContext<'_>| {
            if matches!(
                context.action,
                rusqlite::hooks::AuthAction::Delete { table_name: "meta" }
            ) {
                rusqlite::hooks::Authorization::Deny
            } else {
                rusqlite::hooks::Authorization::Allow
            }
        };
        connection.authorizer(Some(authorizer)).unwrap();
        let _recording = cleanup_tests::recording();
        let transaction = connection.transaction().unwrap();
        let ordinary = transaction.execute("DELETE FROM meta WHERE key>=?1 AND key<?2 AND key!=?3 AND (key>=?4 OR length(key)!=?5 OR substr(key,?6) GLOB '*[^0-9]*')", rusqlite::params!["p:", "p:~", "p:header", "p:0000000000", 12i64, 3i64]).unwrap_err();
        let mut measurements = Measurements::new(true);
        let measured = measurements
            .cleanup(&transaction, "p:", || 0, &CHUNKS_CLEANUP)
            .unwrap_err();
        assert_eq!(measured.to_string(), ordinary.to_string());
        assert_eq!(measurements.values[CHUNKS_CLEANUP.vm_valid].1, 0);
        assert_eq!(measurements.values[CHUNKS_CLEANUP.vm_unavailable].1, 1);
        assert!(!measurements.values.contains_key(CHUNKS_CLEANUP.vm_steps));
        assert!(
            !measurements
                .values
                .contains_key(CHUNKS_CLEANUP.changed_rows)
        );
        assert_eq!(cleanup_tests::status_reads(), 0);
        assert!(cleanup_tests::records().is_empty());
        drop(transaction);
    }

    fn timing_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "semon-native-timing-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn wal_connection(name: &str) -> (std::path::PathBuf, rusqlite::Connection) {
        let root = timing_root(name);
        let connection = rusqlite::Connection::open(root.join("index.sqlite3")).unwrap();
        let journal: String = connection
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "wal");
        (root, connection)
    }

    const PROBE_CATALOG_INSERT: &str = "INSERT INTO session_catalog (namespace, session_key, lifecycle, last_ms, harness, repo, parent_key, metadata) VALUES";
    const PROBE_CATALOG_CONFLICT: &str = "ON CONFLICT(namespace,session_key) DO UPDATE SET lifecycle='current', last_ms=excluded.last_ms, harness=excluded.harness, repo=excluded.repo, parent_key=excluded.parent_key, metadata=excluded.metadata WHERE session_catalog.lifecycle != 'current' OR session_catalog.metadata != excluded.metadata OR session_catalog.parent_key IS NOT excluded.parent_key";

    #[derive(Debug, PartialEq, Eq)]
    struct ProbeCatalogRecord {
        key: String,
        lifecycle: String,
        last: i64,
        harness: String,
        repo: Option<String>,
        parent: Option<String>,
        metadata: String,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct ProbeState {
        catalog: Vec<ProbeCatalogRecord>,
        feed: Vec<(i64, String, String)>,
    }

    fn catalog_probe_connection(root: &Path, name: &str) -> rusqlite::Connection {
        let stem = root.join(name);
        // The fixture initializer creates and restores the same owned Source;
        // use its admitted semantic producer for both statement shapes.
        let connection =
            crate::events::store::catalog_probe_connection(&crate::events::EventCache::path(&stem))
                .unwrap();
        let schema: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(schema, crate::events::store::SCHEMA_VERSION);
        let mode: String = connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        for trigger in [
            "catalog_changes_insert",
            "catalog_changes_update",
            "catalog_changes_delete",
            "catalog_history_changes_insert",
            "catalog_history_changes_update",
            "catalog_history_changes_delete",
        ] {
            assert!(connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='trigger' AND name=?1)",[trigger],|row|row.get::<_,bool>(0)).unwrap());
        }
        assert_eq!(connection.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE type='index' AND tbl_name='session_catalog' AND name NOT LIKE 'sqlite_autoindex_%' AND name<>'session_catalog_pricing_fingerprint'",[],|row|row.get::<_,i64>(0)).unwrap(),21);
        assert!(connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='index' AND tbl_name='session_catalog' AND name='session_catalog_pricing_fingerprint')",[],|row|row.get::<_,bool>(0)).unwrap());
        connection
    }

    fn catalog_probe_rows(version: i64) -> Vec<crate::model::summary::CatalogRow> {
        (0..64)
            .map(|position| {
                let key = format!("probe-{position:02}");
                crate::model::summary::CatalogRow {
                    overview: None,
                    trace: None,
                    query: Some(serde_json::json!({"version":1,"state":"off","last":version})),
                    lifecycle: crate::model::summary::CatalogLifecycle::Current,
                    key: key.clone(),
                    name: format!("Probe {position}"),
                    harness: "claude".into(),
                    kind: "session".into(),
                    presentation_kind: None,
                    native_ids: vec![key],
                    parent: None,
                    parent_source: None,
                    repo: Some(format!("probe-repo-{}", position % 3)),
                    branch: None,
                    model: "probe-model".into(),
                    effort: None,
                    start: Some(1),
                    last: Some(version),
                    tokens: [0.0; 3],
                    cost: crate::pricing::Cost {
                        pricing_fingerprint: String::new(),
                        usd: Some(0.0),
                        unpriced_models: Vec::new(),
                        split_unknown_messages: 0,
                        by_model: BTreeMap::new(),
                        by_day: BTreeMap::new(),
                    },
                    sources: Vec::new(),
                }
            })
            .collect()
    }

    fn catalog_probe_state(connection: &rusqlite::Connection) -> ProbeState {
        let catalog = connection.prepare("SELECT session_key,lifecycle,last_ms,harness,repo,parent_key,metadata FROM session_catalog ORDER BY session_key LIMIT 65").unwrap().query_map([],|row|Ok(ProbeCatalogRecord {key:row.get(0)?,lifecycle:row.get(1)?,last:row.get(2)?,harness:row.get(3)?,repo:row.get(4)?,parent:row.get(5)?,metadata:row.get(6)?})).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        assert!(catalog.len() <= 64);
        // Insert, update, retained seed and promotion each emit at most128
        // feed rows. The unchanged phase and failed transaction emit none.
        let feed = connection.prepare("SELECT revision,session_key,read_scope FROM session_catalog_changes ORDER BY revision LIMIT 513").unwrap().query_map([],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        assert!(feed.len() <= 512);
        ProbeState { catalog, feed }
    }

    fn catalog_probe_values(
        row: &crate::model::summary::CatalogRow,
    ) -> Vec<rusqlite::types::Value> {
        use rusqlite::types::Value;
        vec![
            Value::Text(row.key.clone()),
            Value::Integer(row.last.unwrap_or(0)),
            Value::Text(row.harness.clone()),
            row.repo.clone().map_or(Value::Null, Value::Text),
            row.parent.clone().map_or(Value::Null, Value::Text),
            Value::Text(serde_json::to_string(row).unwrap()),
        ]
    }

    fn catalog_probe_dml(
        connection: &mut rusqlite::Connection,
        rows: &[crate::model::summary::CatalogRow],
        batched: bool,
        phase: &str,
        fail: bool,
    ) -> rusqlite::Result<std::collections::BTreeSet<String>> {
        assert_eq!(rows.len(), 64);
        let single_sql = format!(
            "{PROBE_CATALOG_INSERT} (semon_namespace(),?1,'current',?2,?3,?4,?5,?6) {PROBE_CATALOG_CONFLICT}"
        );
        let mut parameters: Vec<_> = rows.iter().map(catalog_probe_values).collect();
        if fail {
            // Duplicate keys are handled by UPSERT. Use the actual NOT NULL
            // harness constraint in the middle of this64-key batch instead.
            parameters[32][2] = rusqlite::types::Value::Null;
        }
        let mut sql = single_sql.clone();
        if batched {
            let values: Vec<_> = (0..rows.len())
                .map(|position| {
                    let first = position * 6 + 1;
                    format!(
                        "(semon_namespace(),?{first},'current',?{},?{},?{},?{},?{})",
                        first + 1,
                        first + 2,
                        first + 3,
                        first + 4,
                        first + 5
                    )
                })
                .collect();
            sql = format!(
                "{PROBE_CATALOG_INSERT} {} {PROBE_CATALOG_CONFLICT} RETURNING session_key",
                values.join(",")
            );
        }
        let tx = connection.transaction()?;
        if fail {
            // A valid earlier statement must roll back too, not only the
            // partially executed statement that hits the constraint.
            tx.execute(&single_sql, rusqlite::params_from_iter(&parameters[63]))?;
        }
        // Prepare and bind every statement before sampling. The single-row
        // oracle retains64 bounded prepared statements, not a timed rebind.
        let mut statements = Vec::new();
        if batched {
            let mut statement = tx.prepare(&sql)?;
            for (index, value) in parameters.iter().flatten().enumerate() {
                statement.raw_bind_parameter(index + 1, value)?;
            }
            statements.push((String::new(), statement));
        } else {
            for (row, values) in rows.iter().zip(&parameters) {
                let mut statement = tx.prepare(&sql)?;
                for (index, value) in values.iter().enumerate() {
                    statement.raw_bind_parameter(index + 1, value)?;
                }
                statements.push((row.key.clone(), statement));
            }
        }
        let before_io = thread_writes();
        let before_cache = SqliteCache::read(&tx);
        let started = Instant::now();
        let mut changed = std::collections::BTreeSet::new();
        for (key, mut statement) in statements {
            if batched {
                let mut returned = statement.raw_query();
                while let Some(row) = returned.next()? {
                    assert!(changed.insert(row.get::<_, String>(0)?));
                }
            } else if statement.raw_execute()? > 0 {
                assert!(changed.insert(key));
            }
        } // Finalize all statements before after-DML and commit observations.
        let dml_us = started.elapsed().as_micros();
        let after_io = thread_writes();
        let after_cache = SqliteCache::read(&tx);
        let commit_before_io = thread_writes();
        let commit_before_cache = SqliteCache::read(&tx);
        let commit_started = Instant::now();
        tx.commit()?;
        let commit_us = commit_started.elapsed().as_micros();
        let commit_after_io = thread_writes();
        let commit_after_cache = SqliteCache::read(connection);
        // Counter interpretation is qualified by the real commit above. No
        // formatting or stderr is inside either observed DML/commit span.
        let io_delta = after_io
            .zip(before_io)
            .and_then(|(after, before)| after.delta(before));
        let cache_delta = after_cache
            .zip(before_cache)
            .and_then(|(after, before)| after.delta(before));
        let commit_io = commit_after_io
            .zip(commit_before_io)
            .and_then(|(after, before)| after.delta(before));
        let commit_cache = commit_after_cache
            .zip(commit_before_cache)
            .and_then(|(after, before)| after.delta(before));
        let dml_wchar_mod_4100 = io_delta.map(|value| value.wchar % 4100);
        // Test-only direct diagnostic output keeps successful observations in
        // the ordinary CI log without a new invocation or harness setting.
        use std::io::Write;
        writeln!(std::io::stderr().lock(), "catalog-journal-probe phase={phase} batched={batched} rows=64 publication=committed io_scope=calling_thread dml_us={dml_us} dml_io={io_delta:?} dml_wchar_mod_4100={dml_wchar_mod_4100:?} dml_cache={cache_delta:?} commit_us={commit_us} commit_io={commit_io:?} commit_cache={commit_cache:?}").unwrap();
        Ok(changed)
    }

    #[test]
    fn catalog_statement_journal_batch_probe_preserves_catalog_feed_and_transaction_rollback() {
        let root = timing_root("catalog-batch-probe");
        let mut single = catalog_probe_connection(&root, "single");
        let mut batch = catalog_probe_connection(&root, "batch");
        let keys: std::collections::BTreeSet<_> = catalog_probe_rows(1)
            .into_iter()
            .map(|row| row.key)
            .collect();
        for (phase, version, expected_changes, expected_feed) in [
            ("insert", 1, 64, 128),
            ("update", 2, 64, 128),
            ("unchanged", 2, 0, 0),
            ("retained_promotion", 2, 64, 128),
        ] {
            if phase == "retained_promotion" {
                for connection in [&single, &batch] {
                    assert_eq!(connection.execute("UPDATE session_catalog SET lifecycle='retained',metadata=json_set(metadata,'$.lifecycle','retained') WHERE namespace=semon_namespace()",[]) .unwrap(),64);
                }
                assert_eq!(catalog_probe_state(&single), catalog_probe_state(&batch));
            }
            let before = catalog_probe_state(&single);
            let rows = catalog_probe_rows(version);
            let single_changed =
                catalog_probe_dml(&mut single, &rows, false, phase, false).unwrap();
            let batch_changed = catalog_probe_dml(&mut batch, &rows, true, phase, false).unwrap();
            assert_eq!(single_changed, batch_changed);
            assert_eq!(single_changed.len(), expected_changes);
            if expected_changes != 0 {
                assert_eq!(single_changed, keys);
            }
            let after = catalog_probe_state(&single);
            assert_eq!(after.catalog.len(), 64);
            assert_eq!(after.feed.len() - before.feed.len(), expected_feed);
            assert_eq!(after, catalog_probe_state(&batch));
        }
        let prior = catalog_probe_state(&single);
        let replacement = catalog_probe_rows(3);
        for (connection, batched) in [(&mut single, false), (&mut batch, true)] {
            let error =
                catalog_probe_dml(connection, &replacement, batched, "failed", true).unwrap_err();
            assert!(
                matches!(error, rusqlite::Error::SqliteFailure(ref code, _) if code.extended_code==rusqlite::ffi::SQLITE_CONSTRAINT_NOTNULL)
            );
            assert_eq!(catalog_probe_state(connection), prior);
        }
        drop(single);
        drop(batch);
        std::fs::remove_dir_all(root).unwrap();
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
    fn catalog_commit_intervals_keep_unavailable_reset_and_zero_distinct() {
        let cache = SqliteCache {
            spill: 3,
            write: 5,
            hit: 7,
            miss: 11,
            temp_spill_bytes: Some(13),
        };
        let thread = ThreadWrites {
            wchar: 17,
            syscw: 19,
            write_bytes: 23,
            cancelled_write_bytes: 29,
        };
        let before = WriteSample {
            thread: Some(thread),
            cache: Some(cache),
            wal: Some(8),
            sample_us: 31,
        };
        let zero = CatalogCommitInterval::between(before, before, 37);
        assert_eq!(
            zero.work_us, 37,
            "work time is explicit, independent of sample cost"
        );
        assert_eq!(
            zero.cache,
            Some(SqliteCache {
                spill: 0,
                write: 0,
                hit: 0,
                miss: 0,
                temp_spill_bytes: Some(0)
            })
        );
        assert_eq!(
            zero.thread,
            Some(ThreadWrites {
                wchar: 0,
                syscw: 0,
                write_bytes: 0,
                cancelled_write_bytes: 0
            })
        );
        assert_eq!(zero.wal_extent_change, Some(0));
        let after = WriteSample {
            cache: None,
            thread: None,
            wal: None,
            sample_us: 41,
        };
        let unavailable = CatalogCommitInterval::between(before, after, 0);
        assert!(unavailable.cache.is_none());
        assert!(unavailable.thread.is_none());
        assert!(unavailable.wal_extent_change.is_none());
        let reset = WriteSample {
            cache: Some(SqliteCache { write: 4, ..cache }),
            thread: Some(ThreadWrites {
                write_bytes: 22,
                ..thread
            }),
            wal: Some(2),
            sample_us: 43,
        };
        let reset = CatalogCommitInterval::between(before, reset, 0);
        assert!(reset.cache.is_none());
        assert!(reset.thread.is_none());
        assert_eq!(reset.wal_extent_change, Some(-6));
        let optional_reset = WriteSample {
            cache: Some(SqliteCache {
                temp_spill_bytes: Some(12),
                ..cache
            }),
            ..before
        };
        let optional_reset = CatalogCommitInterval::between(before, optional_reset, 0);
        assert!(optional_reset.cache.is_some());
        assert!(optional_reset.cache.unwrap().temp_spill_bytes.is_none());
    }

    #[test]
    fn disabled_diagnostics_retain_no_observation_or_measurements() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let mut activity =
            WriteActivity::new(false, Path::new("/unused"), "index_initial", &connection);
        activity.stage("rows", &connection);
        assert!(activity.0.is_none());
        let mut boundary = CatalogCommitBoundary::new(false, Path::new("/unused"));
        for _ in 0..4 {
            boundary.capture(&connection);
        }
        assert!(boundary.0.is_none());
        boundary.committed();
        let mut measurements = Measurements::default();
        assert!(measurements.start().is_none());
        measurements.record("chunks_prepare", None, 1);
        measurements.count("chunks_page_bytes", 1);
        assert!(measurements.values.is_empty());
    }
}
