//! SQLite adapter of the session storage port. One database is placed per
//! namespace. Writers keep main's single-connection topology: the index writer
//! binds the namespace through the `EventCache` connection it already owns,
//! and readers qualify the stamp and binding inside their read-only snapshot.

use std::{collections::BTreeMap, path::Path, time::Duration};

use rusqlite::{Connection, OpenFlags};

use super::{
    CatalogPublish, CatalogRead, IndexWrite, Namespace, OnSearch, ProjectionTag, SessionStore,
    StorageError, TagCheck, ensure,
};
use crate::{
    ViewerReply,
    catalog::Request,
    events::EventCache,
    model::summary::{CATALOG_VERSION, CatalogLifecycle, CatalogRow},
};

/// The one version stamp of a database this binary writes and reads. It
/// replaces nothing older: a database without it is refused by readers and
/// rebuilt from Raw by the producer's obsolete-schema path.
pub(crate) fn stamp() -> String {
    format!(
        "semon-session-store/1 backend=sqlite schema={} parser={} projector={}",
        crate::events::store::SCHEMA_VERSION,
        crate::events::CACHE_VERSION,
        CATALOG_VERSION
    )
}

/// The database's stamp and namespace binding, read in the caller's snapshot.
pub(crate) fn binding(
    connection: &Connection,
) -> rusqlite::Result<(Option<String>, Option<String>)> {
    connection
        .prepare_cached(
            "SELECT (SELECT value FROM meta WHERE key='storage_stamp'),(SELECT value FROM meta WHERE key='storage_namespace')",
        )?
        .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
}

fn backend(error: impl std::fmt::Display) -> StorageError {
    StorageError::Backend(error.to_string())
}

/// Qualify a reader's snapshot: this binary's stamp, and a database that is
/// unbound or bound to the requested namespace.
pub(crate) fn qualify(connection: &Connection, namespace: &Namespace) -> Result<(), StorageError> {
    let (found, bound) = binding(connection).map_err(backend)?;
    if found.as_deref() != Some(stamp().as_str()) {
        return Err(StorageError::Unsupported(
            "unsupported session store: version stamp".into(),
        ));
    }
    match bound {
        Some(bound) if bound != namespace.binding() => Err(StorageError::WrongNamespace {
            bound,
            requested: namespace.as_str().to_owned(),
        }),
        _ => Ok(()),
    }
}

/// Check the tag of every Current unit about to be served against the
/// committed ledger in the same snapshot. A source the ledger no longer holds
/// is left to the native source-state check; retained History is custody
/// state, not a projection of the current ledger.
pub(crate) fn projection_mismatch(
    connection: &Connection,
    rows: &[CatalogRow],
) -> rusqlite::Result<Option<&'static str>> {
    for row in rows
        .iter()
        .filter(|row| row.lifecycle == CatalogLifecycle::Current)
    {
        for source in &row.sources {
            let path = source.path.to_string_lossy();
            let Some((ledger, epoch)) = crate::events::store::committed_ledger(connection, &path)?
            else {
                continue;
            };
            let published = crate::events::store::published_epoch(connection, &row.key, &path)?;
            let committed = ProjectionTag::of_ledger(&ledger, epoch, CATALOG_VERSION);
            if let TagCheck::Mismatch(reason) =
                ProjectionTag::of(source, published.unwrap_or(epoch), CATALOG_VERSION)
                    .check(&committed)
            {
                return Ok(Some(reason));
            }
        }
    }
    Ok(None)
}

/// Bind a custody writer through the connection that performs its mutation.
/// Call within its write transaction, before any custody changes.
pub(crate) fn bind_connection(
    connection: &Connection,
    namespace: &Namespace,
) -> Result<(), StorageError> {
    qualify(connection, namespace)?;
    connection
        .execute(
            "INSERT OR IGNORE INTO meta(key,value) VALUES('storage_namespace',?1)",
            [namespace.binding()],
        )
        .map_err(backend)?;
    qualify(connection, namespace)
}

/// Bind the writer's database to `namespace` through its own connection, or
/// refuse a database bound to another namespace before any index write.
pub(crate) fn bind(cache: &mut EventCache, namespace: &Namespace) -> Result<(), StorageError> {
    let bound = cache
        .bind_storage_namespace(namespace.binding())
        .map_err(backend)?;
    if bound == namespace.binding() {
        Ok(())
    } else {
        Err(StorageError::WrongNamespace {
            bound,
            requested: namespace.as_str().to_owned(),
        })
    }
}

