//! PostgreSQL projection transaction foundation. This is not yet a selectable
//! native session backend: startup, discovery and Query adoption remain pending.
//! Hosts inject a connected restricted-role client; this module reads no secrets
//! and opens no local database. Schema ownership belongs to the provisioning role.
use super::{Namespace, ProjectionTag, StorageError, TagCheck, ensure};
use tokio_postgres::{Client, IsolationLevel, Transaction};

mod index;
mod producer;
mod raw;
mod runtime;
mod selected;
pub use raw::{PgRawGeneration, PgRawIntent};
// The typed Catalog API is qualified independently before runtime adoption.
#[allow(dead_code, reason = "Catalog runtime integration is pending")]
mod catalog;
#[allow(dead_code, reason = "custody runtime integration is pending")]
mod custody;
#[allow(dead_code, reason = "search runtime integration is pending")]
mod search;

/// Current schema for host provisioning by a role separate from the application.
/// It is installed once; existing/unsupported stores are never migrated here.
pub fn schema() -> String {
    format!(
        "{} INSERT INTO semon_projection.metadata(singleton,stamp) VALUES(true,'{}');",
        include_str!("postgres.sql"),
        stamp()
    )
}

fn backend(error: impl std::fmt::Display) -> StorageError {
    StorageError::Backend(error.to_string())
}

fn stamp() -> String {
    format!(
        "semon-session-store/1 backend=postgres schema=7 parser={} projector={}",
        crate::events::CACHE_VERSION,
        crate::model::summary::CATALOG_VERSION
    )
}

/// The provisioning role installs the schema separately. Application connections
/// must neither own the tables nor be a superuser or a BYPASSRLS role.
pub struct PgProjectionStore {
    client: Client,
    namespace: Namespace,
}

impl PgProjectionStore {
    /// Serve the shared focused Catalog contract from this namespace's
    /// PostgreSQL snapshot. Native source access remains the host's authority.
    pub async fn catalog_reply(
        &mut self,
        namespace: &Namespace,
        options: &crate::Options,
        query: &str,
    ) -> Result<crate::ViewerReply, StorageError> {
        ensure(&self.namespace, namespace)?;
        let request = match crate::catalog::Request::parse(query) {
            Ok(request) => request,
            Err(reply) => return Ok(reply),
        };
        let tx = self.begin(namespace).await?;
        let reply = crate::catalog::postgres_page(&tx, namespace, options, &request)
            .await
            .map_err(backend)?;
        tx.commit(namespace).await?;
        Ok(reply)
    }

    pub async fn new(client: Client, namespace: Namespace) -> Result<Self, StorageError> {
        let role = client.query_one(
            "SELECT r.rolsuper,r.rolbypassrls,EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='semon_projection' AND c.relowner=r.oid) FROM pg_roles r WHERE r.rolname=current_user",
            &[],
        ).await.map_err(backend)?;
        if role.get::<_, bool>(0) || role.get::<_, bool>(1) || role.get::<_, bool>(2) {
            return Err(StorageError::Backend(
                "PostgreSQL requires a restricted non-owner role".into(),
            ));
        }
        Ok(Self { client, namespace })
    }

    pub async fn begin(
        &mut self,
        namespace: &Namespace,
    ) -> Result<PgProjectionTransaction<'_>, StorageError> {
        ensure(&self.namespace, namespace)?;
        let tx = self
            .client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await
            .map_err(backend)?;
        tx.query_one("SELECT set_config('semon.namespace',$1,true),set_config('statement_timeout','5000',true),set_config('lock_timeout','1000',true)", &[&namespace.binding()]).await.map_err(backend)?;
        let found: String = tx
            .query_one(
                "SELECT stamp FROM semon_projection.metadata WHERE singleton=true",
                &[],
            )
            .await
            .map_err(backend)?
            .get(0);
        if found != stamp() {
            return Err(StorageError::Unsupported(
                "unsupported PostgreSQL projection stamp".into(),
            ));
        }
        tx.execute(
            "INSERT INTO semon_projection.namespaces(namespace) VALUES($1) ON CONFLICT DO NOTHING",
            &[&namespace.binding()],
        )
        .await
        .map_err(backend)?;
        Ok(PgProjectionTransaction {
            tx,
            namespace: namespace.clone(),
        })
    }
}

pub struct PgProjectionTransaction<'a> {
    tx: Transaction<'a>,
    namespace: Namespace,
}

fn number(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value)
        .map_err(|_| StorageError::Backend("projection integer exceeds PostgreSQL bigint".into()))
}

fn decode(row: &tokio_postgres::Row) -> Result<ProjectionTag, StorageError> {
    let digest = |index| -> Result<[u8; 32], StorageError> {
        row.try_get::<_, Vec<u8>>(index)
            .map_err(backend)?
            .try_into()
            .map_err(|_| StorageError::Unsupported("invalid projection digest".into()))
    };
    Ok(ProjectionTag {
        dev: row.try_get::<_, i64>(0).map_err(backend)? as u64,
        ino: row.try_get::<_, i64>(1).map_err(backend)? as u64,
        offset: row.try_get::<_, i64>(2).map_err(backend)? as u64,
        prefix_sha256: digest(3)?,
        tail_sha256: digest(4)?,
        epoch: row.try_get(5).map_err(backend)?,
        projector: row.try_get::<_, i32>(6).map_err(backend)? as u32,
    })
}

