#![cfg(target_os = "linux")]
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn hostile_objects_die_in_worker_without_values_in_response() {
    for bytes in [
        b"PRIVATE_ROW_VALUE".to_vec(),
        vec![b'['; 8192],
        vec![0; 8 * 1024 * 1024 + 1],
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_semon"))
            .args(["asi", "object-worker"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(b"origin\n").unwrap();
        // An early resource refusal may close the pipe before the input completes.
        let _ = input.write_all(&bytes);
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert_eq!(output.stdout, b"0");
        assert!(output.stderr.is_empty());
    }
}
