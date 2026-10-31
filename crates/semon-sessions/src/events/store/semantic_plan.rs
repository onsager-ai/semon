//! Shared native owner planning. Ports expose physical metadata lookups only;
//! admission, epoch fences and durable effects remain with the storage owner.
use super::*;

pub(crate) trait OwnerClosurePort {
    fn input(&self, path: &str) -> rusqlite::Result<Option<(String, bool)>>;
    fn links(&self, path: &str) -> rusqlite::Result<(String, Option<String>)>;
    fn members(&self, kind: &str, id: &str) -> rusqlite::Result<Vec<String>>;
}

pub(crate) fn owner_closure_with(
    port: &impl OwnerClosurePort,
    seed: &str,
) -> rusqlite::Result<Vec<String>> {
    if !port.input(seed)?.is_some_and(|(harness, top)| harness == "claude" && top) {
        return Ok(vec![seed.to_owned()]);
    }
    let mut paths = BTreeSet::new();
    let mut pending = vec![seed.to_owned()];
    let mut ids = BTreeSet::new();
    let mut bridges = BTreeSet::new();
    let mut used = 0usize;
    while let Some(path) = pending.pop() {
        if !paths.insert(path.clone()) {
            continue;
        }
        if paths.len() > 64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
        let (id, links) = port.links(&path)?;
        let links: super::super::super::Links = bounded(links, &mut used)?;
        let mut selectors = Vec::new();
        for id in std::iter::once(id).chain(links.session_ids).chain(links.continued_in) {
            if ids.insert(id.clone()) {
                selectors.push(("lineage:claude", id));
            }
        }
        for id in links.bridges {
            if bridges.insert(id.clone()) {
                selectors.push(("bridge:claude", id));
            }
        }
        for (kind, id) in selectors {
            for found in port.members(kind, &id)? {
                if !paths.contains(&found) {
                    pending.push(found);
                }
            }
        }
    }
    Ok(paths.into_iter().collect())
}

pub(crate) trait PublicationPlanPort {
    fn pending(&self) -> rusqlite::Result<Vec<String>>;
    /// Both original and replacement sources for this canonical owner key.
    fn sources(&self, key: &str) -> rusqlite::Result<Vec<String>>;
    /// Both original and replacement physical owners; semantic peers excluded.
    fn owners(&self, path: &str) -> rusqlite::Result<Vec<String>>;
}

/// Pack complete physical groups into one bounded publication. A canonical key
/// joins its original and replacement memberships, including disjoint sources.
pub(crate) fn publication_groups_with(
    port: &impl PublicationPlanPort,
) -> rusqlite::Result<Vec<Vec<String>>> {
    let mut selected = BTreeSet::new();
    let mut owners = BTreeSet::new();
    let mut groups = Vec::new();
    for seed in port.pending()? {
        if owners.contains(&seed) {
            continue;
        }
        let mut pending = BTreeSet::from([seed]);
        let mut group_owners = BTreeSet::new();
        let mut paths = BTreeSet::new();
        while let Some(key) = pending.pop_first() {
            if !group_owners.insert(key.clone()) {
                continue;
            }
            if group_owners.len() > 64 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            for path in port.sources(&key)? {
                if paths.insert(path.clone()) {
                    if paths.len() > 64 {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    pending.extend(port.owners(&path)?);
                }
            }
        }
        if selected.len() + paths.iter().filter(|path| !selected.contains(*path)).count() > 64
            || owners.len() + group_owners.len() > 64
        {
            break;
        }
        owners.extend(group_owners);
        selected.extend(paths.iter().cloned());
        groups.push(paths.into_iter().collect());
    }
    Ok(groups)
}

pub(crate) trait ContextPort {
    fn owners(&self, path: &str) -> rusqlite::Result<Vec<String>>;
    fn children(&self, key: &str, used: &mut usize) -> rusqlite::Result<Vec<String>>;
    fn cost(&self, key: &str) -> rusqlite::Result<Option<f64>>;
    fn parent(&self, key: &str) -> rusqlite::Result<Option<String>>;
    fn endpoint(&self, key: &str, used: &mut usize) -> rusqlite::Result<Option<Endpoint>>;
}

pub(crate) fn context_with(
    port: &impl ContextPort,
    paths: &[String],
    phase: Phase,
    epoch: i64,
) -> rusqlite::Result<Context> {
    let mut owners = BTreeSet::new();
    for path in paths {
        owners.extend(port.owners(path)?);
        if owners.len() > 64 {
            return Err(rusqlite::Error::InvalidQuery);
        }
    }
    let mut selected = owners.clone();
    let mut out = Context {
        epoch: Some(epoch),
        replay_owners: if phase == Phase::Publication {
            owners.iter().cloned().collect()
        } else {
            Vec::new()
        },
        ..Context::default()
    };
    let mut used = 0usize;
    for owner in &owners {
        if phase == Phase::Publication {
            let children = port.children(owner, &mut used)?;
            out.children.insert(owner.clone(), children);
            out.family_costs.insert(owner.clone(), port.cost(owner)?);
        }
        selected.extend(port.parent(owner)?);
    }
    for key in selected {
        if let Some(endpoint) = port.endpoint(&key, &mut used)? {
            out.endpoints.insert(key, endpoint);
        }
    }
    Ok(out)
}
