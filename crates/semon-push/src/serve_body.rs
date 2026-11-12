//! Closed body routes on the shared receiver listener (Part of #594).
use super::*;
use crate::{
    body_enrollment::RegistryEnrollmentSource,
    body_wire::{self as wire, Reason},
    receiver_audit::Identifier,
};

enum Route {
    Poll,
    Response(Identifier),
}
impl Route {
    fn parse(path: &str) -> Option<Self> {
        if path == "/v1/body/poll" {
            return Some(Self::Poll);
        }
        let id = path
            .strip_prefix("/v1/body/requests/")?
            .strip_suffix("/response")?;
        Identifier::try_from(id.to_owned()).ok().map(Self::Response)
    }
    fn cap(&self) -> usize {
        match self {
            Self::Poll => wire::CONTROL_CAP,
            Self::Response(_) => wire::RESPONSE_CAP,
        }
    }
}

fn public_reason(reason: Reason) -> Reason {
    match reason {
        Reason::ForeignRequest | Reason::LateResponse | Reason::Revoked | Reason::Forbidden => {
            Reason::Forbidden
        }
        other => other,
    }
}
fn error(connection: &mut Connection, reason: Reason) -> Next {
    let reason = public_reason(reason);
    if reason == Reason::Forbidden {
        return authority_refusal();
    }
    let status = match reason {
        Reason::Forbidden => 401,
        Reason::PollCapacity
        | Reason::PendingQueueFull
        | Reason::ConnectionCapacity
        | Reason::Unavailable
        | Reason::Deadline => 503,
        Reason::OverBudget => 413,
        _ => 400,
    };
    let body = json!({"version": wire::VERSION, "reason": reason, "retryable": reason.retryable()});
    let _ = connection.respond(status, &body, true, &[]);
    Next::Close
}

pub(super) fn answer_body(
    shared: &Shared,
    slot: &mut Slot,
    connection: &mut Connection,
    head: &Head,
    now: u64,
) -> Next {
    let Some(host) = shared.body.as_ref() else {
        return authority_refusal();
    };
    if host.check_revision().is_err() {
        return authority_refusal();
    }
    let source = match RegistryEnrollmentSource::authenticate(
        shared.tokens.registry(),
        &mut connection.authority,
        head.authorization.as_deref(),
        now,
    ) {
        Ok(source) => source,
        Err(_) => return authority_refusal(),
    };
    if let Err(reason) = host.maintain(&source) {
        return error(connection, reason);
    }
    let Some(route) = Route::parse(&head.target) else {
        return authority_refusal();
    };
    if head.method != "POST" {
        return Next::Refuse(405, "body protocol is POST only", &[("Allow", "POST")]);
    }
    let Some(length) = head.content_length else {
        return Next::Refuse(411, "a Content-Length is required", &[]);
    };
    slot.authenticated();
    let poll_started = Instant::now();
    if length > route.cap() as u64 {
        return error(connection, Reason::OverBudget);
    }
    // Read/decoding is bounded by one control/frame envelope and a 20-second
    // absolute deadline. Do not send 100 Continue before authentication/size checks.
    if head.expect_continue {
        return Next::Refuse(
            417,
            "body protocol requires a bounded complete request",
            &[],
        );
    }
    let saved = connection.limits.body;
    connection.limits.body = saved.min(Duration::from_secs(20));
    let bytes = match route {
        Route::Poll => connection.read_body(length as usize),
        Route::Response(_) => read_frame(connection, length as usize),
    };
    connection.limits.body = saved;
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(read_error) if is_timeout(&read_error) => return error(connection, Reason::Deadline),
        Err(read_error) if read_error.kind() == io::ErrorKind::InvalidData => {
            return error(connection, Reason::InvalidRequest);
        }
        Err(_) => return Next::Close,
    };
    let result = match route {
        Route::Poll => crate::body_broker::handle_poll(
            &host.broker,
            &source,
            &bytes,
            Duration::from_secs(20).saturating_sub(poll_started.elapsed()),
        ),
        Route::Response(id) => {
            crate::body_broker::handle_response(&host.broker, &source, id, &bytes)
        }
    };
    if let Some(reason) = result.reason {
        return error(connection, reason);
    }
    use crate::body_broker::EnrollmentSource;
    if host.maintain(&source).is_err()
        || source
            .authenticated_enrollment(crate::origin_read::EnrollmentKind::BodyChannel)
            .is_err()
    {
        return error(connection, Reason::Forbidden);
    }
    if RegistryEnrollmentSource::authenticate(
        shared.tokens.registry(),
        &mut connection.authority,
        head.authorization.as_deref(),
        host.broker.clock.now_millis() / 1000,
    )
    .is_err()
    {
        return error(connection, Reason::Forbidden);
    }
    // Poll bytes have already passed strict versioned control serialization.
    let body = if result.bytes.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice(&result.bytes) {
            Ok(body) => body,
            Err(_) => return error(connection, Reason::Unavailable),
        }
    };
    if result.status == 204 {
        let close = if head.close {
            "Connection: close\r\n"
        } else {
            ""
        };
        let response = format!(
            "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nCache-Control: no-store\r\n{close}\r\n"
        );
        return if connection.send(response.as_bytes()).is_ok() {
            Next::Continue
        } else {
            Next::Close
        };
    }
    match connection.respond(result.status, &body, head.close, &[]) {
        Ok(()) => Next::Continue,
        Err(_) => Next::Close,
    }
}