impl PgProjectionTransaction<'_> {
    pub async fn ledger(
        &self,
        namespace: &Namespace,
        path: &str,
    ) -> Result<Option<ProjectionTag>, StorageError> {
        ensure(&self.namespace, namespace)?;
        self.tx.query_opt("SELECT dev,ino,consumed,prefix,tail,epoch,projector FROM semon_projection.ledgers WHERE namespace=$1 AND path=$2", &[&namespace.binding(), &path]).await.map_err(backend)?.as_ref().map(decode).transpose()
    }

    /// Ledger CAS and epoch allocation are one transaction. `append` requires
    /// the caller's native byte proof that this extends the consumed prefix.
    /// An append preserves its epoch; a replacement gets a new epoch. A conflicting
    /// caller must roll back and rescan Raw rather than acknowledge its range.
    pub async fn write_ledger(
        &self,
        namespace: &Namespace,
        path: &str,
        expected: Option<ProjectionTag>,
        mut next: ProjectionTag,
        append: bool,
    ) -> Result<Option<ProjectionTag>, StorageError> {
        ensure(&self.namespace, namespace)?;
        if next.projector != crate::model::summary::CATALOG_VERSION {
            return Err(StorageError::Unsupported(
                "unsupported projector version".into(),
            ));
        }
        self.tx
            .query_one(
                "SELECT epoch FROM semon_projection.namespaces WHERE namespace=$1 FOR UPDATE",
                &[&namespace.binding()],
            )
            .await
            .map_err(backend)?;
        let actual = self.ledger(namespace, path).await?;
        if actual != expected {
            return Ok(None);
        }
        next.epoch = match actual.filter(|_| append) {
            Some(previous)
                if previous.check(&ProjectionTag {
                    epoch: previous.epoch,
                    ..next
                }) == TagCheck::Advanced =>
            {
                previous.epoch
            }
            Some(previous) if previous == (ProjectionTag { epoch: previous.epoch, ..next }) => previous.epoch,
            _ => self.tx.query_one("UPDATE semon_projection.namespaces SET epoch=epoch+1 WHERE namespace=$1 RETURNING epoch", &[&namespace.binding()]).await.map_err(backend)?.get(0),
        };
        self.tx.execute("INSERT INTO semon_projection.ledgers(namespace,path,dev,ino,consumed,prefix,tail,epoch,projector) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT(namespace,path) DO UPDATE SET dev=excluded.dev,ino=excluded.ino,consumed=excluded.consumed,prefix=excluded.prefix,tail=excluded.tail,epoch=excluded.epoch,projector=excluded.projector", &[&namespace.binding(),&path,&(next.dev as i64),&(next.ino as i64),&number(next.offset)?,&&next.prefix_sha256[..],&&next.tail_sha256[..],&next.epoch,&(next.projector as i32)]).await.map_err(backend)?;
        Ok(Some(next))
    }

    /// A unit is readable only while its exact published tag agrees with the
    /// committed ledger in this snapshot. Raw proof checks remain independent.
    pub async fn publish_tag(
        &self,
        namespace: &Namespace,
        key: &str,
        path: &str,
        tag: ProjectionTag,
    ) -> Result<(), StorageError> {
        ensure(&self.namespace, namespace)?;
        if tag.projector != crate::model::summary::CATALOG_VERSION
            || self.ledger(namespace, path).await? != Some(tag)
        {
            return Err(StorageError::Backend("projection_stale".into()));
        }
        self.tx.execute("INSERT INTO semon_projection.units(namespace,unit_key,path,dev,ino,consumed,prefix,tail,epoch,projector) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT(namespace,unit_key,path) DO UPDATE SET dev=excluded.dev,ino=excluded.ino,consumed=excluded.consumed,prefix=excluded.prefix,tail=excluded.tail,epoch=excluded.epoch,projector=excluded.projector", &[&namespace.binding(),&key,&path,&(tag.dev as i64),&(tag.ino as i64),&number(tag.offset)?,&&tag.prefix_sha256[..],&&tag.tail_sha256[..],&tag.epoch,&(tag.projector as i32)]).await.map_err(backend)?;
        Ok(())
    }

    pub async fn check_tag(
        &self,
        namespace: &Namespace,
        key: &str,
        path: &str,
    ) -> Result<Option<TagCheck>, StorageError> {
        ensure(&self.namespace, namespace)?;
        let row = self.tx.query_opt("SELECT dev,ino,consumed,prefix,tail,epoch,projector FROM semon_projection.units WHERE namespace=$1 AND unit_key=$2 AND path=$3", &[&namespace.binding(),&key,&path]).await.map_err(backend)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let published = decode(&row)?;
        if published.projector != crate::model::summary::CATALOG_VERSION {
            return Ok(Some(TagCheck::Mismatch("projector_version")));
        }
        Ok(Some(match self.ledger(namespace, path).await? {
            Some(committed) => published.check(&committed),
            None => TagCheck::Mismatch("missing_ledger"),
        }))
    }

    pub async fn commit(self, namespace: &Namespace) -> Result<(), StorageError> {
        ensure(&self.namespace, namespace)?;
        self.tx.commit().await.map_err(backend)
    }
}
