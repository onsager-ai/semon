//! Best-effort, length-preserving redaction of known secret shapes, applied
//! to every byte `semon push` sends. It catches the shapes below; a secret
//! of any other shape gets through.
//!
//! - `sk-…` keys (Anthropic, OpenAI and others);
//! - GitHub tokens: `ghp_`, `gho_`, `ghs_`, `ghu_`, `ghr_`, `github_pat_`;
//! - AWS access key ids: `AKIA…`, `ASIA…`;
//! - Slack tokens: `xoxb-`, `xoxp-`, `xoxa-`, `xoxr-`, `xoxs-`, `xapp-`;
//! - the body of a `-----BEGIN … PRIVATE KEY-----` block;
//! - the token after `Bearer ` (as in `Authorization: Bearer …`);
//! - the value in `NAME=value` where `NAME` is upper case and contains
//!   `KEY`, `TOKEN`, `SECRET` or `PASSWORD`.
//!
//! Every matched byte becomes `*`, so lengths and byte offsets are
//! unchanged. Only bytes in `[A-Za-z0-9_\-./+=]` are ever replaced, and
//! never one inside a backslash escape (`\n`, `\"`, `\u00e9`), so a JSON
//! line stays valid JSON. No pattern spans a newline: redacting a file line
//! by line gives the same bytes as redacting it whole.

/// Whether redaction may replace this byte.
fn allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/' | b'+' | b'=')
}

/// The positions redaction may touch: allowed bytes outside escapes.
fn replaceable(bytes: &[u8]) -> Vec<bool> {
    let mut mask: Vec<bool> = bytes.iter().map(|byte| allowed(*byte)).collect();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            let span = if bytes.get(index + 1) == Some(&b'u') {
                6
            } else {
                2
            };
            for flag in mask.iter_mut().skip(index).take(span) {
                *flag = false;
            }
            index += span;
        } else {
            index += 1;
        }
    }
    mask
}

/// Redacts `bytes` in place.
pub fn redact(bytes: &mut [u8]) {
    let mask = replaceable(bytes);
    let mut hide = vec![false; bytes.len()];
    let words = words(&mask);
    for (position, &(start, end)) in words.iter().enumerate() {
        let word = &bytes[start..end];
        prefixed_tokens(word, start, &mut hide);
        assignment(bytes, &mask, &words, position, &mut hide);
        bearer(bytes, &words, position, &mut hide);
    }
    private_keys(bytes, &mut hide);
    for (index, byte) in bytes.iter_mut().enumerate() {
        if hide[index] && mask[index] {
            *byte = b'*';
        }
    }
}

