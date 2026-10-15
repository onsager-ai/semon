//! Resumable, content-free native JSON record syntax and span production.
//! Native harness classification remains in the event producer. This cursor
//! emits descriptors, never decoded keys, strings or native record bodies.

use super::{MAX_DEPTH, invalid, limit, native_scan};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Span {
    pub(crate) start: u64,
    pub(crate) end: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Container {
    Object,
    Array,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Location {
    pub(crate) parent: Option<u64>,
    pub(crate) key: Option<Span>,
    pub(crate) index: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Token {
    Begin {
        at: u64,
        kind: Container,
        location: Location,
    },
    End {
        at: u64,
        kind: Container,
    },
    Checkpoint {
        start: u64,
        at: u64,
        key: bool,
    },
    String {
        span: Span,
        location: Location,
        key: bool,
    },
    Number {
        span: Span,
        location: Location,
    },
    Literal {
        span: Span,
        location: Location,
        kind: Literal,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Literal {
    True,
    False,
    Null,
}
impl Literal {
    pub(super) fn bytes(self) -> &'static [u8] {
        match self {
            Self::True => b"true",
            Self::False => b"false",
            Self::Null => b"null",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Stage {
    FirstKey,
    Key,
    Colon,
    Value,
    FirstValue,
    Comma,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Frame {
    at: u64,
    kind: Container,
    stage: Stage,
    key: Option<Span>,
    index: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Number {
    Sign,
    Zero,
    Integer,
    Dot,
    Fraction,
    Exponent,
    ExponentSign,
    ExponentDigits,
}
impl Number {
    pub(super) fn complete(self) -> bool {
        matches!(
            self,
            Self::Zero | Self::Integer | Self::Fraction | Self::ExponentDigits
        )
    }
    pub(super) fn next(self, byte: u8) -> io::Result<Option<Self>> {
        let next = match (self, byte) {
            (Self::Sign, b'0') => Self::Zero,
            (Self::Sign, b'1'..=b'9') => Self::Integer,
            (Self::Integer, b'0'..=b'9') => Self::Integer,
            (Self::Zero | Self::Integer, b'.') => Self::Dot,
            (Self::Zero | Self::Integer | Self::Fraction, b'e' | b'E') => Self::Exponent,
            (Self::Dot | Self::Fraction, b'0'..=b'9') => Self::Fraction,
            (Self::Exponent, b'+' | b'-') => Self::ExponentSign,
            (Self::Exponent | Self::ExponentSign | Self::ExponentDigits, b'0'..=b'9') => {
                Self::ExponentDigits
            }
            (_, b' ' | b'\n' | b'\r' | b'\t' | b',' | b']' | b'}') if self.complete() => {
                return Ok(None);
            }
            _ => return Err(invalid()),
        };
        Ok(Some(next))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Mode {
    Normal,
    String {
        cursor: native_scan::Cursor,
        key: bool,
        location: Location,
    },
    Number {
        at: u64,
        state: Number,
        location: Location,
    },
    Literal {
        at: u64,
        position: usize,
        kind: Literal,
        location: Location,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    pub(crate) at: u64,
    frames: Vec<Frame>,
    mode: Mode,
    started: bool,
    complete: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Step {
    pub(crate) consumed: usize,
    pub(crate) tokens: usize,
    pub(crate) complete: bool,
}
impl Cursor {
    pub(crate) fn new() -> Self {
        Self {
            at: 0,
            frames: Vec::new(),
            mode: Mode::Normal,
            started: false,
            complete: false,
        }
    }
    fn value(&mut self) -> io::Result<Location> {
        if self.frames.len() > MAX_DEPTH {
            return Err(limit());
        }
        if let Some(frame) = self.frames.last_mut() {
            let location = match frame.kind {
                Container::Object if frame.stage == Stage::Value => Location {
                    parent: Some(frame.at),
                    key: frame.key.take(),
                    index: None,
                },
                Container::Array if matches!(frame.stage, Stage::FirstValue | Stage::Value) => {
                    let index = frame.index;
                    frame.index = frame.index.checked_add(1).ok_or_else(limit)?;
                    Location {
                        parent: Some(frame.at),
                        key: None,
                        index: Some(index),
                    }
                }
                _ => return Err(invalid()),
            };
            frame.stage = Stage::Comma;
            Ok(location)
        } else if !self.started {
            self.started = true;
            Ok(Location {
                parent: None,
                key: None,
                index: None,
            })
        } else {
            Err(invalid())
        }
    }
    fn primitive_complete(&mut self) {
        if self.frames.is_empty() {
            self.complete = true;
        }
    }
    /// A step consumes at most `budget` plus one complete JSON scalar (eleven
    /// extra bytes), and emits at most `token_budget` descriptors. Checkpoint
    /// emissions count toward the latter. Cursor and sink rows commit together.
    pub(crate) fn step(
        &mut self,
        bytes: &[u8],
        final_input: bool,
        budget: usize,
        token_budget: usize,
        checkpoint_bytes: usize,
        mut emit: impl FnMut(Token) -> io::Result<()>,
    ) -> io::Result<Step> {
        if budget == 0
            || token_budget < 3
            || checkpoint_bytes == 0
            || self.frames.len() > MAX_DEPTH + 1
        {
            return Err(invalid());
        }
        let mut used = 0;
        let mut tokens = 0;
        while used < bytes.len() && used < budget && token_budget - tokens >= 3 {
            let mode = std::mem::replace(&mut self.mode, Mode::Normal);
            match mode {
                Mode::String {
                    mut cursor,
                    key,
                    location,
                } => {
                    let start = cursor.start;
                    let step = cursor.step(
                        &bytes[used..],
                        final_input,
                        (budget - used).min(checkpoint_bytes),
                        checkpoint_bytes,
                        |at| {
                            emit(Token::Checkpoint { start, at, key })?;
                            tokens += 1;
                            Ok(())
                        },
                    )?;
                    used += step.consumed;
                    self.at = cursor.at;
                    if let Some(end) = step.end {
                        self.at = self.at.checked_add(1).ok_or_else(invalid)?;
                        let span = Span { start, end };
                        if key {
                            let frame = self.frames.last_mut().ok_or_else(invalid)?;
                            frame.key = Some(span);
                            frame.stage = Stage::Colon;
                        } else {
                            self.primitive_complete();
                        }
                        emit(Token::String {
                            span,
                            location,
                            key,
                        })?;
                        tokens += 1;
                    } else {
                        self.mode = Mode::String {
                            cursor,
                            key,
                            location,
                        };
                        if step.consumed == 0 {
                            break;
                        }
                    }
                }
                Mode::Number {
                    at,
                    mut state,
                    location,
                } => {
                    let next = state.next(bytes[used])?;
                    if let Some(next) = next {
                        state = next;
                        used += 1;
                        self.at = self.at.checked_add(1).ok_or_else(invalid)?;
                        self.mode = Mode::Number {
                            at,
                            state,
                            location,
                        };
                    } else {
                        emit(Token::Number {
                            span: Span {
                                start: at,
                                end: self.at,
                            },
                            location,
                        })?;
                        tokens += 1;
                        self.primitive_complete();
                    }
                }
                Mode::Literal {
                    at,
                    mut position,
                    kind,
                    location,
                } => {
                    if kind.bytes().get(position) != Some(&bytes[used]) {
                        return Err(invalid());
                    }
                    position += 1;
                    used += 1;
                    self.at = self.at.checked_add(1).ok_or_else(invalid)?;
                    if position == kind.bytes().len() {
                        emit(Token::Literal {
                            span: Span {
                                start: at,
                                end: self.at,
                            },
                            location,
                            kind,
                        })?;
                        tokens += 1;
                        self.primitive_complete();
                    } else {
                        self.mode = Mode::Literal {
                            at,
                            position,
                            kind,
                            location,
                        };
                    }
                }
                Mode::Normal => {
                    let byte = bytes[used];
                    if matches!(byte, b' ' | b'\n' | b'\r' | b'\t') {
                        used += 1;
                        self.at = self.at.checked_add(1).ok_or_else(invalid)?;
                        continue;
                    }
                    if let Some(frame) = self.frames.last_mut() {
                        if frame.stage == Stage::Colon {
                            if byte != b':' {
                                return Err(invalid());
                            }
                            frame.stage = Stage::Value;
                            used += 1;
                            self.at = self.at.checked_add(1).ok_or_else(invalid)?;
                            continue;
                        }
                        let close = match frame.kind {
                            Container::Object => b'}',
                            Container::Array => b']',
                        };
                        if byte == close
                            && matches!(
                                frame.stage,
                                Stage::Comma | Stage::FirstKey | Stage::FirstValue
                            )
                        {
                            let kind = frame.kind;
                            self.frames.pop();
                            emit(Token::End { at: self.at, kind })?;
                            tokens += 1;
                            used += 1;
                            self.at = self.at.checked_add(1).ok_or_else(invalid)?;
                            self.primitive_complete();
                            continue;
                        }
                        if frame.stage == Stage::Comma {
                            if byte != b',' {
                                return Err(invalid());
                            }
                            frame.stage = match frame.kind {
                                Container::Object => Stage::Key,
                                Container::Array => Stage::Value,
                            };
                            used += 1;
                            self.at = self.at.checked_add(1).ok_or_else(invalid)?;
                            continue;
                        }
                        if matches!(frame.stage, Stage::FirstKey | Stage::Key) {
                            if byte != b'"' {
                                return Err(invalid());
                            }
                            let start = self.at.checked_add(1).ok_or_else(invalid)?;
                            let location = Location {
                                parent: Some(frame.at),
                                key: None,
                                index: None,
                            };
                            self.mode = Mode::String {
                                cursor: native_scan::Cursor::new(start),
                                key: true,
                                location,
                            };
                            emit(Token::Checkpoint {
                                start,
                                at: start,
                                key: true,
                            })?;
                            tokens += 1;
                            used += 1;
                            self.at = start;
                            continue;
                        }
                    }
                    let location = self.value()?;
                    let at = self.at;
                    used += 1;
                    self.at = self.at.checked_add(1).ok_or_else(invalid)?;
                    match byte {
                        b'{' | b'[' => {
                            if self.frames.len() > MAX_DEPTH {
                                return Err(limit());
                            }
                            let kind = if byte == b'{' {
                                Container::Object
                            } else {
                                Container::Array
                            };
                            emit(Token::Begin { at, kind, location })?;
                            tokens += 1;
                            self.frames.push(Frame {
                                at,
                                kind,
                                stage: if kind == Container::Object {
                                    Stage::FirstKey
                                } else {
                                    Stage::FirstValue
                                },
                                key: None,
                                index: 0,
                            });
                        }
                        b'"' => {
                            emit(Token::Checkpoint {
                                start: self.at,
                                at: self.at,
                                key: false,
                            })?;
                            tokens += 1;
                            self.mode = Mode::String {
                                cursor: native_scan::Cursor::new(self.at),
                                key: false,
                                location,
                            };
                        }
                        b'-' | b'0'..=b'9' => {
                            let state = match byte {
                                b'-' => Number::Sign,
                                b'0' => Number::Zero,
                                _ => Number::Integer,
                            };
                            self.mode = Mode::Number {
                                at,
                                state,
                                location,
                            };
                        }
                        b't' | b'f' | b'n' => {
                            let kind = match byte {
                                b't' => Literal::True,
                                b'f' => Literal::False,
                                _ => Literal::Null,
                            };
                            self.mode = Mode::Literal {
                                at,
                                position: 1,
                                kind,
                                location,
                            };
                        }
                        _ => return Err(invalid()),
                    }
                }
            }
        }
        if final_input && used == bytes.len() {
            if let Mode::Number {
                at,
                state,
                location,
            } = self.mode
                && state.complete()
                && token_budget - tokens >= 1
            {
                emit(Token::Number {
                    span: Span {
                        start: at,
                        end: self.at,
                    },
                    location,
                })?;
                tokens += 1;
                self.mode = Mode::Normal;
                self.primitive_complete();
            }
            if !self.complete || !self.frames.is_empty() || self.mode != Mode::Normal {
                return Err(invalid());
            }
        }
        Ok(Step {
            consumed: used,
            tokens,
            complete: self.complete && self.frames.is_empty() && self.mode == Mode::Normal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(bytes: &[u8], first: usize, budget: usize, rows: usize) -> Vec<Token> {
        let mut cursor = Cursor::new();
        let mut at = 0;
        let mut result = Vec::new();
        let mut first_range = true;
        loop {
            let end = if first_range {
                first.min(bytes.len())
            } else {
                bytes.len()
            };
            first_range = false;
            let step = cursor
                .step(
                    &bytes[at..end],
                    end == bytes.len(),
                    budget,
                    rows,
                    5,
                    |token| {
                        result.push(token);
                        Ok(())
                    },
                )
                .unwrap();
            assert!(step.tokens <= rows);
            assert!(step.consumed <= budget + 11);
            at += step.consumed;
            let saved = serde_json::to_vec(&cursor).unwrap();
            cursor = serde_json::from_slice(&saved).unwrap();
            if at == bytes.len() && step.complete {
                break;
            }
        }
        result
    }

    #[test]
    fn tokens_and_addresses_survive_every_source_split_and_restart() {
        let record = br#"{"a":[0,-12.5e+7,true,false,null,"\uD83D\uDE00"],"b":{"c":"text"}}"#;
        let expected = collect(record, record.len(), record.len(), usize::MAX);
        for split in 1..record.len() {
            assert_eq!(collect(record, split, 7, 9), expected, "split {split}");
        }
    }

    #[test]
    fn bounded_descriptor_pages_make_progress_in_large_records() {
        for count in [128, 1024, 8192] {
            let mut record = String::from("[");
            for index in 0..count {
                if index != 0 {
                    record.push(',');
                }
                record.push_str("{\"a\":\"text\",\"b\":[true,0]}");
            }
            record.push(']');
            let mut cursor = Cursor::new();
            let mut at = 0;
            let mut steps = 0;
            let mut tokens = 0;
            while at < record.len() {
                let step = cursor
                    .step(
                        &record.as_bytes()[at..],
                        true,
                        1024 * 1024,
                        64,
                        64 * 1024,
                        |_| Ok(()),
                    )
                    .unwrap();
                assert!(step.consumed > 0);
                assert!(step.tokens <= 64);
                assert!(serde_json::to_vec(&cursor).unwrap().len() < 2048);
                at += step.consumed;
                tokens += step.tokens;
                steps += 1;
                cursor = serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap();
            }
            assert!(cursor.complete);
            assert!(steps > 1);
            assert_eq!(tokens, 2 + 15 * count);
        }
    }

    #[test]
    fn native_json_grammar_rejects_incomplete_and_malformed_records() {
        for record in [
            "",
            "[1,]",
            "{\"a\":1,}",
            "{\"a\" 1}",
            "01",
            "1.",
            "1e",
            "--1",
            "tru",
            "true false",
            "[",
            "{\"a\":",
        ] {
            let mut cursor = Cursor::new();
            assert!(
                cursor
                    .step(
                        record.as_bytes(),
                        true,
                        record.len().max(1),
                        usize::MAX,
                        64 * 1024,
                        |_| Ok(())
                    )
                    .is_err(),
                "{record}"
            );
        }
    }
}
