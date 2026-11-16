//! Pairing's user service. Commands are injected so tests never contact an init system.
use std::{env, fs, path::PathBuf, process::Command};

const UNIT: &str = "semon-push.service";
const LABEL: &str = "ai.onsager.semon-push";

#[derive(Clone, Copy)]
enum Platform {
    Linux,
    Mac,
}

struct Service {
    platform: Platform,
    binary: PathBuf,
    home: PathBuf,
    config: PathBuf,
    state: Option<PathBuf>,
    native_environment: Vec<(String, String)>,
    uid: u32,
}

impl Service {
    fn current() -> Result<Self, String> {
        let platform = match env::consts::OS {
            "linux" => Platform::Linux,
            "macos" => Platform::Mac,
            os => return Err(format!("background sync is unsupported on {os}")),
        };
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is unset")?;
        let config = env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        Ok(Self {
            platform,
            binary: env::current_exe().map_err(|e| e.to_string())?,
            home,
            config,
            state: env::var_os("XDG_STATE_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            native_environment: ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "COPILOT_HOME"]
                .into_iter()
                .filter_map(|key| {
                    env::var(key)
                        .ok()
                        .filter(|v| !v.is_empty())
                        .map(|value| (key.into(), value))
                })
                .collect(),
            // SAFETY: getuid has no preconditions.
            uid: unsafe { libc::getuid() },
        })
    }

    fn path(&self) -> PathBuf {
        match self.platform {
            Platform::Linux => self.config.join("systemd/user").join(UNIT),
            Platform::Mac => self
                .home
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
        }
    }

    fn log_path(&self) -> PathBuf {
        self.home.join("Library/Logs/semon-push.log")
    }

    fn render(&self) -> String {
        let binary = self.binary.to_string_lossy();
        let home = self.home.to_string_lossy();
        let config = self.config.to_string_lossy();
        match self.platform {
            Platform::Linux => {
                let mut environment = format!(
                    "Environment={}\nEnvironment={}\n",
                    systemd_quote(&format!("HOME={home}")),
                    systemd_quote(&format!("XDG_CONFIG_HOME={config}"))
                );
                if let Some(state) = &self.state {
                    environment.push_str(&format!(
                        "Environment={}\n",
                        systemd_quote(&format!("XDG_STATE_HOME={}", state.display()))
                    ));
                }
                for (key, value) in &self.native_environment {
                    environment.push_str(&format!(
                        "Environment={}\n",
                        systemd_quote(&format!("{key}={value}"))
                    ));
                }
                format!(
                    "[Unit]\nDescription=Semon background session sync\n\n[Service]\nExecStart={} push --watch\n{environment}Restart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
                    systemd_quote(&binary).replace('$', "$$")
                )
            }
            Platform::Mac => {
                let mut environment = format!(
                    "<key>HOME</key><string>{}</string><key>XDG_CONFIG_HOME</key><string>{}</string>",
                    xml(&home),
                    xml(&config)
                );
                if let Some(state) = &self.state {
                    environment.push_str(&format!(
                        "<key>XDG_STATE_HOME</key><string>{}</string>",
                        xml(&state.to_string_lossy())
                    ));
                }
                for (key, value) in &self.native_environment {
                    environment.push_str(&format!(
                        "<key>{}</key><string>{}</string>",
                        xml(key),
                        xml(value)
                    ));
                }
                let log = xml(&self.log_path().to_string_lossy());
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{LABEL}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>push</string><string>--watch</string></array>\n<key>EnvironmentVariables</key><dict>{environment}</dict>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n<key>ThrottleInterval</key><integer>5</integer>\n<key>StandardOutPath</key><string>{log}</string>\n<key>StandardErrorPath</key><string>{log}</string>\n</dict></plist>\n",
                    xml(&binary)
                )
            }
        }
    }

    fn install(
        &self,
        run: &mut impl FnMut(&str, &[&str]) -> Result<(), String>,
    ) -> Result<(), String> {
        let path = self.path();
        let replacing = path.exists();
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        if matches!(self.platform, Platform::Mac) {
            fs::create_dir_all(self.log_path().parent().unwrap()).map_err(|e| e.to_string())?;
        }
        fs::write(&path, self.render()).map_err(|e| e.to_string())?;
        match self.platform {
            Platform::Linux => {
                // Linger is independent of the user manager: keep pairing even if unavailable.
                let user = self.uid.to_string();
                if run("loginctl", &["enable-linger", &user]).is_err() {
                    eprintln!("Linger was not enabled; run: loginctl enable-linger {user}");
                }
                run("systemctl", &["--user", "daemon-reload"])?;
                run("systemctl", &["--user", "enable", "--now", UNIT])?;
                // enable --now leaves an already-running service alone on re-pair.
                if replacing {
                    run("systemctl", &["--user", "restart", UNIT])?;
                }
            }
            Platform::Mac => {
                let domain = format!("gui/{}", self.uid);
                let target = format!("{domain}/{LABEL}");
                // The first pairing has no job to boot out.
                let _ = run("launchctl", &["bootout", &target]);
                run(
                    "launchctl",
                    &["bootstrap", &domain, &path.to_string_lossy()],
                )?;
            }
        }
        Ok(())
    }

