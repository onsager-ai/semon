use std::{env, fs, path::PathBuf, process::ExitCode};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use semon_claude::{
    ProcessOptions, backfill_raw, candidate_files, default_state_path, default_store_path,
    load_state, process_file,
};
use semon_store::TraceStore;

#[derive(Debug)]
struct Args {
    projects: PathBuf,
    state: PathBuf,
    store: PathBuf,
    repo: String,
    verbose: bool,
    backfill_raw: bool,
    dry_run: bool,
}

fn main() -> ExitCode {
    match parse_args().and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("semon-claude: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    if !args.dry_run {
        ensure_store_dir(&args.store)?;
    }
    let mut store = TraceStore::open(&args.store).map_err(|error| error.to_string())?;
    let mut state = load_state(&args.state).map_err(|error| error.to_string())?;
    if args.backfill_raw {
        let report =
            backfill_raw(&state, &mut store, args.dry_run).map_err(|error| error.to_string())?;
        for path in &report.missing {
            println!("missing: {}", path.display());
        }
        for path in &report.rewritten {
            println!("rewritten: {}", path.display());
        }
        for (path, sequence) in &report.misaligned {
            println!(
                "misaligned: {} (first mismatching sequence {sequence})",
                path.display()
            );
        }
        let label = if args.dry_run {
            "rows would insert"
        } else {
            "rows inserted"
        };
        println!(
            "files scanned: {}, lines scanned: {}, {label}: {}, rows already present: {}, files missing: {}, files rewritten: {}, files misaligned: {}",
            report.files_scanned,
            report.lines_scanned,
            report.rows_inserted,
            report.rows_already_present,
            report.missing.len(),
            report.rewritten.len(),
            report.misaligned.len()
        );
        return Ok(());
    }
    let mut total = 0;
    for path in candidate_files(&args.projects).map_err(|error| error.to_string())? {
        let mut options = ProcessOptions::new(&args.state);
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
/// identical rationale to `semon-codex`'s (see
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
        projects: home.join(".claude/projects"),
        state: default_state_path(),
        store: default_store_path(),
        repo: env::var("SEMON_REPO").unwrap_or_default(),
        verbose: false,
        backfill_raw: false,
        dry_run: false,
    };
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = |arguments: &mut std::iter::Skip<std::env::Args>| {
            arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a path"))
        };
        match argument.as_str() {
            "--projects" => result.projects = value(&mut arguments)?.into(),
            "--state" => result.state = value(&mut arguments)?.into(),
            "--store" => result.store = value(&mut arguments)?.into(),
            "--repo" => result.repo = value(&mut arguments)?,
            "-v" | "--verbose" => result.verbose = true,
            "--backfill-raw" => result.backfill_raw = true,
            "--dry-run" => result.dry_run = true,
            "-h" | "--help" => {
                println!(
                    "Usage: semon-claude [--projects PATH] [--state PATH] [--store PATH] [--repo NAME] [--verbose] [--backfill-raw [--dry-run]]"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    if result.dry_run && !result.backfill_raw {
        return Err("--dry-run requires --backfill-raw".into());
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
            "semon-claude-test-{label}-{}-{unique}",
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
