use std::{env, path::PathBuf, process::ExitCode, time::Duration};
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("semon-copilot: {error}");
            ExitCode::FAILURE
        }
    }
}
fn run() -> Result<(), String> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let state_root = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/state"))
        .join("semon");
    let mut native = env::var_os("COPILOT_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".copilot"));
    let mut state = state_root.join("copilot-capture.json");
    let mut store = env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"))
        .join("semon/traces.sqlite3");
    let mut watch = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--copilot-home" => {
                native = PathBuf::from(args.next().ok_or("missing --copilot-home value")?)
            }
            "--state" => state = PathBuf::from(args.next().ok_or("missing --state value")?),
            "--store" => store = PathBuf::from(args.next().ok_or("missing --store value")?),
            "--watch" => watch = true,
            "--help" | "-h" => {
                println!(
                    "semon-copilot [--copilot-home PATH] [--state PATH] [--store PATH] [--watch]\nRead-only Linux Copilot CLI 1.0.90/1.0.91 schema 1 capture.\nWithout --watch, collect all complete records; --watch polls every two seconds."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    let mut collector = semon_copilot::Collector::open(&native, &state, &store)
        .map_err(|error| error.to_string())?;
    loop {
        let count = collector.collect().map_err(|error| error.to_string())?;
        if count > 0 {
            eprintln!("captured {count} complete persisted records");
        }
        if !watch {
            if count == 0 {
                break;
            }
            continue;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Ok(())
}
