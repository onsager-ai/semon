//! Host custody operations on the source-owned backend. No backend discovery.
#[cfg(feature = "postgres")]
use super::RuntimeStore;
#[cfg(feature = "postgres")]
use std::io;

#[cfg(feature = "postgres")]
impl RuntimeStore {
    pub(crate) fn begin_facts_write(&self, wanted: &crate::Facts) -> io::Result<u64> {
        let mut postgres = self.postgres.as_ref().ok_or_else(|| io::Error::other("PostgreSQL backend required"))?.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let super::Postgres { runtime, store } = &mut *postgres;
        let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
        runtime.block_on(async {
            let tx = store.begin(namespace).await.map_err(io::Error::other)?;
            let revision = tx.begin_facts_write(namespace, wanted).await.map_err(io::Error::other)?;
            tx.commit(namespace).await.map_err(io::Error::other)?;
            Ok(revision)
        })
    }

    pub(crate) fn complete_facts_write(&self, revision: u64, wanted: &crate::Facts) -> io::Result<()> {
        let mut postgres = self.postgres.as_ref().ok_or_else(|| io::Error::other("PostgreSQL backend required"))?.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let super::Postgres { runtime, store } = &mut *postgres;
        let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
        runtime.block_on(async {
            let tx = store.begin(namespace).await.map_err(io::Error::other)?;
            tx.complete_facts_write(namespace, revision, wanted).await.map_err(io::Error::other)?;
            tx.commit(namespace).await.map_err(io::Error::other)
        })
    }

    pub(crate) fn begin_raw_write(
        &self,
        input: &crate::Input,
    ) -> io::Result<crate::PgRawIntent> {
        self.validate()?;
        let mut postgres = self.postgres.as_ref().ok_or_else(|| io::Error::other("PostgreSQL backend required"))?.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let super::Postgres { runtime, store } = &mut *postgres;
        let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
        runtime.block_on(async {
            let tx = store.begin(namespace).await.map_err(io::Error::other)?;
            let intent = tx.begin_raw_write(namespace, input).await.map_err(io::Error::other)?;
            tx.commit(namespace).await.map_err(io::Error::other)?;
            Ok(intent)
        })
    }

    pub(crate) fn complete_raw_write(
        &self,
        intent: crate::PgRawIntent,
        generation: &crate::PgRawGeneration,
    ) -> io::Result<()> {
        let mut postgres = self.postgres.as_ref().ok_or_else(|| io::Error::other("PostgreSQL backend required"))?.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let super::Postgres { runtime, store } = &mut *postgres;
        let namespace = self.namespace.as_ref().expect("PostgreSQL namespace");
        runtime.block_on(async {
            let tx = store.begin(namespace).await.map_err(io::Error::other)?;
            if !tx.complete_raw_write(namespace, intent, generation).await.map_err(io::Error::other)? {
                return Err(io::Error::other("Raw custody intent changed"));
            }
            tx.commit(namespace).await.map_err(io::Error::other)
        })
    }
}
