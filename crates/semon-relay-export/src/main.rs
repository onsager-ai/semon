use semon_relay_export::{Input, Kind};
use std::{ffi::OsString, path::PathBuf, process::ExitCode};

const USAGE: &str = "Usage: semon-relay-export [--receiver-dir DIR] [--config-dir DIR] \
    [--sender-dir DIR] [--recovery-key FILE]... --out NEW_DIRECTORY\n\
    Each input flag may repeat; at least one is required. No default discovery.\n\
    Quiesce every writer first. Preserve all configured custody roots and keys.\n\
    Copies are private and byte-verified; decryption must be qualified separately.";

fn arguments() -> Result<Option<(Vec<Input>, PathBuf)>, String> {
    let mut args = std::env::args_os().skip(1);
    let mut inputs = Vec::new();
    let mut destination = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help")
                if inputs.is_empty() && destination.is_none() && args.next().is_none() =>
            {
                return Ok(None);
            }
            Some("--out") if destination.is_none() => {
                destination = Some(PathBuf::from(value(&mut args)?))
            }
            Some(
                flag @ ("--receiver-dir" | "--config-dir" | "--sender-dir" | "--recovery-key"),
            ) => {
                let kind = match flag {
                    "--receiver-dir" => Kind::Receiver,
                    "--config-dir" => Kind::Config,
                    "--sender-dir" => Kind::Sender,
                    _ => Kind::RecoveryKey,
                };
                inputs.push(Input {
                    kind,
                    path: PathBuf::from(value(&mut args)?),
                });
            }
            _ => return Err(USAGE.into()),
        }
    }
    match destination {
        Some(out) if !inputs.is_empty() => Ok(Some((inputs, out))),
        _ => Err(USAGE.into()),
    }
}

fn value(args: &mut impl Iterator<Item = OsString>) -> Result<OsString, String> {
    args.next()
        .filter(|arg| !arg.is_empty() && !arg.to_string_lossy().starts_with("--"))
        .ok_or_else(|| USAGE.into())
}

fn main() -> ExitCode {
    let (inputs, out) = match arguments() {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    eprintln!("semon-relay-export: {}", semon_relay_export::WARNING);
    match semon_relay_export::export(&inputs, &out) {
        Ok(()) => {
            println!(
                "Verified custody copy written to {}. Decryption is not verified.",
                out.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("semon-relay-export: {error}");
            ExitCode::FAILURE
        }
    }
}
