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
                format!(
                    "[Unit]\nDescription=Semon background session sync\n\n[Service]\nExecStart={} push --watch\n{environment}Restart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
                    systemd_quote(&binary)
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
                run("systemctl", &["--user", "restart", UNIT])?;
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
            .replace('$', "$$")
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

pub fn start() {
    let service = Service::current();
    let result = service
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|s| s.install(&mut command));
    match result {
        Ok(()) => match service.unwrap().platform {
            Platform::Linux => {
                println!("Background sync started; logs: journalctl --user -u semon-push")
            }
            Platform::Mac => println!(
                "Background sync started; logs: {}",
                Service::current().unwrap().log_path().display()
            ),
        },
        Err(error) => {
            let binary = env::current_exe().unwrap_or_else(|_| PathBuf::from("semon"));
            let quoted = format!("'{}'", binary.to_string_lossy().replace('\'', "'\\''"));
            eprintln!("Background sync is not running ({error}); run: {quoted} push --watch");
        }
    }
}

pub fn stop() -> Result<(), String> {
    Service::current()?.remove(&mut command)
}
