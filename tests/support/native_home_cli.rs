use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

pub struct Fixture {
    pub root: PathBuf,
    pub claude: PathBuf,
    pub codex: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("semon-home-cli-{}-{nonce}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let fixture = Self {
            claude: root.join("configured-claude"),
            codex: root.join("configured-codex"),
            root,
        };
        fixture.write(
            "configured-claude/projects/fixture/00000000-0000-4000-8000-000000000001.jsonl",
            include_str!("../fixtures/compatibility/claude-2.1.288/initial-transcript.jsonl")
                .replace(
                    "native-claude-compat",
                    "00000000-0000-4000-8000-000000000001",
                )
                .as_bytes(),
        );
        fixture.write("configured-claude/.claude.json", b"{}");
        fixture.write("configured-codex/sessions/2026/10/03/rollout-2026-10-03T07-49-53-00000000-0000-4000-8000-000000000002.jsonl", include_str!("../fixtures/compatibility/codex-0.159.0-alpha.3/initial-rollout.jsonl").replace("native-codex-compat","00000000-0000-4000-8000-000000000002").as_bytes());
        fs::create_dir(fixture.root.join("empty-home")).unwrap();
        fs::create_dir(fixture.root.join("empty-proc")).unwrap();
        fixture
    }
    pub fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    pub fn command(&self, binary: &str) -> Command {
        let mut command = Command::new(binary);
        // Only the Semon test child gets this fallback home. No native CLI runs,
        // real-home discovery, ambient credentials or global environment mutation.
        command
            .env_clear()
            .env("HOME", self.root.join("empty-home"))
            .env("XDG_STATE_HOME", self.root.join("semon-state"))
            .env("CLAUDE_CONFIG_DIR", &self.claude)
            .env("CODEX_HOME", &self.codex)
            .current_dir(&self.root);
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
