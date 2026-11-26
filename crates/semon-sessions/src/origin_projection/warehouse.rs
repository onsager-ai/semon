//! Runtime-free Index admission and a full scan of committed warehouse input.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
    sync::Arc,
};

use sha2::{Digest, Sha256};

use crate::{
    IndexRecovery, Options,
    events::{self, FileIndex, Ledger, store},
    facts::{MachineFacts, RepoId},
    model::{ActivityInputs, native_copy_identity},
    storage::{Namespace, sqlite},
};

use super::{
    AdmittedBatch, AnalyticsInput, Checkpoint, Connection, Cursor, InputBudget, OpenFlags,
    OptionalExtension, ParentMethod, Reconciliation, Repo, Role, Session, SessionChange,
    SessionEvidence, Status, UNKNOWN_REPO_KEY, decode_checkpoint, decode_prices, encode_checkpoint,
    params, project_compactions, project_session, session_uid,
};

/// Admit the configured current native logs. Discovery is bounded by sources
/// and directory entries per page, and source changes use the normal ledger CAS.
/// A source lost during discovery remains for the next native reconciliation.
pub fn admit_warehouse(options: &Options, cancelled: impl Fn() -> bool) -> io::Result<()> {
    let options = Options {
        index_recovery: IndexRecovery::Refuse,
        ..options.clone()
    };
    let namespace = Namespace::new("local").map_err(io::Error::other)?;
    admit_namespace(
        &options,
        &namespace,
        None,
        false,
        false,
        |_, _| Ok(true),
        cancelled,
    )
    .map(|_| ())
}

