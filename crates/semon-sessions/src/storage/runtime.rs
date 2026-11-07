//! Source-owned runtime access. SQLite keeps its existing writer and snapshots;
//! PostgreSQL runs on a source-owned reactor, outside the embedding reactor.
//! Startup selection remains private until every runtime consumer uses this port.
use super::{Namespace, OnSearch, StorageError};
use crate::{Options, ViewerReply, catalog::Request, events::EventCache};
use std::{collections::BTreeSet, time::Instant};

/// Backend work is owned with its source's runtime handle and single writer.
#[derive(Default)]
pub(crate) struct Work {
    #[cfg(feature = "postgres")]
    pub(crate) postgres_discovery: Option<ColdCatalog>,
    #[cfg(feature = "postgres")]
    pub(crate) postgres_pending: Vec<crate::Input>,
    #[cfg(feature = "postgres")]
    pub(crate) postgres_draining: bool,
    pub(crate) events: Option<EventCache>,
    pub(crate) texts: crate::model::Texts,
    pub(crate) catalog_snapshot: Option<crate::native_evidence::Snapshot>,
    pub(crate) observed_catalog_generation: Option<String>,
    pub(crate) cold_catalog: Option<ColdCatalog>,
    pub(crate) pending_sources: bool,
    pub(crate) tree_repair_pending: bool,
    pub(crate) audit: Option<CatalogAudit>,
    pub(crate) audit_revision: u64,
    pub(crate) built_at: Option<Instant>,
}

#[derive(Default)]
pub(crate) struct CatalogAudit {
    pub(crate) revision: u64,
    pub(crate) cursor: crate::inputs::Discovery,
    pub(crate) discovered: bool,
    pub(crate) after: String,
    pub(crate) checked: bool,
    pub(crate) started: bool,
    pub(crate) snapshot: Option<crate::native_evidence::Snapshot>,
}

#[derive(Default)]
pub(crate) struct ColdCatalog {
    pub(crate) revision: u64,
    pub(crate) cursor: crate::inputs::Discovery,
    pub(crate) pids: BTreeSet<u32>,
    pub(crate) snapshot: Option<crate::native_evidence::Snapshot>,
    pub(crate) started: bool,
    pub(crate) newest: BTreeSet<(
        std::cmp::Reverse<Option<std::time::SystemTime>>,
        crate::Input,
    )>,
}
use std::io;

pub(crate) struct RuntimeStore {
    options: Options,
    namespace: Option<Namespace>,
    error: Option<StorageError>,
    /// The source-owned producer fence and backend work state.
    pub(crate) work: std::sync::Mutex<Work>,
    #[cfg(feature = "postgres")]
    postgres: Option<std::sync::Mutex<Postgres>>,
}

#[cfg(feature = "postgres")]
struct Postgres {
    runtime: tokio::runtime::Runtime,
    store: super::postgres::PgProjectionStore,
}

impl RuntimeStore {
    pub(crate) fn sqlite(options: &Options, namespace: Option<Namespace>) -> Self {
        Self {
            options: options.clone(),
            namespace,
            error: None,
            work: Default::default(),
            #[cfg(feature = "postgres")]
            postgres: None,
        }
    }

    pub(crate) fn for_source(options: &Options, key: &str) -> Self {
        match Namespace::new(key) {
            Ok(namespace) => Self::sqlite(options, Some(namespace)),
            Err(error) => {
                let mut store = Self::sqlite(options, None);
                store.error = Some(error);
                store
            }
        }
    }

