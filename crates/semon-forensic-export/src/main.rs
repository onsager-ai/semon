//! Explicit historical-data access, independent of retired capture/Store APIs.
use std::{ffi::OsString, path::PathBuf, process::ExitCode};

const USAGE: &str = "Usage: semon-forensic-export --store PATH --out NEW_DIRECTORY\n\
    Export historical Canonical Trace Store schemas 0..=7. Pause all writers first.\n\
    Originals remain unchanged. Output contains sensitive forensic evidence.";

fn arguments() -> Result<Option<(PathBuf, PathBuf)>, String> {
    let mut args = std::env::args_os().skip(1);
    let mut source = None;
    let mut destination = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help")
                if source.is_none() && destination.is_none() && args.next().is_none() =>
            {
                return Ok(None);
            }
            Some("--store") if source.is_none() => {
                source = Some(PathBuf::from(value(&mut args)?));
            }
            Some("--out") if destination.is_none() => {
                destination = Some(PathBuf::from(value(&mut args)?));
            }
            _ => return Err(USAGE.into()),
        }
    }
    match (source, destination) {
        (Some(source), Some(destination)) => Ok(Some((source, destination))),
        _ => Err(USAGE.into()),
    }
}

fn value(args: &mut impl Iterator<Item = OsString>) -> Result<OsString, String> {
    args.next()
        .filter(|value| !value.is_empty() && !value.to_string_lossy().starts_with("--"))
        .ok_or_else(|| USAGE.into())
}

fn main() -> ExitCode {
    let (source, destination) = match arguments() {
        Ok(Some(paths)) => paths,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    eprintln!(
        "semon-forensic-export: {}",
        semon_forensic_export::FORENSIC_WARNING
    );
    match semon_forensic_export::export(&source, &destination) {
        Ok(()) => {
            println!(
                "Complete forensic Store export written to {}",
                destination.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("semon-forensic-export: {error}");
            ExitCode::FAILURE
        }
    }
}
