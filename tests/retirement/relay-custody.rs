//! Actual pinned receiver/crypto/snapshot APIs; disposable synthetic data only.
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    net::TcpListener,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use age::{secrecy::ExposeSecret, x25519};
use semon_relay::*;
use semon_relay_export::{Input, Kind};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("relay-pinned-readback-{}", std::process::id()));
        fs::DirBuilder::new().create(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Server(Child);

fn export(inputs: &[Input], destination: &Path) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_custody-exporter"));
    for input in inputs {
        command
            .arg(match input.kind {
                Kind::Receiver => "--receiver-dir",
                Kind::Config => "--config-dir",
                Kind::Sender => "--sender-dir",
                Kind::RecoveryKey => "--recovery-key",
            })
            .arg(&input.path);
    }
    let output = command.arg("--out").arg(destination).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("age/signing keys"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Decryption is not verified"));
}
impl Server {
    fn start(root: &Path, identity: &MachineIdentity) -> (Self, HttpTransport) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let child = Command::new(env!("CARGO_BIN_EXE_pinned-relay"))
            .args(["receive", "--listen", &address.to_string(), "--dir"])
            .arg(root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut server = Self(child);
        let transport = HttpTransport::secure(
            format!("http://{address}"),
            Duration::from_secs(2),
            RequestSigner::new(identity.signing.clone()),
            None,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if transport
                .lease_status(None, &identity.fingerprint())
                .is_ok()
            {
                return (server, transport);
            }
            assert!(
                server.0.try_wait().unwrap().is_none(),
                "synthetic receiver exited"
            );
            assert!(
                Instant::now() < deadline,
                "synthetic receiver did not start"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        self.0.wait().unwrap();
    }
}

type Tree = BTreeMap<PathBuf, (u32, u64, i64, i64, Vec<u8>)>;
fn tree(root: &Path) -> Tree {
    let mut result = BTreeMap::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let path = if relative.as_os_str().is_empty() {
            root.to_owned()
        } else {
            root.join(&relative)
        };
        let metadata = fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            for entry in fs::read_dir(&path).unwrap() {
                pending.push(relative.join(entry.unwrap().file_name()));
            }
        }
        result.insert(
            relative,
            (
                metadata.mode(),
                metadata.ino(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                if metadata.is_file() {
                    fs::read(path).unwrap()
                } else {
                    vec![]
                },
            ),
        );
    }
    result
}

fn inputs(custody: &Path) -> Vec<Input> {
    [
        Kind::Receiver,
        Kind::Config,
        Kind::Sender,
        Kind::RecoveryKey,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, kind)| Input {
        kind,
        path: custody
            .join("inputs")
            .join(format!("{}-{index}", kind.name())),
    })
    .collect()
}

fn history(custody: &Path, output: &Path, complete: bool) -> Value {
    let before = tree(custody);
    let result = Command::new(history_binary())
        .arg("--custody")
        .arg(custody)
        .arg("--out")
        .arg(output)
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(if complete { 0 } else { 1 }),
        "{}: {}",
        output.display(),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("plaintext evidence, credentials, keys")
    );
    assert!(!String::from_utf8_lossy(&result.stdout).contains("AGE-SECRET-KEY"));
    assert!(!String::from_utf8_lossy(&result.stderr).contains("AGE-SECRET-KEY"));
    assert_eq!(tree(custody), before);
    let report: Value =
        serde_json::from_slice(&fs::read(output.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(report["format"], "semon.relay-history-export");
    assert_eq!(report["version"], 1);
    assert_eq!(report["legacy_source"], semon_relay_export::LEGACY_SOURCE);
    assert_eq!(
        report["custody_manifest_sha256"],
        format!(
            "{:x}",
            Sha256::digest(fs::read(custody.join("manifest.json")).unwrap())
        )
    );
    assert_eq!(report["decryption_verified"], complete);
    assert_eq!(report["verification_complete"], complete);
    assert_eq!(report["recovery_verified"], false);
    assert_eq!(report["failures"].as_array().unwrap().is_empty(), complete);
    for (path, (mode, _, _, _, _)) in tree(output) {
        let expected = if output.join(path).is_dir() {
            0o700
        } else {
            0o600
        };
        assert_eq!(mode & 0o777, expected);
    }
    report
}
fn history_binary() -> std::ffi::OsString {
    std::env::var_os("SEMON_RELAY_HISTORY_BINARY")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_semon-relay-history").into())
}

fn failure(report: &Value, reason: &str) -> bool {
    report["failures"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["reason"] == reason)
}

fn qualify_history(
    custody: &Path,
    scratch: &Path,
    root: &str,
    lines: &[&[u8]],
    packets: &[&SnapshotPacket],
) {
    let recovery = load_age_identity(&custody.join("inputs/recovery-key-3")).unwrap();
    for (args, code) in [
        (vec!["--help"], 0),
        (vec![], 2),
        (vec!["--custody", "--out", "unused"], 2),
        (
            vec![
                "--custody",
                "first",
                "--custody",
                "second",
                "--out",
                "unused",
            ],
            2,
        ),
        (
            vec!["--custody", "first", "--out", "unused", "--network"],
            2,
        ),
    ] {
        assert_eq!(
            Command::new(history_binary())
                .args(args)
                .output()
                .unwrap()
                .status
                .code(),
            Some(code)
        );
    }
    // Unknown receiver bytes remain in custody and must block full verification.
    let partial = scratch.join("history-unknown");
    let report = history(custody, &partial, false);
    assert!(failure(
        &report,
        "unclassified_receiver_record_preserved_in_custody"
    ));
    assert_eq!(
        fs::read(partial.join("custody/inputs/receiver-0/future-unrecognized-record")).unwrap(),
        b"opaque future bytes"
    );

    // Remove only the synthetic unknown record on a private disposable copy.
    let known = scratch.join("known-inputs");
    export(&inputs(custody), &known);
    fs::remove_file(known.join("inputs/receiver-0/future-unrecognized-record")).unwrap();
    let recovery_file = known.join("inputs/recovery-key-3");
    let original_key = fs::read_to_string(&recovery_file).unwrap();
    fs::write(
        &recovery_file,
        format!(
            "# original age recovery identities\n\n{}\n{}\n",
            x25519::Identity::generate().to_string().expose_secret(),
            original_key
        ),
    )
    .unwrap();
    let artifact = scratch.join("known-custody");
    export(&inputs(&known), &artifact);
    let output = scratch.join("history-complete");
    let report = history(&artifact, &output, true);
    assert_eq!(report["frames"].as_array().unwrap().len(), 5);
    assert_eq!(report["snapshots"].as_array().unwrap().len(), 4);
    assert_eq!(
        report["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["input"] == "receiver-0" && m["fork_head"] == true)
            .count(),
        2
    );
    let scopes = [
        (false, 0, 0, 0),
        (false, 0, 1, 1),
        (false, 1, 1, 0),
        (true, 0, 0, 1),
    ];
    for (line, (orphan, generation, epoch, seq)) in lines.iter().zip(scopes) {
        let record = report["frames"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| {
                f["session"] == "session"
                    && f["orphan"] == orphan
                    && f["generation"] == generation
                    && f["epoch"] == epoch
                    && f["seq"] == seq
            })
            .unwrap();
        assert_eq!(record["continuity_verified"], true);
        assert_eq!(
            fs::read(output.join(record["output_path"].as_str().unwrap())).unwrap(),
            *line
        );
    }
    let plain = report["frames"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["session"] == "plaintext")
        .unwrap();
    assert_eq!(plain["encrypted"], false);
    assert_eq!(plain["continuity_verified"], true);
    assert_eq!(
        fs::read(output.join(plain["output_path"].as_str().unwrap())).unwrap(),
        b"plaintext evidence\n"
    );
    let leftover = report["blobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|blob| {
            blob["input"] == "config-1"
                && blob["id"] == format!("{:x}", Sha256::digest(b"pending-unreferenced"))
        })
        .unwrap();
    assert_eq!(
        fs::read(output.join(leftover["output_path"].as_str().unwrap())).unwrap(),
        b"pending-unreferenced"
    );
    for (packet, bytes) in
        packets
            .iter()
            .zip([b"first".as_slice(), b"fork-a", b"fork-b", b"pending"])
    {
        let record = report["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == packet.manifest["id"])
            .unwrap();
        assert_eq!(
            fs::read(
                output
                    .join(record["output_path"].as_str().unwrap())
                    .join("files/nested.txt")
            )
            .unwrap(),
            bytes
        );
    }
    assert!(
        report["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["reason"] == "explicitly_deleted_body")
    );
    for input in inputs(&artifact) {
        let private = output
            .join("custody/inputs")
            .join(input.path.file_name().unwrap());
        // Compare byte inventory, including pending deletion state and all keys.
        let bytes = |root: &Path| {
            tree(root)
                .into_iter()
                .map(|(p, (_, _, _, _, b))| (p, b))
                .collect::<BTreeMap<_, _>>()
        };
        assert_eq!(bytes(&input.path), bytes(&private));
    }
    let nested = scratch.join("nested-state-inputs");
    export(&inputs(&artifact), &nested);
    let sender = nested.join("inputs/sender-2");
    fs::create_dir_all(sender.join("custom/nested")).unwrap();
    fs::rename(
        sender.join("relay.json"),
        sender.join("custom/nested/state-without-extension"),
    )
    .unwrap();
    fs::rename(
        sender.join("relay.json.forget"),
        sender.join("custom/nested/state-without-extension.forget"),
    )
    .unwrap();
    let nested_custody = scratch.join("nested-state-custody");
    export(&inputs(&nested), &nested_custody);
    history(&nested_custody, &scratch.join("nested-state-output"), true);

    let live = PathBuf::from(format!(
        "{}/{}/generation-0/epoch-0/frames/00000000000000000000.json",
        hex::encode("session"),
        hex::encode("main")
    ));
    let manifest_path = PathBuf::from("snapshots")
        .join(root)
        .join("manifests")
        .join(packets[2].manifest["id"].as_str().unwrap());
    let blob_path = PathBuf::from("snapshots")
        .join(root)
        .join("blobs")
        .join(packets[2].blobs.keys().next().unwrap());
    for case in [
        "wrong-recovery-key",
        "missing-recovery-key",
        "gap",
        "substitution",
        "blob-corrupt",
        "manifest-substitution",
        "state-version",
        "state-malformed",
        "outbox-version",
        "unknown-key-file",
        "missing-parent",
        "missing-checkpoint-tail",
        "invalid-deletion-queue",
        "snapshot-version",
        "snapshot-path",
    ] {
        let mutated = scratch.join(format!("history-inputs-{case}"));
        export(&inputs(&artifact), &mutated);
        let receiver = mutated.join("inputs/receiver-0");
        let mut selected = inputs(&mutated);
        match case {
            "wrong-recovery-key" => fs::write(
                mutated.join("inputs/recovery-key-3"),
                x25519::Identity::generate().to_string().expose_secret(),
            )
            .unwrap(),
            "missing-recovery-key" => {
                selected.pop();
            }
            "gap" => fs::remove_file(receiver.join(&live)).unwrap(),
            "missing-checkpoint-tail" => {
                let path = receiver.join(format!(
                    "{}/{}/generation-0/state.json",
                    hex::encode("deleted"),
                    hex::encode("main")
                ));
                let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                value["acked"] = json!(999);
                fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "invalid-deletion-queue" => {
                let queue = mutated.join("inputs/sender-2/relay.json.forget");
                let file = tree(&queue)
                    .into_iter()
                    .find(|(path, _)| path.extension().is_some_and(|v| v == "json"))
                    .unwrap()
                    .0;
                let path = queue.join(file);
                let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                value["id"] = json!("00".repeat(32));
                fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "substitution" => {
                let path = receiver.join(&live);
                let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                value["generation"] = json!(20);
                fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "blob-corrupt" => {
                let path = receiver.join(&blob_path);
                let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                value["ciphertext"] = json!("00");
                value["digest"] = json!(format!("{:x}", Sha256::digest(b"00")));
                fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "manifest-substitution" => {
                let path = receiver.join(&manifest_path);
                let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                value["root"] = json!(snapshot_root_id("substitution"));
                fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "snapshot-version" | "snapshot-path" => {
                let path = receiver.join(&manifest_path);
                let mut wire: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                let mut clear = inspect_snapshot_manifest(&wire, &recovery).unwrap();
                if case == "snapshot-version" {
                    clear["version"] = json!(999);
                } else {
                    clear["entries"][0]["path"] = json!("../../escaped-private-file");
                }
                let bytes = serde_json::to_vec(&clear).unwrap();
                wire["id"] = json!(format!("{:x}", Sha256::digest(&bytes)));
                let recipient = recovery.to_public();
                let encryptor = age::Encryptor::with_recipients(std::iter::once(
                    &recipient as &dyn age::Recipient,
                ))
                .unwrap();
                let mut ciphertext = Vec::new();
                let mut writer = encryptor.wrap_output(&mut ciphertext).unwrap();
                writer.write_all(&bytes).unwrap();
                writer.finish().unwrap();
                wire["ciphertext"] = json!(hex::encode(ciphertext));
                fs::remove_file(&path).unwrap();
                fs::write(
                    path.parent().unwrap().join(wire["id"].as_str().unwrap()),
                    serde_json::to_vec(&wire).unwrap(),
                )
                .unwrap();
            }
            "state-version" => fs::write(
                mutated.join("inputs/sender-2/relay.json"),
                b"{\"version\":999,\"streams\":[]}",
            )
            .unwrap(),
            "state-malformed" => {
                fs::remove_file(mutated.join("inputs/sender-2/relay.json")).unwrap();
                fs::write(
                    mutated.join("inputs/sender-2/custom-state"),
                    b"{malformed-private-state",
                )
                .unwrap();
            }
            "outbox-version" => {
                let path = mutated.join(format!(
                    "inputs/config-1/snapshot-outbox/synthetic/{root}.json"
                ));
                let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                value["version"] = json!(999);
                fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
            "unknown-key-file" => {
                fs::write(receiver.join("keys/unrecognized.payload"), b"opaque key").unwrap()
            }
            "missing-parent" => fs::remove_file(
                receiver
                    .join("snapshots")
                    .join(root)
                    .join("manifests")
                    .join(packets[0].manifest["id"].as_str().unwrap()),
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let next_custody = scratch.join(format!("history-custody-{case}"));
        export(&selected, &next_custody);
        let report = history(
            &next_custody,
            &scratch.join(format!("history-output-{case}")),
            false,
        );
        let reason = match case {
            "wrong-recovery-key"
            | "missing-recovery-key"
            | "manifest-substitution"
            | "snapshot-version"
            | "snapshot-path" => "snapshot_manifest_recipient_header_or_integrity_failure",
            "gap" => "missing_prefix_gap_or_chain_mismatch",
            "substitution" => "frame_identity_decryption_or_integrity_failure",
            "blob-corrupt" => "snapshot_blob_recipient_checksum_or_format_failure",
            "state-version" | "state-malformed" | "invalid-deletion-queue" => {
                "unsupported_sender_state_or_deletion_queue"
            }
            "outbox-version" => "pending_snapshot_recipient_or_integrity_failure",
            "unknown-key-file" | "missing-checkpoint-tail" => "unsupported_receiver_metadata",
            "missing-parent" => "snapshot_parent_missing",
            _ => unreachable!(),
        };
        assert!(failure(&report, reason), "missing reason for {case}");
        assert!(!scratch.join("escaped-private-file").exists());
    }

    // Crash-interrupted deletions can leave a body beside its durable tombstone.
    for case in [
        "frame-tombstone",
        "snapshot-pruned",
        "snapshot-cutoff",
        "snapshot-invalid-cutoff",
        "pending-pruned",
        "pending-cutoff",
    ] {
        let mutated = scratch.join(format!("history-inputs-{case}"));
        export(&inputs(&artifact), &mutated);
        let receiver = mutated.join("inputs/receiver-0");
        let base = receiver.join("snapshots").join(root);
        if case == "frame-tombstone" {
            let target = receiver.join(format!(
                "{}/{}/generation-0/epoch-0/frames/00000000000000000000.json",
                hex::encode("deleted"),
                hex::encode("main")
            ));
            // Real original-format ciphertext, with matching body-free receipt.
            fs::copy(receiver.join(&live), target).unwrap();
        } else if case == "snapshot-pruned" || case == "pending-pruned" {
            let packet = if case == "pending-pruned" {
                packets[3]
            } else {
                packets[2]
            };
            fs::write(
                base.join("pruned")
                    .join(packet.manifest["id"].as_str().unwrap()),
                b"forgotten",
            )
            .unwrap();
        } else {
            fs::write(
                base.join("forget-before"),
                if case == "pending-cutoff" {
                    b"5".as_slice()
                } else if case == "snapshot-cutoff" {
                    b"4".as_slice()
                } else {
                    b"invalid-cutoff"
                },
            )
            .unwrap();
        }
        let next_custody = scratch.join(format!("history-custody-{case}"));
        export(&inputs(&mutated), &next_custody);
        let output = scratch.join(format!("history-output-{case}"));
        let report = history(&next_custody, &output, case != "snapshot-invalid-cutoff");
        let reason = match case {
            "frame-tombstone" => "body_shadowed_by_delete_receipt",
            "pending-pruned" | "pending-cutoff" => {
                "pending_snapshot_shadowed_by_receiver_deletion_state"
            }
            _ => "snapshot_shadowed_by_deletion_state",
        };
        assert!(
            report["unavailable"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["reason"] == reason)
        );
        if case.starts_with("pending-") {
            assert!(
                !output
                    .join(format!(
                        "config-1/snapshots/snapshot-outbox/synthetic/{root}.json"
                    ))
                    .exists()
            );
            assert!(
                output
                    .join(format!(
                        "custody/inputs/config-1/snapshot-outbox/synthetic/{root}.json"
                    ))
                    .exists()
            );
        }
        if case != "frame-tombstone" && case != "pending-pruned" {
            assert!(
                !output
                    .join("receiver-0/snapshots")
                    .join(&manifest_path)
                    .exists()
            );
            assert!(
                !output
                    .join("receiver-0/blobs")
                    .join(root)
                    .join(packets[2].blobs.keys().next().unwrap())
                    .exists()
            );
            assert!(
                output
                    .join("custody/inputs/receiver-0")
                    .join(&manifest_path)
                    .exists()
            );
        }
    }

    for case in ["hash", "extra", "missing", "version", "path"] {
        let invalid = scratch.join(format!("history-invalid-{case}"));
        export(&inputs(&artifact), &invalid);
        match case {
            "hash" => {
                fs::write(invalid.join("inputs/receiver-0").join(&live), b"tampered").unwrap()
            }
            "extra" => fs::write(invalid.join("undeclared"), b"extra").unwrap(),
            "missing" => fs::remove_file(invalid.join("inputs/receiver-0").join(&live)).unwrap(),
            _ => {
                let path = invalid.join("manifest.json");
                let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                if case == "version" {
                    value["version"] = json!(999);
                } else {
                    value["inputs"][0]["entries"][1]["path"] = json!("../../escape");
                }
                fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
        }
        let before = tree(&invalid);
        let output = scratch.join(format!("history-refused-{case}"));
        let result = Command::new(history_binary())
            .arg("--custody")
            .arg(&invalid)
            .arg("--out")
            .arg(&output)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(
            !output.exists(),
            "invalid custody must fail before output creation: {case}"
        );
        assert_eq!(tree(&invalid), before);
    }
}

fn frame(
    key: &DataKey,
    machine: &str,
    session: &str,
    position: (u64, u64, u64),
    previous: &[u8; 32],
    line: &[u8],
) -> Frame {
    let frame_key = FrameKey {
        session: session.into(),
        stream: "main".into(),
        generation: position.0,
        epoch: position.1,
        seq: position.2,
    };
    Frame {
        content: FrameContent::Encrypted(
            encrypt_frame(key, &frame_key, machine, &chain_line(previous, line), line).unwrap(),
        ),
        key: frame_key,
        machine: machine.into(),
        sender_wall_ns: 10,
        sender_mono_ns: 10,
        boot_id: "synthetic".into(),
    }
}

#[test]
fn complete_custody_and_pinned_readback_cover_retained_frames_orphans_forks_and_pending_state() {
    let scratch = Scratch::new();
    let receiver_root = scratch.0.join("receiver");
    let config = scratch.0.join("config");
    let sender = scratch.0.join("sender");
    let recovery_path = scratch.0.join("recovery.key");
    init(&config).unwrap();
    let identity = MachineIdentity::load(&config).unwrap();
    let recovery = x25519::Identity::generate();
    fs::write(&recovery_path, recovery.to_string().expose_secret()).unwrap();
    fs::set_permissions(&recovery_path, fs::Permissions::from_mode(0o600)).unwrap();
    enroll_recipient(&config, &identity.age.to_public().to_string(), "synthetic").unwrap();
    let receiver = Receiver::open(&receiver_root).unwrap();
    let machine = enroll_machine(
        &receiver_root,
        &hex::encode(identity.signing.verifying_key().as_bytes()),
        "synthetic",
    )
    .unwrap();
    let key = generate_data_key();
    receiver.acquire("session", &machine).unwrap();
    receiver
        .put_envelope(
            "session",
            &machine,
            &encrypt_envelope(&key, &[identity.age.to_public(), recovery.to_public()]).unwrap(),
            false,
            false,
        )
        .unwrap();
    let lines = [
        b"{\"event\":\"retained\"}\n".as_slice(),
        b"{\"event\":\"next-epoch\"}\n".as_slice(),
        b"{\"event\":\"rewrite\"}\n".as_slice(),
        b"{\"event\":\"orphan\"}\n".as_slice(),
    ];
    receiver
        .accept(&frame(
            &key,
            &machine,
            "session",
            (0, 0, 0),
            &ZERO_CHAIN,
            lines[0],
        ))
        .unwrap();
    receiver.takeover("session", &machine, 0, true).unwrap();
    receiver
        .accept(&frame(
            &key,
            &machine,
            "session",
            (0, 1, 1),
            &chain_line(&ZERO_CHAIN, lines[0]),
            lines[1],
        ))
        .unwrap();
    receiver
        .accept(&frame(
            &key,
            &machine,
            "session",
            (1, 1, 0),
            &ZERO_CHAIN,
            lines[2],
        ))
        .unwrap();
    receiver
        .accept_orphan(&frame(
            &key,
            &machine,
            "session",
            (0, 0, 1),
            &chain_line(&ZERO_CHAIN, lines[0]),
            lines[3],
        ))
        .unwrap();
    receiver.acquire("deleted", &machine).unwrap();
    receiver
        .put_envelope(
            "deleted",
            &machine,
            &encrypt_envelope(&key, &[identity.age.to_public()]).unwrap(),
            false,
            false,
        )
        .unwrap();
    receiver
        .accept(&frame(
            &key,
            &machine,
            "deleted",
            (0, 0, 0),
            &ZERO_CHAIN,
            b"deleted-private-line\n",
        ))
        .unwrap();
    receiver
        .forget(
            &"ab".repeat(32),
            &ForgetSelector {
                session: Some("deleted".into()),
                ..Default::default()
            },
        )
        .unwrap();
    receiver.acquire("plaintext", &machine).unwrap();
    let plain = b"plaintext evidence\n";
    receiver
        .accept(&Frame::plaintext(
            FrameKey {
                session: "plaintext".into(),
                stream: "main".into(),
                generation: 0,
                epoch: 0,
                seq: 0,
            },
            machine.clone(),
            chain_line(&ZERO_CHAIN, plain),
            10,
            10,
            "synthetic".into(),
            plain.to_vec(),
        ))
        .unwrap();
    drop(receiver);

    let memory = scratch.0.join("memory");
    fs::create_dir(&memory).unwrap();
    let root_id = snapshot_root_id("synthetic-root");
    let capture = |value: &[u8], parent: Option<&str>, wall, recipients: &[x25519::Recipient]| {
        fs::write(memory.join("nested.txt"), value).unwrap();
        capture_snapshot(
            &memory,
            &root_id,
            &machine,
            parent,
            wall,
            wall,
            "synthetic",
            None,
            recipients,
        )
        .unwrap()
    };
    let first = capture(
        b"first",
        None,
        1,
        &[identity.age.to_public(), recovery.to_public()],
    );
    let parent = first.manifest["id"].as_str().unwrap();
    let fork_a = capture(
        b"fork-a",
        Some(parent),
        2,
        &[identity.age.to_public(), recovery.to_public()],
    );
    let fork_b = capture(b"fork-b", Some(parent), 3, &[recovery.to_public()]);
    let mut pending = capture(b"pending", Some(parent), 4, &[recovery.to_public()]);
    // The real outbox format permits unreferenced retained blobs as well.
    pending
        .blobs
        .extend(capture(b"pending-unreferenced", None, 5, &[recovery.to_public()]).blobs);
    let (server, transport) = Server::start(&receiver_root, &identity);
    for packet in [&first, &fork_a, &fork_b] {
        publish_snapshot(packet, &transport).unwrap();
    }
    drop(server);
    drop(transport);
    let outbox = config.join("snapshot-outbox/synthetic");
    fs::create_dir_all(&outbox).unwrap();
    persist_snapshot_packet(&outbox.join(format!("{root_id}.json")), &pending).unwrap();
    fs::create_dir(&sender).unwrap();
    let mut sender_state = RelayState::default();
    sender_state.streams.insert(
        ("session".into(), "main".into()),
        StreamState::new(PathBuf::from("/synthetic/native.jsonl")),
    );
    save_state(&sender.join("relay.json"), &sender_state).unwrap();
    queue_forget(
        &sender.join("relay.json"),
        "local",
        &ForgetSelector {
            session: Some("pending-delete".into()),
            ..Default::default()
        },
    )
    .unwrap();
    fs::write(
        receiver_root.join("future-unrecognized-record"),
        b"opaque future bytes",
    )
    .unwrap();
    fs::create_dir(receiver_root.join("empty-retained-directory")).unwrap();

    let originals = [&receiver_root, &config, &sender, &recovery_path].map(|root| tree(root));
    let destination = scratch.0.join("custody");
    export(
        &[
            Input {
                kind: Kind::Receiver,
                path: receiver_root.clone(),
            },
            Input {
                kind: Kind::Config,
                path: config.clone(),
            },
            Input {
                kind: Kind::Sender,
                path: sender.clone(),
            },
            Input {
                kind: Kind::RecoveryKey,
                path: recovery_path.clone(),
            },
        ],
        &destination,
    );
    assert_eq!(
        originals,
        [&receiver_root, &config, &sender, &recovery_path].map(|root| tree(root))
    );
    let custody_before = tree(&destination);
    qualify_history(
        &destination,
        &scratch.0,
        &root_id,
        &lines,
        &[&first, &fork_a, &fork_b, &pending],
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(destination.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["decryption_verified"], false);
    assert_eq!(
        manifest["legacy_source"],
        std::env::var("SEMON_RELAY_QUALIFIED_SOURCE").unwrap()
    );
    for input in manifest["inputs"].as_array().unwrap() {
        for entry in input["entries"].as_array().unwrap() {
            let base = destination.join(input["artifact"].as_str().unwrap());
            let relative = entry["path"].as_str().unwrap();
            let path = if relative.is_empty() {
                base
            } else {
                base.join(relative)
            };
            if entry["kind"] == "file" {
                let bytes = fs::read(&path).unwrap();
                assert_eq!(entry["sha256"], format!("{:x}", Sha256::digest(&bytes)));
                assert_eq!(entry["bytes"].as_u64().unwrap(), bytes.len() as u64);
            } else {
                assert!(path.is_dir());
            }
        }
    }

    // Historical Receiver::open and snapshot routes can write metadata/modes.
    // Work exclusively on a separately exported disposable working copy.
    let working = scratch.0.join("working");
    export(
        &[
            Input {
                kind: Kind::Receiver,
                path: destination.join("inputs/receiver-0"),
            },
            Input {
                kind: Kind::Config,
                path: destination.join("inputs/config-1"),
            },
            Input {
                kind: Kind::Sender,
                path: destination.join("inputs/sender-2"),
            },
            Input {
                kind: Kind::RecoveryKey,
                path: destination.join("inputs/recovery-key-3"),
            },
        ],
        &working,
    );
    let working_receiver = working.join("inputs/receiver-0");
    let working_identity = MachineIdentity::load(&working.join("inputs/config-1")).unwrap();
    let recovery = load_age_identity(&working.join("inputs/recovery-key-3")).unwrap();
    assert_eq!(
        load_state(&working.join("inputs/sender-2/relay.json")).unwrap(),
        sender_state
    );
    struct Offline;
    impl Transport for Offline {
        fn send(&self, _: &Frame) -> Result<u64, TransportError> {
            panic!("historical qualification never sends session frames");
        }
    }
    let queue = working.join("inputs/sender-2/relay.json.forget");
    let queue_before = tree(&queue);
    let pending_delete = flush_forgets(
        &working.join("inputs/sender-2/relay.json"),
        &machine,
        &Offline,
    )
    .unwrap();
    assert_eq!(pending_delete.pending, 1);
    assert_eq!(pending_delete.acknowledged, 0);
    assert_eq!(tree(&queue), queue_before);
    let reader = Receiver::open(&working_receiver).unwrap();
    let envelope = reader.get_envelope("session").unwrap().unwrap();
    let decrypted_key = decrypt_envelope(&envelope, &working_identity.age).unwrap();
    assert_eq!(
        decrypted_key,
        decrypt_envelope(&envelope, &recovery).unwrap()
    );
    assert!(decrypt_envelope(&envelope, &x25519::Identity::generate()).is_err());
    let frames = reader.list_frames("session").unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(
        verify_encrypted_frames("session", frames.clone(), &decrypted_key)
            .unwrap()
            .len(),
        2
    );
    for (frame, expected) in frames.iter().zip(&lines[..3]) {
        let (_, bytes) = decrypt_frame(
            &decrypted_key,
            &frame.key,
            &frame.machine,
            frame.encrypted_payload().unwrap(),
        )
        .unwrap();
        assert_eq!(bytes, *expected);
        let mut substituted = frame.key.clone();
        substituted.generation += 1;
        assert!(
            decrypt_frame(
                &decrypted_key,
                &substituted,
                &frame.machine,
                frame.encrypted_payload().unwrap()
            )
            .is_err()
        );
    }
    assert_eq!(reader.list_orphans().unwrap().len(), 1);
    let orphan = reader
        .read_orphan("session", "main", 0, 0, 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        decrypt_frame(
            &decrypted_key,
            &orphan.key,
            &orphan.machine,
            orphan.encrypted_payload().unwrap()
        )
        .unwrap()
        .1,
        lines[3]
    );
    assert!(
        reader
            .read_frame("deleted", "main", 0, 0, 0)
            .unwrap()
            .is_none()
    );
    assert_eq!(reader.lease_status(None).unwrap().takeovers.len(), 1);
    let mut missing = frames.clone();
    missing.remove(0);
    assert!(verify_encrypted_frames("session", missing, &decrypted_key).is_err());
    let mut corrupt = frames;
    if let FrameContent::Encrypted(payload) = &mut corrupt[0].content {
        payload.ciphertext[0] ^= 1;
    }
    assert!(verify_encrypted_frames("session", corrupt, &decrypted_key).is_err());
    drop(reader);

    let (server, transport) = Server::start(&working_receiver, &working_identity);
    assert_eq!(
        list_snapshot_roots(&machine, &transport).unwrap(),
        vec![root_id.clone()]
    );
    let history = list_snapshot_history(&root_id, &machine, &transport).unwrap();
    assert_eq!(history["manifests"].as_array().unwrap().len(), 3);
    assert_eq!(history["heads"].as_array().unwrap().len(), 2);
    for (packet, expected) in [
        (&first, b"first".as_slice()),
        (&fork_a, b"fork-a".as_slice()),
        (&fork_b, b"fork-b".as_slice()),
    ] {
        inspect_snapshot_manifest(&packet.manifest, &recovery).unwrap();
        let id = packet.manifest["id"].as_str().unwrap();
        assert_eq!(
            read_snapshot_file(&root_id, id, "nested.txt", &machine, &recovery, &transport)
                .unwrap()
                .unwrap(),
            expected
        );
    }
    assert!(
        read_snapshot_file(
            &root_id,
            fork_b.manifest["id"].as_str().unwrap(),
            "nested.txt",
            &machine,
            &working_identity.age,
            &transport
        )
        .is_err()
    );
    let restored = load_snapshot_packet(&working.join(format!(
        "inputs/config-1/snapshot-outbox/synthetic/{root_id}.json"
    )))
    .unwrap();
    assert_eq!(restored.to_value(), pending.to_value());
    inspect_snapshot_manifest(&restored.manifest, &recovery).unwrap();
    assert!(inspect_snapshot_manifest(&restored.manifest, &working_identity.age).is_err());
    // Exercise every pending blob through the unchanged receiver's publish/read
    // protocol on the disposable working copy, never on the custody artifact.
    publish_snapshot(&restored, &transport).unwrap();
    assert_eq!(
        read_snapshot_file(
            &root_id,
            restored.manifest["id"].as_str().unwrap(),
            "nested.txt",
            &machine,
            &recovery,
            &transport
        )
        .unwrap()
        .unwrap(),
        b"pending"
    );
    let mut substituted = first.manifest.clone();
    substituted["root"] = json!(snapshot_root_id("substitution"));
    assert!(inspect_snapshot_manifest(&substituted, &recovery).is_err());
    let blob = fork_b.blobs.keys().next().unwrap();
    let blob_path = working_receiver
        .join("snapshots")
        .join(&root_id)
        .join("blobs")
        .join(blob);
    let mut record: Value = serde_json::from_slice(&fs::read(&blob_path).unwrap()).unwrap();
    record["ciphertext"] = json!("00");
    fs::write(blob_path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(
        read_snapshot_file(
            &root_id,
            fork_b.manifest["id"].as_str().unwrap(),
            "nested.txt",
            &machine,
            &recovery,
            &transport
        )
        .is_err()
    );
    drop(server);
    fs::write(
        working.join("inputs/sender-2/relay.json"),
        b"{\"version\":999,\"streams\":[]}",
    )
    .unwrap();
    assert!(load_state(&working.join("inputs/sender-2/relay.json")).is_err());
    assert_eq!(tree(&destination), custody_before);
    assert_eq!(
        originals,
        [&receiver_root, &config, &sender, &recovery_path].map(|root| tree(root))
    );
}
