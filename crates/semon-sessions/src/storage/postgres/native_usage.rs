//! Physical effects for the shared native-usage state machine. This family
//! borrows the existing transaction; it cannot acknowledge ingest or move a ledger.
use super::{sqlstate, txn::PgTxn};
use crate::{
    events::{
        CACHE_VERSION,
        store::native_usage::algorithm::{Effects, Fact},
    },
    storage::StorageError,
};

#[cfg(test)]
std::thread_local! {
    static FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

pub(super) struct Usage<'a, 'tx> {
    tx: &'a mut PgTxn<'tx>,
}

impl<'a, 'tx> Usage<'a, 'tx> {
    fn after_effect(&mut self) -> Result<(), StorageError> {
        #[cfg(test)]
        if FAIL_AFTER.with(|point| match point.get() {
            Some(1) => {
                point.set(None);
                true
            }
            Some(n) => {
                point.set(Some(n - 1));
                false
            }
            None => false,
        }) {
            return Err(StorageError::Unavailable);
        }
        self.tx.check()
    }

    // Partial private family, awaiting the complete IndexStore integration.
    #[allow(dead_code)]
    pub(super) fn new(tx: &'a mut PgTxn<'tx>) -> Result<Self, StorageError> {
        tx.check()?;
        let namespace = tx.namespace.as_str().as_bytes();
        tx.driver.execute("INSERT INTO semon_pg_source_state(namespace) VALUES($1) ON CONFLICT(namespace_digest) DO NOTHING", &[&namespace]).map_err(sqlstate::error)?;
        let row = tx.driver.query("SELECT namespace FROM semon_pg_source_state WHERE namespace_digest=sha256($1::bytea) AND namespace=$1 FOR UPDATE", &[&namespace]).map_err(sqlstate::error)?;
        if row.len() != 1 {
            return Err(StorageError::ScopeChanged);
        }
        let mut effects = Self { tx };
        effects.after_effect()?;
        Ok(effects)
    }
}

fn text(bytes: Vec<u8>) -> Result<String, StorageError> {
    String::from_utf8(bytes).map_err(|_| StorageError::Unavailable)
}

impl Effects for Usage<'_, '_> {
    fn current(&mut self, path: &str) -> Result<Option<(i64, String)>, StorageError> {
        self.tx.check()?;
        let rows = self.tx.driver.query("SELECT s.source_id,s.receipt FROM semon_pg_native_usage_current c JOIN semon_pg_native_usage_sources s USING(namespace,source_id) WHERE c.namespace_digest=sha256($1::bytea) AND c.namespace=$1 AND c.source_path_digest=semon_pg_native_digest($1,$2,'usage.path') AND semon_pg_native_key_matches(c.source_path,sha256($2::bytea),$2) AND s.parser_version=$3", &[&self.tx.namespace.as_str().as_bytes(),&path.as_bytes(),&i64::from(CACHE_VERSION)]).map_err(sqlstate::error)?;
        rows.first()
            .map(|r| Ok((r.get(0), text(r.get(1))?)))
            .transpose()
    }
    fn create(&mut self, path: &str, receipt: &str) -> Result<i64, StorageError> {
        self.tx.check()?;
        let ns = self.tx.namespace.as_str().as_bytes().to_vec();
        // The namespace source_state lock serializes max+1 and generation changes.
        let id: i64 = self.tx.driver.query_one("INSERT INTO semon_pg_native_usage_sources(namespace,source_id,source_path,receipt,checkpoint,parser_version,usage_ready) VALUES($1,(SELECT coalesce(max(source_id),0)+1 FROM semon_pg_native_usage_sources WHERE namespace=$1),$2,$3,0,$4,false) RETURNING source_id", &[&ns,&path.as_bytes(),&receipt.as_bytes(),&i64::from(CACHE_VERSION)]).map_err(sqlstate::error)?.get(0);
        self.after_effect()?;
        let changed = self.tx.driver.execute("INSERT INTO semon_pg_native_usage_current(namespace,source_path,source_id) VALUES($1,$2,$3) ON CONFLICT(namespace_digest,source_path_digest) DO UPDATE SET source_id=excluded.source_id WHERE semon_pg_native_usage_current.namespace=excluded.namespace AND semon_pg_native_key_matches(semon_pg_native_usage_current.source_path,sha256(excluded.source_path),excluded.source_path)", &[&ns,&path.as_bytes(),&id]).map_err(sqlstate::error)?;
        if changed != 1 { return Err(StorageError::ScopeChanged); }
        self.after_effect()?;
        Ok(id)
    }
    fn receipt(&mut self, id: i64, receipt: &str) -> Result<(), StorageError> {
        self.tx.check()?;
        self.tx.driver.execute("UPDATE semon_pg_native_usage_sources SET receipt=$3 WHERE namespace=$1 AND source_id=$2", &[&self.tx.namespace.as_str().as_bytes(),&id,&receipt.as_bytes()]).map_err(sqlstate::error)?;
        self.after_effect()
    }
    fn unblock(&mut self, path: &str, receipt: &str) -> Result<(), StorageError> {
        self.tx.check()?;
        // Bytea has no LIKE; decode the fixed ASCII prefix only, never user IDs.
        self.tx.driver.execute("UPDATE semon_pg_native_tree_repairs SET state=$4 WHERE namespace=$1 AND source_path=$2 AND substring(state from 1 for 8)=$5 AND (receipt<>$3 OR state=$6)", &[&self.tx.namespace.as_str().as_bytes(),&path.as_bytes(),&receipt.as_bytes(),&b"ready".as_slice(),&b"blocked_".as_slice(),&b"blocked_evidence".as_slice()]).map_err(sqlstate::error)?;
        self.after_effect()
    }
    fn fact(&mut self, id: i64, offset: u64) -> Result<Option<(String, String)>, StorageError> {
        self.tx.check()?;
        let rows = self.tx.driver.query("SELECT payload,record FROM semon_pg_native_usage_facts WHERE namespace=$1 AND source_id=$2 AND native_offset=$3::text::numeric", &[&self.tx.namespace.as_str().as_bytes(),&id,&offset.to_string()]).map_err(sqlstate::error)?;
        rows.first()
            .map(|r| Ok((text(r.get(0))?, text(r.get(1))?)))
            .transpose()
    }
    fn first(
        &mut self,
        id: i64,
        scope: &str,
        identity: &str,
    ) -> Result<Option<String>, StorageError> {
        self.tx.check()?;
        let rows = self.tx.driver.query("SELECT f.payload FROM semon_pg_native_usage_identities i JOIN semon_pg_native_usage_facts f USING(namespace,source_id,native_offset) WHERE i.namespace_digest=sha256($1::bytea) AND i.namespace=$1 AND i.source_id=$2 AND i.scope_digest=semon_pg_native_digest($1,$3,'usage.scope') AND i.identity_digest=semon_pg_native_digest($1,$4,'usage.identity') AND semon_pg_native_key_matches(i.scope,sha256($3::bytea),$3) AND semon_pg_native_key_matches(i.identity,sha256($4::bytea),$4)", &[&self.tx.namespace.as_str().as_bytes(),&id,&scope.as_bytes(),&identity.as_bytes()]).map_err(sqlstate::error)?;
        rows.first().map(|r| text(r.get(0))).transpose()
    }
    fn identity(
        &mut self,
        id: i64,
        scope: &str,
        identity: &str,
        offset: u64,
    ) -> Result<(), StorageError> {
        self.tx.check()?;
        self.tx.driver.execute("INSERT INTO semon_pg_native_usage_identities(namespace,source_id,scope,identity,native_offset) VALUES($1,$2,$3,$4,$5::text::numeric)", &[&self.tx.namespace.as_str().as_bytes(),&id,&scope.as_bytes(),&identity.as_bytes(),&offset.to_string()]).map_err(sqlstate::error)?;
        self.after_effect()
    }
    fn insert(&mut self, fact: Fact<'_>) -> Result<(), StorageError> {
        self.tx.check()?;
        self.tx.driver.execute("INSERT INTO semon_pg_native_usage_facts(namespace,source_id,native_offset,identity,payload,record,outcome,model_outcome) VALUES($1,$2,$3::text::numeric,$4,$5,$6,$7,$8)", &[&self.tx.namespace.as_str().as_bytes(),&fact.source,&fact.offset.to_string(),&fact.identity.as_bytes(),&fact.payload.as_bytes(),&fact.record.as_bytes(),&fact.tree.as_bytes(),&fact.model.as_bytes()]).map_err(sqlstate::error)?;
        self.after_effect()
    }
    fn dimension(
        &mut self,
        id: i64,
        kind: &str,
        key: &str,
        value: &str,
    ) -> Result<(), StorageError> {
        self.tx.check()?;
        let changed = self.tx.driver.execute("INSERT INTO semon_pg_native_usage_dimensions(namespace,source_id,kind,dimension,value) VALUES($1,$2,$3,$4,$5) ON CONFLICT(namespace_digest,source_id,kind_digest,dimension_digest) DO UPDATE SET value=excluded.value WHERE semon_pg_native_usage_dimensions.namespace=excluded.namespace AND semon_pg_native_key_matches(semon_pg_native_usage_dimensions.kind,sha256(excluded.kind),excluded.kind) AND semon_pg_native_key_matches(semon_pg_native_usage_dimensions.dimension,sha256(excluded.dimension),excluded.dimension)", &[&self.tx.namespace.as_str().as_bytes(),&id,&kind.as_bytes(),&key.as_bytes(),&value.as_bytes()]).map_err(sqlstate::error)?;
        if changed != 1 { return Err(StorageError::ScopeChanged); }
        self.after_effect()
    }
    fn ready(&mut self, id: i64, checkpoint: u64) -> Result<(), StorageError> {
        self.tx.check()?;
        self.tx.driver.execute("UPDATE semon_pg_native_usage_sources SET checkpoint=$3::text::numeric,usage_ready=true WHERE namespace=$1 AND source_id=$2", &[&self.tx.namespace.as_str().as_bytes(),&id,&checkpoint.to_string()]).map_err(sqlstate::error)?;
        self.after_effect()
    }
}

#[cfg(test)]
mod tests;
