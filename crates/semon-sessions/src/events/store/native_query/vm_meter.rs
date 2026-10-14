//! Nested complete-connection instruction receipts. A child measurement must
//! restore its caller's observer; replacing it with None loses controller SQL
//! between native primitive calls. These counters never cancel or grant access.
use rusqlite::Connection;
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

struct Counter {
    active: AtomicBool,
    steps: AtomicU64,
}
thread_local! {
    static SCOPES:RefCell<Vec<(usize,Arc<Counter>)>>=const {RefCell::new(Vec::new())};
}

pub(in crate::events::store) struct Scope {
    connection: usize,
    counter: Arc<Counter>,
}

fn install(db: &Connection) {
    let key = std::ptr::from_ref(db) as usize;
    let scopes = SCOPES.with(|scopes| {
        scopes
            .borrow()
            .iter()
            .filter(|(connection, _)| *connection == key)
            .map(|(_, counter)| Arc::clone(counter))
            .collect::<Vec<_>>()
    });
    if scopes.is_empty() {
        db.progress_handler(0, None::<fn() -> bool>);
    } else {
        db.progress_handler(
            1,
            Some(move || {
                for counter in &scopes {
                    if counter.active.load(Ordering::Relaxed) {
                        counter.steps.fetch_add(1, Ordering::Relaxed);
                    }
                }
                false
            }),
        );
    }
}

pub(in crate::events::store) fn begin(db: &Connection) -> Scope {
    let scope = Scope {
        connection: std::ptr::from_ref(db) as usize,
        counter: Arc::new(Counter {
            active: AtomicBool::new(true),
            steps: AtomicU64::new(0),
        }),
    };
    SCOPES.with(|scopes| {
        scopes
            .borrow_mut()
            .push((scope.connection, Arc::clone(&scope.counter)))
    });
    install(db);
    scope
}

impl Scope {
    pub(in crate::events::store) fn finish(self, db: &Connection) -> u64 {
        assert_eq!(self.connection, std::ptr::from_ref(db) as usize);
        let steps = self.counter.steps.load(Ordering::Relaxed);
        self.remove();
        install(db);
        steps
    }
    fn remove(&self) {
        self.counter.active.store(false, Ordering::Relaxed);
        SCOPES.with(|scopes| {
            scopes.borrow_mut().retain(|(connection, counter)| {
                *connection != self.connection || !Arc::ptr_eq(counter, &self.counter)
            })
        });
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        self.remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_meter_restores_parent_and_separates_connections() {
        let db = Connection::open_in_memory().unwrap();
        let other = Connection::open_in_memory().unwrap();
        let outer = begin(&db);
        db.execute_batch("CREATE TABLE measured(value); INSERT INTO measured VALUES(1)")
            .unwrap();
        let child = begin(&db);
        let value: i64 = db
            .query_row("SELECT sum(value) FROM measured", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, 1);
        let child = child.finish(&db);
        assert!(child > 0);
        let independent = begin(&other);
        other
            .execute_batch("CREATE TABLE separate(value); INSERT INTO separate VALUES(1),(2),(3)")
            .unwrap();
        let independent = independent.finish(&other);
        assert!(independent > 0);
        let before = outer.counter.steps.load(Ordering::Relaxed);
        db.execute("INSERT INTO measured VALUES(2)", []).unwrap();
        let outer = outer.finish(&db);
        assert!(
            outer > before && before > child,
            "outer={outer} before={before} child={child}"
        );
        assert!(SCOPES.with(|scopes| scopes.borrow().is_empty()));
    }
}
