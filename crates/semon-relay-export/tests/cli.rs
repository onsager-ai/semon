use std::{fs, process::Command};

#[test]
fn explicit_cli_warns_before_access_exports_private_bytes_and_refuses_defaults() {
    let binary = env!("CARGO_BIN_EXE_semon-relay-export");
    assert!(
        Command::new(binary)
            .arg("--help")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        Command::new(binary).output().unwrap().status.code(),
        Some(2)
    );
    let root = std::env::temp_dir().join(format!("semon-relay-export-cli-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let input = root.join("config");
    fs::create_dir(&input).unwrap();
    let sentinel = b"synthetic private key marker\x00\xff";
    fs::write(input.join("identity.age"), sentinel).unwrap();
    let out = root.join("export");
    let result = Command::new(binary)
        .arg("--config-dir")
        .arg(&input)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stderr).contains("age/signing keys"));
    assert!(String::from_utf8_lossy(&result.stdout).contains("Decryption is not verified"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("private key marker"));
    assert!(!String::from_utf8_lossy(&result.stderr).contains("private key marker"));
    assert_eq!(fs::read(input.join("identity.age")).unwrap(), sentinel);
    assert_eq!(
        fs::read(out.join("inputs/config-0/identity.age")).unwrap(),
        sentinel
    );
    assert!(out.join("manifest.json").is_file());
    let repeat = Command::new(binary)
        .arg("--config-dir")
        .arg(&input)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!repeat.status.success());
    assert_eq!(
        fs::read(out.join("inputs/config-0/identity.age")).unwrap(),
        sentinel
    );
    let missing = Command::new(binary)
        .arg("--config-dir")
        .arg(root.join("missing"))
        .arg("--out")
        .arg(root.join("failed"))
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("age/signing keys"));
    assert!(!root.join("failed").exists());
    fs::remove_dir_all(root).unwrap();
}