    fn remove(
        &self,
        run: &mut impl FnMut(&str, &[&str]) -> Result<(), String>,
    ) -> Result<(), String> {
        match self.platform {
            Platform::Linux => {
                for args in [["--user", "stop", UNIT], ["--user", "disable", UNIT]] {
                    if let Err(error) = run("systemctl", &args) {
                        eprintln!("Background sync cleanup: {error}");
                    }
                }
            }
            Platform::Mac => {
                let target = format!("gui/{}/{LABEL}", self.uid);
                let _ = run("launchctl", &["bootout", &target]);
            }
        }
        match fs::remove_file(self.path()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        if matches!(self.platform, Platform::Linux) {
            let _ = run("systemctl", &["--user", "daemon-reload"]);
        }
        Ok(())
    }
}

fn systemd_quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn command(program: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn start_message(
    service: &Service,
    run: &mut impl FnMut(&str, &[&str]) -> Result<(), String>,
) -> String {
    match service.install(run) {
        Ok(()) => match service.platform {
            Platform::Linux => {
                "Background sync started; logs: journalctl --user -u semon-push".into()
            }
            Platform::Mac => format!(
                "Background sync started; logs: {}",
                service.log_path().display()
            ),
        },
        Err(error) => foreground_message(&service.binary, &error),
    }
}

fn foreground_message(binary: &std::path::Path, error: &str) -> String {
    let quoted = format!("'{}'", binary.to_string_lossy().replace('\'', "'\\''"));
    let error = error.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("Background sync is not running ({error}); run: {quoted} push --watch")
}

pub fn start() {
    let message = match Service::current() {
        Ok(service) => start_message(&service, &mut command),
        Err(error) => foreground_message(
            &env::current_exe().unwrap_or_else(|_| PathBuf::from("semon")),
            &error,
        ),
    };
    println!("{message}");
}

pub fn stop() -> Result<(), String> {
    Service::current()?.remove(&mut command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(root: &std::path::Path, platform: Platform) -> Service {
        Service {
            platform,
            binary: root.join("bin/semon"),
            home: root.join("home"),
            config: root.join("xdg-config"),
            state: Some(root.join("xdg-state")),
            native_environment: vec![("CODEX_HOME".into(), "/custom/codex".into())],
            uid: 1001,
        }
    }

    fn record(calls: &mut Vec<String>, program: &str, args: &[&str]) {
        calls.push(format!("{program} {}", args.join(" ")));
    }

    #[test]
    fn linux_pair_repair_and_unpair_sequence() {
        let root = tempfile::tempdir().unwrap();
        let mut service = service(root.path(), Platform::Linux);
        let mut calls = Vec::new();
        let mut run = |program: &str, args: &[&str]| {
            record(&mut calls, program, args);
            Ok(())
        };
        service.install(&mut run).unwrap();
        service.binary = root.path().join("updated/semon");
        service.install(&mut run).unwrap();
        assert!(
            fs::read_to_string(service.path())
                .unwrap()
                .contains("updated/semon")
        );
        service.remove(&mut run).unwrap();
        // Already removed and no user manager is still a successful cleanup.
        service
            .remove(&mut |_, _| Err("no user manager".into()))
            .unwrap();
        assert!(!service.path().exists());
        assert_eq!(
            calls,
            [
                "loginctl enable-linger 1001",
                "systemctl --user daemon-reload",
                "systemctl --user enable --now semon-push.service",
                "loginctl enable-linger 1001",
                "systemctl --user daemon-reload",
                "systemctl --user enable --now semon-push.service",
                "systemctl --user restart semon-push.service",
                "systemctl --user stop semon-push.service",
                "systemctl --user disable semon-push.service",
                "systemctl --user daemon-reload",
            ]
        );
    }

    #[test]
    fn linux_unit_preserves_paths_and_escapes_exec() {
        let mut service = service(std::path::Path::new("/test"), Platform::Linux);
        service.binary = PathBuf::from("/opt/a b/quote\"$%/semon");
        service.config = PathBuf::from("/config/$literal%dir");
        let text = service.render();
        assert!(text.contains("ExecStart=\"/opt/a b/quote\\\"$$%%/semon\" push --watch\n"));
        assert!(text.contains("Environment=\"XDG_CONFIG_HOME=/config/$literal%%dir\"\n"));
        assert!(text.contains("Environment=\"XDG_STATE_HOME=/test/xdg-state\"\n"));
        assert!(text.contains("Environment=\"HOME=/test/home\"\n"));
        assert!(text.contains("Environment=\"CODEX_HOME=/custom/codex\"\n"));
        assert!(text.contains("Restart=on-failure\nRestartSec=5\n"));
        assert!(text.ends_with("WantedBy=default.target\n"));
        assert_eq!(
            service.path(),
            PathBuf::from("/config/$literal%dir/systemd/user/semon-push.service")
        );
    }

    #[test]
    fn linger_failure_does_not_prevent_start() {
        let root = tempfile::tempdir().unwrap();
        let service = service(root.path(), Platform::Linux);
        let mut calls = Vec::new();
        let message = start_message(&service, &mut |program, args| {
            record(&mut calls, program, args);
            if program == "loginctl" {
                Err("not permitted".into())
            } else {
                Ok(())
            }
        });
        assert!(message.contains("Background sync started; logs: journalctl --user -u semon-push"));
        assert_eq!(calls.len(), 3);
        assert!(service.path().exists());
    }

    #[test]
    fn no_init_system_preserves_install_and_prints_foreground_command() {
        let root = tempfile::tempdir().unwrap();
        let service = service(root.path(), Platform::Linux);
        let mut calls = Vec::new();
        let message = start_message(&service, &mut |program, args| {
            record(&mut calls, program, args);
            Err("cannot connect\nto user bus".into())
        });
        assert_eq!(
            calls,
            [
                "loginctl enable-linger 1001",
                "systemctl --user daemon-reload"
            ]
        );
        assert!(service.path().exists());
        assert!(message.starts_with("Background sync is not running (cannot connect to user bus)"));
        assert!(message.ends_with(&format!("run: '{}' push --watch", service.binary.display())));
        assert_eq!(message.lines().count(), 1);
    }

    #[test]
    fn mac_plist_and_pair_repair_unpair_sequence() {
        let root = tempfile::tempdir().unwrap();
        let mut service = service(root.path(), Platform::Mac);
        service.binary = root.path().join("bin/a&b<\"'>/semon");
        let text = service.render();
        assert!(text.contains("a&amp;b&lt;&quot;&apos;&gt;/semon</string>"));
        assert!(text.contains("<string>push</string><string>--watch</string>"));
        assert!(text.contains("<key>RunAtLoad</key><true/>"));
        assert!(text.contains("<key>KeepAlive</key><true/>"));
        assert!(text.contains("<key>XDG_CONFIG_HOME</key>"));
        assert!(text.contains("<key>XDG_STATE_HOME</key>"));
        assert!(text.contains("<key>CODEX_HOME</key><string>/custom/codex</string>"));
        assert!(text.contains("Library/Logs/semon-push.log</string>"));
        assert_eq!(
            service.path(),
            service
                .home
                .join("Library/LaunchAgents/ai.onsager.semon-push.plist")
        );
        let mut calls = Vec::new();
        let mut run = |program: &str, args: &[&str]| {
            record(&mut calls, program, args);
            // A missing job at first install must not prevent bootstrap.
            if args[0] == "bootout" {
                Err("not loaded".into())
            } else {
                Ok(())
            }
        };
        let message = start_message(&service, &mut run);
        assert!(message.contains(&service.log_path().display().to_string()));
        service.install(&mut run).unwrap();
        service.remove(&mut run).unwrap();
        service
            .remove(&mut |_, _| Err("not loaded".into()))
            .unwrap();
        assert!(!service.path().exists());
        let bootstrap = format!("launchctl bootstrap gui/1001 {}", service.path().display());
        assert_eq!(
            calls,
            [
                "launchctl bootout gui/1001/ai.onsager.semon-push".into(),
                bootstrap.clone(),
                "launchctl bootout gui/1001/ai.onsager.semon-push".into(),
                bootstrap,
                "launchctl bootout gui/1001/ai.onsager.semon-push".into(),
            ]
        );
    }
}
