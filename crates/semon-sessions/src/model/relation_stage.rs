//! Content-free native relationship observations. Physical source indexes stay
//! selected; complete incident placements are replayed before native turn/slot
//! derivation. The producer, rather than this representation, owns epoch CAS.
use super::*;

/// Explicit collector admission withdrawal, independent of a missing source
/// or absent parser observation. Runtime fingerprints exclude source inventory,
/// so retain its exact facts authority as well as the physical generation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Withdrawal {
    pub(crate) path: String,
    pub(crate) relative: String,
    pub(crate) stat: crate::events::Stat,
    pub(crate) facts_path: PathBuf,
    pub(crate) facts_identity: crate::facts::source_authority::Identity,
    pub(crate) runtime: String,
}

impl Withdrawal {
    pub(crate) fn authority_current(&self) -> io::Result<bool> {
        let authority = crate::facts::source_authority::CurrentSources::open(&self.facts_path)?;
        let identity =
            crate::facts::source_authority::Identity::of(&fs::symlink_metadata(&self.facts_path)?)?;
        if identity != self.facts_identity || !authority.selection_known() {
            return Ok(false);
        }
        authority.validate()?;
        Ok(true)
    }

    pub(crate) fn current(&self) -> io::Result<bool> {
        if !crate::is_input_path("codex", &self.relative)
            || !self.relative.ends_with(".jsonl")
            || !self.authority_current()?
        {
            return Ok(false);
        }
        let source = crate::open_input(Path::new(&self.path))?;
        let before = crate::events::Stat::from_metadata(&source.metadata()?)?;
        if before != self.stat {
            return Ok(false);
        }
        let authority = crate::facts::source_authority::CurrentSources::open(&self.facts_path)?;
        let withdrawn = authority.codex_current(&self.relative)? == Some(false);
        authority.validate()?;
        let current_source = crate::open_input(Path::new(&self.path))?;
        Ok(withdrawn
            && crate::events::Stat::from_metadata(&source.metadata()?)? == before
            && crate::events::Stat::from_metadata(&current_source.metadata()?)? == before
            && self.authority_current()?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Phase {
    Indexing,
    Identity,
    Relations,
    Publication,
}

#[derive(Clone, Debug)]
pub(crate) struct Batch {
    pub(crate) epoch: i64,
    pub(crate) phase: Phase,
    pub(crate) paths: Vec<String>,
    pub(crate) base_generation: Option<String>,
    pub(crate) source_revision: i64,
    pub(crate) runtime: String,
    pub(crate) context: Context,
    pub(crate) relation_cursor: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Context {
    pub(crate) epoch: Option<i64>,
    pub(crate) endpoints: BTreeMap<String, Endpoint>,
    pub(crate) relations: BTreeMap<String, Relation>,
    pub(crate) family_costs: BTreeMap<String, Option<f64>>,
    pub(crate) children: BTreeMap<String, Vec<String>>,
    pub(crate) native_events: Vec<NativeEvent>,
    pub(crate) companions: Vec<String>,
    pub(crate) matching_locations: Option<Vec<Location>>,
}

#[derive(Clone, Debug)]
pub(crate) enum Selector {
    NativeKey {
        native: String,
        harness: String,
    },
    ClaudeName {
        name: String,
    },
    ClaudeCall {
        id: String,
    },
    CodexSibling {
        parent: String,
        task: String,
    },
    CodexRelay {
        root: String,
        target: String,
    },
    CodexWait {
        root: String,
        target: String,
        exclude: String,
    },
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Selection {
    pub(crate) count: usize,
    pub(crate) endpoint: Option<Endpoint>,
    pub(crate) location: Option<Location>,
    pub(crate) key: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Endpoint {
    pub(crate) row: summary::CatalogRow,
    pub(crate) paths: Vec<String>,
    pub(crate) nickname: Option<String>,
    pub(crate) names: Vec<String>,
    pub(crate) alive: bool,
    pub(crate) busy: bool,
    pub(crate) waiting: bool,
    pub(crate) lane: bool,
    pub(crate) role: bool,
    pub(crate) state: String,
    pub(crate) parent: Option<String>,
    pub(crate) native_parents: Vec<(String, String)>,
    pub(crate) root: String,
    pub(crate) root_alive: bool,
    pub(crate) tokens: crate::Tokens,
    pub(crate) facts_kind: String,
    pub(crate) pid: Option<u32>,
    pub(crate) observed_alive: Option<bool>,
    pub(crate) recorded_status: Option<String>,
    pub(crate) run: Option<BTreeMap<String, String>>,
    pub(crate) turns_truncated: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Location {
    pub(crate) source: summary::CatalogSource,
    pub(crate) offset: u64,
    pub(crate) block: u32,
    pub(crate) kind: Kind,
    pub(crate) occurrence: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeEvent {
    pub(crate) owner: String,
    pub(crate) at: Location,
    pub(crate) event: Event,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Placement {
    pub(crate) at: Location,
    pub(crate) outgoing: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preview {
    pub(crate) owner: String,
    pub(crate) at: Location,
    pub(crate) reply: bool,
    pub(crate) kind: String,
    pub(crate) arg: Option<String>,
    pub(crate) skip: usize,
}

/// Deliberately excludes Handoff brief/result/answer strings. Those remain
/// native, request-local fields; only their recorded presence influences slots.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Relation {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) from: String,
    pub(crate) to: Option<String>,
    pub(crate) target: Option<String>,
    pub(crate) at: i64,
    pub(crate) done: Option<i64>,
    pub(crate) status: String,
    pub(crate) has_result: bool,
    pub(crate) unmatched: bool,
    pub(crate) ambiguous: bool,
    pub(crate) placements: Vec<Placement>,
    pub(crate) heads: Vec<String>,
    pub(crate) after: Vec<Location>,
    pub(crate) previews: BTreeMap<String, Preview>,
    pub(crate) order: (u8, i64, String, u64, u32),
    pub(crate) explicit_previews: bool,
    pub(crate) stubs: Vec<Stub>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stub {
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) harness: String,
    pub(crate) start: i64,
    pub(crate) last: i64,
}

fn static_state(value: &str) -> io::Result<&'static str> {
    match value {
        "new" => Ok("new"),
        "work" => Ok("work"),
        "wait" => Ok("wait"),
        "idle" => Ok("idle"),
        "done" => Ok("done"),
        "err" => Ok("err"),
        _ => Err(io::Error::other("unsupported staged native state")),
    }
}

fn static_harness(value: &str) -> io::Result<&'static str> {
    match value {
        "claude" => Ok("claude"),
        "codex" => Ok("codex"),
        "copilot" => Ok("copilot"),
        _ => Err(io::Error::other("unsupported staged native harness")),
    }
}

fn preview_field(value: &str) -> io::Result<&'static str> {
    match value {
        "brief" => Ok("brief"),
        "result" => Ok("result"),
        _ => Err(io::Error::other("unsupported staged native preview field")),
    }
}

fn preview_kind(value: &str) -> io::Result<&'static str> {
    match value {
        "spawn" => Ok("spawn"),
        "trace_prompt" => Ok("trace_prompt"),
        "assistant" => Ok("assistant"),
        "received_agent" => Ok("received_agent"),
        "tool_result" => Ok("tool_result"),
        "relay" => Ok("relay"),
        "notification" => Ok("notification"),
        "handback" => Ok("handback"),
        "received_cross" => Ok("received_cross"),
        _ => Err(io::Error::other("unsupported staged native preview kind")),
    }
}

#[derive(Debug)]
struct SourceGenerationChanged;

impl std::fmt::Display for SourceGenerationChanged {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("staged native source generation changed")
    }
}

impl std::error::Error for SourceGenerationChanged {}

pub(crate) fn source_generation_changed(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|cause| cause.is::<SourceGenerationChanged>())
}

impl Location {
    fn capture(builder: &Builder<'_>, at: Ref) -> Self {
        let found = event(builder.files, at);
        Self {
            source: summary::source(&builder.files[at.0]),
            offset: found.o,
            block: found.b,
            kind: found.k,
            occurrence: builder.files[at.0].summary.events[..at.1]
                .iter()
                .filter(|event| event.o == found.o && event.b == found.b && event.k == found.k)
                .count(),
        }
    }

    /// A foreign source has no local Ref. A selected source must match the
    /// entire consumed-generation witness before translating its location.
    pub(super) fn local(&self, builder: &Builder<'_>) -> io::Result<Option<Ref>> {
        let Some(file) = builder
            .files
            .iter()
            .position(|file| file.path == self.source.path)
        else {
            return Ok(None);
        };
        if summary::source(&builder.files[file]) != self.source {
            return Err(io::Error::other(SourceGenerationChanged));
        }
        let mut matches = builder.files[file]
            .summary
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| {
                event.o == self.offset && event.b == self.block && event.k == self.kind
            });
        let Some((at, _)) = matches.nth(self.occurrence) else {
            return Err(io::Error::other("staged native event location disappeared"));
        };
        Ok(Some((file, at)))
    }
}

impl Endpoint {
    pub(crate) fn facts(&self) -> SessionFacts {
        let kind = match self.facts_kind.as_str() {
            "session" => "session",
            "subagent" => "subagent",
            "codex-run" => "codex-run",
            "copilot-session" => "copilot-session",
            _ => "stub",
        };
        SessionFacts {
            kind,
            parent: self.parent.clone(),
            pid: self.pid,
            alive: self.observed_alive,
            recorded_status: self.recorded_status.clone(),
            run: self.run.clone(),
            first: self.row.start,
            last: self.row.last,
            tokens: (kind != "stub").then(|| self.tokens.clone()),
            turns_truncated: self.turns_truncated,
        }
    }
}

impl Context {
    pub(super) fn capture(
        builder: &Builder<'_>,
        observed: Option<&BTreeMap<String, SessionFacts>>,
    ) -> Self {
        let rows = summary::catalog(
            builder,
            &builder
                .handoffs
                .iter()
                .map(|h| h.out.clone())
                .collect::<Vec<_>>(),
        );
        let fallback = builder.session_facts(&BTreeSet::new(), |_| (None, None), |_, _| None);
        let facts = observed.unwrap_or(&fallback);
        let endpoints: BTreeMap<_, _> = rows
            .into_iter()
            .map(|row| {
                let session = &builder.sessions[builder.by_key[&row.key]];
                let observed = &facts[&row.key];
                let mut paths = Vec::new();
                let mut nickname = None;
                for file in &session.files {
                    if let Role::Codex(meta) = &builder.files[*file].role {
                        paths.extend(meta.path.iter().cloned());
                        nickname = meta.nickname.clone();
                    }
                }
                let endpoint = Endpoint {
                    paths,
                    nickname,
                    names: session.names.iter().cloned().collect(),
                    alive: session.alive,
                    busy: session.busy,
                    waiting: session.waiting,
                    lane: session.out.lane,
                    role: session.out.role,
                    state: session.out.state.to_owned(),
                    parent: session
                        .parent
                        .map(|parent| builder.sessions[parent].key.clone()),
                    native_parents: session
                        .files
                        .iter()
                        .filter_map(|at| {
                            let file = &builder.files[*at];
                            match &file.role {
                                Role::Agent(meta) => {
                                    Some(("claude".to_owned(), meta.parent.clone()))
                                }
                                Role::Codex(meta) if !meta.guardian_review => meta
                                    .parent_thread
                                    .as_ref()
                                    .map(|parent| ("codex".to_owned(), parent.clone()))
                                    .or_else(|| {
                                        file.marker.as_ref().map(|marker| {
                                            (marker.harness.clone(), marker.parent_id.clone())
                                        })
                                    }),
                                _ => None,
                            }
                        })
                        .collect(),
                    root: builder.sessions[builder.root(builder.by_key[&row.key])]
                        .key
                        .clone(),
                    root_alive: builder.graph_root_alive(builder.by_key[&row.key]),
                    tokens: session.tokens.clone(),
                    facts_kind: observed.kind.to_owned(),
                    pid: observed.pid,
                    observed_alive: observed.alive,
                    recorded_status: observed.recorded_status.clone(),
                    run: observed.run.clone(),
                    turns_truncated: observed.turns_truncated,
                    row,
                };
                (endpoint.row.key.clone(), endpoint)
            })
            .collect();
        let relations = builder
            .handoffs
            .iter()
            .enumerate()
            .filter(|(_, h)| matches!(h.out.kind, "spawn" | "relay"))
            .map(|(index, h)| {
                let previews = h
                    .previews
                    .iter()
                    .flat_map(|previews| previews.iter())
                    .map(|(field, preview)| {
                        (
                            (*field).to_owned(),
                            Preview {
                                owner: builder.sessions[builder.of_file[preview.at.0]].key.clone(),
                                at: Location::capture(builder, preview.at),
                                reply: preview.reply,
                                kind: preview.kind.to_owned(),
                                arg: preview.arg.clone(),
                                skip: preview.skip,
                            },
                        )
                    })
                    .collect();
                let mut relation = Relation {
                    id: h.out.id.clone(),
                    kind: h.out.kind.to_owned(),
                    from: h.out.from.clone(),
                    to: h.out.to.clone(),
                    target: h.out.target.clone(),
                    at: h.out.at,
                    done: h.out.done,
                    status: h.out.status.to_owned(),
                    has_result: h.out.result.is_some(),
                    unmatched: h.out.unmatched,
                    ambiguous: h.out.ambiguous,
                    placements: builder
                        .placed
                        .iter()
                        .flat_map(|(at, entries)| {
                            entries
                                .iter()
                                .filter(move |(handoff, _)| *handoff == index)
                                .map(|(_, place)| Placement {
                                    at: Location::capture(builder, *at),
                                    outgoing: *place == Place::Out,
                                })
                        })
                        .collect(),
                    heads: builder
                        .head
                        .iter()
                        .filter(|&(_, handoff)| *handoff == index)
                        .map(|(owner, _)| builder.sessions[*owner].key.clone())
                        .collect(),
                    after: builder
                        .after
                        .iter()
                        .filter(|&(_, handoff)| *handoff == index)
                        .map(|(at, _)| Location::capture(builder, *at))
                        .collect(),
                    previews,
                    explicit_previews: h.previews.is_some(),
                    stubs: [h.from, h.to]
                        .into_iter()
                        .flatten()
                        .filter_map(|at| {
                            let session = &builder.sessions[at];
                            (session.kind == SessKind::Stub
                                && !builder.staged_endpoints.contains_key(&session.key))
                            .then(|| Stub {
                                key: session.key.clone(),
                                name: session.out.name.clone(),
                                harness: session.out.harness.to_owned(),
                                start: session.out.start,
                                last: session.out.last,
                            })
                        })
                        .collect(),
                    order: {
                        let owner = if h.out.kind == "spawn" {
                            h.out.to.as_ref().unwrap_or(&h.out.from)
                        } else {
                            &h.out.from
                        };
                        let endpoint = endpoints.get(owner);
                        let phase = match (
                            h.out.kind,
                            endpoint.map(|endpoint| endpoint.row.harness.as_str()),
                        ) {
                            ("spawn", Some("claude")) => 0,
                            ("spawn", _) => 1,
                            ("relay", Some("claude")) => 2,
                            _ => 3,
                        };
                        let first = endpoint
                            .and_then(|endpoint| endpoint.row.start)
                            .unwrap_or(i64::MAX);
                        let path = endpoint
                            .and_then(|endpoint| endpoint.row.sources.first())
                            .map(|source| source.path.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let location = builder.placed.iter().find_map(|(at, entries)| {
                            entries
                                .iter()
                                .any(|(handoff, place)| *handoff == index && *place == Place::Out)
                                .then(|| event(builder.files, *at))
                        });
                        (
                            phase,
                            if h.out.kind == "spawn" { first } else { 0 },
                            path,
                            if h.out.kind == "spawn" {
                                0
                            } else {
                                location.map_or(0, |event| event.o)
                            },
                            if h.out.kind == "spawn" {
                                0
                            } else {
                                location.map_or(0, |event| event.b)
                            },
                        )
                    },
                };
                relation.placements.sort_by(|a, b| {
                    (
                        &a.at.source.path,
                        a.at.offset,
                        a.at.block,
                        a.at.occurrence,
                        a.outgoing,
                    )
                        .cmp(&(
                            &b.at.source.path,
                            b.at.offset,
                            b.at.block,
                            b.at.occurrence,
                            b.outgoing,
                        ))
                });
                relation.heads.sort();
                relation.after.sort_by(|a, b| {
                    (&a.source.path, a.offset, a.block, a.occurrence).cmp(&(
                        &b.source.path,
                        b.offset,
                        b.block,
                        b.occurrence,
                    ))
                });
                (relation.id.clone(), relation)
            })
            .collect();
        Self {
            endpoints,
            relations,
            ..Default::default()
        }
    }

    pub(super) fn install_endpoints(&self, builder: &mut Builder<'_>) -> io::Result<()> {
        for endpoint in self.endpoints.values() {
            Self::install_endpoint(endpoint, builder)?;
        }
        for endpoint in self.endpoints.values() {
            if let Some(&at) = builder.by_key.get(&endpoint.row.key) {
                builder.sessions[at].parent = endpoint
                    .parent
                    .as_ref()
                    .and_then(|parent| builder.by_key.get(parent).copied());
            }
        }
        Ok(())
    }

    pub(super) fn install_endpoint(
        endpoint: &Endpoint,
        builder: &mut Builder<'_>,
    ) -> io::Result<()> {
        builder
            .staged_endpoints
            .insert(endpoint.row.key.clone(), endpoint.clone());
        if builder.by_key.contains_key(&endpoint.row.key) {
            return Ok(());
        }
        let harness = static_harness(&endpoint.row.harness)?;
        let mut out = builder.blank(endpoint.row.name.clone(), harness);
        out.model.clone_from(&endpoint.row.model);
        out.effort.clone_from(&endpoint.row.effort);
        out.repo.clone_from(&endpoint.row.repo);
        out.branch.clone_from(&endpoint.row.branch);
        out.cost.clone_from(&endpoint.row.cost);
        out.tokens = endpoint.row.tokens;
        out.state = static_state(&endpoint.state)?;
        out.lane = endpoint.lane;
        out.role = endpoint.role;
        out.parent_source.clone_from(&endpoint.row.parent_source);
        out.start = endpoint.row.start.unwrap_or(0);
        out.last = endpoint.row.last.unwrap_or(0);
        // External descriptions supply graph context, never a source owner
        // or a transcript/control selection in this selected publication.
        let at = builder.add_session(endpoint.row.key.clone(), SessKind::Stub, Vec::new(), out);
        let session = &mut builder.sessions[at];
        session.alive = endpoint.alive;
        session.busy = endpoint.busy;
        session.waiting = endpoint.waiting;
        session.first = endpoint.row.start;
        session.last = endpoint.row.last;
        session.tokens.clone_from(&endpoint.tokens);
        for native in &endpoint.row.native_ids {
            builder.by_native.insert((harness, native.clone()), at);
        }
        builder.sessions[at].parent = endpoint
            .parent
            .as_ref()
            .and_then(|parent| builder.by_key.get(parent).copied());
        Ok(())
    }

    pub(super) fn install(&self, builder: &mut Builder<'_>) -> io::Result<()> {
        self.install_endpoints(builder)?;
        // Matching retains freshly observed native baselines; only final
        // replay restores the matcher-derived state before native turn assembly.
        for endpoint in self.endpoints.values() {
            if let Some(&at) = builder.by_key.get(&endpoint.row.key) {
                let session = &mut builder.sessions[at];
                if !session.files.is_empty() {
                    session.alive = endpoint.alive;
                    session.busy = endpoint.busy;
                    session.waiting = endpoint.waiting;
                    session.out.state = static_state(&endpoint.state)?;
                    session.out.lane = endpoint.lane;
                    session.out.role = endpoint.role;
                    session
                        .out
                        .parent_source
                        .clone_from(&endpoint.row.parent_source);
                }
            }
        }
        let mut relations: Vec<_> = self.relations.values().collect();
        relations.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
        for relation in relations {
            if builder.ids.contains_key(&relation.id) {
                return Err(io::Error::other("duplicate staged native relation"));
            }
            let kind = match relation.kind.as_str() {
                "spawn" => "spawn",
                "relay" => "relay",
                _ => return Err(io::Error::other("unsupported staged native relation")),
            };
            for stub in &relation.stubs {
                if !builder.by_key.contains_key(&stub.key) {
                    let mut out = builder.blank(stub.name.clone(), static_harness(&stub.harness)?);
                    out.stub = true;
                    out.lane = true;
                    out.role = true;
                    out.start = stub.start;
                    out.last = stub.last;
                    builder.add_session(stub.key.clone(), SessKind::Stub, Vec::new(), out);
                }
            }
            let from = builder.by_key.get(&relation.from).copied();
            let to = relation
                .to
                .as_ref()
                .and_then(|to| builder.by_key.get(to).copied());
            if from.is_none() || (relation.to.is_some() && to.is_none() && !relation.unmatched) {
                return Err(io::Error::other("staged native relation endpoint missing"));
            }
            let index = builder.add_handoff(
                Handoff {
                    id: relation.id.clone(),
                    kind,
                    ask: None,
                    from: relation.from.clone(),
                    to: relation.to.clone(),
                    target: relation.target.clone(),
                    at: relation.at,
                    done: relation.done,
                    status: static_state(&relation.status)?,
                    brief: String::new(),
                    result: relation.has_result.then(String::new),
                    answer: None,
                    answers: None,
                    declined: false,
                    unmatched: relation.unmatched,
                    ambiguous: relation.ambiguous,
                },
                from,
                to,
            );
            let mut previews = BTreeMap::new();
            for (field, preview) in &relation.previews {
                builder.handoffs[index]
                    .preview_owners
                    .insert(field.clone(), preview.owner.clone());
                if let Some(at) = preview.at.local(builder)? {
                    previews.insert(
                        preview_field(field)?,
                        PreviewRef {
                            at,
                            reply: preview.reply,
                            kind: preview_kind(&preview.kind)?,
                            arg: preview.arg.clone(),
                            skip: preview.skip,
                        },
                    );
                }
            }
            builder.handoffs[index].previews = relation.explicit_previews.then_some(previews);
            for placement in &relation.placements {
                if let Some(at) = placement.at.local(builder)? {
                    builder.place(
                        at,
                        index,
                        if placement.outgoing {
                            Place::Out
                        } else {
                            Place::In
                        },
                    );
                }
            }
            for owner in &relation.heads {
                if let Some(&at) = builder.by_key.get(owner)
                    && !builder.sessions[at].files.is_empty()
                    && builder.head.insert(at, index).is_some()
                {
                    return Err(io::Error::other("duplicate staged native head"));
                }
            }
            for location in &relation.after {
                if let Some(at) = location.local(builder)?
                    && builder.after.insert(at, index).is_some()
                {
                    return Err(io::Error::other("duplicate staged native after placement"));
                }
            }
        }
        Ok(())
    }

    /// Resolve native ownership only after the identity frontier is exhausted.
    pub(crate) fn parent_paths(&self, selected: &[String]) -> Vec<String> {
        selected
            .iter()
            .filter_map(|path| {
                self.endpoints.values().find(|endpoint| {
                    endpoint
                        .row
                        .sources
                        .iter()
                        .any(|source| source.path.to_str() == Some(path))
                })
            })
            .filter_map(|endpoint| {
                endpoint
                    .parent
                    .as_ref()
                    .and_then(|parent| self.endpoints.get(parent))
            })
            .flat_map(|endpoint| {
                endpoint
                    .row
                    .sources
                    .iter()
                    .map(|source| source.path.to_string_lossy().into_owned())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// A parent loaded for native call matching supplies evidence; only the
    /// selected task owners' incident relationships enter this batch's spool.
    pub(crate) fn restrict_relations(&mut self, paths: &[String]) {
        let owners: BTreeSet<_> = self
            .endpoints
            .values()
            .filter(|endpoint| {
                endpoint
                    .row
                    .sources
                    .iter()
                    .any(|source| paths.iter().any(|path| source.path.to_str() == Some(path)))
            })
            .map(|endpoint| endpoint.row.key.clone())
            .collect();
        self.relations.retain(|_, relation| {
            owners.contains(&relation.from)
                || relation.to.as_ref().is_some_and(|to| owners.contains(to))
        });
    }
}