/// The namespace-bound Event index writer: main's single scoped writer
/// connection, opened for the producer and the index-writer capability alike.
pub(crate) fn open_index(
    options: &crate::Options,
    namespace: &Namespace,
) -> Result<EventCache, StorageError> {
    let mut cache = EventCache::open_scoped(&options.cache).map_err(StorageError::Backend)?;
    bind(&mut cache, namespace)?;
    Ok(cache)
}

fn read_only(path: &Path) -> Result<Connection, StorageError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(backend)?;
    connection
        .busy_timeout(Duration::from_millis(100))
        .map_err(backend)?;
    Ok(connection)
}

/// Namespaces placed on their own session index databases.
pub(crate) struct SqliteSessionStore {
    placements: BTreeMap<Namespace, crate::Options>,
}

impl SqliteSessionStore {
    pub(crate) fn new(placements: impl IntoIterator<Item = (Namespace, crate::Options)>) -> Self {
        Self {
            placements: placements.into_iter().collect(),
        }
    }

    fn placement(&self, namespace: &Namespace) -> Result<&crate::Options, StorageError> {
        self.placements
            .get(namespace)
            .ok_or_else(|| StorageError::Unplaced(namespace.as_str().to_owned()))
    }
}

impl SessionStore for SqliteSessionStore {
    fn reader(&self, namespace: &Namespace) -> Result<Box<dyn CatalogRead>, StorageError> {
        Ok(Box::new(Reader {
            namespace: namespace.clone(),
            options: self.placement(namespace)?.clone(),
        }))
    }

    fn index_writer(&self, namespace: &Namespace) -> Result<Box<dyn IndexWrite>, StorageError> {
        let options = self.placement(namespace)?.clone();
        let cache = open_index(&options, namespace)?;
        Ok(Box::new(IndexWriter {
            namespace: namespace.clone(),
            options,
            cache,
        }))
    }

    fn publisher(&self, namespace: &Namespace) -> Result<Box<dyn CatalogPublish>, StorageError> {
        let options = self.placement(namespace)?.clone();
        let path = EventCache::path(&options.cache);
        if !path.is_file() {
            return Err(StorageError::Unbound(namespace.as_str().to_owned()));
        }
        // A binding is written once and never changed, so the writer opened
        // after this check publishes into the database it qualified.
        let connection = read_only(&path)?;
        qualify(&connection, namespace)?;
        if binding(&connection).map_err(backend)?.1.is_none() {
            return Err(StorageError::Unbound(namespace.as_str().to_owned()));
        }
        drop(connection);
        let cache = EventCache::open_scoped(&options.cache).map_err(StorageError::Backend)?;
        Ok(Box::new(Publisher {
            namespace: namespace.clone(),
            cache,
        }))
    }
}

struct Reader {
    namespace: Namespace,
    options: crate::Options,
}

impl CatalogRead for Reader {
    fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    fn page(
        &self,
        namespace: &Namespace,
        request: &Request,
        on_search: OnSearch<'_>,
    ) -> Result<ViewerReply, StorageError> {
        ensure(&self.namespace, namespace)?;
        crate::catalog::read_page(&self.options, namespace, request, on_search).map_err(|error| {
            match error.downcast::<StorageError>() {
                Ok(error) => *error,
                Err(error) => backend(error),
            }
        })
    }
}

struct IndexWriter {
    namespace: Namespace,
    options: crate::Options,
    cache: EventCache,
}

impl IndexWrite for IndexWriter {
    fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    fn observe(&mut self, namespace: &Namespace) -> Result<(), StorageError> {
        ensure(&self.namespace, namespace)?;
        crate::collect_with_index(&self.options, &mut self.cache)
            .map(|_| ())
            .map_err(backend)
    }

    fn ledger_tag(
        &self,
        namespace: &Namespace,
        path: &str,
    ) -> Result<Option<ProjectionTag>, StorageError> {
        ensure(&self.namespace, namespace)?;
        let connection = read_only(&EventCache::path(&self.options.cache))?;
        Ok(crate::events::store::committed_ledger(&connection, path)
            .map_err(backend)?
            .map(|(ledger, epoch)| ProjectionTag::of_ledger(&ledger, epoch, CATALOG_VERSION)))
    }
}

struct Publisher {
    namespace: Namespace,
    cache: EventCache,
}

impl CatalogPublish for Publisher {
    fn namespace(&self) -> &Namespace {
        &self.namespace
    }

    fn generation(&self, namespace: &Namespace) -> Result<Option<String>, StorageError> {
        ensure(&self.namespace, namespace)?;
        Ok(self.cache.session_catalog_generation())
    }

    fn publish(
        &mut self,
        namespace: &Namespace,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
    ) -> Result<Option<String>, StorageError> {
        ensure(&self.namespace, namespace)?;
        self.cache
            .publish_catalog_projection_fenced(publication, expected_generation)
            .map_err(backend)
    }
}