/// Validate the u32/header before buffering raw payload. Content-Length was
/// checked by the caller; all three reads share one absolute read deadline.
fn read_frame(connection: &mut Connection, length: usize) -> io::Result<Vec<u8>> {
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    if length < 4 {
        return Err(invalid());
    }
    let until = Instant::now() + connection.limits.body;
    let mut bytes = connection.read_body(4)?;
    let header_length =
        u32::from_be_bytes(bytes.as_slice().try_into().map_err(|_| invalid())?) as usize;
    if header_length > wire::CONTROL_CAP || header_length > length - 4 {
        return Err(invalid());
    }
    connection.limits.body = until.saturating_duration_since(Instant::now());
    let header = connection.read_body(header_length)?;
    let parsed: wire::FrameHeader = serde_json::from_slice(&header).map_err(|_| invalid())?;
    if parsed.version != wire::VERSION
        || parsed.payload_bytes as usize != length - 4 - header_length
    {
        return Err(invalid());
    }
    match &parsed.content {
        wire::FrameContent::Header { .. } if parsed.frame != 0 || parsed.payload_bytes != 0 => {
            return Err(invalid());
        }
        wire::FrameContent::Data if parsed.frame == 0 || parsed.payload_bytes == 0 => {
            return Err(invalid());
        }
        wire::FrameContent::Refused { .. }
            if parsed.frame != 0 || !parsed.final_frame || parsed.payload_bytes != 0 =>
        {
            return Err(invalid());
        }
        _ => {}
    }
    bytes.extend_from_slice(&header);
    connection.limits.body = until.saturating_duration_since(Instant::now());
    bytes.extend_from_slice(&connection.read_body(parsed.payload_bytes as usize)?);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct At;
    impl crate::origin_read::ReadClock for At {
        fn now_millis(&self) -> u64 {
            100_000
        }
    }
    static AT: At = At;
    fn shared(body: bool) -> (auth_tests::PrivateDir, Arc<Shared>) {
        use crate::enrollment::{Binding, ClaimEpoch, Issuer, IssuerKey, IssuerKind};
        let dir = auth_tests::PrivateDir::new();
        let mut registry = auth_tests::records(true);
        registry.enrollments[0].credential_kinds[0].kind = Kind::BodyChannel;
        registry.credentials[0].kind = Kind::BodyChannel;
        registry.enrollments[0].binding = Binding::Admitted {
            namespace: "asi-0123456789abcdef0123456789abcdef".into(),
            origin_incarnation: "origin-1".into(),
            index_incarnation: "index-1".into(),
            publisher_claim_epoch: ClaimEpoch::Assigned(7),
            issuer: Issuer {
                kind: IssuerKind::Receiver,
                origin_trust_acknowledged: true,
            },
            assertion_issuer_keys: vec![IssuerKey {
                key_id: "issuer-1".into(),
                public_key: [4; 32],
            }],
        };
        let mut store = crate::enrollment::Store::initialize(&dir.0).unwrap();
        store.commit(0, &registry).unwrap();
        let host = body.then(|| crate::body_host::BodyHost::for_test(&dir.0, &AT));
        let shared = Arc::new(Shared {
            receiver: Receiver::new(dir.0.clone()).unwrap(),
            tokens: Authenticator::open(&dir.0).unwrap(),
            tls: Some(
                tls_config(
                    &TlsFiles {
                        certificate: auth_tests::fixture("test-cert.pem"),
                        private_key: auth_tests::fixture("test-key.pem"),
                    },
                    &dir.0,
                )
                .unwrap(),
            ),
            limits: Limits::default(),
            admission: Mutex::new(Admission::default()),
            body: host,
            provisioning: None,
            provisioning_clock: Some(Arc::new(|| 100)),
            provisioning_timeout: None,
            provisioning_terminal: None,
        });
        (dir, shared)
    }
    fn connection(cert: bool) -> (TcpStream, Connection) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (socket, _) = listener.accept().unwrap();
        let mut authority = ConnectionAuthority::default();
        if cert {
            authority.verified_client_identity = Some(auth_tests::identity());
        }
        (
            client,
            Connection {
                stream: Stream::Plain(Timed {
                    bytes_read: 0,
                    socket,
                    deadline: Instant::now() + Duration::from_secs(10),
                    gap: Duration::from_secs(10),
                }),
                authority,
                head_only: false,
                buffer: vec![],
                limits: Limits::default(),
            },
        )
    }
    #[test]
    fn authentication_and_inert_host_refuse_before_reading() {
        for (configured, cert, credential) in [
            (false, true, Some("Bearer test-bearer")),
            (true, false, Some("Bearer test-bearer")),
            (true, true, None),
            (true, true, Some("Bearer foreign")),
        ] {
            let (_dir, shared) = shared(configured);
            let (_client, mut connection) = connection(cert);
            let mut slot = Slot::admit(&shared, "127.0.0.1".parse().unwrap()).unwrap();
            let head = Head {
                method: "POST".into(),
                target: "/v1/body/poll".into(),
                authorization: credential.map(str::to_owned),
                content_length: Some(100),
                ..Head::default()
            };
            assert!(matches!(
                answer(&shared, &mut slot, &mut connection, &head),
                Next::Refuse(401, "request refused", _)
            ));
            assert_eq!(connection.stream.timed().bytes_read, 0);
        }
    }
    #[test]
    fn oversized_envelopes_and_invalid_frame_headers_stop_before_payload() {
        let (_dir, shared) = shared(true);
        for path in [
            "/v1/body/poll".to_owned(),
            format!("/v1/body/requests/{}/response", "ab".repeat(32)),
        ] {
            let (_client, mut connection) = connection(true);
            let mut slot = Slot::admit(&shared, "127.0.0.1".parse().unwrap()).unwrap();
            let head = Head {
                method: "POST".into(),
                target: path,
                authorization: Some("Bearer test-bearer".into()),
                content_length: Some(wire::RESPONSE_CAP as u64 + 1),
                ..Head::default()
            };
            assert!(matches!(
                answer(&shared, &mut slot, &mut connection, &head),
                Next::Close
            ));
            assert_eq!(connection.stream.timed().bytes_read, 0);
        }
        let (mut client, mut connection) = connection(true);
        client
            .write_all(&((wire::CONTROL_CAP + 1) as u32).to_be_bytes())
            .unwrap();
        assert_eq!(
            read_frame(&mut connection, wire::RESPONSE_CAP)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(connection.stream.timed().bytes_read, 4);
    }
    #[test]
    fn listener_dispatches_pending_request_and_delivers_origin_refusal() {
        use crate::{
            body_assertion::{self, Audience, Claims, MemoryKey},
            body_broker::EnrollmentSource,
            origin_read::{EnrollmentKind, OpaqueId},
            receiver_audit::Destination,
        };
        let (_dir, shared) = shared(true);
        let (_client, mut conn) = connection(true);
        let source = RegistryEnrollmentSource::authenticate(
            shared.tokens.registry(),
            &mut conn.authority,
            Some("Bearer test-bearer"),
            100,
        )
        .unwrap();
        let scope = source
            .authenticated_enrollment(EnrollmentKind::BodyChannel)
            .unwrap();
        let host = shared.body.as_ref().unwrap();
        let poll_bytes = wire::control(&wire::PollRequest {
            version: wire::VERSION,
            channel_binding: [3; 32],
        })
        .unwrap();
        drop(host.broker.begin_poll(&source, &poll_bytes).unwrap());
        let id = Identifier::from_bytes([7; 32]);
        let mut request = wire::Request {
            version: wire::VERSION,
            request_id: id,
            assertion: String::new(),
            source_handle: OpaqueId::new("handle").unwrap(),
            anchor: OpaqueId::new("anchor").unwrap(),
            operation: wire::Operation::ReadPage {
                page: OpaqueId::new("page").unwrap(),
            },
            max_bytes: wire::RESPONSE_CAP as u32,
            deadline_millis: 110_000,
            destination: Destination::Viewer,
        };
        let claims = Claims {
            issuer_key_id: "issuer".into(),
            issued_at: 100,
            expiry: 110,
            audience: Audience {
                origin_incarnation: scope.origin_incarnation.as_str().into(),
                namespace: scope.namespace.as_str().into(),
            },
            claim_epoch: 7,
            nonce: [7; 32],
            subject_principal: "subject".into(),
            machine: scope.machine.as_str().into(),
            namespace: scope.namespace.as_str().into(),
            acl_grant_version: "acl".into(),
            receiver_policy_version: "policy".into(),
            request_digest: request.digest().unwrap(),
            deadline: 110,
            channel_binding: [3; 32],
        };
        let signing = MemoryKey::from_seed(zeroize::Zeroizing::new([7; 32]));
        request.assertion = hex::encode(body_assertion::encode(
            &body_assertion::sign(claims, &signing, || 100).unwrap(),
        ));
        host.broker
            .enqueue(&source, scope, [3; 32], request)
            .unwrap();
        let (mut client, mut conn) = connection(true);
        client.write_all(&poll_bytes).unwrap();
        let mut slot = Slot::admit(&shared, "127.0.0.1".parse().unwrap()).unwrap();
        let head = Head {
            method: "POST".into(),
            target: "/v1/body/poll".into(),
            authorization: Some("Bearer test-bearer".into()),
            content_length: Some(poll_bytes.len() as u64),
            ..Head::default()
        };
        assert!(matches!(
            answer(&shared, &mut slot, &mut conn, &head),
            Next::Continue
        ));
        let bytes = wire::encode_frame(&wire::Frame {
            header: wire::FrameHeader {
                version: wire::VERSION,
                request_id: id,
                frame: 0,
                final_frame: true,
                payload_bytes: 0,
                content: wire::FrameContent::Refused {
                    reason: Reason::Unavailable,
                    retryable: true,
                },
            },
            payload: vec![],
        })
        .unwrap();
        let (mut client, mut conn) = connection(true);
        client.write_all(&bytes).unwrap();
        let head = Head {
            target: format!("/v1/body/requests/{}/response", String::from(id)),
            content_length: Some(bytes.len() as u64),
            ..head
        };
        assert!(matches!(
            answer(&shared, &mut slot, &mut conn, &head),
            Next::Continue
        ));
        assert!(matches!(
            host.broker.take_result(&source, id),
            Err(Reason::Unavailable)
        ));
    }
    #[test]
    fn unknown_response_id_reaches_broker_and_refuses_uniformly() {
        let (_dir, shared) = shared(true);
        let id = Identifier::from_bytes([9; 32]);
        let bytes = wire::encode_frame(&wire::Frame {
            header: wire::FrameHeader {
                version: wire::VERSION,
                request_id: id,
                frame: 0,
                final_frame: true,
                payload_bytes: 0,
                content: wire::FrameContent::Refused {
                    reason: Reason::Unavailable,
                    retryable: true,
                },
            },
            payload: vec![],
        })
        .unwrap();
        let (mut client, mut connection) = connection(true);
        client.write_all(&bytes).unwrap();
        let mut slot = Slot::admit(&shared, "127.0.0.1".parse().unwrap()).unwrap();
        let head = Head {
            method: "POST".into(),
            target: format!("/v1/body/requests/{}/response", String::from(id)),
            authorization: Some("Bearer test-bearer".into()),
            content_length: Some(bytes.len() as u64),
            ..Head::default()
        };
        assert!(matches!(
            answer(&shared, &mut slot, &mut connection, &head),
            Next::Refuse(401, "request refused", _)
        ));
        assert_eq!(connection.stream.timed().bytes_read, bytes.len());
    }
    #[test]
    fn poll_capacity_and_malformed_controls_use_closed_results() {
        let (_dir, shared) = shared(true);
        let mut authority = ConnectionAuthority::default();
        authority.verified_client_identity = Some(auth_tests::identity());
        let source = RegistryEnrollmentSource::authenticate(
            shared.tokens.registry(),
            &mut authority,
            Some("Bearer test-bearer"),
            100,
        )
        .unwrap();
        let bytes = wire::control(&wire::PollRequest {
            version: wire::VERSION,
            channel_binding: [3; 32],
        })
        .unwrap();
        let _reservation = shared
            .body
            .as_ref()
            .unwrap()
            .broker
            .begin_poll(&source, &bytes)
            .unwrap();
        for (body, reason) in [
            (bytes, "poll_capacity"),
            (
                b"{\"version\":1,\"extra\":true}".to_vec(),
                "invalid_request",
            ),
        ] {
            let (mut client, mut connection) = connection(true);
            client.write_all(&body).unwrap();
            let mut slot = Slot::admit(&shared, "127.0.0.1".parse().unwrap()).unwrap();
            let head = Head {
                method: "POST".into(),
                target: "/v1/body/poll".into(),
                authorization: Some("Bearer test-bearer".into()),
                content_length: Some(body.len() as u64),
                ..Head::default()
            };
            assert!(matches!(
                answer(&shared, &mut slot, &mut connection, &head),
                Next::Close
            ));
            let mut response = [0; 4096];
            let count = client.read(&mut response).unwrap();
            let response = std::str::from_utf8(&response[..count]).unwrap();
            assert!(response.contains(reason), "{response}");
            assert!(response.contains("Connection: close"));
            assert!(!response.contains("Location:"));
        }
    }
    #[test]
    fn wrong_kinds_and_revocation_refuse_without_buffering() {
        for kind in [Kind::MirrorPush, Kind::AnalyticsPublish, Kind::BodyChannel] {
            let (dir, shared) = shared(true);
            let mut store = crate::enrollment::Store::open(&dir.0).unwrap();
            let mut registry = store.snapshot().unwrap();
            if kind == Kind::BodyChannel {
                registry.credentials[0].revoked_at = Some(100);
            } else {
                let mut credential = registry.credentials[0].clone();
                credential.credential_id = "c".repeat(64);
                credential.kind = kind;
                credential.sha256 = crate::credentials::Secret::new("wrong-kind".into()).hash();
                registry.credentials.push(credential);
                registry.enrollments[0]
                    .credential_kinds
                    .push(crate::enrollment::KindScope::new(kind, 1));
            }
            store.commit(registry.revision(), &registry).unwrap();
            let credential = if kind == Kind::BodyChannel {
                "Bearer test-bearer"
            } else {
                "Bearer wrong-kind"
            };
            let (_client, mut connection) = connection(true);
            let mut slot = Slot::admit(&shared, "127.0.0.1".parse().unwrap()).unwrap();
            let head = Head {
                method: "POST".into(),
                target: "/v1/body/poll".into(),
                authorization: Some(credential.into()),
                content_length: Some(100),
                ..Head::default()
            };
            assert!(matches!(
                answer(&shared, &mut slot, &mut connection, &head),
                Next::Refuse(401, "request refused", _)
            ));
            assert_eq!(connection.stream.timed().bytes_read, 0);
        }
    }
    #[test]
    fn missing_body_kind_or_trust_constructs_no_state() {
        let dir = auth_tests::PrivateDir::new();
        crate::enrollment::Store::initialize(&dir.0).unwrap();
        assert!(
            crate::body_host::BodyHost::configured(
                &dir.0,
                crate::body_broker::Config::default(),
                &AT
            )
            .unwrap()
            .is_none()
        );
        assert!(!dir.0.join(crate::receiver_custody::CACHE).exists());
        assert!(!dir.0.join(crate::receiver_custody::AUDIT).exists());
        let (_dir, shared) = shared(false);
        assert!(
            crate::body_host::BodyHost::configured(
                shared.receiver.dir(),
                crate::body_broker::Config::default(),
                &AT
            )
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn paths_accept_only_opaque_ids_and_closed_endpoints() {
        let id = "ab".repeat(32);
        assert!(matches!(Route::parse("/v1/body/poll"), Some(Route::Poll)));
        assert!(matches!(
            Route::parse(&format!("/v1/body/requests/{id}/response")),
            Some(Route::Response(_))
        ));
        for path in [
            "/v1/body/poll?selector=secret",
            "/v1/body/resolve",
            "/v1/body/requests/../response",
            "/v1/body/requests/path/to/file/response",
            "/v1/body/poll/",
            "/v1/body/requests/ABC/response",
        ] {
            assert!(Route::parse(path).is_none(), "{path}");
        }
    }
    #[test]
    fn hidden_request_states_have_one_public_reason() {
        for reason in [
            Reason::ForeignRequest,
            Reason::LateResponse,
            Reason::Revoked,
            Reason::Forbidden,
        ] {
            assert_eq!(public_reason(reason), Reason::Forbidden);
        }
    }
}
