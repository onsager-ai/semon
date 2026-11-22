//! Resolve relationships within the selected graph using indexed fact probes.
use super::{ReadError, index};
use rusqlite::{CachedStatement, Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const OFFER: &str = include_str!("../events/store/warehouse_parent_offer.sql");
const CHILDREN: &str = include_str!("../events/store/warehouse_parent_children.sql");

#[derive(Clone)]
pub(super) struct Parent {
    pub(super) source: i64,
    pub(super) call: Option<String>,
    pub(super) method: String,
}

pub(super) struct Parents<'a> {
    offer: CachedStatement<'a>,
    children: CachedStatement<'a>,
    component: CachedStatement<'a>,
    owner: CachedStatement<'a>,
    chosen: BTreeMap<i64, Option<Parent>>,
    qualified: BTreeMap<i64, Option<Parent>>,
    owner_parents: BTreeMap<i64, BTreeSet<i64>>,
}

impl<'a> Parents<'a> {
    pub(super) fn new(db: &'a Connection) -> Result<Self, ReadError> {
        Ok(Self {
            offer: db.prepare_cached(OFFER).map_err(index)?,
            children: db.prepare_cached(CHILDREN).map_err(index)?,
            component: db
                .prepare_cached(
                    "SELECT m.source FROM lineage_member r
                 CROSS JOIN lineage_member m ON m.component=r.component WHERE r.source=?1",
                )
                .map_err(index)?,
            owner: db
                .prepare_cached("SELECT owner FROM bridge_session_owner WHERE source=?1")
                .map_err(index)?,
            chosen: BTreeMap::new(),
            qualified: BTreeMap::new(),
            owner_parents: BTreeMap::new(),
        })
    }

    fn offer(&mut self, source: i64, fallback: bool) -> Result<Option<Parent>, ReadError> {
        self.offer
            .query_row(params![source, fallback], |row| {
                Ok(Parent {
                    source: row.get(0)?,
                    call: row.get(1)?,
                    method: row.get(2)?,
                })
            })
            .optional()
            .map_err(index)
    }

    fn chosen(&mut self, source: i64) -> Result<Option<Parent>, ReadError> {
        if let Some(parent) = self.chosen.get(&source) {
            return Ok(parent.clone());
        }
        let parent = self.offer(source, false)?;
        self.chosen.insert(source, parent.clone());
        Ok(parent)
    }

    pub(super) fn parent(&mut self, source: i64) -> Result<Option<Parent>, ReadError> {
        if let Some(parent) = self.qualified.get(&source) {
            return Ok(parent.clone());
        }
        let mut parent = self.chosen(source)?;
        if let Some(first) = parent.as_ref().filter(|p| p.method != "native") {
            let mut node = first.source;
            let mut visited = BTreeSet::new();
            while visited.insert(node) {
                if node == source {
                    // Only an observed edge belonging to the cycle is revoked.
                    parent = self.offer(source, true)?;
                    break;
                }
                let Some(next) = self.chosen(node)? else {
                    break;
                };
                node = next.source;
            }
        }
        self.qualified.insert(source, parent.clone());
        Ok(parent)
    }

    pub(super) fn owner(&mut self, source: i64) -> Result<Option<i64>, ReadError> {
        self.owner
            .query_row([source], |row| row.get(0))
            .optional()
            .map_err(index)
    }

    pub(super) fn owner_parents(&mut self, owner: i64) -> Result<BTreeSet<i64>, ReadError> {
        if let Some(parents) = self.owner_parents.get(&owner) {
            return Ok(parents.clone());
        }
        let members = self
            .component
            .query_map([owner], |row| row.get::<_, i64>(0))
            .map_err(index)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(index)?;
        let mut parents = BTreeSet::new();
        for member in members {
            if let Some(parent) = self.parent(member)?
                && let Some(parent) = self.owner(parent.source)?
                && parent != owner
            {
                parents.insert(parent);
            }
        }
        self.owner_parents.insert(owner, parents.clone());
        Ok(parents)
    }

    pub(super) fn roots(
        &mut self,
        parent: i64,
    ) -> Result<(BTreeSet<i64>, BTreeSet<i64>), ReadError> {
        let mut ancestors = BTreeSet::new();
        let mut roots = BTreeSet::new();
        let mut frontier = vec![parent];
        while let Some(owner) = frontier.pop() {
            if !ancestors.insert(owner) {
                continue;
            }
            let parents = self.owner_parents(owner)?;
            if parents.is_empty() {
                roots.insert(owner);
            }
            frontier.extend(parents);
        }
        Ok((ancestors, roots))
    }

    pub(super) fn owner_children(&mut self, owner: i64) -> Result<BTreeSet<i64>, ReadError> {
        let candidates = self
            .children
            .query_map([owner], |row| row.get::<_, i64>(0))
            .map_err(index)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(index)?;
        let mut children = BTreeSet::new();
        for candidate in candidates {
            if let (Some(edge), Some(child)) = (self.parent(candidate)?, self.owner(candidate)?)
                && child != owner
                && self.owner(edge.source)? == Some(owner)
            {
                children.insert(child);
            }
        }
        Ok(children)
    }

    pub(super) fn ancestors(&mut self, owner: i64) -> Result<(Vec<i64>, bool), ReadError> {
        let mut ancestors = Vec::new();
        let mut seen = BTreeSet::from([owner]);
        let mut frontier = VecDeque::from([owner]);
        let mut incomplete = false;
        'ancestors: while let Some(current) = frontier.pop_front() {
            let parents = self.owner_parents(current)?;
            incomplete |= parents.len() > 1;
            for parent in parents {
                if !seen.insert(parent) {
                    incomplete = true;
                    continue;
                }
                if ancestors.len() == 32 {
                    incomplete = true;
                    break 'ancestors;
                }
                ancestors.push(parent);
                frontier.push_back(parent);
            }
        }
        ancestors.reverse();
        Ok((ancestors, incomplete))
    }

    pub(super) fn relatives(
        &mut self,
        owner: i64,
    ) -> Result<(Vec<i64>, Vec<i64>, bool), ReadError> {
        let (ancestors, mut incomplete) = self.ancestors(owner)?;
        let seen: BTreeSet<_> = std::iter::once(owner)
            .chain(ancestors.iter().copied())
            .collect();
        let mut descendants = BTreeSet::new();
        let mut frontier = VecDeque::from([owner]);
        'descendants: while let Some(current) = frontier.pop_front() {
            for child in self.owner_children(current)? {
                if seen.contains(&child) {
                    incomplete = true;
                    continue;
                }
                if descendants.contains(&child) {
                    continue;
                }
                if descendants.len() == 99 {
                    incomplete = true;
                    break 'descendants;
                }
                descendants.insert(child);
                frontier.push_back(child);
            }
        }
        Ok((ancestors, descendants.into_iter().collect(), incomplete))
    }
}
