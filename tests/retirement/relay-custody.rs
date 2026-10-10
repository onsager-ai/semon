//! Actual pinned receiver/crypto/snapshot APIs; disposable synthetic data only.
use std::{
    collections::BTreeMap,
    fs,
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

fn frame(
    key: &DataKey,
    machine: &str,
    session: &str,
    generation: u64,
    epoch: u64,
    seq: u64,
    previous: &[u8; 32],
    line: &[u8],
) -> Frame {
    let frame_key = FrameKey {
        session: session.into(),
        stream: "main".into(),
        generation,
        epoch,
        seq,
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
            0,
            0,
            0,
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
            0,
            1,
            1,
            &chain_line(&ZERO_CHAIN, lines[0]),
            lines[1],
        ))
        .unwrap();
    receiver
        .accept(&frame(
            &key,
            &machine,
            "session",
            1,
            1,
            0,
            &ZERO_CHAIN,
            lines[2],
        ))
        .unwrap();
    receiver
        .accept_orphan(&frame(
            &key,
            &machine,
            "session",
            0,
            0,
            1,
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
            0,
            0,
            0,
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
    let pending = capture(b"pending", Some(parent), 4, &[recovery.to_public()]);
    let (server, transport) = Server::start(&receiver_root, &identity);
    for packet in [&first, &fork_a, &fork_b] {
        publish_snapshot(packet, &transport).unwrap();
    }
    drop(server);
    drop(transport);
    let outbox = config.join("snapshot-outbox/synthetic");
    fs::create_dir_all(&outbox).unwrap();
    persist_snapshot_packet(&outbox.join("pending.json"), &pending).unwrap();
    fs::write(
        outbox.join("head.json"),
        serde_json::to_vec(&fork_a.manifest).unwrap(),
    )
    .unwrap();
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
    let restored = load_snapshot_packet(
        &working.join("inputs/config-1/snapshot-outbox/synthetic/pending.json"),
    )
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
