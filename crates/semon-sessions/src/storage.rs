//! Backend-neutral native session storage port (#477, slice 1; see
//! docs/design/session-storage-rebuild-plan.md).
//!
//! The session store is a projection cache over Raw. Every port operation
//! takes an explicit logical [`Namespace`]; a namespace is a source binding,
//! never a path, a row ID or a credential. Reading, Event index/ledger writing
//! and Catalog publication are distinct capability types, and a capability
//! bound to one namespace refuses another before any I/O, so a refusal has no
//! partial effect. Each database carries one version stamp, and each Catalog
//! unit carries a [`ProjectionTag`] that readers check against the committed
//! ledger before serving it.
#[cfg(feature = "postgres")]
pub(crate) mod postgres;
pub(crate) mod sqlite;
pub(crate) mod runtime;

#[cfg(test)]
mod conformance;

use std::fmt;

use crate::{
    ViewerReply,
    catalog::{CatalogSearchAdmission, Request},
    model::summary::CatalogSource,
};

/// The longest namespace accepted, the same bound as a Catalog source key.
const NAMESPACE_MAX: usize = 4096;

/// A logical source namespace. Machine identity, native session identity and
/// public addressing never depend on where its database is placed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Namespace(String);

impl Namespace {
    pub fn new(name: &str) -> Result<Self, StorageError> {
        if name.len() > NAMESPACE_MAX || name.contains('\0') {
            return Err(StorageError::InvalidNamespace);
        }
        Ok(Self(name.to_owned()))
    }

    /// The local source's namespace: its key is empty, bound as `local`.
    #[cfg(test)]
    pub(crate) fn local() -> Self {
        Self(String::new())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The name a database is bound to. The local source's empty key is an
    /// alias of `local`, so both spellings address one binding while replies
    /// and cursors keep the key the caller used.
    pub(crate) fn binding(&self) -> &str {
        if self.0.is_empty() { "local" } else { &self.0 }
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why a port operation was refused. Every variant is returned before the
/// operation changes durable state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageError {
    InvalidNamespace,
    /// The capability or database is bound to another namespace.
    WrongNamespace {
        bound: String,
        requested: String,
    },
    /// No database is placed for this namespace in this store.
    Unplaced(String),
    /// The database's version stamp is missing or not this binary's.
    Unsupported(String),
    /// A publication needs a database whose namespace an index writer bound.
    Unbound(String),
    Backend(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNamespace => formatter.write_str("invalid storage namespace"),
            Self::WrongNamespace { .. } => {
                formatter.write_str("session store is bound to another namespace")
            }
            Self::Unplaced(_) => formatter.write_str("no session store is placed for namespace"),
            Self::Unsupported(reason) | Self::Backend(reason) => formatter.write_str(reason),
            Self::Unbound(_) => formatter.write_str("session store namespace is not bound"),
        }
    }
}

impl std::error::Error for StorageError {}

/// Refuse an operation for a namespace other than the capability's own.
fn ensure(bound: &Namespace, requested: &Namespace) -> Result<(), StorageError> {
    if bound.binding() == requested.binding() {
        Ok(())
    } else {
        Err(StorageError::WrongNamespace {
            bound: bound.0.clone(),
            requested: requested.0.clone(),
        })
    }
}

/// The cheap tag of one projected source: the Raw identity, the consumed
/// prefix digest that main's ledger and byte proofs qualify against the native
/// bytes, the ledger epoch and the projector version that derived the unit.
/// The epoch advances whenever the ledger is replaced rather than appended,
/// so a same-identity rewrite that grows past the published boundary is told
/// apart from an append.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectionTag {
    pub dev: u64,
    pub ino: u64,
    pub offset: u64,
    pub prefix_sha256: [u8; 32],
    pub tail_sha256: [u8; 32],
    pub epoch: i64,
    pub projector: u32,
}

/// How a stored tag relates to the committed ledger of the same source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagCheck {
    /// The unit was projected from exactly the committed prefix.
    Current,
    /// The ledger appended to the same Raw identity; the published prefix
    /// remains a readable selected prefix while the producer republishes.
    Advanced,
    /// The ledger moved on at another Raw identity (for example a copied
    /// file). Whether the published prefix survives is a byte question: a
    /// selected read's byte proof qualifies it against the native bytes.
    Relocated,
    /// The unit no longer describes its Raw source and must not be served.
    Mismatch(&'static str),
}

impl ProjectionTag {
    pub(crate) fn of(source: &CatalogSource, epoch: i64, projector: u32) -> Self {
        Self {
            dev: source.dev,
            ino: source.ino,
            offset: source.offset,
            prefix_sha256: source.prefix_sha256,
            tail_sha256: source.tail_sha256,
            epoch,
            projector,
        }
    }

    pub(crate) fn of_ledger(ledger: &crate::events::Ledger, epoch: i64, projector: u32) -> Self {
        Self {
            dev: ledger.stat.dev,
            ino: ledger.stat.ino,
            offset: ledger.offset,
            prefix_sha256: ledger.prefix,
            tail_sha256: ledger.tail,
            epoch,
            projector,
        }
    }

