//! Offline access to the fixed historical Relay format. Built separately against
//! the clean legacy pin; never linked into the current product workspace.
use age::x25519;
use semon_relay::{
    AGE_IDENTITY_FILE, DataKey, ForgetSelector, Frame, FrameContent, FrameKey, MACHINES_FILE,
    RECIPIENTS_FILE, Receiver, SIGNING_KEY_FILE, SnapshotPacket, ZERO_CHAIN, chain_line,
    decrypt_envelope, decrypt_frame, inspect_snapshot_manifest, load_machines, load_recipients,
    load_signing_key, load_snapshot_packet, load_state,
};
use semon_relay_export::{Input, Kind, LEGACY_SOURCE, with_verified_inputs};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::ExitCode,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const USAGE: &str = "Usage: semon-relay-history --custody DIRECTORY --out NEW_DIRECTORY\n\
    Offline only; original identities must be included in the custody artifact.\n\
    Private output includes plaintext, credentials, keys and ciphertext copies.\n\
    All retained scopes are inspected; no native home restore or network request.";

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_json(path: &Path, limit: u64) -> Result<Value> {
    let file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Err("legacy object exceeds its supported size limit".into());
    }
    Ok(serde_json::from_reader(file)?)
}
fn read_bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("legacy object exceeds its supported size limit".into());
    }
    Ok(bytes)
}
fn sync_directories(root: &Path) -> Result<()> {
    let mut pending = vec![root.to_owned()];
    let mut directories = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            }
        }
        directories.push(directory);
    }
    for directory in directories.into_iter().rev() {
        File::open(directory)?.sync_all()?;
    }
    Ok(())
}
fn directory(path: &Path) -> Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    Ok(())
}
fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    directory(path.parent().ok_or("output lacks a parent")?)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
fn identifier(value: &str) -> Result<&str> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("unsupported snapshot identifier".into());
    }
    Ok(value)
}
fn files(root: &Path) -> Result<Vec<PathBuf>> {
    if root.is_file() {
        return Ok(vec![PathBuf::new()]);
    }
    let mut pending = vec![PathBuf::new()];
    let mut found = Vec::new();
    while let Some(relative) = pending.pop() {
        for entry in fs::read_dir(root.join(&relative))? {
            let entry = entry?;
            let path = relative.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                pending.push(path);
            } else if entry.file_type()?.is_file() {
                found.push(path);
            } else {
                return Err("working input contains an unsafe file type".into());
            }
        }
    }
    found.sort();
    Ok(found)
}

#[derive(Default)]
struct Report {
    frames: Vec<Value>,
    snapshots: Vec<Value>,
    blobs: Vec<Value>,
    unavailable: Vec<Value>,
    metadata: Vec<Value>,
    failures: Vec<Value>,
}
impl Report {
    fn failure(&mut self, scope: &str, path: &Path, reason: &str) {
        // Never print decoder errors containing decrypted paths or records.
        self.failures
            .push(json!({"input":scope,"path":path,"reason":reason}));
    }
}
fn identities(inputs: &[Input], report: &mut Report) -> Vec<x25519::Identity> {
    let mut identities = Vec::new();
    for (index, input) in inputs.iter().enumerate() {
        let path = match input.kind {
            Kind::RecoveryKey => input.path.clone(),
            Kind::Config => input.path.join(AGE_IDENTITY_FILE),
            _ => continue,
        };
        if !path.exists() && input.kind == Kind::Config {
            continue;
        }
        let scope = format!("{}-{index}", input.kind.name());
        let result = (|| -> Result<Vec<x25519::Identity>> {
            let bytes = read_bytes(&path, 64 * 1024)?;
            let text = std::str::from_utf8(&bytes)?;
            let mut keys = Vec::new();
            for line in text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
            {
                keys.push(
                    line.parse::<x25519::Identity>()
                        .map_err(|_| "unsupported identity material")?,
                );
            }
            if keys.is_empty() {
                return Err("identity file has no supported recipients".into());
            }
            Ok(keys)
        })();
        match result {
            Ok(keys) => identities.extend(keys),
            Err(_) => report.failure(
                &scope,
                Path::new(AGE_IDENTITY_FILE),
                "unsupported_or_unreadable_identity",
            ),
        }
    }
    identities
}

