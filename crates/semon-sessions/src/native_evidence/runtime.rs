//! Query observation fences for a row's runtime dependencies. This is a
//! fingerprint, never runtime or control authority, and reads no transcripts.
use super::*;
use sha2::{Digest, Sha256};

const PREFIX: &str = "scoped-v1:";
pub(crate) struct RuntimeObservations {
    facts: Option<crate::Facts>,
    local: bool,
    pids: BTreeMap<String, Vec<crate::model::PidFile>>,
    names: BTreeMap<(String, String), usize>,
    runs: BTreeMap<String, Vec<usize>>,
}
/// Runtime dependencies captured for native publication.
pub(crate) struct ScopedRuntimeObservations(RuntimeObservations);
impl ScopedRuntimeObservations {
    pub(crate) fn scope(
        &self,
        row: &crate::model::summary::CatalogRow,
        cwds: &BTreeSet<String>,
    ) -> io::Result<String> {
        self.0.scope(row, cwds)
    }
}
impl RuntimeObservations {
    pub(crate) fn new(options: &Options) -> Self {
        Self::new_measured(options, &mut crate::native_timing::Measurements::default())
    }
    pub(crate) fn new_measured(
        options: &Options,
        measurements: &mut crate::native_timing::Measurements,
    ) -> Self {
        Self::fresh_measured(options, measurements)
    }
    /// Only for a publication that captured this initial runtime successfully
    /// and independently rechecks it after deriving all scoped Query rows.
    pub(crate) fn for_publication_measured(
        options: &Options,
        _initial_runtime: &str,
        measurements: &mut crate::native_timing::Measurements,
    ) -> ScopedRuntimeObservations {
        Self::fresh_measured(options, measurements).into_scoped()
    }
    pub(crate) fn into_scoped(self) -> ScopedRuntimeObservations {
        ScopedRuntimeObservations(self)
    }
    fn fresh_measured(
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
        Self::from_facts_measured(options, facts, local, measurements)
    }

    #[cfg(feature = "postgres")]
    pub(crate) fn recorded(options: &Options, facts: crate::Facts) -> Self {
        Self::from_facts_measured(
            options,
            Some(facts),
            false,
            &mut crate::native_timing::Measurements::default(),
        )
    }

    fn from_facts_measured(
        options: &Options,
        facts: Option<crate::Facts>,
        local: bool,
        measurements: &mut crate::native_timing::Measurements,
    ) -> Self {
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
        Self {
            facts,
            local,
            pids,
            names,
            runs,
        }
    }
    pub(crate) fn scope(
        &self,
        row: &crate::model::summary::CatalogRow,
        cwds: &BTreeSet<String>,
    ) -> io::Result<String> {
        let parts = dependencies(row, cwds)
            .into_iter()
            .map(|key| {
                let value = self.dependency(&key)?;
                Ok((key, value))
            })
            .collect::<io::Result<Vec<_>>>()?;
        scope_parts(&parts)
    }

    #[cfg(feature = "postgres")]
    pub(crate) fn projections(&self) -> io::Result<Vec<(RuntimeDependency, String)>> {
        let facts = self.facts.as_ref().ok_or_else(unavailable)?;
        let mut keys = BTreeSet::from([RuntimeDependency::Common]);
        keys.extend(
            facts
                .codex_locks
                .keys()
                .chain(self.pids.keys())
                .chain(self.runs.keys())
                .cloned()
                .map(RuntimeDependency::Native),
        );
        keys.extend(
            self.names
                .keys()
                .cloned()
                .map(|(harness, id)| RuntimeDependency::Name(harness, id)),
        );
        keys.extend(facts.repos.keys().cloned().map(RuntimeDependency::Repo));
        keys.into_iter()
            .map(|key| {
                let value = self.dependency(&key)?.ok_or_else(unavailable)?;
                Ok((key, value))
            })
            .collect()
    }

