//! Opt-in Linux local session with an executor confined outside owner authority.
//! This is not attachment to an arbitrary existing terminal and enables no remote service.
use crate::{
    Journal, RequestStore,
    codex::{Codex, VERSION},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

/// Owns the confined session's native server and current local connection.
pub struct LocalSession {
    /// Native connection consumed by the viewer.
    pub driver: Arc<Codex>,
    /// Dedicated home for existing transcript collection.
    pub home: PathBuf,
    /// Owner-only native socket, also usable by `codex --remote`.
    pub socket: PathBuf,
    process: Child,
    command: Command,
    credential_env: &'static str,
}
fn refuse(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}
// Model selectors outrank feature flags in the pinned native release. Export
// native metadata offline and adapt capabilities, never model-owned instructions.
fn adapt_standalone_catalog(mut catalog: serde_json::Value) -> io::Result<serde_json::Value> {
    let models = catalog["models"]
        .as_array_mut()
        .filter(|models| !models.is_empty())
        .ok_or_else(|| refuse("native bundled model catalog must contain models"))?;
    for model in models {
        if !model.is_object() || model["slug"].as_str().is_none_or(str::is_empty) {
            return Err(refuse("invalid native bundled model descriptor"));
        }
        model["tool_mode"] = json!("direct");
        model["multi_agent_version"] = json!("disabled");
        model["experimental_supported_tools"] = json!([]);
        model["use_responses_lite"] = json!(false);
    }
    Ok(catalog)
}
fn standalone_catalog(binary: &Path, home: &Path) -> io::Result<serde_json::Value> {
    // --bundled bypasses configuration, credential discovery and remote refresh.
    let output = Command::new(binary)
        .args(["debug", "models", "--bundled"])
        .env_clear()
        .env("CODEX_HOME", home)
        .current_dir(home)
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        return Err(refuse("native bundled model catalog export failed"));
    }
    let catalog = adapt_standalone_catalog(serde_json::from_slice(&output.stdout)?)?;
    fs::write(
        home.join("standalone-profile.json"),
        serde_json::to_vec(&json!({
            "profile": "standalone-direct-v1",
            "native_version": VERSION,
            "instruction_source": "native debug models --bundled",
            "native_catalog_sha256": hex::encode(Sha256::digest(&output.stdout)),
            "effective_catalog_sha256": hex::encode(Sha256::digest(serde_json::to_vec(&catalog)?)),
            "capability_overrides": {
                "tool_mode": "direct", "multi_agent_version": "disabled",
                "experimental_supported_tools": [], "use_responses_lite": false
            }
        }))?,
    )?;
    Ok(catalog)
}
/// Fixed outer executor boundary. No host home, run/tmp brokers or owner procfs is mounted.
/// Model networking stays in the owner app-server; tool networking is unavailable in this slice.
pub fn executor_args(package: &Path, workspace: &Path) -> io::Result<Vec<String>> {
    let text = |p: &Path| {
        p.to_str()
            .map(str::to_owned)
            .ok_or_else(|| refuse("paths must be valid UTF-8"))
    };
    let mut args = vec![
        "--unshare-all".into(),
        "--die-with-parent".into(),
        "--new-session".into(),
        "--clearenv".into(),
    ];
    for path in ["/usr", "/bin", "/lib", "/lib64"] {
        if Path::new(path).exists() {
            args.extend(["--ro-bind".into(), path.into(), path.into()]);
        }
    }
    args.extend([
        "--ro-bind".into(),
        text(package)?,
        "/opt/codex".into(),
        "--dev".into(),
        "/dev".into(),
        "--proc".into(),
        "/proc".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--dir".into(),
        "/home/agent/.codex".into(),
        "--bind".into(),
        text(workspace)?,
        text(workspace)?,
        "--chdir".into(),
        text(workspace)?,
        "--setenv".into(),
        "HOME".into(),
        "/home/agent".into(),
        "--setenv".into(),
        "CODEX_HOME".into(),
        "/home/agent/.codex".into(),
        "--setenv".into(),
        "PATH".into(),
        "/usr/bin:/bin:/opt/codex/bin".into(),
        "--".into(),
        "/opt/codex/bin/codex".into(),
        "exec-server".into(),
        "--listen".into(),
        "stdio://".into(),
    ]);
    Ok(args)
}
impl LocalSession {
    /// Starts a pinned package in a fresh private state directory. Existing homes are never changed.
    /// `config` supplies native model configuration, not executor or owner-authentication policy.
    pub fn launch(
        binary: &Path,
        workspace: &Path,
        state: &Path,
        config: Option<&Path>,
    ) -> io::Result<Self> {
        Self::launch_with_api_key(binary, workspace, state, config, None)
    }
    /// Private coordinator delivery; the credential is restricted to the model
    /// process. It never enters the executor, manifest, or control journal.
    pub fn launch_with_api_key(
        binary: &Path,
        workspace: &Path,
        state: &Path,
        config: Option<&Path>,
        api_key: Option<&str>,
    ) -> io::Result<Self> {
        Self::launch_private(binary, workspace, state, config, api_key, None)
    }
    /// Opaque official ChatGPT file, delivered only to the trusted model home.
    /// The caller must own the single refresh writer until confirmed shutdown.
    pub fn launch_with_auth_artifact(
        binary: &Path,
        workspace: &Path,
        state: &Path,
        artifact: &[u8],
    ) -> io::Result<Self> {
        validate_auth_artifact(artifact)?;
        Self::launch_private(binary, workspace, state, None, None, Some(artifact))
    }
    fn launch_private(
        binary: &Path,
        workspace: &Path,
        state: &Path,
        config: Option<&Path>,
        api_key: Option<&str>,
        artifact: Option<&[u8]>,
    ) -> io::Result<Self> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err(refuse(
                "local control requires qualified Linux namespace isolation",
            ));
        }
        let binary = binary.canonicalize()?;
        let workspace = workspace.canonicalize()?;
        let package = binary
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| refuse("use a complete native Codex package"))?;
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(package.join("codex-package.json"))?)?;
        let version = Command::new(&binary).arg("--version").output()?;
        if !version.status.success()
            || String::from_utf8_lossy(&version.stdout).trim() != format!("codex-cli {VERSION}")
            || manifest["version"] != VERSION
        {
            return Err(refuse("Codex package is not the qualified pinned release"));
        }
        let parent = state
            .parent()
            .ok_or_else(|| refuse("state needs an existing private parent"))?
            .canonicalize()?;
        let state = parent.join(
            state
                .file_name()
                .ok_or_else(|| refuse("invalid state path"))?,
        );
        if state.starts_with(&workspace)
            || workspace.starts_with(&state)
            || [
                "/usr",
                "/bin",
                "/lib",
                "/lib64",
                "/proc",
                "/dev",
                "/tmp",
                "/opt/codex",
            ]
            .iter()
            .any(|p| workspace.starts_with(p) || Path::new(p).starts_with(&workspace))
            || state.starts_with(package)
            || package.starts_with(&workspace)
        {
            return Err(refuse(
                "workspace overlaps protected state or runtime mounts",
            ));
        }
        // Higher-priority legacy managed settings cannot be overridden by session flags.
        // MCP servers and host programs must never be inherited into this owner server.
        for path in [
            "/etc/codex/managed_config.toml",
            "/etc/codex/requirements.toml",
        ] {
            if Path::new(path).exists() {
                return Err(refuse(
                    "managed native configuration requires separate qualification",
                ));
            }
        }
        if Path::new("/etc/codex/config.toml").exists() {
            let system: toml::Value =
                toml::from_str(&fs::read_to_string("/etc/codex/config.toml")?)
                    .map_err(|_| refuse("invalid system native configuration"))?;
            if !system.as_table().is_some_and(|table| {
                table
                    .keys()
                    .all(|key| matches!(key.as_str(), "plugins" | "marketplaces"))
            }) {
                return Err(refuse(
                    "system native configuration requires separate qualification",
                ));
            }
        }
        // Kernel capability probe fails closed; no network or process-ancestry fallback.
        let args = executor_args(package, &workspace)?;
        let mut probe = args.clone();
        probe.truncate(probe.len() - 5);
        probe.extend(["--".into(), "/usr/bin/true".into()]);
        if !Command::new("/usr/bin/bwrap")
            .args(&probe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()?
            .success()
        {
            return Err(refuse("required namespace isolation unavailable"));
        }
        fs::DirBuilder::new().mode(0o700).create(&state)?;
        let home = state.join("home");
        fs::DirBuilder::new().mode(0o700).create(&home)?;
        if let Some(bytes) = artifact {
            write_auth_artifact(&home, bytes, false)?;
        }
        let catalog = home.join("standalone-models.json");
        fs::write(
            &catalog,
            serde_json::to_vec(&standalone_catalog(&binary, &home)?)?,
        )?;
        let mut native_config = config
            .map(fs::read_to_string)
            .transpose()?
            .unwrap_or_default();
        // This fresh home only accepts model settings. Project configuration stays untrusted.
        // A supplied config must not select alternative executors or relax inherited tool env.
        let parsed: toml::Value =
            toml::from_str(&native_config).map_err(|_| refuse("invalid model config"))?;
        if !parsed.as_table().is_some_and(|table| {
            table.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "model" | "model_provider" | "model_providers" | "model_reasoning_effort"
                )
            })
        }) {
            return Err(refuse(
                "config may contain only native model/provider settings",
            ));
        }
        native_config.push_str("\n[shell_environment_policy]\ninherit=\"none\"\n");
        fs::write(home.join("config.toml"), native_config)?;
        // Namespace isolation does not hide host sockets placed inside the workspace.
        // The executor has only stdio transport: disallow socket APIs and io_uring bypass.
        // Audit architecture first, including alternate-ABI syscall-number rejection.
        let mut filter = Vec::new();
        let mut instruction = |code: u16, jt: u8, jf: u8, k: u32| {
            filter.extend(code.to_le_bytes());
            filter.extend([jt, jf]);
            filter.extend(k.to_le_bytes());
        };
        instruction(0x20, 0, 0, 4); // seccomp_data.arch
        instruction(0x15, 1, 0, 0xc000003e); // AUDIT_ARCH_X86_64
        instruction(0x06, 0, 0, 0x00050001); // SECCOMP_RET_ERRNO | EPERM
        instruction(0x20, 0, 0, 0); // seccomp_data.nr
        instruction(0x35, 0, 1, 0x40000000); // x32 ABI
        instruction(0x06, 0, 0, 0x00050001);
        for syscall in [41, 42, 425] {
            // socket, connect, io_uring_setup
            instruction(0x15, 0, 1, syscall);
            instruction(0x06, 0, 0, 0x00050001);
        }
        // Tokio needs anonymous Unix stream pairs. They are already connected;
        // connect/socket are denied, and datagram pairs cannot address a host pathname.
        instruction(0x15, 0, 6, 53); // socketpair
        instruction(0x20, 0, 0, 16); // args[0]: family
        instruction(0x15, 0, 3, 1); // AF_UNIX
        instruction(0x20, 0, 0, 24); // args[1]: type
        instruction(0x54, 0, 0, 15); // exclude CLOEXEC/NONBLOCK flags
        instruction(0x15, 1, 0, 1); // SOCK_STREAM only
        instruction(0x06, 0, 0, 0x00050001);
        instruction(0x06, 0, 0, 0x7fff0000); // SECCOMP_RET_ALLOW
        let policy = state.join("executor.bpf");
        fs::write(&policy, filter)?;
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\"'\"'"));
        let executor = state.join("executor");
        fs::write(
            &executor,
            format!(
                "#!/bin/sh\nset -eu\nexec 3< {}\nexec /usr/bin/bwrap --seccomp 3 {}\n",
                quote(&policy.to_string_lossy()),
                args.iter()
                    .map(|arg| quote(arg))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        )?;
        fs::set_permissions(&executor, fs::Permissions::from_mode(0o700))?;
        let probe_script = format!(
            "exec 3< {}\nexec /usr/bin/bwrap --seccomp 3 {}",
            quote(&policy.to_string_lossy()),
            probe
                .iter()
                .map(|arg| quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
        );
        if !Command::new("/bin/sh")
            .args(["-c", &probe_script])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()?
            .success()
        {
            return Err(refuse("required syscall isolation unavailable"));
        }
        fs::write(
            home.join("environments.toml"),
            format!(
                "default=\"semon-local\"\ninclude_local=false\n[[environments]]\nid=\"semon-local\"\nprogram={}\nargs=[]\n",
                json!(executor)
            ),
        )?;
        let store = Arc::new(RequestStore::new(Journal::open(
            &state.join("journal.jsonl"),
        )?));
        let socket = state.join("native.sock");
        let log = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(state.join("native.log"))?;
        fs::set_permissions(state.join("native.log"), fs::Permissions::from_mode(0o600))?;
        let mut command = Command::new(&binary);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home);
        // Credentials stay in the trusted model transport, never in executor environments.
        if let Some(providers) = parsed
            .get("model_providers")
            .and_then(toml::Value::as_table)
        {
            for provider in providers.values() {
                if let Some(key) = provider.get("env_key").and_then(toml::Value::as_str) {
                    if !(key.ends_with("_KEY") || key.ends_with("_TOKEN"))
                        || ["CODEX_", "SEMON_", "LD_"]
                            .iter()
                            .any(|prefix| key.starts_with(prefix))
                    {
                        return Err(refuse(
                            "provider credential must use a non-reserved _KEY or _TOKEN variable",
                        ));
                    }
                    if api_key.is_none()
                        && artifact.is_none()
                        && let Some(value) = std::env::var_os(key)
                    {
                        command.env(key, value);
                    }
                }
            }
        }
        for key in [
            "OPENAI_API_KEY",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ] {
            if key == "OPENAI_API_KEY" && (api_key.is_some() || artifact.is_some()) {
                continue;
            }
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let credential_env = if parsed.get("model_provider").and_then(toml::Value::as_str)
            == Some("openai_chatgpt_plan")
        {
            "ACCESS_TOKEN"
        } else {
            "OPENAI_API_KEY"
        };
        if let Some(key) = api_key {
            if key.is_empty() || key.len() > 4096 || key.bytes().any(|b| b.is_ascii_control()) {
                return Err(refuse("invalid model credential"));
            }
            command
                .env_remove("OPENAI_API_KEY")
                .env_remove("ACCESS_TOKEN")
                .env(credential_env, key);
        }
        if artifact.is_some() {
            command
                .env_remove("OPENAI_API_KEY")
                .env_remove("ACCESS_TOKEN");
            command.arg("-c").arg("cli_auth_credentials_store=\"file\"");
        }
        let mut process = command
            .args([
                "app-server",
                "--disable",
                "hooks",
                "--disable",
                "plugins",
                "--disable",
                "apps",
                "--disable",
                "plugin_hooks",
                "--disable",
                "code_mode",
                "--disable",
                "code_mode_only",
                "--disable",
                "code_mode_host",
                "--disable",
                "js_repl",
                "--disable",
                "multi_agent",
                "--disable",
                "multi_agent_v2",
                "-c",
                "agents.enabled=false",
                "-c",
                "shell_environment_policy.inherit=\"none\"",
                "-c",
            ])
            .arg(format!("model_catalog_json={}", json!(catalog)))
            .arg("-c")
            .arg(format!(
                "projects.{}.trust_level=\"untrusted\"",
                json!(workspace)
            ))
            .arg("--listen")
            .arg(format!("unix://{}", socket.display()))
            .current_dir(&workspace)
            .env("CODEX_HOME", &home)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !socket.exists() && Instant::now() < deadline {
            if process.try_wait()?.is_some() {
                return Err(refuse("native server exited; inspect private native.log"));
            }
            thread::sleep(Duration::from_millis(25));
        }
        let driver =
            match Codex::connect(&socket, None, Some(&workspace), store, Some(process.id())) {
                Ok(driver) => driver,
                Err(e) => {
                    let _ = process.kill();
                    let _ = process.wait();
                    return Err(e);
                }
            };
        let mut manifest = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(state.join("session.json"))?;
        manifest.write_all(json!({"version":VERSION,"thread":driver.thread_id(),"workspace":workspace,"socket":socket,"scope":"semon-started isolated executor; network off"}).to_string().as_bytes())?;
        Ok(Self {
            driver,
            home,
            socket,
            process,
            command,
            credential_env,
        })
    }
}
impl LocalSession {
    /// Explicit idle credential renewal through the same native driver and home.
    /// Old native writes are never reconstructed or replayed. The caller owns
    /// credential generation claims and must reconcile a lost renewal receipt.
    pub fn renew_credential(&mut self, key: &str) -> io::Result<()> {
        if key.is_empty() || key.len() > 4096 || key.bytes().any(|b| b.is_ascii_control()) {
            return Err(refuse("invalid model credential"));
        }
        if !self.driver.snapshot()["activeTurn"].is_null() {
            return Err(refuse(
                "finish or explicitly end the active turn before credential renewal",
            ));
        }
        self.restart_with(|session| {
            session
                .command
                .env_remove("OPENAI_API_KEY")
                .env_remove("ACCESS_TOKEN")
                .env(session.credential_env, key);
            Ok(())
        })
    }
    /// Replace opaque credentials only after stopping the old native writer.
    pub fn renew_auth_artifact(&mut self, bytes: &[u8]) -> io::Result<()> {
        validate_auth_artifact(bytes)?;
        self.restart_with(|session| {
            write_auth_artifact(&session.home, bytes, true)?;
            session
                .command
                .env_remove("OPENAI_API_KEY")
                .env_remove("ACCESS_TOKEN");
            Ok(())
        })
    }
    fn restart_with(
        &mut self,
        install: impl FnOnce(&mut Self) -> io::Result<()>,
    ) -> io::Result<()> {
        if !self.driver.snapshot()["activeTurn"].is_null() {
            return Err(refuse(
                "finish or end the active turn before credential renewal",
            ));
        }
        let thread = self.driver.thread_id();
        let store = self.driver.store.clone();
        self.driver.lost();
        self.end()?;
        if self.socket.symlink_metadata().is_ok() {
            fs::remove_file(&self.socket)?;
        }
        install(self)?;
        self.process = self.command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.socket.exists() && Instant::now() < deadline {
            if self.process.try_wait()?.is_some() {
                return Err(refuse("native renewal exited"));
            }
            thread::sleep(Duration::from_millis(25));
        }
        self.driver = Codex::connect(
            &self.socket,
            Some(&thread),
            None,
            store,
            Some(self.process.id()),
        )?;
        Ok(())
    }
    /// Stops the native server, leaving workspace, transcript and compute intact.
    pub fn end(&mut self) -> io::Result<()> {
        if self.process.try_wait()?.is_none() {
            self.process.kill()?;
        }
        self.process.wait()?;
        Ok(())
    }
}
/// Read only after the native writer has been stopped by the coordinator.
pub fn read_auth_artifact(home: &Path) -> io::Result<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(home.join("auth.json"))?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != fs::metadata(home)?.uid()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > 32768
    {
        return Err(refuse("invalid private authentication artifact"));
    }
    let mut bytes = Vec::new();
    file.take(32769).read_to_end(&mut bytes)?;
    validate_auth_artifact(&bytes)?;
    Ok(bytes)
}
/// Validate the auth kind and bound the official file without extracting tokens.
pub fn validate_auth_artifact(bytes: &[u8]) -> io::Result<()> {
    if bytes.is_empty() || bytes.len() > 32768 {
        return Err(refuse("invalid authentication artifact size"));
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| refuse("invalid authentication artifact"))?;
    if !value["OPENAI_API_KEY"].is_null()
        || value.get("auth_mode").is_some_and(|v| v != "chatgpt")
        || !value["tokens"]
            .as_object()
            .is_some_and(|tokens| !tokens.is_empty())
    {
        return Err(refuse("ChatGPT authentication artifact required"));
    }
    Ok(())
}
fn write_auth_artifact(home: &Path, bytes: &[u8], replace: bool) -> io::Result<()> {
    validate_auth_artifact(bytes)?;
    let temp = home.join("auth.delivery.json");
    let path = if replace {
        &temp
    } else {
        &home.join("auth.json")
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if replace {
        fs::rename(temp, home.join("auth.json"))?;
    }
    Ok(())
}
impl Drop for LocalSession {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_auth_is_private_bound_and_cannot_switch_to_api_billing() {
        use std::os::unix::fs::symlink;
        let root = crate::journal::tests::scratch("opaque-auth");
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let artifact = include_bytes!("../../../tests/fixtures/codex-0.160.0/managed-auth.json");
        write_auth_artifact(&root, artifact, false).unwrap();
        assert_eq!(read_auth_artifact(&root).unwrap(), artifact);
        assert_eq!(
            fs::metadata(root.join("auth.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(write_auth_artifact(&root, artifact, false).is_err());
        for invalid in [
            br#"{"OPENAI_API_KEY":"synthetic-key","tokens":{"access_token":"synthetic"}}"#
                .as_slice(),
            br#"{"auth_mode":"apikey","tokens":{"access_token":"synthetic"}}"#,
            b"secret-not-json",
        ] {
            assert!(validate_auth_artifact(invalid).is_err());
        }
        assert!(validate_auth_artifact(&vec![b' '; 32769]).is_err());
        fs::set_permissions(root.join("auth.json"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_auth_artifact(&root).is_err());
        fs::remove_file(root.join("auth.json")).unwrap();
        fs::write(root.join("other"), artifact).unwrap();
        symlink(root.join("other"), root.join("auth.json")).unwrap();
        assert!(read_auth_artifact(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn standalone_capabilities_preserve_native_messages_and_metadata() {
        let original = json!({"models": [{
            "slug": "gpt-6-luna", "tool_mode": "code_mode_only",
            "multi_agent_version": "v2", "experimental_supported_tools": ["unsupported"],
            "use_responses_lite": true, "context_window": 272000,
            "model_messages": {
                "instructions_template": "Native model-specific instructions, not a Semon prompt",
                "instructions_variables": {"personality": "native"},
                "tools": {"direct": "native direct guidance"},
                "multi_agent": {"role": {"root": "native role guidance"}}
            }
        }, {"slug": "other", "model_messages": null}]});
        let catalog = adapt_standalone_catalog(original.clone()).unwrap();
        for (before, after) in original["models"]
            .as_array()
            .unwrap()
            .iter()
            .zip(catalog["models"].as_array().unwrap())
        {
            assert_eq!(after["tool_mode"], "direct");
            assert_eq!(after["multi_agent_version"], "disabled");
            assert_eq!(after["experimental_supported_tools"], json!([]));
            assert_eq!(after["use_responses_lite"], false);
            let mut restored = after.clone();
            for key in [
                "tool_mode",
                "multi_agent_version",
                "experimental_supported_tools",
                "use_responses_lite",
            ] {
                if let Some(value) = before.get(key) {
                    restored[key] = value.clone();
                } else {
                    restored.as_object_mut().unwrap().remove(key);
                }
            }
            assert_eq!(&restored, before);
        }
    }

    #[test]
    fn standalone_catalog_refuses_invalid_native_metadata() {
        for catalog in [
            json!({}),
            json!({"models": []}),
            json!({"models": [null]}),
            json!({"models": [{"slug": ""}]}),
        ] {
            assert!(adapt_standalone_catalog(catalog).is_err());
        }
    }
}