    /// Internal qualification entry point, not startup backend configuration.
    /// Call off-reactor; the restricted role and current stamp are checked here.
    #[cfg(all(test, feature = "postgres"))]
    pub(crate) fn postgres(options: &Options, namespace: Namespace, dsn: &str) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let store = runtime.block_on(async {
            let (client, connection) = tokio_postgres::connect(dsn, tokio_postgres::NoTls)
                .await
                .map_err(io::Error::other)?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            let mut store = super::postgres::PgProjectionStore::new(client, namespace.clone())
                .await
                .map_err(io::Error::other)?;
            store
                .begin(&namespace)
                .await
                .map_err(io::Error::other)?
                .commit(&namespace)
                .await
                .map_err(io::Error::other)?;
            Ok::<_, io::Error>(store)
        })?;
        Ok(Self {
            options: options.clone(),
            namespace: Some(namespace),
            error: None,
            work: Default::default(),
            postgres: Some(std::sync::Mutex::new(Postgres { runtime, store })),
        })
    }

    pub(crate) fn namespace(&self) -> Option<&Namespace> {
        self.namespace.as_ref()
    }

    pub(crate) fn validate(&self) -> io::Result<()> {
        match &self.error {
            Some(error) => Err(io::Error::new(io::ErrorKind::InvalidInput, error.clone())),
            None => Ok(()),
        }
    }

    pub(crate) fn is_postgres(&self) -> bool {
        #[cfg(feature = "postgres")]
        if self.postgres.is_some() {
            return true;
        }
        false
    }

    /// SQLite's single writer is still opened once by its existing owner.
    pub(crate) fn open_index(&self) -> io::Result<EventCache> {
        self.validate()?;
        if self.is_postgres() {
            return Err(io::Error::other("PostgreSQL has no SQLite index writer"));
        }
        match &self.namespace {
            Some(namespace) => {
                super::sqlite::open_index(&self.options, namespace).map_err(io::Error::other)
            }
            None => EventCache::open_scoped(&self.options.cache).map_err(io::Error::other),
        }
    }

    pub(crate) fn open_native_index(&self) -> io::Result<EventCache> {
        self.validate()?;
        if self.is_postgres() {
            return Err(io::Error::other("PostgreSQL has no SQLite index writer"));
        }
        let mut cache = EventCache::open(&self.options.cache).map_err(io::Error::other)?;
        if let Some(namespace) = &self.namespace
            && cache.persistent()
        {
            super::sqlite::bind(&mut cache, namespace).map_err(io::Error::other)?;
        }
        Ok(cache)
    }

    /// SQLite's selected snapshot remains owned by the renderer, including its
    /// lazy byte proofs. PostgreSQL selected windows use the typed port above.
    pub(crate) fn selected_connection(
        &self,
        key: &str,
    ) -> Result<rusqlite::Connection, StorageError> {
        self.validate()
            .map_err(|error| StorageError::Backend(error.to_string()))?;
        if self.is_postgres() {
            return Err(StorageError::Backend(
                "PostgreSQL has no SQLite selected snapshot".into(),
            ));
        }
        super::sqlite::selected_connection(&self.options, key)
    }

    pub(crate) fn generation(&self) -> io::Result<Option<String>> {
        self.validate()?;
        #[cfg(feature = "postgres")]
        if let Some(postgres) = &self.postgres {
            let mut postgres = postgres
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Postgres { runtime, store } = &mut *postgres;
            let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
            return runtime.block_on(async {
                let tx = store.begin(namespace).await.map_err(io::Error::other)?;
                let generation = tx
                    .catalog_generation(namespace)
                    .await
                    .map_err(io::Error::other)?;
                tx.commit(namespace).await.map_err(io::Error::other)?;
                Ok(generation)
            });
        }
        use rusqlite::OptionalExtension;
        let key = self.namespace.as_ref().map_or("local", Namespace::as_str);
        let connection = self.selected_connection(key).map_err(io::Error::other)?;
        crate::events::store::require_current(&connection).map_err(io::Error::other)?;
        if let Some(namespace) = &self.namespace {
            super::sqlite::qualify(&connection, namespace).map_err(io::Error::other)?;
        }
        connection
            .query_row(
                "SELECT value FROM meta WHERE key='catalog_generation'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)
    }

    pub(crate) fn selection(
        &self,
        key: &str,
        catalog_key: &str,
        scope: crate::CatalogReadScope,
        require_complete: bool,
    ) -> Result<
        Option<(crate::CatalogSessionIdentity, serde_json::Value)>,
        crate::CatalogIdentityError,
    > {
        let query = format!(
            "sid={}&scope={}",
            crate::viewer::percent_encode(catalog_key),
            crate::catalog_search::scope_key(scope)
        );
        let mut request =
            Request::parse(&query).map_err(|_| crate::CatalogIdentityError::InvalidArguments)?;
        request.require_complete = require_complete;
        crate::catalog::selection_from_reply(
            self.page(key, &request, None),
            key,
            catalog_key,
            scope,
            require_complete,
        )
    }

    pub(crate) fn resolve_native(
        &self,
        key: &str,
        harness: &str,
        native_id: &str,
        scope: crate::CatalogReadScope,
    ) -> Result<crate::CatalogNativeResolution, crate::CatalogIdentityError> {
        if !crate::native_resolution::valid_resolution_arguments(key, harness, native_id) {
            return Err(crate::CatalogIdentityError::InvalidArguments);
        }
        self.validate()
            .map_err(|_| crate::CatalogIdentityError::Unavailable)?;
        #[cfg(feature = "postgres")]
        if let Some(postgres) = &self.postgres {
            let namespace =
                Namespace::new(key).map_err(|_| crate::CatalogIdentityError::InvalidArguments)?;
            super::ensure(
                self.namespace.as_ref().expect("PostgreSQL namespace"),
                &namespace,
            )
            .map_err(|_| crate::CatalogIdentityError::Unavailable)?;
            let mut postgres = postgres
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Postgres { runtime, store } = &mut *postgres;
            return runtime
                .block_on(store.resolve_native(
                    &namespace,
                    &self.options,
                    harness,
                    native_id,
                    scope,
                ))
                .map_err(|_| crate::CatalogIdentityError::Unavailable);
        }
        crate::native_resolution::sqlite_resolve_native(
            &self.options,
            key,
            harness,
            native_id,
            scope,
        )
    }

    pub(crate) fn page_ready(&self) -> io::Result<bool> {
        self.validate()?;
        #[cfg(feature = "postgres")]
        if let Some(postgres) = &self.postgres {
            let mut postgres = postgres
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Postgres { runtime, store } = &mut *postgres;
            let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
            return runtime.block_on(async {
                let tx = store.begin(namespace).await.map_err(io::Error::other)?;
                let ready = tx
                    .catalog_generation(namespace)
                    .await
                    .map_err(io::Error::other)?
                    .is_some();
                tx.commit(namespace).await.map_err(io::Error::other)?;
                Ok(ready)
            });
        }
        crate::catalog::sqlite_page_ready(&self.options)
    }

    pub(crate) fn page(
        &self,
        key: &str,
        request: &Request,
        on_search: OnSearch<'_>,
    ) -> ViewerReply {
        if self.validate().is_err() {
            return unavailable();
        }
        #[cfg(feature = "postgres")]
        if let Some(postgres) = &self.postgres {
            let Ok(namespace) = Namespace::new(key) else {
                return unavailable();
            };
            if self
                .namespace
                .as_ref()
                .is_none_or(|bound| super::ensure(bound, &namespace).is_err())
            {
                return unavailable();
            }
            let mut postgres = postgres
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Postgres { runtime, store } = &mut *postgres;
            return runtime
                .block_on(async {
                    let tx = store.begin(&namespace).await?;
                    let reply =
                        crate::catalog::postgres_page(&tx, &namespace, &self.options, request)
                            .await
                            .map_err(|error| StorageError::Backend(error.to_string()))?;
                    tx.commit(&namespace).await?;
                    Ok::<_, StorageError>(reply)
                })
                .unwrap_or_else(|_| unavailable());
        }
        super::catalog_page(&self.options, key, request, on_search)
    }

    /// Only the injected PostgreSQL path handles this call. The SQLite caller
    /// continues using its existing snapshot/field/attachment implementation.
    #[cfg(feature = "postgres")]
    pub(crate) fn transcript(
        &self,
        key: &str,
        query: &str,
        provider: Option<&dyn crate::SessionSourceReader>,
    ) -> ViewerReply {
        let Some(postgres) = &self.postgres else {
            return unavailable();
        };
        let Ok(namespace) = Namespace::new(key) else {
            return unavailable();
        };
        if self
            .namespace
            .as_ref()
            .is_none_or(|bound| super::ensure(bound, &namespace).is_err())
        {
            return unavailable();
        }
        let mut postgres = postgres
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Postgres { runtime, store } = &mut *postgres;
        runtime
            .block_on(store.transcript_query(&namespace, &self.options, query, provider))
            .unwrap_or_else(|_| unavailable())
    }

    #[cfg(feature = "postgres")]
    pub(crate) fn discover_sources(
        &self,
        inputs: &[crate::Input],
        start: bool,
        complete: bool,
    ) -> io::Result<()> {
        let mut postgres = self
            .postgres
            .as_ref()
            .expect("PostgreSQL store")
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Postgres { runtime, store } = &mut *postgres;
        let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
        runtime
            .block_on(store.discover_sources(namespace, &self.options, inputs, start, complete))
            .map_err(io::Error::other)
    }

    #[cfg(feature = "postgres")]
    pub(crate) fn pending_sources(&self) -> io::Result<Vec<crate::Input>> {
        let mut postgres = self
            .postgres
            .as_ref()
            .expect("PostgreSQL store")
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Postgres { runtime, store } = &mut *postgres;
        let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
        runtime
            .block_on(store.pending_sources(namespace))
            .map_err(io::Error::other)
    }

    #[cfg(feature = "postgres")]
    pub(crate) fn publish_discovered(&self, inputs: &[crate::Input]) -> io::Result<()> {
        let mut postgres = self
            .postgres
            .as_ref()
            .expect("PostgreSQL store")
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Postgres { runtime, store } = &mut *postgres;
        let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
        runtime
            .block_on(store.publish_discovered(namespace, &self.options, inputs))
            .map_err(io::Error::other)
    }

    #[cfg(all(test, feature = "postgres"))]
    pub(crate) fn publish_sources(&self, inputs: &[crate::Input]) -> io::Result<()> {
        let namespace = self
            .namespace
            .as_ref()
            .expect("PostgreSQL namespace")
            .clone();
        self.publish_for(&namespace, inputs)
    }

    #[cfg(all(test, feature = "postgres"))]
    pub(crate) fn publish_for(
        &self,
        namespace: &Namespace,
        inputs: &[crate::Input],
    ) -> io::Result<()> {
        self.validate()?;
        super::ensure(
            self.namespace
                .as_ref()
                .ok_or_else(|| io::Error::other("missing namespace"))?,
            namespace,
        )
        .map_err(io::Error::other)?;
        let postgres = self
            .postgres
            .as_ref()
            .ok_or_else(|| io::Error::other("not PostgreSQL"))?;
        let mut postgres = postgres
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Postgres { runtime, store } = &mut *postgres;
        runtime
            .block_on(store.publish_sources(namespace, &self.options, inputs))
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("native Catalog generation changed"))?;
        Ok(())
    }

    pub(crate) fn catalog_query_part(
        &self,
        now: i64,
        bytes_left: &mut usize,
    ) -> Result<crate::query::catalog::Part, crate::QueryError> {
        self.validate()
            .map_err(|error| crate::QueryError::io(&error))?;
        #[cfg(feature = "postgres")]
        if let Some(postgres) = &self.postgres {
            let mut postgres = postgres
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Postgres { runtime, store } = &mut *postgres;
            let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
            return runtime
                .block_on(store.catalog_query_part(namespace, &self.options, now, bytes_left))
                .map_err(|error| crate::QueryError::new("unavailable", error.to_string()));
        }
        crate::query::catalog::sqlite_read(&self.options, now, bytes_left)
    }

    pub(crate) fn recheck_catalog_query(
        &self,
        part: &crate::query::catalog::Part,
    ) -> Result<(), crate::QueryError> {
        #[cfg(feature = "postgres")]
        if let Some(postgres) = &self.postgres {
            let mut postgres = postgres
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Postgres { runtime, store } = &mut *postgres;
            let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
            return runtime
                .block_on(async {
                    let tx = store.begin(namespace).await?;
                    if tx.catalog_generation(namespace).await?.as_deref()
                        != Some(part.generation.as_str())
                    {
                        return Err(StorageError::Backend(
                            "Catalog Query publication changed".into(),
                        ));
                    }
                    for (key, expected) in &part.witnesses {
                        let actual = tx
                            .catalog_row(namespace, key, crate::CatalogReadScope::Current)
                            .await?;
                        if actual
                            .and_then(|row| row.query)
                            .map(|value| value.to_string())
                            .as_deref()
                            != Some(expected.as_str())
                        {
                            return Err(StorageError::Backend(
                                "Catalog Query facts changed".into(),
                            ));
                        }
                    }
                    tx.commit(namespace).await
                })
                .map_err(|error| crate::QueryError::new("unavailable", error.to_string()));
        }
        crate::query::catalog::sqlite_recheck(&self.options, part)
    }

    pub(crate) fn with_selected_query<T>(
        &self,
        key: &str,
        provider: Option<&dyn crate::SessionSourceReader>,
        mode: crate::SessionSourceReadMode,
        now: i64,
        read: impl FnOnce(crate::model::QueryData) -> T,
    ) -> io::Result<T> {
        self.validate()?;
        #[cfg(feature = "postgres")]
        if let Some(postgres) = &self.postgres {
            let mut postgres = postgres
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Postgres { runtime, store } = &mut *postgres;
            let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
            return runtime.block_on(async {
                store
                    .selected_query(namespace, &self.options, key, provider, mode, now, read)
                    .await
                    .map_err(io::Error::other)
            });
        }
        crate::overview_repair::with_query(&self.options, key, provider, mode, now, read)
    }

    #[cfg(feature = "postgres")]
    pub(crate) fn query(&self, now: i64) -> io::Result<crate::model::QueryData> {
        let postgres = self
            .postgres
            .as_ref()
            .ok_or_else(|| io::Error::other("not PostgreSQL"))?;
        let mut postgres = postgres
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Postgres { runtime, store } = &mut *postgres;
        let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
        runtime
            .block_on(store.native_query(namespace, &self.options, now))
            .map_err(io::Error::other)
    }
}

fn unavailable() -> ViewerReply {
    crate::catalog::error(
        503,
        "catalog_unavailable",
        "The session store is unavailable. Retry after observation completes.",
        true,
    )
}
