//! A physical window's independent native collision owners. This admission
//! preserves the selected owner's complete native recipes; source or actual
//! relationship omissions require the paged native producer instead.
use super::*;
use rusqlite::params;

#[cfg(test)]
thread_local! {
    pub(crate) static AFTER_IDENTITY: std::cell::RefCell<Option<(u8, Box<dyn FnOnce()>)>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn after_identity_snapshot(_stage: u8) {
    #[cfg(test)]
    AFTER_IDENTITY.with(|slot| {
        let hook = {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_some_and(|(stage, _)| *stage == _stage) {
                slot.take()
            } else {
                None
            }
        };
        if let Some((_, hook)) = hook {
            hook();
        }
    });
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Headers {
    private: Option<String>,
    trace: Option<String>,
    slots: Option<String>,
    history: Option<String>,
    points: Option<String>,
}

fn headers(db: &Connection, row: &CatalogRow) -> io::Result<Headers> {
    let generation = |table: &str| -> io::Result<Option<String>> {
        db.query_row(
            &format!("SELECT generation FROM {table} WHERE session_key=?1"),
            [&row.key],
            |r| r.get(0),
        )
        .optional()
        .map_err(failure)
    };
    Ok(Headers {
        private: crate::query_projection::header(db, &crate::query_projection::identity(row))
            .map_err(failure)?,
        trace: generation("session_trace_headers")?,
        slots: generation("session_slot_projections")?,
        history: generation("session_history_projections")?,
        points: crate::events::store::native_query::point_publication::header(db, row)
            .map_err(failure)?,
    })
}

pub(crate) struct Identity {
    pub(crate) owner: CatalogRow,
    pub(crate) canonical: String,
    fence: Fence,
}

/// Both positive and negative identity decisions bind the entire selection.
pub(crate) struct Fence {
    original: String,
    context: Vec<CatalogRow>,
    headers: Vec<Headers>,
    dependencies: Vec<crate::events::store::native_query::SourceWitness>,
}

pub(crate) enum Resolution {
    Known(Identity),
    Unknown(Fence),
}

fn native_seed(
    store: &crate::events::store::native_query::NativeStore,
    row: &CatalogRow,
    dependencies: &[crate::events::store::native_query::SourceWitness],
) -> io::Result<Option<String>> {
    if row.sources.is_empty() {
        return Ok(None);
    }
    if row.sources.len() == 1 {
        return Ok(Some(row.sources[0].native_id.clone()));
    }
    if row.harness != "claude" || row.kind != "session" {
        return Ok(None);
    }
    let mut chosen: Option<(i64, bool, String)> = None;
    for source in &row.sources {
        let witness = dependencies
            .iter()
            .find(|witness| witness.source == *source)
            .ok_or_else(|| io::Error::other("native identity source witness missing"))?;
        let Some(header) = store.source_header(witness)? else {
            return Ok(None);
        };
        if !header.primitives_ready {
            return Ok(None);
        }
        let empty = store.source_continued_empty(witness)?;
        let (first, empty, id) =
            crate::model::identity_stage::source_order(header.first, empty, &source.native_id);
        if chosen
            .as_ref()
            .is_none_or(|old| (first, empty, id) < (old.0, old.1, old.2.as_str()))
        {
            chosen = Some((first, empty, id.to_owned()));
        }
    }
    Ok(chosen.map(|(_, _, id)| id))
}

impl Fence {
    fn metadata_unchanged(&self, db: &Connection) -> io::Result<bool> {
        let actual = query_selection(db, &self.original)?;
        if actual.incomplete
            || serde_json::to_value(&actual.rows).map_err(failure)?
                != serde_json::to_value(&self.context).map_err(failure)?
        {
            return Ok(false);
        }
        for (row, expected) in actual.rows.iter().zip(&self.headers) {
            if headers(db, row)? != *expected {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(crate) fn unchanged(&self, db: &Connection, options: &Options) -> io::Result<bool> {
        if !db.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='catalog_completeness' AND value='complete')", [], |r|r.get::<_,bool>(0)).map_err(failure)? {
            return Ok(false);
        }
        if !self.metadata_unchanged(db)? {
            return Ok(false);
        }
        source_refs(options, &self.context)?;
        let sources: Vec<_> = self
            .context
            .iter()
            .flat_map(|row| row.sources.iter().cloned())
            .collect();
        // Recheck omitted owners too: their native files can enter the window
        // during provider I/O before the next Catalog publication.
        if !sources
            .iter()
            .all(|source| crate::catalog::source_state(source) == "cached")
            || !EventCache::selected_roles_current(db, &sources).unwrap_or(false)
        {
            return Ok(false);
        }
        let store = crate::events::store::native_query::NativeStore::open_read(options)?;
        for witness in &self.dependencies {
            if !store.source_witness_current(witness)? {
                return Ok(false);
            }
        }
        // Outside a held snapshot this also catches new owners joining during
        // the filesystem/custody checks above. Consumers repeat after release.
        self.metadata_unchanged(db)
    }
}

impl Identity {
    pub(crate) fn unchanged(&self, db: &Connection, options: &Options) -> io::Result<bool> {
        self.fence.unchanged(db, options)
    }
}

pub(crate) fn resolve(
    db: &Connection,
    options: &Options,
    key: &str,
    requested: &str,
    now: i64,
) -> io::Result<Option<Resolution>> {
    if !options.scan_window || options.all || !db.query_row("SELECT EXISTS(SELECT 1 FROM meta WHERE key='catalog_completeness' AND value='complete')", [], |r| r.get::<_,bool>(0)).map_err(failure)? {
        return Ok(None);
    }
    let selected = query_selection(db, key)?;
    if selected.incomplete
        || selected
            .rows
            .iter()
            .any(|row| row.lifecycle != crate::model::summary::CatalogLifecycle::Current)
    {
        return Ok(None);
    }
    source_refs(options, &selected.rows)?;
    let cutoff = crate::model::scan_cutoff(options, now);
    let eligible: Vec<_> = selected
        .rows
        .iter()
        .filter(|row| {
            row.sources
                .iter()
                .any(|source| crate::model::catalog_source_in_window(source, cutoff))
        })
        .collect();
    if selected.rows.len() == 1 || eligible.len() == selected.rows.len() {
        return Ok(None);
    }
    if eligible.iter().any(|row| {
        row.sources
            .iter()
            .any(|source| !crate::model::catalog_source_in_window(source, cutoff))
    }) {
        return Ok(None);
    }
    let omitted: Vec<_> = selected
        .rows
        .iter()
        .filter(|row| !eligible.iter().any(|kept| kept.key == row.key))
        .collect();
    for owner in &eligible {
        // Claude copied-usage evidence relates physical owners even without a
        // handoff or parent. Omitting one changes exclusive tokens and evidence,
        // so those owners require the native usage reducer as well.
        if owner.harness == "claude"
            && omitted.iter().any(|row| row.harness == "claude")
            && owner.query.as_ref().is_some_and(|query| {
                !query["rows"][&owner.key]["summary"]["claude_usage"].is_null()
            })
        {
            return Ok(None);
        }
        if owner.parent.is_some()
            || selected
                .rows
                .iter()
                .any(|row| row.parent.as_deref() == Some(&owner.key))
        {
            return Ok(None);
        }
        // Every omitted row must be an independent namespace dependency, never
        // a parent, child or handoff endpoint whose omission changes native slots.
        for peer in selected.rows.iter().filter(|row| row.key != owner.key) {
            let connected: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM session_trace_edges INDEXED BY session_trace_from WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?2 AND session_key=?1 UNION ALL SELECT 1 FROM session_trace_edges INDEXED BY session_trace_to WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?2 AND session_key=?1)", params![owner.key,peer.key], |r| r.get(0)).map_err(failure)?;
            if connected {
                return Ok(None);
            }
        }
    }
    let sources: Vec<_> = selected
        .rows
        .iter()
        .flat_map(|row| row.sources.iter().cloned())
        .collect();
    if sources
        .iter()
        .any(|source| crate::catalog::source_state(source) != "cached")
        || !EventCache::selected_roles_current(db, &sources).unwrap_or(false)
    {
        return Ok(None);
    }
    let store = crate::events::store::native_query::NativeStore::open_read(options)?;
    let dependencies = sources
        .iter()
        .map(|source| store.source_witness(source))
        .collect::<io::Result<Vec<_>>>()?;
    // Capture headers before consulting separate native metadata snapshots.
    // A raced lookup can produce a negative decision just as easily as a
    // positive one, so both carry the same final publication/source fence.
    let witnesses = selected
        .rows
        .iter()
        .map(|row| headers(db, row))
        .collect::<io::Result<Vec<_>>>()?;
    let mut names = std::collections::BTreeMap::<&str, std::collections::BTreeSet<&str>>::new();
    for row in &eligible {
        for id in &row.native_ids {
            names.entry(id.as_str()).or_default().insert(&row.harness);
        }
    }
    let mut allocated = std::collections::BTreeSet::new();
    let mut owner = None;
    for row in &eligible {
        let Some(native) = native_seed(&store, row, &dependencies)? else {
            return Ok(None);
        };
        if !row.native_ids.contains(&native) {
            return Ok(None);
        }
        let collision = names
            .get(native.as_str())
            .is_some_and(|harnesses| harnesses.iter().any(|harness| *harness != row.harness));
        let mut spelling =
            crate::model::identity_stage::KeyCursor::new(&native, &row.harness, collision);
        // This independent-owner admission needs no invented allocation order:
        // qualify against all native literal occupants, then require distinct
        // final candidates. A dependency on another allocated spelling stays
        // with the native component adapter that preserves allocation order.
        let canonical = loop {
            if let Some(key) = spelling.step(|candidate| names.contains_key(candidate)) {
                break key;
            }
        };
        if !allocated.insert(canonical.clone()) {
            return Ok(None);
        }
        if canonical == requested {
            if owner.is_some() {
                return Ok(None);
            }
            owner = Some(((*row).clone(), canonical));
        }
    }
    after_identity_snapshot(0);
    let fence = Fence {
        original: key.to_owned(),
        context: selected.rows,
        headers: witnesses,
        dependencies,
    };
    let Some((owner, canonical)) = owner else {
        return Ok(Some(Resolution::Unknown(fence)));
    };
    Ok(Some(Resolution::Known(Identity {
        owner,
        canonical,
        fence,
    })))
}
