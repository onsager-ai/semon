//! Opt-in producer measurements. Identifiers are opaque diagnostics, never
//! source authority, and contain no cache paths or native content.
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, time::Instant};

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
    wal: Option<u64>,
    sample_us: u128,
}

impl WriteSample {
    fn read(wal: &Path) -> Self {
        let started = Instant::now();
        let thread = thread_writes();
        let wal = std::fs::metadata(wal).ok().map(|value| value.len());
        Self {
            thread,
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
    pub(crate) fn new(enabled: bool, index: &Path, group: &'static str) -> Self {
        Self(enabled.then(|| {
            let mut wal = index.as_os_str().to_os_string();
            wal.push("-wal");
            let wal = std::path::PathBuf::from(wal);
            let before = WriteSample::read(&wal);
            let initial_sample_us = before.sample_us;
            WriteState {
                producer: id(index),
                group,
                wal,
                before,
                initial_sample_us,
            }
        }))
    }

    pub(crate) fn stage(&mut self, phase: &'static str) {
        let Some(state) = &mut self.0 else {
            return;
        };
        let after = WriteSample::read(&state.wal);
        let delta = after
            .thread
            .zip(state.before.thread)
            .and_then(|(after, before)| after.delta(before));
        let wal_change = wal_change(after.wal, state.before.wal);
        let sampling_us = state.initial_sample_us.saturating_add(after.sample_us);
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
        let mut activity = WriteActivity::new(false, Path::new("/unused"), "index_initial");
        activity.stage("rows");
        assert!(activity.0.is_none());
        let mut measurements = Measurements::default();
        assert!(measurements.start().is_none());
        measurements.record("chunks_prepare", None, 1);
        measurements.count("chunks_page_bytes", 1);
        assert!(measurements.values.is_empty());
    }
}
