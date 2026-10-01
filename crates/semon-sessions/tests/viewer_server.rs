//! The viewer answers every connection of a burst promptly, however many
//! other kept-alive connections are open. A browser opens several
//! connections at once for a page's script, styles and fonts and keeps them
//! alive; CI saw one of them sit unread for 9 s (#68).

use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::{Ipv4Addr, Shutdown, TcpListener, TcpStream},
    path::Path,
    sync::{Arc, Barrier},
    thread,
    time::{Duration, Instant},
};

use semon_sessions::{Options, ServeOptions, serve_listener};

/// Bursts to open. Each races the server's hand-off of new connections.
const ROUNDS: usize = 40;
/// Connections opened at once in a burst, about what a browser opens per host.
const BURST: usize = 8;
/// Held connections closed before each burst, so a few server threads are
/// idle when it arrives rather than none or many.
const FREED: usize = 2;
/// Leave room for both client and server descriptors under macOS's 256 cap.
const HELD_MAX: usize = 48;
/// Well under the 9 s stall, well over a loopback answer on a busy runner.
const BOUND: Duration = Duration::from_secs(2);
/// How long a stuck connection is given, once the held ones close, to show
/// that it was queued rather than lost.
const LATE_BOUND: Duration = Duration::from_secs(15);

fn options(root: &Path) -> Options {
    let options = Options {
        claude_home: root.join("claude"),
        claude_json: root.join(".claude.json"),
        codex_home: root.join("codex"),
        proc_root: root.join("proc"),
        cache: root.join("index.json"),
        all: true,
        since: Duration::from_secs(86400),
        session: None,
        facts: None,
        scan_window: false,
    };
    fs::create_dir_all(&options.proc_root).unwrap();
    fs::write(options.proc_root.join("locks"), "").unwrap();
    options
}

/// Starts the viewer on a free loopback port and waits until it accepts.
fn start_viewer() -> u16 {
    let root = std::env::temp_dir().join(format!("semon-viewer-server-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let sessions = options(&root);
    thread::spawn(move || {
        serve_listener(
            ServeOptions {
                sessions,
                machines: Vec::new(),
                received: None,
                listen: format!("127.0.0.1:{port}"),
            },
            listener,
        )
        .unwrap();
    });
    let started = Instant::now();
    while TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "viewer did not listen"
        );
        thread::sleep(Duration::from_millis(10));
    }
    port
}

/// Reads until the status line has arrived, or `bound` passes.
fn status_line(stream: &mut TcpStream, bound: Duration) -> Result<String, String> {
    let deadline = Instant::now() + bound;
    let mut received = Vec::new();
    let mut buffer = [0_u8; 512];
    loop {
        if let Some(end) = received.windows(2).position(|pair| pair == b"\r\n") {
            return Ok(String::from_utf8_lossy(&received[..end]).into_owned());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("no answer".to_owned());
        }
        stream.set_read_timeout(Some(left)).unwrap();
        match stream.read(&mut buffer) {
            Ok(0) => return Err("closed without an answer".to_owned()),
            Ok(read) => received.extend_from_slice(&buffer[..read]),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err("no answer".to_owned());
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Opens `BURST` connections at once, sends one request on each and keeps
/// every connection alive. Returns each with how its answer went.
fn burst(port: u16) -> Vec<(TcpStream, Result<Duration, String>)> {
    let barrier = Arc::new(Barrier::new(BURST));
    let workers: Vec<_> = (0..BURST)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
                let sent = Instant::now();
                write!(
                    stream,
                    "GET /shell.css HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
                )
                .unwrap();
                let answer = status_line(&mut stream, BOUND).map(|line| {
                    assert!(line.starts_with("HTTP/1.1 "), "not a status line: {line:?}");
                    sent.elapsed()
                });
                (stream, answer)
            })
        })
        .collect();
    workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect()
}

fn close(stream: TcpStream) {
    let _ = stream.shutdown(Shutdown::Both);
}

#[test]
fn every_connection_of_a_burst_is_answered_while_others_are_held() {
    let port = start_viewer();
    let mut held: Vec<TcpStream> = Vec::new();
    for round in 0..ROUNDS {
        for stream in held.drain(..FREED.min(held.len())) {
            close(stream);
        }
        // Let the server see those closes, so their threads are idle.
        thread::sleep(Duration::from_millis(50));
        let answers = burst(port);
        let stuck = answers.iter().filter(|(_, answer)| answer.is_err()).count();
        if stuck == 0 {
            held.extend(answers.into_iter().map(|(stream, _)| stream));
            for stream in held.drain(..held.len().saturating_sub(HELD_MAX)) {
                close(stream);
            }
            continue;
        }
        // Closing every other connection frees the server's threads. An
        // answer that then arrives was queued behind them, not lost.
        let open = held.len() + BURST - stuck;
        for stream in held.drain(..) {
            close(stream);
        }
        let mut late = Vec::new();
        for (mut stream, answer) in answers {
            match answer {
                Ok(_) => close(stream),
                Err(error) => {
                    let freed = Instant::now();
                    let after = status_line(&mut stream, LATE_BOUND)
                        .map(|_| {
                            format!(
                                "answered {} ms after the others closed",
                                freed.elapsed().as_millis()
                            )
                        })
                        .unwrap_or_else(|late_error| {
                            format!("{late_error} even after they closed")
                        });
                    late.push(format!("{error} within {BOUND:?}; {after}"));
                }
            }
        }
        panic!(
            "round {round}: {stuck} of {BURST} connections unanswered while {open} others were open: {late:?}"
        );
    }
    for stream in held {
        close(stream);
    }
    let root = std::env::temp_dir().join(format!("semon-viewer-server-{}", std::process::id()));
    fs::remove_dir_all(root).unwrap();
}
