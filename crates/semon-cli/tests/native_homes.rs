#[path = "../../../tests/support/native_home_cli.rs"]
mod fixture;
#[test]
fn native_environment_selects_session_discovery_defaults() {
    let fixture = fixture::Fixture::new();
    let claude = fixture
        .claude
        .join("projects/fixture/00000000-0000-4000-8000-000000000001.jsonl");
    let codex=fixture.codex.join("sessions/2026/10/03/rollout-2026-10-03T07-49-53-00000000-0000-4000-8000-000000000002.jsonl");
    let before = [
        std::fs::read(&claude).unwrap(),
        std::fs::read(&codex).unwrap(),
        std::fs::read(fixture.claude.join(".claude.json")).unwrap(),
    ];
    let result = fixture
        .command(env!("CARGO_BIN_EXE_semon"))
        .args(["sessions", "--all", "--json", "--proc-root"])
        .arg(fixture.root.join("empty-proc"))
        .arg("--cache")
        .arg(fixture.root.join("cache/index.json"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(
        output.contains("00000000-0000-4000-8000-000000000001"),
        "{output}"
    );
    assert!(output.contains("00000000-0000-4000-8000-000000000002"));
    assert_eq!(std::fs::read(claude).unwrap(), before[0]);
    assert_eq!(std::fs::read(codex).unwrap(), before[1]);
    assert_eq!(
        std::fs::read(fixture.claude.join(".claude.json")).unwrap(),
        before[2]
    );
}

#[test]
fn archived_codex_discovery_uses_the_configured_read_only_home() {
    let fixture = fixture::Fixture::new();
    let active = fixture.codex.join("sessions/2026/10/03/rollout-2026-10-03T07-49-53-00000000-0000-4000-8000-000000000002.jsonl");
    let archived = fixture
        .codex
        .join("archived_sessions")
        .join(active.file_name().unwrap());
    std::fs::create_dir_all(archived.parent().unwrap()).unwrap();
    std::fs::rename(&active, &archived).unwrap();
    let bytes = std::fs::read(&archived).unwrap();
    for pass in 0..2 {
        let result = fixture
            .command(env!("CARGO_BIN_EXE_semon"))
            .args(["sessions", "--all", "--json", "--proc-root"])
            .arg(fixture.root.join("empty-proc"))
            .arg("--cache")
            .arg(fixture.root.join("archive-cache/index.json"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "pass {pass}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = String::from_utf8(result.stdout).unwrap();
        assert!(
            output.contains("00000000-0000-4000-8000-000000000002"),
            "{output}"
        );
        assert_eq!(std::fs::read(&archived).unwrap(), bytes);
        assert!(!active.exists());
    }
}
