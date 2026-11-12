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
fn encode(envelope: &mut Envelope) -> Vec<u8> {
    envelope.manifest.sessions = envelope.payload.sessions.len() as u64;
    envelope.manifest.rows = counts(&envelope.payload);
    let size = serde_json::to_vec(&envelope.payload).unwrap().len() as u64;
    envelope.manifest.payload_bytes = size;
    envelope.manifest.decoded_bytes = size;
    serde_json::to_vec(envelope).unwrap()
}

fn populated() -> Envelope {
    let fields = schema::FACT_SESSION
        .columns
        .iter()
        .filter(|column| !column.nullable)
        .map(|column| {
            let value = if column.name == "session_uid" {
                Value::from(format!("ses:{}", "a".repeat(64)))
            } else if !column.values.is_empty() {
                Value::from(column.values[0])
            } else {
                match column.data_type {
                    schema::Type::Text => Value::from("fixture"),
                    schema::Type::Boolean => Value::from(false),
                    _ => Value::from(0),
                }
            };
            (column.name.into(), value)
        })
        .collect();
    let mut envelope = empty();
    envelope.payload.sessions.push(Session {
        rows: vec![Row {
            table: "fact_session".into(),
            fields,
        }],
        policy_versions: vec![PolicyVersion {
            source_handle: format!("src:{}", "b".repeat(64)),
            policy_version: format!("sha256:{}", "c".repeat(64)),
        }],
    });
    envelope
}

#[test]
fn flat_shape_namespace_and_duplicate_validation() {
    let mut envelope = populated();
    let bytes = encode(&mut envelope);
    validate(&bytes, "origin").unwrap();
    assert_eq!(validate(&bytes, "foreign"), Err(Failure::Invalid));
    for invalid in [
        format!("{}/receiver/{}", hex::encode("origin"), "a".repeat(64)),
        format!("ses:{}", "A".repeat(64)),
        format!("ses:{}", "a".repeat(63)),
        format!("stp:{}", "a".repeat(64)),
        format!("ses:{}", "a".repeat(65)),
        "ses:malformed".into(),
    ] {
        let mut envelope = populated();
        envelope.payload.sessions[0].rows[0]
            .fields
            .insert("session_uid".into(), Value::from(invalid));
        assert_eq!(
            validate(&encode(&mut envelope), "origin"),
            Err(Failure::Invalid)
        );
    }
    for invalid in [
        format!("foreign/source/{}", "b".repeat(64)),
        format!("ses:{}", "b".repeat(64)),
        format!("src:{}", "B".repeat(64)),
        format!("src:{}", "b".repeat(63)),
    ] {
        let mut envelope = populated();
        envelope.payload.sessions[0].policy_versions[0].source_handle = invalid;
        assert_eq!(
            validate(&encode(&mut envelope), "origin"),
            Err(Failure::Invalid)
        );
    }
    let mut envelope = populated();
    envelope.payload.sessions[0].policy_versions[0].source_handle =
        format!("src:{}", "a".repeat(64));
    assert_eq!(
        validate(&encode(&mut envelope), "origin"),
        Err(Failure::Invalid)
    );
    let mut envelope = populated();
    envelope.payload.sessions[0]
        .policy_versions
        .push(PolicyVersion {
            source_handle: format!("src:{}", "b".repeat(64)),
            policy_version: format!("sha256:{}", "c".repeat(64)),
        });
    assert_eq!(
        validate(&encode(&mut envelope), "origin"),
        Err(Failure::Invalid)
    );
    let mut envelope = populated();
    envelope
        .payload
        .sessions
        .push(populated().payload.sessions.remove(0));
    assert_eq!(
        validate(&encode(&mut envelope), "origin"),
        Err(Failure::Invalid)
    );
    for (field, tag) in [
        ("session_uid", "ses"),
        ("parent_session_uid", "ses"),
        ("lane_root_uid", "ses"),
        ("step_uid", "stp"),
        ("turn_uid", "trn"),
        ("compaction_uid", "cmp"),
    ] {
        let column = schema::TABLES
            .iter()
            .find_map(|table| table.column(field))
            .unwrap();
        governance::validate_receiver_value(
            column,
            &Value::from(format!("{tag}:{}", "d".repeat(64))),
        )
        .unwrap();
        assert!(
            governance::validate_receiver_value(
                column,
                &Value::from(format!("src:{}", "d".repeat(64)))
            )
            .is_err()
        );
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
        let mut envelope = populated();
        envelope.payload.sessions[0].rows[0]
            .fields
            .insert(field.into(), Value::from("PRIVATE_ROW_VALUE"));
        assert!(validate(&encode(&mut envelope), "origin").is_err());
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