/// Maximal runs of replaceable positions, as `(start, end)`.
fn words(mask: &[bool]) -> Vec<(usize, usize)> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, flag) in mask.iter().enumerate() {
        match (*flag, start) {
            (true, None) => start = Some(index),
            (false, Some(begin)) => {
                words.push((begin, index));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(begin) = start {
        words.push((begin, mask.len()));
    }
    words
}

fn mark(hide: &mut [bool], start: usize, end: usize) {
    for flag in &mut hide[start..end] {
        *flag = true;
    }
}

struct Shape {
    prefix: &'static [u8],
    body: fn(u8) -> bool,
    min: usize,
    exact: bool,
}

fn token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}
fn alnum(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}
fn alnum_underscore(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}
fn upper_digit(byte: u8) -> bool {
    byte.is_ascii_uppercase() || byte.is_ascii_digit()
}
fn alnum_dash(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

const SHAPES: &[Shape] = &[
    Shape {
        prefix: b"sk-",
        body: token_char,
        min: 20,
        exact: false,
    },
    Shape {
        prefix: b"ghp_",
        body: alnum,
        min: 20,
        exact: false,
    },
    Shape {
        prefix: b"gho_",
        body: alnum,
        min: 20,
        exact: false,
    },
    Shape {
        prefix: b"ghs_",
        body: alnum,
        min: 20,
        exact: false,
    },
    Shape {
        prefix: b"ghu_",
        body: alnum,
        min: 20,
        exact: false,
    },
    Shape {
        prefix: b"ghr_",
        body: alnum,
        min: 20,
        exact: false,
    },
    Shape {
        prefix: b"github_pat_",
        body: alnum_underscore,
        min: 20,
        exact: false,
    },
    Shape {
        prefix: b"AKIA",
        body: upper_digit,
        min: 16,
        exact: true,
    },
    Shape {
        prefix: b"ASIA",
        body: upper_digit,
        min: 16,
        exact: true,
    },
    Shape {
        prefix: b"xoxb-",
        body: alnum_dash,
        min: 10,
        exact: false,
    },
    Shape {
        prefix: b"xoxp-",
        body: alnum_dash,
        min: 10,
        exact: false,
    },
    Shape {
        prefix: b"xoxa-",
        body: alnum_dash,
        min: 10,
        exact: false,
    },
    Shape {
        prefix: b"xoxr-",
        body: alnum_dash,
        min: 10,
        exact: false,
    },
    Shape {
        prefix: b"xoxs-",
        body: alnum_dash,
        min: 10,
        exact: false,
    },
    Shape {
        prefix: b"xapp-",
        body: alnum_dash,
        min: 10,
        exact: false,
    },
];

/// Tokens that start with a known prefix, not inside a longer name.
fn prefixed_tokens(word: &[u8], offset: usize, hide: &mut [bool]) {
    for start in 0..word.len() {
        if start > 0 && word[start - 1].is_ascii_alphanumeric() {
            continue;
        }
        for shape in SHAPES {
            if !word[start..].starts_with(shape.prefix) {
                continue;
            }
            let body_start = start + shape.prefix.len();
            let body = word[body_start..]
                .iter()
                .take_while(|byte| (shape.body)(**byte))
                .count();
            let fits = if shape.exact {
                body == shape.min
                    && word
                        .get(body_start + body)
                        .is_none_or(|next| !next.is_ascii_alphanumeric())
            } else {
                body >= shape.min
            };
            if fits {
                mark(hide, offset + start, offset + body_start + body);
            }
        }
    }
}

fn secret_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
        && [&b"KEY"[..], b"TOKEN", b"SECRET", b"PASSWORD"]
            .iter()
            .any(|needle| name.windows(needle.len()).any(|window| window == *needle))
}

/// `NAME=value` with a secret-looking `NAME`: the value is hidden. A value
/// that opens with a quote (`\"`, `"` or `'`) is the next word.
fn assignment(
    bytes: &[u8],
    mask: &[bool],
    words: &[(usize, usize)],
    position: usize,
    hide: &mut [bool],
) {
    let (start, end) = words[position];
    let word = &bytes[start..end];
    for (index, byte) in word.iter().enumerate() {
        if *byte != b'=' {
            continue;
        }
        let name_start = word[..index]
            .iter()
            .rposition(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
            .map_or(0, |at| at + 1);
        if !secret_name(&word[name_start..index]) {
            continue;
        }
        if index + 1 < word.len() {
            mark(hide, start + index + 1, end);
        } else {
            // The value is quoted: skip one opening quote, then take the
            // word that starts right after it.
            let after = end;
            let quote = if bytes[after..].starts_with(b"\\\"") {
                2
            } else if matches!(bytes.get(after), Some(b'"' | b'\'')) {
                1
            } else {
                0
            };
            let value = after + quote;
            if quote > 0
                && mask.get(value) == Some(&true)
                && let Some(&(next_start, next_end)) = words.get(position + 1)
                && next_start == value
            {
                mark(hide, next_start, next_end);
            }
        }
        return;
    }
}

/// `Bearer <token>`: the token is hidden when it is 8 bytes or longer.
fn bearer(bytes: &[u8], words: &[(usize, usize)], position: usize, hide: &mut [bool]) {
    let (start, end) = words[position];
    if !bytes[start..end].eq_ignore_ascii_case(b"bearer") {
        return;
    }
    let spaces = bytes[end..]
        .iter()
        .take_while(|byte| **byte == b' ')
        .count();
    if spaces == 0 {
        return;
    }
    if let Some(&(next_start, next_end)) = words.get(position + 1)
        && next_start == end + spaces
        && next_end - next_start >= 8
    {
        mark(hide, next_start, next_end);
    }
}

/// The body of a `-----BEGIN … PRIVATE KEY-----` block, to its `-----END`
/// or the end of the line.
fn private_keys(bytes: &[u8], hide: &mut [bool]) {
    const BEGIN: &[u8] = b"-----BEGIN ";
    const HEADER_END: &[u8] = b"PRIVATE KEY-----";
    const END: &[u8] = b"-----END";
    let mut from = 0;
    while let Some(found) = find(&bytes[from..], BEGIN) {
        let begin = from + found;
        let line_end = bytes[begin..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |at| begin + at);
        let Some(header) = find(&bytes[begin..line_end], HEADER_END) else {
            from = begin + BEGIN.len();
            continue;
        };
        let body_start = begin + header + HEADER_END.len();
        let body_end =
            find(&bytes[body_start..line_end], END).map_or(line_end, |at| body_start + at);
        mark(hide, body_start, body_end);
        from = body_end.max(begin + BEGIN.len());
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn redacted(text: &str) -> String {
        let mut bytes = text.as_bytes().to_vec();
        redact(&mut bytes);
        assert_eq!(bytes.len(), text.len(), "length preserved");
        String::from_utf8(bytes).unwrap()
    }

    /// Each shape inside a JSON string: hidden, the line still parses, the
    /// length is kept and only allowed bytes changed.
    #[test]
    fn every_shape_is_hidden_in_a_json_line() {
        let cases = [
            (
                "key sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123",
                "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123",
            ),
            (
                "openai sk-proj-ABCDEFGHIJKLMNOPQRSTUV",
                "sk-proj-ABCDEFGHIJKLMNOPQRSTUV",
            ),
            (
                "gh ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
                "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            ),
            (
                "gho_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 x",
                "gho_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            ),
            (
                "ghs_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
                "ghs_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            ),
            (
                "ghu_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
                "ghu_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
            ),
            (
                "pat github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
                "github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
            ),
            ("aws AKIAIOSFODNN7EXAMPLE done", "AKIAIOSFODNN7EXAMPLE"),
            ("aws ASIAIOSFODNN7EXAMPLE", "ASIAIOSFODNN7EXAMPLE"),
            (
                "slack xoxb-1234567890-abcdefghij",
                "xoxb-1234567890-abcdefghij",
            ),
            (
                "slack xapp-1-A0123456789-abcdef",
                "xapp-1-A0123456789-abcdef",
            ),
            (
                "curl -H 'Authorization: Bearer abcdef0123456789.xyz'",
                "abcdef0123456789.xyz",
            ),
            ("export OPENAI_API_KEY=sk-short", "sk-short"),
            ("DB_PASSWORD=hunter2hunter2", "hunter2hunter2"),
            (
                "GITHUB_TOKEN=\"quoted-value/with+chars=\"",
                "quoted-value/with+chars=",
            ),
            ("MY_SECRET='single'", "single"),
        ];
        for (text, secret) in cases {
            let line = json!({"type":"user","message":{"content": text}}).to_string();
            let out = redacted(&line);
            assert!(!out.contains(secret), "{text}: {out}");
            let parsed: Value = serde_json::from_str(&out).expect("still JSON");
            let content = parsed["message"]["content"].as_str().unwrap();
            assert!(content.contains(&"*".repeat(secret.len())), "{content}");
            for (before, after) in line.bytes().zip(out.bytes()) {
                assert!(before == after || (after == b'*' && allowed(before)));
            }
        }
    }

    #[test]
    fn a_private_key_block_is_hidden_but_its_escapes_stay() {
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nQyNTUxOQAAACB+/=\n-----END OPENSSH PRIVATE KEY-----\n";
        let line = json!({"text": format!("key:\n{pem}after")}).to_string();
        let out = redacted(&line);
        assert!(!out.contains("b3BlbnNzaC1rZXktdjEAAAAA"));
        assert!(!out.contains("QyNTUxOQAAACB"));
        assert!(out.contains("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(out.contains("-----END OPENSSH PRIVATE KEY-----"));
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["text"].as_str().unwrap().lines().count(), 6);
    }

    #[test]
    fn escapes_and_ordinary_text_are_left_alone() {
        for text in [
            "a task-runner and a desk-lamp",
            "sk-short",
            "max_tokens=4096 and api_key=lowercase-is-code",
            "Bearer short",
            "AKIAIOSFODNN7EXAMPLEX",
            "path/to/file.rs line 12",
            "unicode \u{e9}t\u{e9} and a tab\t",
        ] {
            let line = json!({"content": text}).to_string();
            assert_eq!(redacted(&line), line, "{text}");
        }
        // A secret right after an escaped newline is still found, and the
        // escape itself is kept.
        let line = json!({"content": "one\nsk-abcdefghijklmnopqrstuvwxyz"}).to_string();
        let out = redacted(&line);
        assert!(out.contains("one\\n***"), "{out}");
        serde_json::from_str::<Value>(&out).unwrap();
    }

    /// No match spans a newline: line by line equals whole, at any split
    /// between lines.
    #[test]
    fn redacting_by_lines_equals_redacting_whole() {
        let lines = [
            json!({"a":"export API_KEY=abcdef"}).to_string(),
            json!({"b":"Bearer"}).to_string(),
            json!({"c":"0123456789abcdef token after a newline"}).to_string(),
            json!({"d":"-----BEGIN RSA PRIVATE KEY-----\nMIIEow"}).to_string(),
            json!({"e":"MIIEowIBAAKCAQEA not a key: no header on this line"}).to_string(),
            json!({"f":"sk-abcdefghijklmnopqrstuvwxyz AKIAIOSFODNN7EXAMPLE"}).to_string(),
        ];
        let whole = lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>();
        let expected = redacted(&whole);
        for split in 1..lines.len() {
            let (head, tail) = lines.split_at(split);
            let head: String = head.iter().map(|line| format!("{line}\n")).collect();
            let tail: String = tail.iter().map(|line| format!("{line}\n")).collect();
            assert_eq!(
                redacted(&head) + &redacted(&tail),
                expected,
                "split {split}"
            );
        }
        assert_eq!(redacted(&expected), expected, "idempotent");
    }
}