fn decode_component(value: &str) -> Result<String> {
    let bytes = hex::decode(value)?;
    if hex::encode(&bytes) != value {
        return Err("noncanonical legacy storage component".into());
    }
    Ok(String::from_utf8(bytes)?)
}
fn numbered(value: &str, prefix: &str) -> Result<u64> {
    let number = value
        .strip_prefix(prefix)
        .ok_or("unsupported frame scope")?
        .parse::<u64>()?;
    if format!("{prefix}{number}") != value {
        return Err("noncanonical frame scope".into());
    }
    Ok(number)
}
fn frame_path(path: &Path) -> Result<Option<(bool, bool, FrameKey)>> {
    let components: Vec<_> = path
        .iter()
        .map(|p| p.to_str().ok_or("unsupported storage path"))
        .collect::<std::result::Result<_, _>>()?;
    let orphan = components.first() == Some(&"orphans");
    let parts = if orphan {
        &components[1..]
    } else {
        &components[..]
    };
    if !parts.contains(&"frames") {
        return Ok(None);
    }
    if parts.len() != 6 || parts[4] != "frames" {
        return Err("unsupported frame layout".into());
    }
    let deleted = parts[5].ends_with(".deleted");
    let sequence = parts[5]
        .strip_suffix(if deleted { ".deleted" } else { ".json" })
        .ok_or("unsupported frame record suffix")?;
    let seq: u64 = sequence.parse()?;
    if format!("{seq:020}") != sequence {
        return Err("noncanonical frame sequence".into());
    }
    Ok(Some((
        orphan,
        deleted,
        FrameKey {
            session: decode_component(parts[0])?,
            stream: decode_component(parts[1])?,
            generation: numbered(parts[if orphan { 3 } else { 2 }], "generation-")?,
            epoch: numbered(parts[if orphan { 2 } else { 3 }], "epoch-")?,
            seq,
        },
    )))
}

