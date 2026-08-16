use std::{env, path::PathBuf, process::ExitCode};

use semon_store::{REPLICATION_ENDPOINT_ENV, TraceStore, ship};

fn main() -> ExitCode {
    match parse_args().and_then(run) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("semon: {error}");
            ExitCode::FAILURE
        }
    }
}

struct ShipArgs {
    store: PathBuf,
    endpoint: Option<String>,
}

fn parse_args() -> Result<ShipArgs, String> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        Some("ship") => {}
        Some("-h" | "--help") => return Err(usage()),
        Some(command) => return Err(format!("unknown command: {command}\n{}", usage())),
        None => return Err(usage()),
    }

    let mut store = default_store_path();
    let mut endpoint = env::var(REPLICATION_ENDPOINT_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    while let Some(argument) = arguments.next() {
        let value = |arguments: &mut std::iter::Skip<std::env::Args>| {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))
        };
        match argument.as_str() {
            "--store" => store = value(&mut arguments)?.into(),
            "--endpoint" => endpoint = Some(value(&mut arguments)?),
            "-h" | "--help" => return Err(usage()),
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(ShipArgs { store, endpoint })
}

fn run(args: ShipArgs) -> Result<String, String> {
    let Some(endpoint) = args
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(format!(
            "ship: no endpoint configured ({REPLICATION_ENDPOINT_ENV} unset); skipping"
        ));
    };

    let store = TraceStore::open(&args.store).map_err(|error| error.to_string())?;
    let report = ship(&store, Some(endpoint)).map_err(|error| error.to_string())?;
    Ok(format!(
        "shipped {} trace(s) to {endpoint}",
        report.shipped()
    ))
}

fn default_store_path() -> PathBuf {
    if let Some(root) = env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(root).join("semon/traces.sqlite3");
    }
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/semon/traces.sqlite3")
}

fn usage() -> String {
    format!(
        "Usage: semon ship [--store PATH] [--endpoint URL]\n\
         Endpoint defaults to ${REPLICATION_ENDPOINT_ENV}; when unset, ship succeeds without reading the store."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_ship_succeeds_without_opening_the_store() {
        let args = ShipArgs {
            store: PathBuf::from("this-store-does-not-exist.sqlite3"),
            endpoint: None,
        };

        let message = run(args).unwrap();

        assert!(message.contains("skipping"));
    }
}
