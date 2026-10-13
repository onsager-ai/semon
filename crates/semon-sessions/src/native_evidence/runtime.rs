//! Query observation fences for a row's runtime dependencies. This is a
//! fingerprint, never runtime or control authority, and reads no transcripts.
use super::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};

const PREFIX: &str = "scoped-v1:";
pub(crate) struct RuntimeObservations {
    facts: Option<crate::Facts>,
    local: bool,
    pids: BTreeMap<String, Vec<crate::model::PidFile>>,
    names: BTreeMap<(String, String), usize>,
    runs: BTreeMap<String, Vec<usize>>,
    connection: Option<Connection>,
    fallback: Option<String>,
    legacy: Option<String>,
    preserved: Option<(String, String)>,
}
impl RuntimeObservations {
    pub(crate) fn new(options: &Options) -> Self {
        Self::new_measured(options, &mut crate::native_timing::Measurements::default())
    }
    pub(crate) fn new_measured(
        options: &Options,
        measurements: &mut crate::native_timing::Measurements,
    ) -> Self {
        let started = measurements.start();
        let local = options.facts.is_none();
        let facts = options
            .facts
            .as_ref()
            .map(|path| {
                crate::read_regular_at_most(path, 64 * 1024 * 1024)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<crate::Facts>(&bytes).ok())
                    .unwrap_or_default()
            })
            .or_else(|| {
                let mut facts = crate::local_runtime_facts(options).ok()?;
                facts.native_names = crate::facts::MachineFacts::Local.native_names(options);
                facts.reported_runs = crate::events::current_reported_runs(&options.claude_json);
                for pid in facts.codex_locks.values() {
                    if let Some(start) = proc_start(&options.proc_root, *pid) {
                        facts.proc_starts.insert(*pid, start);
                    }
                }
                Some(facts)
            });
        measurements.record("runtime_ctor_facts", started, 1);
        let started = measurements.start();
        let mut pids = BTreeMap::<String, Vec<_>>::new();
        let mut names = BTreeMap::new();
        let mut runs = BTreeMap::<String, Vec<_>>::new();
        if let Some(facts) = &facts {
            for pid in crate::model::pid_files(
                options,
                &crate::facts::MachineFacts::Recorded(Box::new(facts.clone())),
            ) {
                pids.entry(pid.session.clone()).or_default().push(pid);
            }
            for (index, name) in facts.native_names.iter().enumerate() {
                names.insert((name.harness.clone(), name.native_id.clone()), index);
            }
            for (index, run) in facts.reported_runs.iter().enumerate() {
                runs.entry(run.last_session_id.clone())
                    .or_default()
                    .push(index);
            }
        }
        measurements.record("runtime_ctor_indices", started, 1);
        let started = measurements.start();
        let connection = Connection::open_with_flags(
            EventCache::path(&options.cache),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok();
        measurements.record("runtime_ctor_connection", started, 1);
        let started = measurements.start();
        let preserved = connection.as_ref().and_then(|db| db.query_row("SELECT (SELECT value FROM meta WHERE key='catalog_legacy_runtime_anchor'),(SELECT value FROM meta WHERE key='catalog_legacy_runtime_semantics')",[],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?))).ok());
        measurements.record("runtime_ctor_preserved", started, 1);
        let started = measurements.start();
        let fallback = query_runtime_generation(options).ok();
        measurements.record("runtime_ctor_fallback", started, 1);
        let started = measurements.start();
        let legacy = legacy_runtime_generation(options).ok();
        measurements.record("runtime_ctor_legacy", started, 1);
        Self {
            facts,
            local,
            pids,
            names,
            runs,
            connection,
            fallback,
            legacy,
            preserved,
        }
    }
    pub(crate) fn expected(&self, row: &crate::model::summary::CatalogRow) -> Option<String> {
        let saved = row
            .query
            .as_ref()
            .and_then(|query| query["runtime"].as_str());
        if saved.is_some_and(|runtime| runtime.starts_with(PREFIX)) {
            return self.scope(row).ok();
        }
        if saved == self.fallback.as_deref() {
            return self.fallback.clone();
        }
        if saved == self.legacy.as_deref() {
            return self.legacy.clone();
        }
        if let Some((anchor, semantics)) = &self.preserved
            && saved == Some(anchor.as_str())
            && self.fallback.as_deref() == Some(semantics.as_str())
            && self.legacy_dependencies_clean(row).unwrap_or(false)
        {
            return Some(anchor.clone());
        }
        self.legacy.clone()
    }
    fn legacy_dependencies_clean(
        &self,
        row: &crate::model::summary::CatalogRow,
    ) -> rusqlite::Result<bool> {
        let Some(db) = &self.connection else {
            return Ok(false);
        };
        for source in &row.sources {
            if db.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path=?1)",
                [source.path.to_string_lossy()],
                |row| row.get::<_, bool>(0),
            )? {
                return Ok(false);
            }
        }
        let mut ids = row
            .native_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if let Some(rows) = row
            .query
            .as_ref()
            .and_then(|query| query["rows"].as_object())
        {
            ids.extend(rows.keys().map(String::as_str));
        }
        if ids.len() > 64 {
            return Ok(false);
        }
        let mut pending=db.prepare("SELECT EXISTS(SELECT 1 FROM session_catalog_invalidations WHERE source_path IN (SELECT source_path FROM session_catalog_sources WHERE lifecycle='current' AND harness=?1 AND native_id=?2 UNION SELECT source_path FROM session_catalog_sources WHERE lifecycle='current' AND session_key=?2))")?;
        for id in ids {
            if pending.query_row(rusqlite::params![row.harness, id], |row| {
                row.get::<_, bool>(0)
            })? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(crate) fn scope(&self, row: &crate::model::summary::CatalogRow) -> io::Result<String> {
        let Some(facts) = &self.facts else {
            return self
                .fallback
                .clone()
                .ok_or_else(|| io::Error::other("runtime observation unavailable"));
        };
        let mut ids = row
            .native_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if let Some(rows) = row
            .query
            .as_ref()
            .and_then(|query| query["rows"].as_object())
        {
            ids.extend(rows.keys().map(String::as_str));
        }
        let mut cwds = BTreeSet::new();
        let Some(connection) = self.connection.as_ref() else {
            // Compatibility/in-memory model builders have no durable source
            // metadata. Keep their original observation fence; only a scoped
            // persisted producer can qualify dependency-specific recipes.
            return self
                .fallback
                .clone()
                .ok_or_else(|| io::Error::other("runtime observation unavailable"));
        };
        let mut cwd = connection
            .prepare("SELECT cwd FROM files WHERE path=?1")
            .map_err(io::Error::other)?;
        for source in &row.sources {
            let value = cwd
                .query_row([source.path.to_string_lossy()], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .optional()
                .map_err(io::Error::other)?
                .flatten();
            if let Some(value) = value {
                cwds.insert(value);
            }
        }
        let repos = cwds
            .iter()
            .map(|cwd| {
                let repo = if self.local {
                    Some(model::repo_of(cwd, facts.home.as_deref().map(Path::new)))
                } else {
                    facts.repos.get(cwd).cloned()
                };
                (cwd, repo)
            })
            .collect::<Vec<_>>();
        let locks = ids
            .iter()
            .filter_map(|id| facts.codex_locks.get(*id).map(|value| (*id, value)))
            .collect::<Vec<_>>();
        let codex_processes = locks
            .iter()
            .map(|(_, pid)| {
                (
                    *pid,
                    facts.proc_starts.get(pid),
                    facts.runs.get(pid),
                    facts.process_ancestors.get(pid),
                )
            })
            .collect::<Vec<_>>();
        let names = ids
            .iter()
            .filter_map(|id| self.names.get(&(row.harness.clone(), (*id).to_owned())))
            .map(|index| {
                let name = &facts.native_names[*index];
                (
                    &name.native_id,
                    &name.name,
                    &name.source,
                    &name.updated_at,
                    &name.freshness,
                )
            })
            .collect::<Vec<_>>();
        let pids = ids
            .iter()
            .filter_map(|id| self.pids.get(*id))
            .flatten()
            .collect::<Vec<_>>();
        let processes = pids
            .iter()
            .map(|pid| {
                (
                    pid,
                    facts.proc_starts.get(&pid.pid),
                    facts.runs.get(&pid.pid),
                    facts.process_ancestors.get(&pid.pid),
                )
            })
            .collect::<Vec<_>>();
        let runs = ids
            .iter()
            .filter_map(|id| self.runs.get(*id))
            .flatten()
            .map(|index| &facts.reported_runs[*index])
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&(
            facts.version,
            &facts.hostname,
            &facts.home,
            &facts.offline_since,
            repos,
            locks,
            codex_processes,
            names,
            processes,
            runs,
        ))
        .map_err(io::Error::other)?;
        Ok(format!("{PREFIX}{:x}", Sha256::digest(bytes)))
    }
}
