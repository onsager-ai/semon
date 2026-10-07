//! Offline fixture consumer. Only synthetic private bootstrap is accepted stdin.
use semon_ssh::{
    Credential, Target,
    controller::{self, Binding, Install},
    discover,
};
use serde_json::Value;
use std::io::{self, Read};
use zeroize::Zeroizing;
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().take(131073).read_to_string(&mut input)?;
    let value: Value = serde_json::from_str(&input)?;
    let target = Target {
        address: "127.0.0.1".parse()?,
        port: std::env::var("SEMON_SSH_PORT")?.parse()?,
        username: "fixture".into(),
    };
    let key = Credential(Zeroizing::new(std::fs::read_to_string(std::env::var(
        "SEMON_SSH_KEY_FILE",
    )?)?));
    // This exclusively created local fixture's key is the authorized host. An
    // application must retain its independently verified pin instead.
    let pin = discover(&target).await?;
    if value["action"] == "capture" {
        let binary = std::fs::read(value["mirror"].as_str().ok_or("mirror required")?)?;
        let result = semon_ssh::bootstrap_native_receiver(
            &target,
            &pin,
            &key,
            semon_ssh::Bootstrap {
                operation: value["enrollment"].as_str().ok_or("enrollment required")?,
                destination: value["destination"]
                    .as_str()
                    .ok_or("destination required")?,
                token: value["token"].as_str().ok_or("token required")?,
                binary: &binary,
            },
            value["receiver"].as_str().ok_or("receiver required")?,
        )
        .await?;
        println!("{}", serde_json::json!({"running":result.running}));
        return Ok(());
    }
    let binary = std::fs::read(value["controller"].as_str().ok_or("controller required")?)?;
    let result = controller::install(
        &target,
        &pin,
        &key,
        Install {
            enrollment: value["enrollment"].as_str().ok_or("enrollment required")?,
            launch: value["launch"].as_str().ok_or("launch required")?,
            expires: value["expires"].as_u64().ok_or("expires required")?,
            binding: Binding {
                owner: "owner-307".into(),
                workspace: "workspace-307".into(),
                connection: "ssh-307".into(),
                session: "session-307".into(),
                model_connection: "codex-307".into(),
                model_generation: 1,
                epoch: 1,
            },
            native_directory: "/opt/semon-codex",
            controller: &binary,
            model_config: value["config"].as_str().ok_or("config required")?,
            model_key: "synthetic-private-307-key",
        },
    )
    .await?;
    println!(
        "{}",
        serde_json::json!({"launch":result.launch,"state":result.state,"thread":result.thread})
    );
    Ok(())
}
