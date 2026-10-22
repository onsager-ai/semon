//! Shared borrowed decoder for Codex unified-exec response headers.

/// The leading digits of `text` as a number, and what follows them.
fn digits(text: &str) -> Option<(u64, &str)> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    Some((text.get(..end)?.parse().ok()?, &text[end..]))
}

/// Unified exec's response header is short. Bound every newline search as
/// well as the number of lines so a large first output line is never scanned
/// just to decide whether a formatter header is present.
const EXEC_RESPONSE_HEADER_MAX_BYTES: usize = 2 * 1024;
const EXEC_RESPONSE_HEADER_MAX_LINES: usize = 8;

/// The borrowed result of decoding unified exec's response header. The
/// display fields retain Semon's older permissive frame recognition; only a
/// complete header in the pinned field order sets `native_qualified` and may
/// supply exit/session evidence.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExecResponseFrame<'a> {
    pub(crate) body: &'a str,
    pub(crate) original_token_count: Option<u64>,
    pub(crate) exit_code: Option<i32>,
    /// The source spelling is kept intact; this is not a liveness or join.
    pub(crate) session_id: Option<&'a str>,
    pub(crate) native_qualified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExecResponseOutcome {
    Exited(i32),
    SessionOnly,
    NoExit,
}

impl ExecResponseFrame<'_> {
    /// Classify only a qualified header. A session-only header is distinct
    /// source evidence, but does not settle the call outcome.
    pub(crate) fn native_outcome(&self) -> Option<ExecResponseOutcome> {
        if !self.native_qualified {
            return None;
        }
        Some(match (self.exit_code, self.session_id) {
            (Some(code), _) => ExecResponseOutcome::Exited(code),
            (None, Some(_)) => ExecResponseOutcome::SessionOnly,
            (None, None) => ExecResponseOutcome::NoExit,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeHeaderStage {
    Start,
    WallTime,
    AfterWallTime,
    AfterExit,
    AfterSession,
    AfterTokenCount,
    Invalid,
}

fn exact_i32(value: &str) -> Option<i32> {
    let parsed = value.parse::<i32>().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

fn exact_usize(value: &str) -> Option<usize> {
    let parsed = value.parse::<usize>().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

fn native_wall_time(line: &str) -> bool {
    let Some(number) = line
        .strip_prefix("Wall time: ")
        .and_then(|line| line.strip_suffix(" seconds"))
    else {
        return false;
    };
    let Some((whole, fraction)) = number.split_once('.') else {
        return false;
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(whole)
        || fraction.len() != 4
        || !digits(fraction)
        || (whole.len() > 1 && whole.starts_with('0'))
    {
        return false;
    }
    number
        .parse::<f64>()
        .is_ok_and(|seconds| seconds.is_finite() && seconds >= 0.0)
}

fn native_header_line<'a>(
    stage: &mut NativeHeaderStage,
    line: &'a str,
    exit_code: &mut Option<i32>,
    session_id: &mut Option<&'a str>,
) {
    if *stage == NativeHeaderStage::Start {
        if let Some(chunk_id) = line.strip_prefix("Chunk ID: ") {
            if chunk_id.is_empty() {
                *stage = NativeHeaderStage::Invalid;
            } else {
                *stage = NativeHeaderStage::WallTime;
            }
            return;
        }
        *stage = NativeHeaderStage::WallTime;
    }

    *stage = match *stage {
        NativeHeaderStage::WallTime if native_wall_time(line) => NativeHeaderStage::AfterWallTime,
        NativeHeaderStage::AfterWallTime => {
            if let Some(code) = line
                .strip_prefix("Process exited with code ")
                .and_then(exact_i32)
            {
                *exit_code = Some(code);
                NativeHeaderStage::AfterExit
            } else if let Some(session) = line
                .strip_prefix("Process running with session ID ")
                .filter(|value| exact_i32(value).is_some())
            {
                *session_id = Some(session);
                NativeHeaderStage::AfterSession
            } else if line
                .strip_prefix("Original token count: ")
                .and_then(exact_usize)
                .is_some()
            {
                NativeHeaderStage::AfterTokenCount
            } else {
                NativeHeaderStage::Invalid
            }
        }
        NativeHeaderStage::AfterExit => {
            if let Some(session) = line
                .strip_prefix("Process running with session ID ")
                .filter(|value| exact_i32(value).is_some())
            {
                *session_id = Some(session);
                NativeHeaderStage::AfterSession
            } else if line
                .strip_prefix("Original token count: ")
                .and_then(exact_usize)
                .is_some()
            {
                NativeHeaderStage::AfterTokenCount
            } else {
                NativeHeaderStage::Invalid
            }
        }
        NativeHeaderStage::AfterSession => {
            if line
                .strip_prefix("Original token count: ")
                .and_then(exact_usize)
                .is_some()
            {
                NativeHeaderStage::AfterTokenCount
            } else {
                NativeHeaderStage::Invalid
            }
        }
        _ => NativeHeaderStage::Invalid,
    };
}

/// Decode one unified-exec response. `Output:` is the end of the bounded
/// header; every following byte belongs to the command. The shared decoder
/// retains legacy display qualification while restricting process evidence
/// to the complete, ordered pinned header.
pub(crate) fn decode_exec_response(text: &str) -> Option<ExecResponseFrame<'_>> {
    let mut rest = text;
    let mut scanned = 0usize;
    let mut lines = 0usize;
    let mut display_timed = false;
    let mut display_tokens = None;
    let mut native_stage = NativeHeaderStage::Start;
    let mut exit_code = None;
    let mut session_id = None;

    while lines < EXEC_RESPONSE_HEADER_MAX_LINES {
        let remaining = EXEC_RESPONSE_HEADER_MAX_BYTES.saturating_sub(scanned);
        let search = rest.len().min(remaining);
        let newline = rest
            .as_bytes()
            .get(..search)?
            .iter()
            .position(|byte| *byte == b'\n')?;
        let line = &rest[..newline];
        let after = &rest[newline + 1..];
        let consumed = newline + 1;
        scanned += consumed;
        lines += 1;

        if line == "Output:" {
            let native_qualified = matches!(
                native_stage,
                NativeHeaderStage::AfterWallTime
                    | NativeHeaderStage::AfterExit
                    | NativeHeaderStage::AfterSession
                    | NativeHeaderStage::AfterTokenCount
            );
            return display_timed.then_some(ExecResponseFrame {
                body: after,
                original_token_count: display_tokens,
                exit_code: native_qualified.then_some(exit_code).flatten(),
                session_id: native_qualified.then_some(session_id).flatten(),
                native_qualified,
            });
        }

        if let Some(number) = line.strip_prefix("Original token count: ") {
            display_tokens = digits(number)
                .filter(|(_, rest)| rest.is_empty())
                .map(|(number, _)| number);
        } else if line.starts_with("Wall time: ") && line.ends_with(" seconds") {
            display_timed = true;
        } else if ![
            "Chunk ID: ",
            "Process exited with code ",
            "Process running with session ID ",
        ]
        .iter()
        .any(|prefix| line.starts_with(prefix))
        {
            return None;
        }

        native_header_line(&mut native_stage, line, &mut exit_code, &mut session_id);
        rest = after;
    }
    None
}
