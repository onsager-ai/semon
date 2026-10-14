//! A physical window's independent native collision owners. This admission
//! preserves the selected owner's complete native recipes; source or actual
//! relationship omissions require the paged native producer instead.
use super::*;
use rusqlite::params;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Headers {
    private: Option<String>,
    trace: Option<String>,
    slots: Option<String>,
    history: Option<String>,
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
    })
}

pub(crate) struct Identity {
    pub(crate) owner: CatalogRow,
    pub(crate) canonical: String,
    original: String,
    context: Vec<CatalogRow>,
    headers: Vec<Headers>,
}

impl Identity {
    pub(crate) fn unchanged(&self, db: &Connection, options: &Options) -> io::Result<bool> {
        let actual = query_selection(db, &self.original)?;
        if actual.incomplete
            || serde_json::to_value(&actual.rows).map_err(failure)?
                != serde_json::to_value(&self.context).map_err(failure)?
        {
            return Ok(false);
        }
        source_refs(options, &actual.rows)?;
        for (row, expected) in actual.rows.iter().zip(&self.headers) {
            if headers(db, row)? != *expected {
                return Ok(false);
            }
        }
        let sources: Vec<_> = actual
            .rows
            .iter()
            .flat_map(|row| row.sources.iter().cloned())
            .collect();
        // Recheck omitted owners too: their native files can enter the window
        // during provider I/O before the next Catalog publication.
        Ok(sources
            .iter()
            .all(|source| crate::catalog::source_state(source) == "cached")
            && EventCache::selected_roles_current(db, &sources).unwrap_or(false))
    }
}

pub(crate) fn resolve(
    db: &Connection,
    options: &Options,
    key: &str,
    now: i64,
) -> io::Result<Option<Identity>> {
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
    if eligible.len() != 1 || selected.rows.len() == 1 {
        return Ok(None);
    }
    let owner = eligible[0];
    if owner
        .sources
        .iter()
        .any(|source| !crate::model::catalog_source_in_window(source, cutoff))
        || owner.parent.is_some()
        || selected
            .rows
            .iter()
            .any(|row| row.parent.as_deref() == Some(&owner.key))
    {
        return Ok(None);
    }
    // Every omitted row must be an independent namespace dependency, never
    // a parent, child or handoff endpoint whose omission changes native slots.
    for omitted in selected.rows.iter().filter(|row| row.key != owner.key) {
        let connected: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM session_trace_edges INDEXED BY session_trace_from WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.from') END=?2 AND session_key=?1 UNION ALL SELECT 1 FROM session_trace_edges INDEXED BY session_trace_to WHERE CASE WHEN json_valid(metadata) THEN json_extract(metadata,'$.to') END=?2 AND session_key=?1)", params![owner.key,omitted.key], |r| r.get(0)).map_err(failure)?;
        if connected {
            return Ok(None);
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
    let native = if owner.native_ids.contains(&owner.key) {
        owner.key.as_str()
    } else {
        let Some(native) = owner
            .key
            .trim_start_matches(':')
            .strip_prefix(&format!("{}:", owner.harness))
        else {
            return Ok(None);
        };
        if !owner.native_ids.iter().any(|id| id == native) {
            return Ok(None);
        }
        native
    };
    // With exactly one complete surviving native owner, all its native IDs
    // belong to that harness. The shared native key kernel therefore restores
    // the unqualified spelling without probing omitted literal occupants.
    let mut spelling = crate::model::identity_stage::KeyCursor::new(native, &owner.harness, false);
    let canonical = spelling
        .step(|_| unreachable!("unambiguous native owner"))
        .expect("unambiguous native key");
    let owner = owner.clone();
    let witnesses = selected
        .rows
        .iter()
        .map(|row| headers(db, row))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(Some(Identity {
        owner,
        canonical,
        original: key.to_owned(),
        context: selected.rows,
        headers: witnesses,
    }))
}