    /// Compare this stored tag with the tag of the committed ledger. The
    /// consumed-prefix digest decides at the published boundary; a content
    /// identical copy at another Raw identity is still the same prefix. Past
    /// the boundary at the same Raw identity only an unchanged ledger epoch is
    /// an append: a replaced ledger no longer extends the published prefix.
    pub(crate) fn check(&self, committed: &Self) -> TagCheck {
        let digests = (self.prefix_sha256, self.tail_sha256);
        if self.projector != committed.projector {
            TagCheck::Mismatch("projector_version")
        } else if committed.offset < self.offset {
            TagCheck::Mismatch("consumed_prefix")
        } else if committed.offset == self.offset {
            if digests == (committed.prefix_sha256, committed.tail_sha256) {
                TagCheck::Current
            } else {
                TagCheck::Mismatch("consumed_prefix")
            }
        } else if (self.dev, self.ino) == (committed.dev, committed.ino) {
            if self.epoch == committed.epoch {
                TagCheck::Advanced
            } else {
                TagCheck::Mismatch("ledger_epoch")
            }
        } else {
            TagCheck::Relocated
        }
    }
}

/// The search callback a Catalog page may admit after its snapshot closes.
pub(crate) type OnSearch<'a> = Option<&'a mut dyn FnMut(CatalogSearchAdmission) -> bool>;

/// A backend holding one database per placed namespace.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "slice 2 routes the producer through the write capabilities"
    )
)]
pub(crate) trait SessionStore {
    /// A read capability. Opening it performs no I/O; each read qualifies the
    /// version stamp and namespace binding in its own snapshot.
    fn reader(&self, namespace: &Namespace) -> Result<Box<dyn CatalogRead>, StorageError>;
    /// The Event index/ledger writer. It binds an unbound database to the
    /// namespace through its own connection.
    fn index_writer(&self, namespace: &Namespace) -> Result<Box<dyn IndexWrite>, StorageError>;
    /// The Catalog publisher. It never binds: the database must already be
    /// bound to this namespace by an index writer.
    fn publisher(&self, namespace: &Namespace) -> Result<Box<dyn CatalogPublish>, StorageError>;
}

/// Catalog pages, metadata search, retained History and selected identities.
/// Search is a page with `q`, History a page in the retained-History scope and
/// a selected identity a page with `sid`; all read one snapshot.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "slice 2 routes the producer through the write capabilities"
    )
)]
pub(crate) trait CatalogRead {
    fn namespace(&self) -> &Namespace;
    fn page(
        &self,
        namespace: &Namespace,
        request: &Request,
        on_search: OnSearch<'_>,
    ) -> Result<ViewerReply, StorageError>;
}

/// Event index and consumed-prefix ledger writes.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "slice 2 routes the producer through the write capabilities"
    )
)]
pub(crate) trait IndexWrite {
    fn namespace(&self) -> &Namespace;
    /// Observe the namespace's native sources into Event rows and the ledger.
    fn observe(&mut self, namespace: &Namespace) -> Result<(), StorageError>;
    /// The committed tag of one source, checked by readers.
    fn ledger_tag(
        &self,
        namespace: &Namespace,
        path: &str,
    ) -> Result<Option<ProjectionTag>, StorageError>;
}

/// Source-validated Catalog publication.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "slice 2 routes the producer through the write capabilities"
    )
)]
pub(crate) trait CatalogPublish {
    fn namespace(&self) -> &Namespace;
    fn generation(&self, namespace: &Namespace) -> Result<Option<String>, StorageError>;
    fn publish(
        &mut self,
        namespace: &Namespace,
        publication: &crate::slot_projection::Publication<'_>,
        expected_generation: Option<&str>,
    ) -> Result<Option<String>, StorageError>;
}

/// The configured local placement: the Options' own session index database
/// under the caller's source key.
pub(crate) fn local(
    options: &crate::Options,
    source_key: &str,
) -> Result<(Namespace, sqlite::SqliteSessionStore), StorageError> {
    let namespace = Namespace::new(source_key)?;
    let store = sqlite::SqliteSessionStore::new([(namespace.clone(), options.clone())]);
    Ok((namespace, store))
}

/// One Catalog read through the port for the viewer, union, Query and
/// bookmark readers. Refusals keep the existing retryable unavailable reply.
pub(crate) fn catalog_page(
    options: &crate::Options,
    source_key: &str,
    request: &Request,
    on_search: OnSearch<'_>,
) -> ViewerReply {
    let read = local(options, source_key).and_then(|(namespace, store)| {
        store
            .reader(&namespace)?
            .page(&namespace, request, on_search)
    });
    read.unwrap_or_else(|_| {
        crate::catalog::error(
            503,
            "catalog_unavailable",
            "The session catalog is unavailable. Retry after the background observation completes.",
            true,
        )
    })
}
