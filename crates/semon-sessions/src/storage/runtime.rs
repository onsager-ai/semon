//! Source-owned runtime access. SQLite keeps its existing writer and snapshots;
//! PostgreSQL runs on a source-owned reactor, outside the embedding reactor.
//! Startup selection remains private until every runtime consumer uses this port.
use super::{Namespace, OnSearch, StorageError};
use crate::{Options, ViewerReply, catalog::Request, events::EventCache};
use std::io;

pub(crate) struct RuntimeStore {
    options: Options,
    namespace: Option<Namespace>,
    error: Option<StorageError>,
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
            postgres: Some(std::sync::Mutex::new(Postgres { runtime, store })),
        })
    }

    #[cfg(test)]
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
    pub(crate) fn publish_sources(&self, inputs: &[crate::Input]) -> io::Result<()> {
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
            .block_on(store.publish_sources(namespace, &self.options, inputs))
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("native Catalog generation changed"))?;
        Ok(())
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
