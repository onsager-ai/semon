//! Synthetic native sender proves checkpoint identity across real watcher repair.
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};
const OPERATION: &str = "11111111-1111-1111-1111-111111111111";
struct Home(tempfile::TempDir);
impl Drop for Home {
    fn drop(&mut self) {
        let path = self
            .0
            .path()
            .join(".local/state/semon-ssh")
            .join(OPERATION)
            .join("receipt");
        if let Ok(bytes) = fs::read(path)
            && let Ok(v) = serde_json::from_slice::<Value>(&bytes)
            && let Some(pid) = v["pid"].as_u64()
        {
            let _ = Command::new("/bin/kill")
                .args(["-TERM", "--", &format!("-{pid}")])
                .status();
        }
    }
}
fn setup(home: &Path, receiver: &str, token: &str) {
    let binary = br#"#!/usr/bin/python3
import json, os, pathlib, sys, time
if '--version' in sys.argv:
    sys.exit()
state=pathlib.Path(os.environ.get('XDG_STATE_HOME',str(pathlib.Path.home()/'.local/state')))
state.mkdir(mode=0o700,parents=True,exist_ok=True)
marker=state/'acknowledged-original-history'
if not marker.exists():
    marker.write_text('original history')
    with (pathlib.Path.home()/'uploads').open('a') as stream:
        stream.write(str(state)+'\n')
while True:
    time.sleep(1)
"#;
    let input = json!({"version":1,"operation":OPERATION,"receiver_identity":receiver,"destination":"http://127.0.0.1:1234/mirror","token":token,"binary":STANDARD.encode(binary),"sha256":format!("{:x}",Sha256::digest(binary))});
    let mut child = Command::new("/usr/bin/python3")
        .args(["-I", "-c", include_str!("../src/bootstrap.py")])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&input).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "bootstrap failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["running"],
        true
    );
}
#[test]
fn repair_and_rotation_keep_one_checkpoint_but_new_receiver_uploads_originals() {
    let home = Home(tempfile::tempdir().unwrap());
    setup(home.0.path(), "receiver-one", "synthetic-token-one");
    let uploads = fs::read_to_string(home.0.path().join("uploads")).unwrap();
    assert_eq!(uploads.lines().count(), 1);
    setup(home.0.path(), "receiver-one", "synthetic-token-one");
    setup(home.0.path(), "receiver-one", "synthetic-rotated-token");
    assert_eq!(
        fs::read_to_string(home.0.path().join("uploads")).unwrap(),
        uploads
    );
    setup(home.0.path(), "receiver-two", "synthetic-token-two");
    let uploads = fs::read_to_string(home.0.path().join("uploads")).unwrap();
    assert_eq!(uploads.lines().count(), 2);
    let paths = uploads.lines().collect::<Vec<_>>();
    assert_ne!(paths[0], paths[1]);
    assert!(
        Path::new(paths[0])
            .join("acknowledged-original-history")
            .is_file()
    );
    assert!(
        Path::new(paths[1])
            .join("acknowledged-original-history")
            .is_file()
    );
    assert!(!uploads.contains("synthetic-token"));
}