pub(crate) fn admit_namespace(
    options: &Options,
    namespace: &Namespace,
    cutoff: Option<i64>,
    include_claude_parent: bool,
    defer_sidecar_changes: bool,
    include: impl Fn(&crate::Input, &std::path::Path) -> io::Result<bool>,
    cancelled: impl Fn() -> bool,
) -> io::Result<BTreeSet<String>> {
    let _facts_fence = options
        .facts
        .as_ref()
        .map(|_| crate::source_change::SourceLock::acquire(options, "facts", false))
        .transpose()?;
    let machine = match &options.facts {
        Some(path) => MachineFacts::Recorded(Box::new(crate::facts::read_facts(path)?)),
        None => MachineFacts::Local,
    };
    let mut cache = sqlite::open_index(options, namespace).map_err(io::Error::other)?;
    let mut discovery = crate::inputs::Discovery::current_native();
    let projects = options.claude_home.join("projects");
    let in_window = |path: &std::path::Path| {
        cutoff.is_none_or(|cutoff| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .is_none_or(|time| time.as_millis() >= u128::try_from(cutoff).unwrap_or(0))
        })
    };
    let scan_file = if defer_sidecar_changes {
        events::scan_query_file
    } else {
        events::scan_file
    };
    let mut skipped = BTreeSet::new();
    loop {
        if cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Index admission cancelled",
            ));
        }
        let page = discovery.next(options, 64, 512)?;
        let mut seen = BTreeSet::new();
        for input in page.inputs {
            let path = input.full_path(options);
            let claude_top = input.root == crate::InputRoot::Claude
                && path.parent().and_then(std::path::Path::parent) == Some(projects.as_path());
            if !in_window(&path) {
                if claude_top && let Some(id) = path.file_stem().and_then(|id| id.to_str()) {
                    skipped.insert(id.to_owned());
                }
                continue;
            }
            if input.root == crate::InputRoot::Codex
                && !machine.codex_rollout_is_current(options, &path)
            {
                continue;
            }
            if input.root == crate::InputRoot::Claude
                && !claude_top
                && !crate::native_sidecar::agent_path(&path)
            {
                continue;
            }
            if !include(&input, &path)? {
                continue;
            }
            let parent = if include_claude_parent
                && input.root == crate::InputRoot::Claude
                && crate::native_sidecar::agent_path(&path)
            {
                path.parent()
                    .and_then(std::path::Path::parent)
                    .and_then(|session| {
                        let project = session.parent()?;
                        if project.parent()? != projects {
                            return None;
                        }
                        let mut filename = session.file_name()?.to_os_string();
                        filename.push(".jsonl");
                        let parent = project.join(filename);
                        in_window(&parent).then_some(parent)
                    })
            } else {
                None
            };
            for path in std::iter::once(path).chain(parent) {
                if seen.contains(path.to_string_lossy().as_ref()) {
                    continue;
                }
                match scan_file(&path, input.root.as_str(), &mut cache, &mut false) {
                    Ok(_) => {
                        cache.committed_rebuild_ledger(&path)?;
                        seen.insert(path.to_string_lossy().into_owned());
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::WouldBlock
                        ) =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        if options.facts.is_none() {
            cache.clear_chain_admissions(&seen);
            cache.archive_consumed(options, &seen);
        }
        cache.release_published_indexes();
        if page.complete {
            return Ok(skipped);
        }
    }
}

/// One full inventory over a stable SQLite read snapshot. Open a fresh input
/// for each one-shot or watch drain. An interrupted inventory may resume at
/// a matching warehouse boundary.
pub struct WarehouseAnalyticsInput {
    connection: Connection,
    namespace: String,
    storage_namespace: Namespace,
    incarnation: String,
    revision: i64,
    through: i64,
    table: Arc<crate::pricing::EffectiveTable>,
    options: Options,
    machine: MachineFacts,
}

impl WarehouseAnalyticsInput {
    /// Open the local Index after admission. `namespace` is the analytics
    /// producer's namespace, distinct from the local Index storage binding.
    pub fn open(options: &Options, namespace: impl Into<String>) -> rusqlite::Result<Self> {
        let index = events::EventCache::path(&options.cache);
        let connection = Connection::open_with_flags(
            index,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        let storage_namespace =
            Namespace::new("local").map_err(|_| rusqlite::Error::InvalidQuery)?;
        connection.execute_batch("BEGIN DEFERRED")?;
        store::require_current_sql(&connection)?;
        sqlite::qualify(&connection, &storage_namespace)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let (incarnation, revision, through) = connection.query_row(
            "SELECT (SELECT value FROM meta WHERE key='incarnation'),
                    coalesce((SELECT max(revision) FROM warehouse_prefix),0),
                    coalesce((SELECT max(source) FROM warehouse_current),0)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let machine = options
            .facts
            .as_ref()
            .map(|path| crate::facts::source_authority::CurrentSources::open(path)?.read_snapshot())
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?
            .map_or(MachineFacts::Local, |facts| {
                MachineFacts::Recorded(Box::new(facts))
            });
        Ok(Self {
            connection,
            namespace: namespace.into(),
            storage_namespace,
            incarnation,
            revision,
            through,
            table: crate::pricing::snapshot(),
            options: options.clone(),
            machine,
        })
    }

    fn inventory(&self, after: i64, limit: i64) -> rusqlite::Result<Vec<Owner>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT d.source,d.harness,d.native_id,d.lineage,d.parent_id,
                    coalesce(json_extract(header.metadata,'$.guardian_review'),0)
             FROM dim_session d
             CROSS JOIN warehouse_current c ON c.source=d.source
             CROSS JOIN warehouse_source s ON s.source=d.source AND s.origin=?1
             CROSS JOIN lineage_member m ON m.source=d.source
             CROSS JOIN lineage_group g ON g.id=m.component AND g.owner=d.source
             LEFT JOIN fact_session_observation header ON header.source=d.source
                 AND header.pos=-1 AND header.block=0 AND header.kind='header' AND header.occurrence=0
             WHERE d.source>?2 AND d.source<=?3
             ORDER BY d.source LIMIT ?4",
        )?;
        statement
            .query_map(
                params![self.storage_namespace.binding(), after, self.through, limit],
                |row| {
                    Ok(Owner {
                        source: row.get(0)?,
                        harness: row.get(1)?,
                        native_id: row.get(2)?,
                        lineage: row.get(3)?,
                        parent_id: row.get(4)?,
                        guardian: row.get(5)?,
                    })
                },
            )?
            .collect()
    }
}

impl AnalyticsInput for WarehouseAnalyticsInput {
    type Error = rusqlite::Error;

    fn read_admitted(
        &mut self,
        checkpoint: Option<&str>,
        budget: InputBudget,
    ) -> rusqlite::Result<AdmittedBatch> {
        if budget.max_sessions == 0 || budget.max_steps == 0 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let saved = checkpoint.map(decode_checkpoint).transpose()?;
        if saved
            .as_ref()
            .is_some_and(|saved| saved.namespace != self.namespace)
        {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let through = format!("warehouse-source/{:020}", self.through);
        let start = match saved {
            Some(Checkpoint {
                incarnation,
                cursor:
                    Cursor::Inventory {
                        snapshot,
                        through: end,
                        after,
                    },
                ..
            }) if incarnation == self.incarnation
                && snapshot == self.revision
                && end == through =>
            {
                after
                    .strip_prefix("warehouse-source/")
                    .and_then(|key| key.parse::<i64>().ok())
                    .filter(|&offset| offset > 0 && offset < self.through)
                    .ok_or(rusqlite::Error::InvalidQuery)?
            }
            _ => 0,
        };
        let limit =
            i64::try_from(budget.max_sessions).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let mut changes = Vec::new();
        let mut after = start;
        let mut steps = 0usize;
        for owner in self.inventory(after, limit)? {
            let uid = owner.uid(&self.namespace);
            match self.read_session(&owner, uid.clone())? {
                Some(session) => {
                    if after != start
                        && session.steps.len() > budget.max_steps.saturating_sub(steps)
                    {
                        break;
                    }
                    steps = steps.saturating_add(session.steps.len());
                    // Retire aliases in the same publication that takes over
                    // their facts. Pending owners leave prior facts in place.
                    if owner.lineage {
                        changes.extend(self.retired_aliases(&owner)?);
                    }
                    changes.push(SessionChange::SourceRestored { session_uid: uid });
                    changes.push(SessionChange::Admitted(Box::new(session)));
                }
                None => changes.push(SessionChange::Pending { session_uid: uid }),
            }
            after = owner.source;
            if steps >= budget.max_steps {
                break;
            }
        }
        let complete = self.inventory(after, 1)?.is_empty();
        let (cursor, reconciliation) = if complete {
            let mut session_uids = BTreeSet::new();
            let mut next = 0;
            loop {
                let owners = self.inventory(next, 64)?;
                let Some(last) = owners.last() else { break };
                next = last.source;
                session_uids.extend(owners.iter().map(|owner| owner.uid(&self.namespace)));
            }
            (
                Cursor::Incremental {
                    after: self.revision,
                },
                Reconciliation::ReplaceNamespace { session_uids },
            )
        } else {
            (
                Cursor::Inventory {
                    snapshot: self.revision,
                    through,
                    after: format!("warehouse-source/{after:020}"),
                },
                Reconciliation::Incremental,
            )
        };
        let checkpoint = encode_checkpoint(&Checkpoint {
            namespace: self.namespace.clone(),
            incarnation: self.incarnation.clone(),
            cursor,
        })?;
        Ok(AdmittedBatch {
            namespace: self.namespace.clone(),
            checkpoint,
            complete,
            reconciliation,
            changes,
        })
    }
}

struct Owner {
    source: i64,
    harness: String,
    native_id: String,
    lineage: bool,
    parent_id: Option<String>,
    guardian: bool,
}

impl Owner {
    fn uid(&self, namespace: &str) -> String {
        session_uid(namespace, &(self.harness.clone(), self.native_id.clone()))
    }
}

struct Member {
    source: i64,
    native_id: String,
    path: String,
    ledger: Ledger,
    cwd: Option<String>,
    branch: Option<String>,
    start: Option<i64>,
}

impl WarehouseAnalyticsInput {
    fn retired_aliases(&self, owner: &Owner) -> rusqlite::Result<Vec<SessionChange>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT DISTINCT d.native_id FROM lineage_member root
             JOIN lineage_member m ON m.component=root.component
             JOIN dim_session d ON d.source=m.source
             WHERE root.source=?1 AND d.native_id<>?2 AND NOT EXISTS (
               SELECT 1 FROM dim_session active
               JOIN bridge_session_owner o ON o.source=active.source AND o.owner=active.source
               JOIN warehouse_current c ON c.source=active.source
               WHERE active.harness=d.harness AND active.native_id=d.native_id
             )",
        )?;
        statement
            .query_map(params![owner.source, owner.native_id], |row| {
                let native_id = row.get(0)?;
                Ok(SessionChange::Removed {
                    session_uid: session_uid(&self.namespace, &(owner.harness.clone(), native_id)),
                })
            })?
            .collect()
    }

    fn members(&self, owner: i64) -> rusqlite::Result<Vec<Member>> {
        let mut statement = self.connection.prepare_cached(
            "SELECT d.source,d.native_id,c.path,s.ledger,p.ledger,p.pos,
                    coalesce(a.cwd,f.cwd),coalesce(a.branch,f.branch),d.first_ts
             FROM lineage_member root
             CROSS JOIN lineage_member m ON m.component=root.component
             CROSS JOIN dim_session d ON d.source=m.source
             LEFT JOIN warehouse_source s ON s.source=m.source AND s.origin=?2
             LEFT JOIN warehouse_current c ON c.source=s.source
             LEFT JOIN warehouse_prefix p ON p.source=s.source AND p.superseded=0
             LEFT JOIN files f ON f.path=c.path
             LEFT JOIN maintenance_cursor mc ON mc.source=d.source
             LEFT JOIN fact_agent_observation a ON a.source=d.source AND a.revision=mc.sidecar_revision AND a.present
             WHERE root.source=?1
             ORDER BY d.first_ts IS NULL,d.first_ts,d.continued DESC,d.native_id,c.path",
        )?;
        let mut rows = statement.query(params![owner, self.storage_namespace.binding()])?;
        let mut members = Vec::new();
        while let Some(row) = rows.next()? {
            let (Some(path), Some(ledger), Some(prefix), Some(offset)) = (
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ) else {
                return Ok(Vec::new());
            };
            let ledger: Ledger =
                serde_json::from_str(&ledger).map_err(|_| rusqlite::Error::InvalidQuery)?;
            let prefix: Ledger =
                serde_json::from_str(&prefix).map_err(|_| rusqlite::Error::InvalidQuery)?;
            if i64::try_from(ledger.offset).ok() != Some(offset)
                || prefix.offset != ledger.offset
                || prefix.prefix != ledger.prefix
                || prefix.tail != ledger.tail
            {
                return Err(rusqlite::Error::InvalidQuery);
            }
            members.push(Member {
                source: row.get(0)?,
                native_id: row.get(1)?,
                path,
                ledger,
                cwd: row.get(6)?,
                branch: row.get(7)?,
                start: row.get(8)?,
            });
        }
        Ok(members)
    }

    fn read_session(
        &self,
        owner: &Owner,
        uid: String,
    ) -> rusqlite::Result<Option<semon_asi::input::AdmittedSession>> {
        if owner.harness == "copilot" {
            return Ok(None);
        }
        let members = self.members(owner.source)?;
        if members.is_empty() {
            return Ok(None);
        }
        let mut files = Vec::with_capacity(members.len());
        let mut sources = Vec::with_capacity(members.len());
        for member in &members {
            let path = std::path::Path::new(&member.path);
            if crate::native_evidence::catalog_input(path, &self.options)
                .is_none_or(|input| input.root.as_str() != owner.harness)
                || (owner.harness == "codex"
                    && !self.machine.codex_rollout_is_current(&self.options, path))
            {
                return Ok(None);
            }
            let Some(file) =
                store::warehouse_activity_file(&self.connection, &member.path, &member.ledger)?
            else {
                return Ok(None);
            };
            files.push(file);
            sources.push(
                member
                    .ledger
                    .source(&PathBuf::from(&member.path), &member.native_id),
            );
        }
        sources.sort_by(|a, b| a.path.cmp(&b.path));
        let mut inputs = ActivityInputs::default();
        for (index, (member, file)) in members.iter().zip(&files).enumerate() {
            let copied = copied_events(index, &members, &files);
            inputs.add(&member.path, &owner.harness, file, |i| copied.contains(&i));
            if owner.harness == "codex" {
                let usage = self.codex_usage(member)?;
                if usage.is_empty()
                    && !store::warehouse_has_codex_usage(
                        &self.connection,
                        &member.path,
                        &member.ledger,
                    )?
                {
                    return Ok(None);
                }
                inputs.usage.extend(usage);
            }
        }
        let shared = if owner.harness == "claude" {
            self.shared(owner.source)?
        } else {
            BTreeMap::new()
        };
        let projection = inputs.derive(owner.native_id.clone(), &shared, self.table.clone());
        let price_version = projection.activity.pricing_fingerprint.clone();
        let prices = decode_prices(&projection.pricing_snapshot, &price_version)?;
        let source_generation = crate::retention::source_generation(&sources)
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let evidence = serde_json::to_vec(&(
            &source_generation,
            &projection.activity,
            projection.facts.iter().collect::<Vec<_>>(),
        ))
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
        let generation = format!("{:x}", Sha256::digest(evidence));
        let Some(compactions) =
            project_compactions(&uid, &sources, projection.activity.compactions)?
        else {
            return Ok(None);
        };
        let parent = self.native_parent(owner)?;
        let repo = self.repo(
            members
                .iter()
                .rev()
                .find_map(|member| member.cwd.as_deref()),
        )?;
        let branch = (repo.key != UNKNOWN_REPO_KEY)
            .then(|| {
                members
                    .iter()
                    .rev()
                    .find_map(|member| member.branch.clone())
            })
            .flatten();
        let session = Session {
            machine: self.namespace.clone(),
            harness: owner.harness.clone(),
            native_session_id: owner.native_id.clone(),
            role: if owner.guardian {
                Role::Reviewer
            } else if (owner.harness == "claude" && !owner.lineage) || parent.is_some() {
                Role::Worker
            } else {
                Role::Solo
            },
            parent_method: if parent.is_some() {
                ParentMethod::Native
            } else {
                ParentMethod::None
            },
            parent_session_uid: parent
                .map(|id| session_uid(&self.namespace, &(owner.harness.clone(), id))),
            lane_root_uid: None,
            repo,
            branch,
            status: Status::Unknown,
            started_at: members.iter().filter_map(|member| member.start).min(),
            ended_at: None,
            reported_usd: None,
        };
        let mut result = project_session(SessionEvidence {
            session_uid: uid,
            session,
            sources: &sources,
            price_version,
            generation: &generation,
            steps: projection.activity.steps,
            facts: projection.facts,
            compactions,
            prices,
        })?;
        if let Some(session) = &mut result {
            session.derivation.index_incarnation = Some(self.incarnation.clone());
        }
        Ok(result)
    }

    fn codex_usage(
        &self,
        member: &Member,
    ) -> rusqlite::Result<Vec<events::session_activity::UsageStep>> {
        use events::session_activity::{SourcePosition, UsageStep};
        let mut query = self.connection.prepare_cached(
            "SELECT u.pos,u.message_id,u.model,b.billing,u.ts FROM fact_step_observation u
             JOIN fact_step_billing b USING(source,pos,block,kind,occurrence)
             WHERE u.source=?1 AND u.kind='codex-response' AND u.accepted AND u.pos<?2
             ORDER BY u.pos,u.block,u.occurrence",
        )?;
        query
            .query_map(
                params![
                    member.source,
                    i64::try_from(member.ledger.offset)
                        .map_err(|_| rusqlite::Error::InvalidQuery)?
                ],
                |row| {
                    let identity: String = row.get(1)?;
                    let billing: String = row.get(3)?;
                    Ok(UsageStep::normalized_codex(
                        serde_json::from_str(&identity)
                            .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        SourcePosition {
                            source: member.path.clone(),
                            offset: u64::try_from(row.get::<_, i64>(0)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        },
                        events::CodexUsageEvent {
                            model: row.get(2)?,
                            tokens: serde_json::from_str(&billing)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                            timestamp: row.get(4)?,
                        },
                    ))
                },
            )?
            .collect()
    }

    fn shared(&self, owner: i64) -> rusqlite::Result<crate::claude_usage::Shared> {
        let mut statement = self.connection.prepare_cached(
            "SELECT u.message_id,min(u.uuid) FROM lineage_member root
             CROSS JOIN lineage_member member ON member.component=root.component
             CROSS JOIN fact_step_observation u ON u.source=member.source AND u.kind='claude'
             WHERE root.source=?1 AND u.uuid IS NOT NULL AND EXISTS (
               SELECT 1 FROM fact_step_observation other INDEXED BY usage_message
               CROSS JOIN lineage_member om ON om.source=other.source
               CROSS JOIN warehouse_current oc ON oc.source=other.source
               CROSS JOIN warehouse_source os ON os.source=other.source AND os.origin=?2
               WHERE other.message_id=u.message_id AND other.uuid=u.uuid AND other.kind='claude'
                 AND om.component<>root.component
             ) GROUP BY u.message_id",
        )?;
        statement
            .query_map(params![owner, self.storage_namespace.binding()], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?
            .collect()
    }

    fn native_parent(&self, owner: &Owner) -> rusqlite::Result<Option<String>> {
        if owner.harness == "codex" {
            return self
                .connection
                .query_row(
                    "SELECT d.native_id FROM dim_session d JOIN warehouse_current c USING(source)
                 WHERE d.harness='codex' AND d.native_id=?1 AND d.source<>?2
                 GROUP BY d.native_id HAVING count(*)=1",
                    params![owner.parent_id, owner.source],
                    |row| row.get(0),
                )
                .optional();
        }
        if owner.lineage {
            return Ok(None);
        }
        self.connection.query_row(
            "WITH agent AS (
               SELECT a.storage_parent,a.call_id FROM maintenance_cursor cursor
               JOIN fact_agent_observation a ON a.source=cursor.source AND a.revision=cursor.sidecar_revision
               WHERE cursor.source=?1
             ), storage AS (
               SELECT min(m.owner) AS owner FROM agent a JOIN dim_session d ON d.harness='claude' AND d.lineage AND d.native_id=a.storage_parent
               JOIN bridge_session_owner m USING(source) HAVING count(DISTINCT m.owner)=1
             ), calls AS (
               SELECT DISTINCT m.owner FROM agent a CROSS JOIN fact_claude_call c INDEXED BY claude_call_identity ON c.call_id=a.call_id
               JOIN bridge_session_owner m USING(source)
             )
             SELECT native_id FROM dim_session WHERE source=coalesce(
               (SELECT s.owner FROM storage s JOIN calls c USING(owner)),
               (SELECT min(owner) FROM calls HAVING count(*)=1),
               (SELECT owner FROM storage)) AND source<>?1",
            [owner.source], |row| row.get(0),
        ).optional()
    }

    fn repo(&self, cwd: Option<&str>) -> rusqlite::Result<Repo> {
        let id = cwd
            .and_then(|cwd| self.machine.recorded_repo_id(cwd))
            .flatten();
        if let Some(RepoId::Remote(id)) = &id {
            return Ok(Repo {
                key: id.clone(),
                repo_id: Some(id.clone()),
                local_path: None,
                display_name: "remote repository".into(),
            });
        }
        if let Some(RepoId::Path(path)) = &id
            && let Some(key) = self
                .connection
                .query_row(
                    "SELECT repo_key FROM silver_repo_identity WHERE repo_path=?1",
                    [path],
                    |row| row.get(0),
                )
                .optional()?
        {
            return Ok(Repo {
                key,
                repo_id: None,
                local_path: Some(path.clone()),
                display_name: "local".into(),
            });
        }
        Ok(Repo {
            key: UNKNOWN_REPO_KEY.into(),
            repo_id: None,
            local_path: None,
            display_name: "unknown".into(),
        })
    }
}

fn copied_events(index: usize, members: &[Member], files: &[FileIndex]) -> BTreeSet<usize> {
    let copied: BTreeSet<_> = files
        .iter()
        .enumerate()
        .filter(|(at, file)| {
            *at != index && file.links.continued_in.contains(&members[index].native_id)
        })
        .flat_map(|(_, file)| file.events.iter().filter_map(native_copy_identity))
        .collect();
    files[index]
        .events
        .iter()
        .enumerate()
        .filter_map(|(at, event)| {
            native_copy_identity(event)
                .filter(|id| copied.contains(id))
                .map(|_| at)
        })
        .collect()
}

#[cfg(test)]
mod tests;