struct Decoded {
    frame: Frame,
    chain: [u8; 32],
    line: Vec<u8>,
    relative: PathBuf,
    orphan: bool,
}
fn session_key(reader: &Receiver, session: &str, keys: &[x25519::Identity]) -> Result<DataKey> {
    let envelope = reader
        .get_envelope(session)?
        .ok_or("missing session envelope")?;
    keys.iter()
        .find_map(|key| decrypt_envelope(&envelope, key).ok())
        .ok_or_else(|| "unable to authenticate envelope with declared recipients".into())
}
fn receiver_frames(
    root: &Path,
    out: &Path,
    scope: &str,
    keys: &[x25519::Identity],
    report: &mut Report,
) -> Result<()> {
    // This is a private working copy, never the verified source artifact.
    let reader = Receiver::open(root)?;
    let mut decoded = Vec::new();
    let mut session_keys = BTreeMap::new();
    for session in reader.list_envelope_sessions()? {
        match session_key(&reader, &session, keys) {
            Ok(key) => {
                session_keys.insert(session.clone(), key);
                report
                    .metadata
                    .push(json!({"input":scope,"session":session,"wrapped_key_verified":true}));
            }
            Err(_) => report.failure(
                scope,
                Path::new("keys"),
                "wrapped_session_key_recipient_or_integrity_failure",
            ),
        }
    }
    for relative in files(root)? {
        let parsed = match frame_path(&relative) {
            Ok(value) => value,
            Err(_) => {
                report.failure(scope, &relative, "unsupported_frame_layout");
                continue;
            }
        };
        let Some((orphan, deleted, expected)) = parsed else {
            continue;
        };
        let decode = (|| -> Result<Option<Decoded>> {
            if deleted {
                let value = read_json(&root.join(&relative), 65 * 1024 * 1024)?;
                if ["line", "ciphertext", "nonce"]
                    .iter()
                    .any(|key| value.get(key).is_some())
                    || value["session"] != expected.session
                    || value["stream"] != expected.stream
                    || value["generation"] != expected.generation
                    || value["epoch"] != expected.epoch
                    || value["seq"] != expected.seq
                    || !value["delete_id"].is_string()
                {
                    return Err("invalid body-free deletion receipt".into());
                }
                identifier(value["delete_id"].as_str().ok_or("missing deletion id")?)?;
                if !value["deleted_bytes"].is_u64() || !value["machine"].is_string() {
                    return Err("unsupported deletion receipt metadata".into());
                }
                match value["mode"].as_str() {
                    Some("plaintext") => {
                        identifier(value["chain"].as_str().ok_or("missing deleted chain")?)?;
                        identifier(
                            value["content_hash"]
                                .as_str()
                                .ok_or("missing deleted content hash")?,
                        )?;
                    }
                    Some("encrypted") => {
                        identifier(value["tag"].as_str().ok_or("missing deleted tag")?)?;
                    }
                    _ => return Err("unsupported deletion receipt mode".into()),
                }
                report
                    .unavailable
                    .push(json!({"input":scope,"path":relative,
                    "reason":"explicitly_deleted_body","record":value}));
                return Ok(None);
            }
            if root.join(relative.with_extension("deleted")).exists() {
                report.unavailable.push(json!({"input":scope,"path":relative,"reason":"body_shadowed_by_delete_receipt"}));
                return Ok(None);
            }
            if fs::metadata(root.join(&relative))?.len() > 65 * 1024 * 1024 {
                return Err("frame exceeds legacy size bound".into());
            }
            let frame = if orphan {
                reader.read_orphan(
                    &expected.session,
                    &expected.stream,
                    expected.generation,
                    expected.epoch,
                    expected.seq,
                )?
            } else {
                reader.read_frame(
                    &expected.session,
                    &expected.stream,
                    expected.generation,
                    expected.epoch,
                    expected.seq,
                )?
            }
            .ok_or("frame body unavailable")?;
            if frame.key != expected {
                return Err("stored frame identity/path substitution".into());
            }
            let (chain, line) = match &frame.content {
                FrameContent::Plaintext { chain, line } => (*chain, line.clone()),
                FrameContent::Encrypted(payload) => {
                    if !session_keys.contains_key(&expected.session) {
                        session_keys.insert(
                            expected.session.clone(),
                            session_key(&reader, &expected.session, keys)?,
                        );
                    }
                    decrypt_frame(
                        &session_keys[&expected.session],
                        &frame.key,
                        &frame.machine,
                        payload,
                    )?
                }
            };
            write(
                &out.join(scope)
                    .join("frames")
                    .join(&relative)
                    .with_extension("jsonl"),
                &line,
            )?;
            Ok(Some(Decoded {
                frame,
                chain,
                line,
                relative: relative.clone(),
                orphan,
            }))
        })();
        match decode {
            Ok(Some(value)) => decoded.push(value),
            Ok(None) => {}
            Err(_) => report.failure(
                scope,
                &relative,
                "frame_identity_decryption_or_integrity_failure",
            ),
        }
    }
    let mut live_chains = BTreeMap::new();
    let mut all_chains = BTreeMap::new();
    for value in &decoded {
        let key = &value.frame.key;
        let identity = (
            key.session.clone(),
            key.stream.clone(),
            key.generation,
            key.seq,
        );
        if !value.orphan && live_chains.insert(identity, value.chain).is_some() {
            report.failure(
                scope,
                &value.relative,
                "conflicting_live_sequence_across_epochs",
            );
        }
        all_chains.insert(
            (
                value.orphan,
                key.session.clone(),
                key.stream.clone(),
                key.generation,
                if value.orphan { key.epoch } else { 0 },
                key.seq,
            ),
            value.chain,
        );
    }
    for value in &decoded {
        let key = &value.frame.key;
        let previous = if key.seq == 0 {
            Some(ZERO_CHAIN)
        } else {
            all_chains
                .get(&(
                    value.orphan,
                    key.session.clone(),
                    key.stream.clone(),
                    key.generation,
                    if value.orphan { key.epoch } else { 0 },
                    key.seq - 1,
                ))
                .copied()
                .or_else(|| {
                    live_chains
                        .get(&(
                            key.session.clone(),
                            key.stream.clone(),
                            key.generation,
                            key.seq - 1,
                        ))
                        .copied()
                })
        };
        let continuity =
            previous.is_some_and(|previous| chain_line(&previous, &value.line) == value.chain);
        if !continuity {
            report.failure(
                scope,
                &value.relative,
                "missing_prefix_gap_or_chain_mismatch",
            );
        }
        report.frames.push(json!({"input":scope,"source_path":value.relative,"session":key.session,
            "stream":key.stream,"generation":key.generation,"epoch":key.epoch,"seq":key.seq,
            "orphan":value.orphan,"machine":value.frame.machine,"sender_wall_ns":value.frame.sender_wall_ns,
            "sender_mono_ns":value.frame.sender_mono_ns,"boot_id":value.frame.boot_id,
            "sha256":digest(&value.line),"bytes":value.line.len(),"chain":hex::encode(value.chain),"continuity_verified":continuity,
            "output_path":Path::new(scope).join("frames").join(&value.relative).with_extension("jsonl"),
            "encrypted":matches!(value.frame.content,FrameContent::Encrypted(_))}));
    }
    Ok(())
}

