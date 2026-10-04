//! Private bounded transport for explicit provider/guest actions. The embedding
//! commits intent before calling, resolves credentials, and reconciles lost replies.
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
use zeroize::Zeroizing;

#[derive(Serialize)]
pub struct Request<'a> {
    pub method: &'a str,
    pub api_key: &'a str,
    pub runtime: Option<&'a str>,
    pub labels: &'a super::Labels,
    pub payload: serde_json::Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    pub status: String,
    pub runtime: Option<String>,
    pub thread: Option<String>,
}
pub struct Worker<'a> {
    pub python: &'a Path,
    pub script: &'a Path,
    pub environment_exclusions: &'a [OsString],
}
impl Worker<'_> {
    /// Errors are deliberately fixed; provider output never enters diagnostics.
    pub async fn execute(&self, request: &Request<'_>) -> Result<Reply, &'static str> {
        if !self.python.is_absolute() || !self.script.is_absolute() {
            return Err("invalid runtime worker configuration");
        }
        let input =
            Zeroizing::new(serde_json::to_vec(request).map_err(|_| "invalid runtime request")?);
        if input.len() > 262144 {
            return Err("runtime request too large");
        }
        let mut command = Command::new(self.python);
        for name in self.environment_exclusions {
            command.env_remove(name);
        }
        for name in [
            "E2B_API_KEY",
            "E2B_ACCESS_TOKEN",
            "E2B_API_URL",
            "E2B_DOMAIN",
            "E2B_DEBUG",
            "E2B_SANDBOX_URL",
            "OPENAI_API_KEY",
            "CODEX_ACCESS_TOKEN",
        ] {
            command.env_remove(name);
        }
        let mut child = command
            .args(["-I", "-B"])
            .arg(self.script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "runtime worker unavailable")?;
        let mut stdin = child.stdin.take().ok_or("runtime worker unavailable")?;
        let stdout = child.stdout.take().ok_or("runtime worker unavailable")?;
        let result = tokio::time::timeout(Duration::from_secs(120), async {
            let mut output = Zeroizing::new(Vec::new());
            tokio::try_join!(
                async {
                    stdin.write_all(&input).await?;
                    drop(stdin);
                    Ok::<_, std::io::Error>(())
                },
                async { stdout.take(8193).read_to_end(&mut output).await.map(|_| ()) }
            )
            .map_err(|_| "runtime worker unavailable")?;
            if output.len() > 8192
                || !child
                    .wait()
                    .await
                    .map_err(|_| "runtime worker unavailable")?
                    .success()
            {
                return Err("runtime outcome unknown");
            }
            serde_json::from_slice(&output).map_err(|_| "runtime outcome unknown")
        })
        .await
        .unwrap_or(Err("runtime outcome unknown"));
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
        }
        result
    }
}
