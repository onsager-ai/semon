use std::{env, fs, path::PathBuf, process::ExitCode};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use semon_codex::{
    ProcessOptions, candidate_files, default_state_path, default_store_path, load_state,
    process_file,
};
use semon_store::TraceStore;

#[derive(Debug)]
struct Args {
    sessions: PathBuf,
    history: PathBuf,
    state: PathBuf,
    store: PathBuf,
    repo: String,
    verbose: bool,
}

fn main() -> ExitCode {
    match parse_args().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("semon-codex: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    ensure_store_dir(&args.store)?;
    let mut store = TraceStore::open(&args.store).map_err(|error| error.to_string())?;
    let mut state = load_state(&args.state).map_err(|error| error.to_string())?;
    let mut total = 0;
    for path in candidate_files(&args.sessions, &args.history).map_err(|error| error.to_string())? {
        let mut options = ProcessOptions::new(&args.state, &args.history);
        options.repo_override = &args.repo;
        total += process_file(&path, &mut state, &mut store, &options)
            .map_err(|error| error.to_string())?;
    }
    if args.verbose {
        eprintln!("processed {total} record(s)");
    }
    Ok(())
}

/// Creates the store's parent directory and keeps it owner-only (`0700` on
/// Unix), re-asserted on every run rather than trusted from a prior one —
/// the store this directory holds can carry prompts, responses, source
/// code, credentials, and machine paths (see
/// `docs/design/forensic-retention-and-exposure.md`); the store file itself
/// is separately tightened by `TraceStore::open`.
fn ensure_store_dir(store: &std::path::Path) -> Result<(), String> {
    let Some(parent) = store
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn parse_args() -> Result<Args, String> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut result = Args {
        sessions: home.join(".codex/sessions"),
        history: home.join(".codex/history.jsonl"),
        state: default_state_path(),
        store: default_store_path(),
        repo: env::var("SEMON_REPO").unwrap_or_default(),
        verbose: false,
    };
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = |arguments: &mut std::iter::Skip<std::env::Args>| {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a path"))
        };
        match argument.as_str() {
            "--sessions" => result.sessions = value(&mut arguments)?.into(),
            "--history" => result.history = value(&mut arguments)?.into(),
            "--state" => result.state = value(&mut arguments)?.into(),
            "--store" => result.store = value(&mut arguments)?.into(),
            "--repo" => result.repo = value(&mut arguments)?,
            "-v" | "--verbose" => result.verbose = true,
            "-h" | "--help" => {
                println!(
                    "Usage: semon-codex [--sessions PATH] [--history PATH] \\\n+                     [--state PATH] [--store PATH] [--repo NAME] [--verbose]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(result)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn unique_temp_dir(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "semon-codex-test-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn store_dir_is_created_owner_only() {
        let dir = unique_temp_dir("created");
        let store = dir.join("traces.sqlite3");

        ensure_store_dir(&store).unwrap();

        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "store dir must be created 0700, got {mode:o}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn loosened_store_dir_permissions_are_retightened() {
        let dir = unique_temp_dir("retightened");
        let store = dir.join("traces.sqlite3");
        ensure_store_dir(&store).unwrap();

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let loosened = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            loosened, 0o755,
            "test setup did not actually loosen permissions"
        );

        ensure_store_dir(&store).unwrap();

        let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "reopening must re-tighten the store dir rather than trust it, got {mode:o}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
