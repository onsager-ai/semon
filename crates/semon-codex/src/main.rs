use std::{env, fs, path::PathBuf, process::ExitCode};

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
    if let Some(parent) = args
        .store
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
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