    fn dependency(&self, key: &RuntimeDependency) -> io::Result<Option<String>> {
        let facts = self.facts.as_ref().ok_or_else(unavailable)?;
        let value = match key {
            RuntimeDependency::Common => serde_json::to_vec(&(
                facts.version,
                &facts.hostname,
                &facts.home,
                &facts.offline_since,
            )),
            RuntimeDependency::Native(id) => {
                let lock = facts.codex_locks.get(id);
                let pids = self.pids.get(id).map(Vec::as_slice).unwrap_or_default();
                let runs = self.runs.get(id).map(Vec::as_slice).unwrap_or_default();
                if lock.is_none() && pids.is_empty() && runs.is_empty() {
                    return Ok(None);
                }
                let process = |pid: &u32| {
                    (
                        *pid,
                        facts.proc_starts.get(pid),
                        facts.runs.get(pid),
                        facts.process_ancestors.get(pid),
                    )
                };
                let processes = pids
                    .iter()
                    .map(|pid| (pid, process(&pid.pid)))
                    .collect::<Vec<_>>();
                let runs = runs
                    .iter()
                    .map(|index| &facts.reported_runs[*index])
                    .collect::<Vec<_>>();
                serde_json::to_vec(&(lock.map(process), processes, runs))
            }
            RuntimeDependency::Name(harness, id) => {
                let Some(index) = self.names.get(&(harness.clone(), id.clone())) else {
                    return Ok(None);
                };
                let name = &facts.native_names[*index];
                serde_json::to_vec(&(&name.name, &name.source, &name.updated_at, &name.freshness))
            }
            RuntimeDependency::Repo(cwd) => {
                let repo = if self.local {
                    Some(model::repo_of(cwd, facts.home.as_deref().map(Path::new)))
                } else {
                    facts.repos.get(cwd).cloned()
                };
                let Some(repo) = repo else { return Ok(None) };
                serde_json::to_vec(&repo)
            }
        }
        .map_err(io::Error::other)?;
        Ok(Some(format!("{:x}", Sha256::digest(value))))
    }
}

fn unavailable() -> io::Error {
    io::Error::other("runtime dependency observation unavailable")
}

/// Point-key inputs for the shared per-session runtime recipe. Adapters own
/// their physical reads; these values grant no Raw or runtime authority.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub(crate) enum RuntimeDependency {
    Common,
    Native(String),
    Name(String, String),
    Repo(String),
}

pub(crate) fn dependencies(
    row: &crate::model::summary::CatalogRow,
    cwds: &BTreeSet<String>,
) -> BTreeSet<RuntimeDependency> {
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
    let mut keys = BTreeSet::from([RuntimeDependency::Common]);
    for id in ids {
        keys.insert(RuntimeDependency::Native(id.to_owned()));
        keys.insert(RuntimeDependency::Name(row.harness.clone(), id.to_owned()));
    }
    keys.extend(cwds.iter().cloned().map(RuntimeDependency::Repo));
    keys
}

pub(crate) fn cwd_fingerprint(cwds: &BTreeSet<String>) -> String {
    // Set order and byte lengths identify exact CWDs without copying long paths.
    let mut hash = Sha256::new();
    hash.update(b"semon-runtime-cwds-v1\0");
    hash.update((cwds.len() as u64).to_be_bytes());
    for cwd in cwds {
        hash.update((cwd.len() as u64).to_be_bytes());
        hash.update(cwd.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

pub(crate) fn dependency_identity(
    row: &crate::model::summary::CatalogRow,
    cwd_fingerprint: &str,
) -> io::Result<String> {
    let bytes = serde_json::to_vec(&(
        "semon-runtime-dependencies-v1",
        dependencies(row, &BTreeSet::new()),
        cwd_fingerprint,
    ))
    .map_err(io::Error::other)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn scope_parts(parts: &[(RuntimeDependency, Option<String>)]) -> io::Result<String> {
    if !parts
        .iter()
        .any(|(key, value)| *key == RuntimeDependency::Common && value.is_some())
    {
        return Err(unavailable());
    }
    let bytes = serde_json::to_vec(parts).map_err(io::Error::other)?;
    Ok(format!("{PREFIX}{:x}", Sha256::digest(bytes)))
}
