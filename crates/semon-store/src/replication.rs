//! Canonical-only replication from the local source-of-truth store.

use std::time::Duration;

use reqwest::{StatusCode, blocking::Client};
use thiserror::Error;

use crate::{StoreError, TraceStore};

/// Environment variable read by the `semon ship` command when no endpoint
/// argument is supplied.
pub const REPLICATION_ENDPOINT_ENV: &str = "SEMON_REPLICATION_ENDPOINT";

const CONTENT_HASH_HEADER: &str = "x-semon-content-hash";
const TRACE_FORMAT_HEADER: &str = "x-semon-trace-version";
const TRACE_FORMAT_VERSION: &str = "1";
const PAGE_SIZE: u32 = 256;

/// The outcome of one replication pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShipReport {
    configured: bool,
    shipped: usize,
}

impl ShipReport {
    /// Reports whether this pass had a non-empty endpoint configured.
    pub fn endpoint_configured(&self) -> bool {
        self.configured
    }

    /// Returns the number of canonical traces accepted by the endpoint.
    pub fn shipped(&self) -> usize {
        self.shipped
    }
}

/// Errors produced by configured replication.
#[derive(Debug, Error)]
pub enum ReplicationError {
    /// Reading canonical traces from the local store failed.
    #[error("cannot read local traces for replication: {0}")]
    Store(#[from] StoreError),

    /// The configured HTTP endpoint could not be reached or read.
    #[error("replication request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The endpoint rejected a canonical trace.
    #[error("replication endpoint answered {status}: {body}")]
    Endpoint {
        /// HTTP status returned by the configured endpoint.
        status: StatusCode,
        /// A bounded response excerpt useful for diagnostics.
        body: String,
    },
}

/// Replicates every canonical trace to an optional HTTP ingest endpoint.
///
/// With no endpoint (or a blank endpoint), this is deliberately a successful
/// no-op. When configured, each trace is sent independently as canonical JSON.
/// Its semantic content hash is carried in `X-Semon-Content-Hash`; receivers
/// use that key for idempotent upsert/deduplication. Raw forensic bytes and
/// carrier labels are never read by this path and therefore cannot be shipped.
pub fn ship(store: &TraceStore, endpoint: Option<&str>) -> Result<ShipReport, ReplicationError> {
    let Some(endpoint) = endpoint.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(ShipReport {
            configured: false,
            shipped: 0,
        });
    };

    let transport = HttpTransport {
        client: Client::builder().timeout(Duration::from_secs(60)).build()?,
    };
    ship_with_transport(store, endpoint, &transport)
}

trait ReplicationTransport {
    fn send(
        &self,
        endpoint: &str,
        content_hash: &str,
        body: Vec<u8>,
    ) -> Result<(), ReplicationError>;
}

struct HttpTransport {
    client: Client,
}

impl ReplicationTransport for HttpTransport {
    fn send(
        &self,
        endpoint: &str,
        content_hash: &str,
        body: Vec<u8>,
    ) -> Result<(), ReplicationError> {
        let response = self
            .client
            .post(endpoint)
            .header("content-type", "application/json")
            .header(CONTENT_HASH_HEADER, content_hash)
            .header(TRACE_FORMAT_HEADER, TRACE_FORMAT_VERSION)
            .body(body)
            .send()?;
        let status = response.status();
        if !status.is_success() {
            let mut body = response.text().unwrap_or_default();
            body.truncate(1024);
            return Err(ReplicationError::Endpoint {
                status,
                body: body.trim().to_owned(),
            });
        }
        Ok(())
    }
}

fn ship_with_transport(
    store: &TraceStore,
    endpoint: &str,
    transport: &impl ReplicationTransport,
) -> Result<ShipReport, ReplicationError> {
    let mut after = None;
    let mut shipped = 0;

    loop {
        let traces = store.list_traces(after.as_ref(), PAGE_SIZE)?;
        if traces.is_empty() {
            break;
        }

        for trace in &traces {
            let body = trace.semantic_core().canonical_json()?;
            transport.send(endpoint, trace.id().as_str(), body)?;
            shipped += 1;
        }

        after = traces.last().map(|trace| trace.id().clone());
        if traces.len() < PAGE_SIZE as usize {
            break;
        }
    }

    Ok(ShipReport {
        configured: true,
        shipped,
    })
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Mutex};

    use serde_json::json;

    use super::*;
    use crate::{NewRawCarrierRecord, SemanticCore};

    #[test]
    fn no_endpoint_is_a_successful_no_op() {
        let store = TraceStore::open_in_memory().unwrap();

        let report = ship(&store, None).unwrap();

        assert!(!report.endpoint_configured());
        assert_eq!(report.shipped(), 0);
    }

    #[test]
    fn shipping_the_same_content_hash_twice_is_idempotent() {
        let mut store = TraceStore::open_in_memory().unwrap();
        let semantic = SemanticCore::from_value(json!({
            "kind": "intent",
            "content": "synthetic replication test"
        }))
        .unwrap();
        let occurrence = |session: &'static str| crate::NewOccurrence {
            session,
            sequence: 0,
            timestamp: 0,
            repo: "",
            repo_source: crate::RepoSource::None,
            parent_sequence: None,
            agent: None,
            authored_by: crate::AuthoredBy::Unknown,
        };
        let trace_id = store
            .capture(
                &semantic,
                NewRawCarrierRecord::new("synthetic-carrier", b"forensic-one"),
                occurrence("session-a"),
            )
            .unwrap()
            .trace_id()
            .clone();
        store
            .capture(
                &semantic,
                NewRawCarrierRecord::new("different-carrier", b"forensic-two"),
                occurrence("session-b"),
            )
            .unwrap();

        let endpoint = "in-test://canonical-traces";
        let receiver = RecordingEndpoint::default();

        assert_eq!(
            ship_with_transport(&store, endpoint, &receiver)
                .unwrap()
                .shipped(),
            1
        );
        assert_eq!(
            ship_with_transport(&store, endpoint, &receiver)
                .unwrap()
                .shipped(),
            1
        );

        let received = receiver.received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(
            received.get(trace_id.as_str()),
            Some(&semantic.canonical_json().unwrap())
        );
        let wire = String::from_utf8(received[trace_id.as_str()].clone()).unwrap();
        assert!(!wire.contains("synthetic-carrier"));
        assert!(!wire.contains("different-carrier"));
        assert!(!wire.contains("forensic-one"));
        assert!(!wire.contains("forensic-two"));
    }

    #[derive(Default)]
    struct RecordingEndpoint {
        received: Mutex<BTreeMap<String, Vec<u8>>>,
    }

    impl ReplicationTransport for RecordingEndpoint {
        fn send(
            &self,
            endpoint: &str,
            content_hash: &str,
            body: Vec<u8>,
        ) -> Result<(), ReplicationError> {
            assert_eq!(endpoint, "in-test://canonical-traces");
            self.received
                .lock()
                .unwrap()
                .entry(content_hash.to_owned())
                .or_insert(body);
            Ok(())
        }
    }
}