fn decrypt_blob(ciphertext: &str, id: &str, keys: &[x25519::Identity]) -> Result<Vec<u8>> {
    identifier(id)?;
    let ciphertext = hex::decode(ciphertext)?;
    let decryptor = age::Decryptor::new(ciphertext.as_slice())?;
    let identities: Vec<&dyn age::Identity> =
        keys.iter().map(|key| key as &dyn age::Identity).collect();
    let mut reader = decryptor
        .decrypt(identities.into_iter())?
        .take(8 * 1024 * 1024 + 1);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 || digest(&bytes) != id {
        return Err("snapshot plaintext checksum/limit failure".into());
    }
    Ok(bytes)
}
fn snapshot(
    wire: &Value,
    blobs: &BTreeMap<String, String>,
    out: &Path,
    scope: &str,
    label: &Path,
    keys: &[x25519::Identity],
    report: &mut Report,
) -> Result<()> {
    let root = identifier(wire["root"].as_str().ok_or("snapshot root missing")?)?;
    let id = identifier(wire["id"].as_str().ok_or("snapshot id missing")?)?;
    let clear = keys
        .iter()
        .find_map(|key| inspect_snapshot_manifest(wire, key).ok())
        .ok_or("snapshot manifest recipient/integrity/version failure")?;
    let entries = clear["entries"]
        .as_array()
        .ok_or("snapshot entries missing")?;
    for entry in entries {
        let blob = entry["blob"].as_str().ok_or("snapshot blob id missing")?;
        let ciphertext = blobs.get(blob).ok_or("snapshot referenced blob missing")?;
        let bytes = decrypt_blob(ciphertext, blob, keys)?;
        // The unchanged verifier checked relative paths, duplicates and modes.
        let path = entry["path"].as_str().ok_or("snapshot path missing")?;
        write(
            &out.join(scope)
                .join("snapshots")
                .join(label)
                .join("files")
                .join(path),
            &bytes,
        )?;
    }
    write(
        &out.join(scope)
            .join("snapshots")
            .join(label)
            .join("manifest.json"),
        &serde_json::to_vec_pretty(&clear)?,
    )?;
    report.snapshots.push(
        json!({"input":scope,"source_path":label,"root":root,"id":id,
        "parent":clear["parent"],"machine":clear["machine"],"session":clear["session"],
        "epoch":clear["epoch"],"taken_at_wall":clear["taken_at_wall"],"entries":entries.len(),
        "output_path":Path::new(scope).join("snapshots").join(label)}),
    );
    Ok(())
}
fn receiver_snapshots(
    root: &Path,
    out: &Path,
    scope: &str,
    keys: &[x25519::Identity],
    report: &mut Report,
) -> Result<()> {
    let paths = files(root)?;
    let mut manifests = Vec::new();
    let mut active_refs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut deletion_roots = BTreeSet::new();
    let mut blocked_roots = BTreeSet::new();
    let mut cutoffs = BTreeMap::new();
    let mut pruned = BTreeSet::new();
    let mut deleted_manifests = BTreeSet::new();
    for path in paths.iter().filter(|p| p.starts_with("snapshots")) {
        let parts: Vec<_> = path.iter().filter_map(|p| p.to_str()).collect();
        if parts.len() < 3 || !matches!(parts[2], "pruned" | "forget-before") {
            continue;
        }
        deletion_roots.insert(parts[1].to_owned());
        let result = (|| -> Result<()> {
            identifier(parts[1])?;
            if parts.len() == 3 && parts[2] == "forget-before" {
                let bytes = read_bytes(&root.join(path), 32)?;
                let cutoff = std::str::from_utf8(&bytes)?.parse::<u64>()?;
                cutoffs.insert(parts[1].to_owned(), cutoff);
                report.unavailable.push(json!({"input":scope,"path":path,
                    "reason":"snapshot_deletion_cutoff","before_wall_ms":cutoff}));
            } else if parts.len() == 4 && parts[2] == "pruned" {
                identifier(parts[3])?;
                if read_bytes(&root.join(path), 32)? != b"forgotten" {
                    return Err("unsupported snapshot deletion marker".into());
                }
                pruned.insert((parts[1].to_owned(), parts[3].to_owned()));
                report.unavailable.push(json!({"input":scope,"path":path,
                    "reason":"explicitly_pruned_snapshot"}));
            } else {
                return Err("unsupported snapshot deletion layout".into());
            }
            Ok(())
        })();
        if result.is_err() {
            blocked_roots.insert(parts[1].to_owned());
            report.failure(scope, path, "unsupported_snapshot_deletion_state");
        }
    }
    for path in paths.iter().filter(|p| p.starts_with("snapshots")) {
        let parts: Vec<_> = path
            .iter()
            .map(|p| p.to_str().ok_or("unsupported snapshot path"))
            .collect::<std::result::Result<_, _>>()?;
        if parts.len() == 4 && parts[2] == "manifests" {
            let result = (|| -> Result<()> {
                identifier(parts[1])?;
                identifier(parts[3])?;
                let wire = read_json(&root.join(path), 2 * (2 * 1024 * 1024 + 8192) + 65536)?;
                if wire["root"] != parts[1] || wire["id"] != parts[3] {
                    return Err("snapshot path/header substitution".into());
                }
                let cutoff = cutoffs.get(parts[1]).copied().unwrap_or(0);
                if blocked_roots.contains(parts[1])
                    || pruned.contains(&(parts[1].to_owned(), parts[3].to_owned()))
                    || wire["taken_at_wall"]
                        .as_u64()
                        .ok_or("missing snapshot time")?
                        < cutoff
                {
                    deleted_manifests.insert((parts[1].to_owned(), parts[3].to_owned()));
                    report.unavailable.push(json!({"input":scope,"path":path,"reason":"snapshot_shadowed_by_deletion_state"}));
                    return Ok(());
                }
                let clear = keys
                    .iter()
                    .find_map(|key| inspect_snapshot_manifest(&wire, key).ok())
                    .ok_or("snapshot manifest recipient/integrity/version failure")?;
                for entry in clear["entries"]
                    .as_array()
                    .ok_or("snapshot entries missing")?
                {
                    active_refs.entry(parts[1].to_owned()).or_default().insert(
                        entry["blob"]
                            .as_str()
                            .ok_or("snapshot blob missing")?
                            .to_owned(),
                    );
                }
                manifests.push((path.clone(), wire));
                Ok(())
            })();
            if result.is_err() {
                report.failure(
                    scope,
                    path,
                    "snapshot_manifest_recipient_header_or_integrity_failure",
                );
            }
        } else if !(parts.len() == 4 && matches!(parts[2], "blobs" | "pruned"))
            && !(parts.len() == 3 && parts[2] == "forget-before")
        {
            report.failure(scope, path, "unsupported_snapshot_storage_record");
        }
    }
    let mut by_root: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for path in paths.iter().filter(|p| p.starts_with("snapshots")) {
        let parts: Vec<_> = path
            .iter()
            .map(|p| p.to_str().ok_or("unsupported snapshot path"))
            .collect::<std::result::Result<_, _>>()?;
        if parts.len() != 4 || parts[2] != "blobs" {
            continue;
        }
        if identifier(parts[1]).is_err() || identifier(parts[3]).is_err() {
            report.failure(scope, path, "unsupported_snapshot_identifier");
            continue;
        }
        if blocked_roots.contains(parts[1])
            || deletion_roots.contains(parts[1])
                && !active_refs
                    .get(parts[1])
                    .is_some_and(|refs| refs.contains(parts[3]))
        {
            report.unavailable.push(
                json!({"input":scope,"path":path,"reason":"unreferenced_blob_with_deletion_state"}),
            );
            continue;
        }
        let result = (|| -> Result<()> {
            identifier(parts[1])?;
            identifier(parts[3])?;
            let value = read_json(&root.join(path), 18 * 1024 * 1024)?;
            let cipher = value["ciphertext"]
                .as_str()
                .ok_or("blob ciphertext missing")?;
            if value["digest"] != digest(cipher.as_bytes()) {
                return Err("stored ciphertext checksum mismatch".into());
            }
            let bytes = decrypt_blob(cipher, parts[3], keys)?;
            write(
                &out.join(scope).join("blobs").join(parts[1]).join(parts[3]),
                &bytes,
            )?;
            report.blobs.push(
                json!({"input":scope,"root":parts[1],"id":parts[3],"bytes":bytes.len(),
                    "output_path":Path::new(scope).join("blobs").join(parts[1]).join(parts[3])}),
            );
            by_root
                .entry(parts[1].to_owned())
                .or_default()
                .insert(parts[3].to_owned(), cipher.to_owned());
            Ok(())
        })();
        if result.is_err() {
            report.failure(
                scope,
                path,
                "snapshot_blob_recipient_checksum_or_format_failure",
            );
        }
    }
    let retained: BTreeSet<_> = manifests
        .iter()
        .filter_map(|(_, wire)| {
            Some((
                wire["root"].as_str()?.to_owned(),
                wire["id"].as_str()?.to_owned(),
            ))
        })
        .collect();
    for (path, wire) in manifests {
        let root_id = wire["root"].as_str().ok_or("snapshot root missing")?;
        if let Some(parent) = wire["parent"].as_str() {
            let identity = (root_id.to_owned(), parent.to_owned());
            if !retained.contains(&identity) {
                if pruned.contains(&identity) || deleted_manifests.contains(&identity) {
                    report.unavailable.push(json!({"input":scope,"path":path,
                        "reason":"snapshot_parent_unavailable_with_deletion_state","parent":parent}));
                } else {
                    report.failure(scope, &path, "snapshot_parent_missing");
                }
            }
        }
        let empty = BTreeMap::new();
        if snapshot(
            &wire,
            by_root.get(root_id).unwrap_or(&empty),
            out,
            scope,
            &path,
            keys,
            report,
        )
        .is_err()
        {
            report.failure(
                scope,
                &path,
                "snapshot_manifest_recipient_reference_or_integrity_failure",
            );
        }
    }
    let parents: BTreeSet<_> = report
        .snapshots
        .iter()
        .filter(|m| m["input"] == scope)
        .filter_map(|m| m["parent"].as_str().map(str::to_owned))
        .collect();
    for value in report.snapshots.iter_mut().filter(|m| m["input"] == scope) {
        value["fork_head"] = json!(!parents.contains(value["id"].as_str().unwrap_or("")));
    }
    Ok(())
}
fn selector(value: &Value) -> Result<ForgetSelector> {
    let string = |name: &str| -> Result<Option<String>> {
        match value.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(value)) => Ok(Some(value.clone())),
            _ => Err("unsupported deletion selector".into()),
        }
    };
    let before_ns = match value.get("before_ns") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_u64().ok_or("invalid deletion cutoff")?),
    };
    let selector = ForgetSelector {
        session: string("session")?,
        before_ns,
        memory_root: string("memory_root")?,
    };
    selector.validate()?;
    Ok(selector)
}
fn receiver_metadata(root: &Path, path: &Path) -> Result<bool> {
    let parts: Vec<_> = path.iter().filter_map(|p| p.to_str()).collect();
    let file = root.join(path);
    match parts.as_slice() {
        [MACHINES_FILE] => {
            load_machines(&file)?;
        }
        ["lease-register.json" | "takeovers.jsonl"] => { /* Checked by Receiver::open. */ }
        ["rewraps.jsonl"] => {
            for line in read_bytes(&file, 64 * 1024 * 1024)?
                .split(|b| *b == b'\n')
                .filter(|s| !s.is_empty())
            {
                let value: Value = serde_json::from_slice(line)?;
                identifier(
                    value["previous_sha256"]
                        .as_str()
                        .ok_or("missing old envelope hash")?,
                )?;
                identifier(
                    value["replacement_sha256"]
                        .as_str()
                        .ok_or("missing new envelope hash")?,
                )?;
                if !value["session"].is_string()
                    || !value["machine"].is_string()
                    || !value["forced"].is_boolean()
                    || !value["replaced_at_ms"].is_u64()
                {
                    return Err("unsupported rewrap metadata".into());
                }
            }
        }
        ["keys", name] => {
            decode_component(
                name.strip_suffix(".age")
                    .ok_or("unsupported wrapped key path")?,
            )?;
            if fs::metadata(file)?.len() > 64 * 1024 * 1024 {
                return Err("wrapped key exceeds supported limit".into());
            }
        }
        ["session-modes", name] => {
            decode_component(name)?;
            let bytes = read_bytes(&file, 32)?;
            if !matches!(
                std::str::from_utf8(&bytes)?.trim(),
                "plaintext" | "encrypted"
            ) {
                return Err("unsupported session mode".into());
            }
        }
        ["deletions", name] => {
            let id = identifier(
                name.strip_suffix(".json")
                    .ok_or("unsupported deletion receipt path")?,
            )?;
            let value = read_json(&file, 64 * 1024 * 1024)?;
            selector(&value["selector"])?;
            if value["report"]["id"] != id
                || !value["report"]["frames"].is_u64()
                || !value["report"]["bytes"].is_u64()
            {
                return Err("unsupported deletion acknowledgement".into());
            }
        }
        [session, stream, generation, "state.json"] => {
            decode_component(session)?;
            decode_component(stream)?;
            numbered(generation, "generation-")?;
            let value = read_json(&file, 64 * 1024 * 1024)?;
            if !value["epoch"].is_u64() || !value["acked"].is_u64() {
                return Err("unsupported receipt checkpoint".into());
            }
            match value["mode"].as_str() {
                None | Some("plaintext") => {
                    identifier(value["chain"].as_str().ok_or("missing chain")?)?;
                }
                Some("encrypted") => {
                    if !value["chain"].is_null() {
                        identifier(value["chain"].as_str().ok_or("invalid chain")?)?;
                    }
                }
                _ => return Err("unsupported receipt mode".into()),
            }
            if !value["tag"].is_null() {
                identifier(value["tag"].as_str().ok_or("invalid tag")?)?;
            }
            let seq = value["acked"]
                .as_u64()
                .ok_or("missing checkpoint sequence")?;
            let epoch = value["epoch"].as_u64().ok_or("missing checkpoint epoch")?;
            let body = root
                .join(session)
                .join(stream)
                .join(generation)
                .join(format!("epoch-{epoch}/frames/{seq:020}.json"));
            let deleted = body.with_extension("deleted");
            let record = read_json(
                if deleted.exists() { &deleted } else { &body },
                65 * 1024 * 1024,
            )?;
            let mode = value["mode"].as_str().unwrap_or("plaintext");
            if record["session"] != decode_component(session)?
                || record["stream"] != decode_component(stream)?
                || record["generation"] != numbered(generation, "generation-")?
                || record["epoch"] != epoch
                || record["seq"] != seq
                || record["mode"].as_str().unwrap_or("plaintext") != mode
                || (if mode == "plaintext" {
                    record["chain"] != value["chain"]
                } else {
                    record["tag"] != value["tag"]
                })
            {
                return Err("receipt checkpoint references missing or mismatched history".into());
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}
fn sender_metadata(root: &Path, relative: &Path) -> Result<()> {
    let parts: Vec<_> = relative.iter().filter_map(|p| p.to_str()).collect();
    let file = root.join(relative);
    if parts.len() >= 3 && parts[parts.len() - 3].ends_with(".forget") {
        let origin = parts[parts.len() - 2];
        let name = parts[parts.len() - 1];
        identifier(origin)?;
        let id = identifier(
            name.strip_suffix(".json")
                .ok_or("unsupported queued deletion")?,
        )?;
        let value = read_json(&file, 64 * 1024 * 1024)?;
        if value["id"] != id {
            return Err("queued deletion identity mismatch".into());
        }
        selector(&value["selector"])?;
        return Ok(());
    }
    // State filenames are configurable and need not have a JSON extension.
    read_json(&file, 64 * 1024 * 1024)?;
    load_state(&file)?;
    Ok(())
}
fn pending_snapshot_deleted(inputs: &[Input], wire: &Value) -> Result<bool> {
    let root = identifier(wire["root"].as_str().ok_or("missing snapshot root")?)?;
    let id = identifier(wire["id"].as_str().ok_or("missing snapshot id")?)?;
    let wall = wire["taken_at_wall"]
        .as_u64()
        .ok_or("missing snapshot time")?;
    let mut deleted = false;
    for input in inputs.iter().filter(|input| input.kind == Kind::Receiver) {
        let base = input.path.join("snapshots").join(root);
        if base.join("pruned").exists() {
            for entry in fs::read_dir(base.join("pruned"))? {
                let entry = entry?;
                let name = entry.file_name();
                let name = identifier(name.to_str().ok_or("unsupported deletion path")?)?;
                if !entry.file_type()?.is_file() || read_bytes(&entry.path(), 32)? != b"forgotten" {
                    return Err("unsupported snapshot deletion marker".into());
                }
                deleted |= name == id;
            }
        }
        let cutoff = base.join("forget-before");
        if cutoff.exists() {
            let bytes = read_bytes(&cutoff, 32)?;
            deleted |= wall < std::str::from_utf8(&bytes)?.parse::<u64>()?;
        }
    }
    Ok(deleted)
}
fn decode(inputs: &[Input], output: &Path) -> Result<Report> {
    let mut report = Report::default();
    let keys = identities(inputs, &mut report);
    for (index, input) in inputs.iter().enumerate() {
        let scope = format!("{}-{index}", input.kind.name());
        if input.kind == Kind::Receiver {
            if receiver_frames(&input.path, output, &scope, &keys, &mut report).is_err() {
                report.failure(
                    &scope,
                    Path::new(""),
                    "unsupported_receiver_register_or_storage",
                );
            }
            if receiver_snapshots(&input.path, output, &scope, &keys, &mut report).is_err() {
                report.failure(
                    &scope,
                    Path::new("snapshots"),
                    "unsupported_snapshot_storage",
                );
            }
        }
        for relative in files(&input.path)? {
            let path = if relative.as_os_str().is_empty() {
                input.path.clone()
            } else {
                input.path.join(&relative)
            };
            if input.kind == Kind::Config
                && relative.starts_with("snapshot-outbox")
                && path.extension().is_some_and(|ext| ext == "json")
            {
                let result = (|| -> Result<()> {
                    // The pinned outbox wrapper is unversioned. Never silently
                    // interpret a newer declared format through its old parser.
                    let value = read_json(&path, 64 * 1024 * 1024)?;
                    if value.as_object().is_none_or(|object| {
                        object
                            .keys()
                            .any(|key| !matches!(key.as_str(), "manifest" | "blobs"))
                    }) {
                        return Err("unsupported pending snapshot wrapper".into());
                    }
                    let packet: SnapshotPacket = load_snapshot_packet(&path)?;
                    let stem = path
                        .file_stem()
                        .and_then(|v| v.to_str())
                        .ok_or("invalid pending snapshot filename")?;
                    if stem != "pending"
                        && (identifier(stem).is_err() || packet.manifest["root"] != stem)
                    {
                        return Err("pending snapshot root/filename mismatch".into());
                    }
                    if pending_snapshot_deleted(inputs, &packet.manifest)? {
                        report
                            .unavailable
                            .push(json!({"input":scope,"path":relative,
                            "reason":"pending_snapshot_shadowed_by_receiver_deletion_state"}));
                        return Ok(());
                    }
                    // Verify all pending blobs, including unreferenced leftovers.
                    for (id, cipher) in &packet.blobs {
                        let bytes = decrypt_blob(cipher, id, &keys)?;
                        let target = Path::new(&scope).join("blobs").join(&relative).join(id);
                        write(&output.join(&target), &bytes)?;
                        report.blobs.push(json!({"input":scope,"source_path":relative,"root":packet.manifest["root"],
                            "id":id,"bytes":bytes.len(),"output_path":target}));
                    }
                    snapshot(
                        &packet.manifest,
                        &packet.blobs,
                        output,
                        &scope,
                        &relative,
                        &keys,
                        &mut report,
                    )
                })();
                if result.is_err() {
                    report.failure(
                        &scope,
                        &relative,
                        "pending_snapshot_recipient_or_integrity_failure",
                    );
                }
            } else if input.kind == Kind::Sender {
                if sender_metadata(&input.path, &relative).is_err() {
                    report.failure(
                        &scope,
                        &relative,
                        "unsupported_sender_state_or_deletion_queue",
                    );
                }
            } else if input.kind == Kind::Config {
                let checked = match relative.to_str() {
                    Some(AGE_IDENTITY_FILE) => true, // Checked with all declared recipient files.
                    Some(SIGNING_KEY_FILE) => load_signing_key(&path).is_ok(),
                    Some(RECIPIENTS_FILE) => load_recipients(&path).is_ok(),
                    Some(MACHINES_FILE) => load_machines(&path).is_ok(),
                    _ => false,
                };
                if !checked {
                    report.failure(
                        &scope,
                        &relative,
                        "unsupported_config_record_preserved_in_custody",
                    );
                }
            } else if input.kind == Kind::Receiver
                && !relative.starts_with("snapshots")
                && frame_path(&relative).ok().flatten().is_none()
            {
                match receiver_metadata(&input.path, &relative) {
                    Ok(true) => {}
                    Ok(false) => report.failure(
                        &scope,
                        &relative,
                        "unclassified_receiver_record_preserved_in_custody",
                    ),
                    Err(_) => report.failure(&scope, &relative, "unsupported_receiver_metadata"),
                }
            }
            report.metadata.push(json!({"input":scope,"source_path":relative,"custody_path":format!("custody/inputs/{scope}"),
                "preserved":true}));
        }
    }
    Ok(report)
}

fn run(custody: &Path, output: &Path) -> Result<bool> {
    let parent = fs::canonicalize(
        output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let output = parent.join(
        output
            .file_name()
            .ok_or("new output directory name required")?,
    );
    let custody_root = fs::canonicalize(custody)?;
    if output.starts_with(&custody_root) || output.exists() {
        return Err("output must be new and outside the custody artifact".into());
    }
    let (report, custody_digest) = with_verified_inputs(custody, |inputs| {
        let custody_digest = digest(&fs::read(custody_root.join("manifest.json"))?);
        DirBuilder::new().mode(0o700).create(&output)?;
        semon_relay_export::export(inputs, &output.join("custody"))?;
        // Old readers can mutate modes/metadata; only open this new private copy.
        let working: Vec<_> = inputs
            .iter()
            .enumerate()
            .map(|(index, input)| Input {
                kind: input.kind,
                path: output
                    .join("custody/inputs")
                    .join(format!("{}-{index}", input.kind.name())),
            })
            .collect();
        let report = decode(&working, &output)?;
        sync_directories(&output)?;
        Ok((report, custody_digest))
    })?;
    let complete = report.failures.is_empty();
    let manifest = json!({"format":"semon.relay-history-export","version":1,"legacy_source":LEGACY_SOURCE,
        "custody_manifest_sha256":custody_digest,
        "scope":"all declared bytes preserved; all supported present historical payloads inspected",
        "decryption_verified":complete,"verification_complete":complete,"recovery_verified":false,
        "may_include_private_keys":true,"frames":report.frames,"snapshots":report.snapshots,
        "blobs":report.blobs,"unavailable":report.unavailable,"metadata":report.metadata,"failures":report.failures});
    write(
        &output.join("manifest.json.partial"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::rename(
        output.join("manifest.json.partial"),
        output.join("manifest.json"),
    )?;
    File::open(&output)?.sync_all()?;
    File::open(parent)?.sync_all()?;
    Ok(complete)
}
fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let mut custody = None;
    let mut output = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help") if custody.is_none() && output.is_none() && args.next().is_none() => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Some("--custody" | "--out") => {
                let target = if arg == "--custody" {
                    &mut custody
                } else {
                    &mut output
                };
                let Some(value) = args
                    .next()
                    .filter(|v| !v.is_empty() && !v.to_string_lossy().starts_with("--"))
                else {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                };
                if target.replace(PathBuf::from(value)).is_some() {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            }
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(custody), Some(output)) = (custody, output) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    eprintln!(
        "Private historical export includes plaintext evidence, credentials, keys, metadata and ciphertext copies. Originals are never restored or modified."
    );
    match run(&custody, &output) {
        Ok(true) => {
            println!("Private historical export verified. This is not recovery qualification.");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            eprintln!(
                "Private report records unavailable verification, gaps, unsupported records or integrity failures; inspect it before retiring access."
            );
            ExitCode::from(1)
        }
        Err(_) => {
            eprintln!(
                "Historical export refused or failed; originals remain intact. Any new output is private and incomplete."
            );
            ExitCode::from(1)
        }
    }
}
