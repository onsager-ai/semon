use super::*;
fn empty() -> Envelope {
    let payload = Payload {
        profile: PROFILE.into(),
        sessions: Vec::new(),
    };
    let size = serde_json::to_vec(&payload).unwrap().len() as u64;
    Envelope {
        manifest: Manifest {
            format: 1,
            schema: schema::ASI_SCHEMA_VERSION,
            profile: PROFILE.into(),
            namespace: "origin".into(),
            encoding: "json_identity".into(),
            payload_bytes: size,
            decoded_bytes: size,
            sessions: 0,
            rows: BTreeMap::new(),
        },
        payload,
    }
}
#[test]
fn closed_format_sizes_profiles_and_sanitized_errors() {
    let bytes = serde_json::to_vec(&empty()).unwrap();
    validate(&bytes, "origin").unwrap();
    for field in ["metadata", "body", "unknown"] {
        let mut value = serde_json::to_value(empty()).unwrap();
        value[field] = Value::from("PRIVATE_ROW_VALUE");
        let error = validate(&serde_json::to_vec(&value).unwrap(), "origin").unwrap_err();
        assert!(!format!("{error:?} {error}").contains("PRIVATE_ROW_VALUE"));
    }
    for change in 0..5 {
        let mut envelope = empty();
        match change {
            0 => envelope.manifest.profile = "hub_token_v1".into(),
            1 => envelope.manifest.decoded_bytes += 1,
            2 => envelope.manifest.schema += 1,
            3 => envelope.manifest.encoding = "zstd".into(),
            _ => envelope.manifest.sessions += 1,
        }
        assert!(validate(&serde_json::to_vec(&envelope).unwrap(), "origin").is_err());
    }
    assert!(validate(&vec![b'['; 8192], "origin").is_err());
    assert_eq!(
        validate(&vec![0; MAX_BYTES as usize + 1], "origin"),
        Err(Failure::Limit)
    );
    assert!(validate(b"not JSON PRIVATE_ROW_VALUE", "origin").is_err());
}
#[test]
fn body_unknown_columns_and_row_budget_refuse() {
    for field in ["body", "_gen", "metadata", "unknown"] {
        let mut envelope = empty();
        envelope.payload.sessions.push(Session {
            rows: vec![Row {
                table: "fact_session".into(),
                fields: Map::from_iter([(field.into(), Value::from("PRIVATE_ROW_VALUE"))]),
            }],
            policy_versions: vec![PolicyVersion {
                source_handle: "a".repeat(64),
                policy_version: "b".repeat(64),
            }],
        });
        envelope.manifest.sessions = 1;
        envelope.manifest.rows = counts(&envelope.payload);
        let size = serde_json::to_vec(&envelope.payload).unwrap().len() as u64;
        envelope.manifest.payload_bytes = size;
        envelope.manifest.decoded_bytes = size;
        assert!(validate(&serde_json::to_vec(&envelope).unwrap(), "origin").is_err());
    }
    let mut envelope = empty();
    envelope.payload.sessions.push(Session {
        rows: (0..=MAX_ROWS)
            .map(|_| Row {
                table: "fact_session".into(),
                fields: Map::new(),
            })
            .collect(),
        policy_versions: Vec::new(),
    });
    envelope.manifest.sessions = 1;
    envelope.manifest.rows = counts(&envelope.payload);
    let size = serde_json::to_vec(&envelope.payload).unwrap().len() as u64;
    envelope.manifest.payload_bytes = size;
    envelope.manifest.decoded_bytes = size;
    assert_eq!(
        validate(&serde_json::to_vec(&envelope).unwrap(), "origin"),
        Err(Failure::Limit)
    );
}
