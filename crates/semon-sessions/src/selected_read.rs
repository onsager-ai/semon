//! Request-local limits for complete selected native preparation only.
use std::{
    cell::{Cell, RefCell},
    io,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Cancellation and deadline for one selected native read/repair. This contains
/// no source authority, credentials, or shared cache. Hosts retain their guard
/// until blocking work exits and set `cancelled` when the request is dropped.
/// Nested selected preparation retains the outer request's control and budget.
#[derive(Clone)]
pub struct SelectedReadControl {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    rows: usize,
    bytes: usize,
}

impl Default for SelectedReadControl {
    fn default() -> Self {
        Self::new(
            Instant::now() + Duration::from_secs(20),
            Arc::new(AtomicBool::new(false)),
        )
    }
}

impl SelectedReadControl {
    pub fn new(deadline: Instant, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            deadline,
            cancelled,
            rows: 131_072,
            bytes: 64 * 1024 * 1024,
        }
    }

    pub fn check(&self) -> io::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Selected native request cancelled",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Selected native request expired; narrow the selection or retry",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn with_limits(mut self, rows: usize, bytes: usize) -> Self {
        self.rows = rows;
        self.bytes = bytes;
        self
    }
}

struct Budget {
    control: SelectedReadControl,
    rows: Cell<usize>,
    bytes: Cell<usize>,
    exhausted: Cell<bool>,
}
#[cfg(test)]
type TestHook = (&'static str, usize, Box<dyn FnOnce()>);
thread_local! {
    static ACTIVE: RefCell<Option<Rc<Budget>>> = const { RefCell::new(None) };
    #[cfg(test)]
    static HOOK: RefCell<Option<TestHook>> = const { RefCell::new(None) };
}

pub(crate) struct Scope(Option<Rc<Budget>>);
/// Owned request limits for synchronous backend I/O. Never lend the TLS budget
/// to a driver or retain its row/byte accounting beyond the current call.
#[cfg(feature = "postgres")]
pub(crate) fn io_control() -> Option<(Arc<AtomicBool>, Instant)> {
    ACTIVE.with(|active| {
        active.borrow().as_ref().map(|budget| {
            (
                budget.control.cancelled.clone(),
                budget.control.deadline,
            )
        })
    })
}
impl Scope {
    pub(crate) fn enter(control: &SelectedReadControl) -> Self {
        Self(ACTIVE.with(|active| {
            let mut active = active.borrow_mut();
            let previous = active.clone();
            if active.is_none() {
                *active = Some(Rc::new(Budget {
                    control: control.clone(),
                    rows: Cell::new(0),
                    bytes: Cell::new(0),
                    exhausted: Cell::new(false),
                }));
            }
            previous
        }))
    }
    pub(crate) fn ensure() -> Option<Self> {
        ACTIVE
            .with(|active| active.borrow().is_none())
            .then(|| Self::enter(&SelectedReadControl::default()))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        ACTIVE.with(|active| {
            active.replace(self.0.take());
        });
    }
}

fn exhausted() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "Selected native preparation exceeds 131072 rows or its 64 MiB accounting budget; use a narrower source/window selection",
    )
}

pub(crate) fn limit_error() -> io::Error {
    ACTIVE.with(|active| {
        if let Some(budget) = active.borrow().as_ref() {
            budget.exhausted.set(true);
        }
    });
    exhausted()
}

pub(crate) fn check() -> io::Result<()> {
    ACTIVE.with(|active| match active.borrow().as_ref() {
        Some(budget) => {
            budget.control.check()?;
            if budget.exhausted.get() {
                return Err(exhausted());
            }
            Ok(())
        }
        None => Ok(()),
    })
}

pub(crate) fn checkpoint(_phase: &'static str) -> io::Result<()> {
    if !active() {
        return Ok(());
    }
    #[cfg(test)]
    {
        let hook = HOOK.with(|hook| {
            let mut hook = hook.borrow_mut();
            if let Some((phase, skip, _)) = hook.as_mut()
                && *phase == _phase
            {
                if *skip == 0 {
                    return hook.take().map(|(_, _, hook)| hook);
                }
                *skip -= 1;
            }
            None
        });
        if let Some(hook) = hook {
            hook();
        }
    }
    check()
}

#[inline]
pub(crate) fn continue_at(index: usize) -> bool {
    !index.is_multiple_of(256) || checkpoint("history_event").is_ok()
}

/// Fail before allocating decoded strings/JSON or growing selected vectors.
/// Raw dynamic cells are charged eightfold for decoded container/string work;
/// fixed row storage is charged separately. This is an admission accounting
/// bound, not a claim of total allocator/process peak usage.
pub(crate) fn row(row: &rusqlite::Row<'_>, fixed: usize) -> rusqlite::Result<()> {
    if !active() {
        return Ok(());
    }
    check().map_err(sql_error)?;
    let mut dynamic = 0usize;
    for column in 0..row.as_ref().column_count() {
        let length = match row.get_ref(column)? {
            rusqlite::types::ValueRef::Text(value) | rusqlite::types::ValueRef::Blob(value) => {
                value.len()
            }
            _ => 0,
        };
        dynamic = dynamic.saturating_add(length);
    }
    charge(fixed, dynamic).map_err(sql_error)
}

pub(crate) fn active() -> bool {
    ACTIVE.with(|active| active.borrow().is_some())
}

pub(crate) fn charge(fixed: usize, dynamic: usize) -> io::Result<()> {
    check()?;
    ACTIVE.with(|active| {
        let active = active.borrow();
        let Some(budget) = active.as_ref() else {
            return Ok(());
        };
        let bytes = fixed.saturating_add(dynamic.saturating_mul(8));
        let rows = budget.rows.get().saturating_add(1);
        let bytes = budget.bytes.get().saturating_add(bytes);
        if rows > budget.control.rows || bytes > budget.control.bytes {
            budget.exhausted.set(true);
            return Err(exhausted());
        }
        budget.rows.set(rows);
        budget.bytes.set(bytes);
        Ok(())
    })
}

pub(crate) fn sql_error(error: io::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

pub(crate) fn install(connection: &rusqlite::Connection) -> io::Result<()> {
    let control = ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()
            .map(|budget| budget.control.clone())
    });
    if let Some(control) = control {
        connection
            .set_limit(
                rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
                32 * 1024 * 1024,
            )
            .map_err(io::Error::other)?;
        connection
            .progress_handler(1000, Some(move || control.check().is_err()))
            .map_err(io::Error::other)?;
    }
    check()
}

#[cfg(test)]
pub(crate) fn set_hook(phase: &'static str, skip: usize, hook: impl FnOnce() + 'static) {
    HOOK.with(|value| *value.borrow_mut() = Some((phase, skip, Box::new(hook))));
}
