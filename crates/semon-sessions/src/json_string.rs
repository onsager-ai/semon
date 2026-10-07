//! Bounded decoding of validated native JSON string fields.
//!
//! Producers walk the record once and retain only allowlisted native field
//! pointers, raw spans and scalar boundaries. Arbitrary object keys may contain
//! user text: callers must not persist every pointer this lexer discovers.
//! Readers bind spans/cursors to an authorized source generation and begin at a
//! verified checkpoint or a prior chunk's cursor. A raw offset is not authority
//! or proof of a scalar boundary. No prefix or complete record is needed here.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, io,
    ops::Range,
};

pub const JSON_STRING_SPAN_VERSION: u32 = 1;
pub const JSON_STRING_CHECKPOINT_BYTES: u64 = 64 * 1024;
pub const JSON_STRING_CHUNK_DECODED_MAX: usize = 128 * 1024;
/// Largest incomplete scalar in a two-layer JSON string is 72 raw bytes.
pub const JSON_STRING_LAYERED_SCALAR_SLACK: usize = 71;
const MAX_DEPTH: usize = 128;
const MAX_FIELDS: usize = 8192;
const MAX_POINTER_BYTES: usize = 4096;

/// Offsets refer to the supplied native record, excluding the string's quotes.
/// Checkpoints include `start` and `end`, with gaps at most 64 KiB + 11 bytes.
/// Each checkpoint is before a complete UTF-8 scalar or JSON escape, including
/// an entire surrogate pair. The source identity/generation belongs to the
/// containing projection, never to an inferred filename or this descriptor.
#[derive(Clone, PartialEq, Eq)]
pub struct JsonStringSpan {
    pub json_pointer: String,
    pub start: u64,
    pub end: u64,
    pub checkpoints: Vec<u64>,
}

impl fmt::Debug for JsonStringSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonStringSpan")
            .field("pointer_bytes", &self.json_pointer.len())
            .field("start", &self.start)
            .field("end", &self.end)
            .field("checkpoints", &self.checkpoints)
            .finish()
    }
}

/// A request-local native text chunk. It deliberately has no Serialize impl;
/// Debug reports sizes instead of writing source text to logs.
#[derive(Clone, PartialEq, Eq)]
pub struct JsonStringChunk {
    pub text: String,
    /// Raw bytes consumed at complete scalar boundaries, not decoded bytes.
    pub consumed: usize,
    pub complete: bool,
}

impl fmt::Debug for JsonStringChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsonStringChunk")
            .field("text_bytes", &self.text.len())
            .field("consumed", &self.consumed)
            .field("complete", &self.complete)
            .finish()
    }
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid JSON string or record")
}
fn limit() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "native JSON field metadata exceeds supported limits",
    )
}
fn partial<T>(at_end: bool) -> io::Result<Option<T>> {
    if at_end { Err(invalid()) } else { Ok(None) }
}
fn hex(bytes: &[u8]) -> io::Result<u16> {
    bytes.iter().try_fold(0, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return Err(invalid()),
        };
        Ok(value * 16 + u16::from(digit))
    })
}

/// Decode one scalar, never consuming a partial escape/UTF-8/surrogate pair.
fn scalar(bytes: &[u8], at_end: bool) -> io::Result<Option<(char, usize)>> {
    let Some(&first) = bytes.first() else {
        return partial(at_end);
    };
    if first == b'\\' {
        let Some(&escape) = bytes.get(1) else {
            return partial(at_end);
        };
        let simple = match escape {
            b'"' => Some('"'),
            b'\\' => Some('\\'),
            b'/' => Some('/'),
            b'b' => Some('\u{8}'),
            b'f' => Some('\u{c}'),
            b'n' => Some('\n'),
            b'r' => Some('\r'),
            b't' => Some('\t'),
            b'u' => None,
            _ => return Err(invalid()),
        };
        if let Some(value) = simple {
            return Ok(Some((value, 2)));
        }
        let Some(digits) = bytes.get(2..6) else {
            return partial(at_end);
        };
        let first = hex(digits)?;
        let (value, width) = if (0xd800..=0xdbff).contains(&first) {
            let Some(pair) = bytes.get(6..12) else {
                return partial(at_end);
            };
            if &pair[..2] != b"\\u" {
                return Err(invalid());
            }
            let second = hex(&pair[2..])?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err(invalid());
            }
            (
                0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00,
                12,
            )
        } else {
            if (0xdc00..=0xdfff).contains(&first) {
                return Err(invalid());
            }
            (u32::from(first), 6)
        };
        return char::from_u32(value)
            .map(|value| Some((value, width)))
            .ok_or_else(invalid);
    }
    if first < 0x20 || first == b'"' {
        return Err(invalid());
    }
    if first < 0x80 {
        return Ok(Some((char::from(first), 1)));
    }
    let width = match first {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Err(invalid()),
    };
    let Some(encoded) = bytes.get(..width) else {
        return partial(at_end);
    };
    let value = std::str::from_utf8(encoded)
        .map_err(|_| invalid())?
        .chars()
        .next()
        .ok_or_else(invalid)?;
    Ok(Some((value, width)))
}

/// Decode only the supplied field fragment, from a verified scalar boundary.
/// `at_end` means these bytes reach the field's recorded end, not merely the
/// end of a provider read. Incomplete escapes remain unconsumed for the next
/// bounded read. Output and allocation never exceed `max_decoded`; a budget
/// smaller than the first scalar is rejected to prevent a zero-progress loop.
pub fn decode_json_string_chunk(
    bytes: &[u8],
    at_end: bool,
    max_decoded: usize,
) -> io::Result<JsonStringChunk> {
    decode_json_string_layered_chunk(bytes, 1, at_end, max_decoded)
}

/// Decode one or two JSON-string escape layers from a verified combined scalar
/// boundary. Layer two uses a fixed 12-byte scratch buffer, never a decoded
/// prefix or intermediate field allocation. Partial combined scalars remain
/// entirely unconsumed. Native recipe qualification and source authority belong
/// to the caller, including whether a string contains an encoded JSON wrapper.
pub fn decode_json_string_layered_chunk(
    bytes: &[u8],
    layers: u8,
    at_end: bool,
    max_decoded: usize,
) -> io::Result<JsonStringChunk> {
    if !matches!(layers, 1 | 2) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "unsupported JSON string escape layers",
        ));
    }
    if max_decoded == 0 || max_decoded > JSON_STRING_CHUNK_DECODED_MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "decoded chunk budget must be 1..=128 KiB",
        ));
    }
    let mut text = String::with_capacity(bytes.len().min(max_decoded));
    let mut consumed = 0;
    while consumed < bytes.len() && text.len() < max_decoded {
        let Some((value, width)) = layered_scalar(&bytes[consumed..], layers, at_end)? else {
            break;
        };
        if value.len_utf8() > max_decoded - text.len() {
            if text.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "decoded chunk budget cannot fit the next Unicode scalar",
                ));
            }
            break;
        }
        text.push(value);
        consumed += width;
    }
    Ok(JsonStringChunk {
        text,
        consumed,
        complete: at_end && consumed == bytes.len(),
    })
}

fn layered_scalar(bytes: &[u8], layers: u8, at_end: bool) -> io::Result<Option<(char, usize)>> {
    if layers == 1 {
        return scalar(bytes, at_end);
    }
    let mut encoded = [0u8; 12];
    let mut filled = 0;
    let mut consumed = 0;
    loop {
        let Some((value, width)) = scalar(&bytes[consumed..], at_end)? else {
            return Ok(None);
        };
        let next = filled + value.len_utf8();
        if next > encoded.len() {
            return Err(invalid());
        }
        value.encode_utf8(&mut encoded[filled..next]);
        filled = next;
        consumed += width;
        if let Some((decoded, inner_width)) = scalar(&encoded[..filled], false)? {
            if inner_width != filled {
                return Err(invalid());
            }
            return Ok(Some((decoded, consumed)));
        }
    }
}

fn field_bytes<'a>(record: &'a [u8], span: &JsonStringSpan) -> io::Result<&'a [u8]> {
    let start = usize::try_from(span.start).map_err(|_| invalid())?;
    let end = usize::try_from(span.end).map_err(|_| invalid())?;
    if start == 0
        || start > end
        || record.get(start - 1) != Some(&b'"')
        || record.get(end) != Some(&b'"')
    {
        return Err(invalid());
    }
    record.get(start..end).ok_or_else(invalid)
}

/// Producer-only slicing by decoded UTF-8 byte offsets. Both ends must be
/// complete scalar boundaries. Offsets/checkpoints stay relative to the
/// original native record; the producer walks this field once, not each request.
pub fn slice_json_string_span(
    record: &[u8],
    outer: &JsonStringSpan,
    decoded_range: Range<usize>,
) -> io::Result<JsonStringSpan> {
    if decoded_range.start > decoded_range.end {
        return Err(invalid());
    }
    let bytes = field_bytes(record, outer)?;
    let mut decoded = 0usize;
    let mut raw = 0usize;
    let mut start = None;
    let mut checkpoints = Vec::new();
    let mut checkpoint = 0usize;
    loop {
        if decoded == decoded_range.start && start.is_none() {
            start = Some(raw);
            checkpoint = raw;
            checkpoints.push(outer.start + raw as u64);
        }
        if decoded == decoded_range.end {
            let start = start.ok_or_else(invalid)?;
            let end = outer.start + raw as u64;
            if checkpoints.last() != Some(&end) {
                checkpoints.push(end);
            }
            return Ok(JsonStringSpan {
                json_pointer: outer.json_pointer.clone(),
                start: outer.start + start as u64,
                end,
                checkpoints,
            });
        }
        if raw == bytes.len()
            || decoded > decoded_range.start && start.is_none()
            || decoded > decoded_range.end
        {
            return Err(invalid());
        }
        let (value, width) = scalar(&bytes[raw..], true)?.ok_or_else(invalid)?;
        raw += width;
        decoded = decoded.checked_add(value.len_utf8()).ok_or_else(limit)?;
        if start.is_some() && raw - checkpoint >= JSON_STRING_CHECKPOINT_BYTES as usize {
            checkpoints.push(outer.start + raw as u64);
            checkpoint = raw;
        }
    }
}

/// Producer-only qualification of string fields inside one encoded JSON
/// wrapper. Returned pointers address the decoded wrapper; persist only native
/// allowlisted pointers. Returned spans/checkpoints address the original record
/// and require layer-two decoding. Arbitrary complex wrappers remain unsupported
/// by native callers until their semantics are separately qualified.
pub fn index_nested_json_string_spans(
    record: &[u8],
    outer: &JsonStringSpan,
) -> io::Result<Vec<JsonStringSpan>> {
    let bytes = field_bytes(record, outer)?;
    let mut decoded = String::with_capacity(bytes.len().min(JSON_STRING_CHUNK_DECODED_MAX));
    let mut at = 0;
    while at < bytes.len() {
        let (value, width) = scalar(&bytes[at..], true)?.ok_or_else(invalid)?;
        decoded.push(value);
        at += width;
    }
    // Each decoded outer byte expands to at most six raw bytes. A completed
    // inner scalar overshoots its checkpoint threshold by at most 11 bytes.
    let mut spans = index_with_checkpoints(
        decoded.as_bytes(),
        (JSON_STRING_CHECKPOINT_BYTES as usize / 6) - 11,
    )?;
    let wanted: BTreeSet<u64> = spans
        .iter()
        .flat_map(|span| span.checkpoints.iter().copied())
        .collect();
    let mut mapped = BTreeMap::new();
    let mut raw = 0;
    let mut offset = 0u64;
    let mut wanted = wanted.into_iter().peekable();
    loop {
        if wanted.peek() == Some(&offset) {
            mapped.insert(offset, outer.start + raw as u64);
            wanted.next();
        }
        if wanted.peek().is_some_and(|next| *next < offset) {
            return Err(invalid());
        }
        if raw == bytes.len() {
            break;
        }
        let (value, width) = scalar(&bytes[raw..], true)?.ok_or_else(invalid)?;
        raw += width;
        offset += value.len_utf8() as u64;
    }
    if wanted.peek().is_some() {
        return Err(invalid());
    }
    for span in &mut spans {
        span.start = *mapped.get(&span.start).ok_or_else(invalid)?;
        span.end = *mapped.get(&span.end).ok_or_else(invalid)?;
        for checkpoint in &mut span.checkpoints {
            *checkpoint = *mapped.get(checkpoint).ok_or_else(invalid)?;
        }
    }
    Ok(spans)
}

/// Index syntactically valid JSON without retaining string values. Duplicate
/// keys, deep/oversized field maps and unqualified complex native body formats
/// must remain explicit unsupported/incomplete outcomes in the caller.
pub fn index_json_string_spans(record: &[u8]) -> io::Result<Vec<JsonStringSpan>> {
    index_with_checkpoints(record, JSON_STRING_CHECKPOINT_BYTES as usize)
}

fn index_with_checkpoints(
    record: &[u8],
    checkpoint_bytes: usize,
) -> io::Result<Vec<JsonStringSpan>> {
    let mut lexer = Lexer {
        bytes: record,
        checkpoint_bytes,
        at: 0,
        fields: 0,
        spans: Vec::new(),
    };
    lexer.value("", 0)?;
    lexer.ws();
    if lexer.at != record.len() {
        return Err(invalid());
    }
    Ok(lexer.spans)
}

struct Lexer<'a> {
    bytes: &'a [u8],
    checkpoint_bytes: usize,
    at: usize,
    fields: usize,
    spans: Vec<JsonStringSpan>,
}
impl Lexer<'_> {
    fn ws(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.at += 1;
        }
    }
    fn take(&mut self, byte: u8) -> io::Result<()> {
        self.ws();
        if self.bytes.get(self.at) != Some(&byte) {
            return Err(invalid());
        }
        self.at += 1;
        Ok(())
    }
    fn string(&mut self, pointer: Option<&str>) -> io::Result<(usize, usize)> {
        self.take(b'"')?;
        let start = self.at;
        let mut checkpoints = pointer.map(|_| vec![start as u64]).unwrap_or_default();
        let mut checkpoint = start;
        loop {
            if self.bytes.get(self.at) == Some(&b'"') {
                break;
            }
            let (_, width) = scalar(self.bytes.get(self.at..).ok_or_else(invalid)?, true)?
                .ok_or_else(invalid)?;
            self.at += width;
            if pointer.is_some() && self.at - checkpoint >= self.checkpoint_bytes {
                checkpoints.push(self.at as u64);
                checkpoint = self.at;
            }
        }
        let end = self.at;
        self.at += 1;
        if let Some(pointer) = pointer {
            if self.spans.len() >= MAX_FIELDS {
                return Err(limit());
            }
            if checkpoints.last() != Some(&(end as u64)) {
                checkpoints.push(end as u64);
            }
            self.spans.push(JsonStringSpan {
                json_pointer: pointer.into(),
                start: start as u64,
                end: end as u64,
                checkpoints,
            });
        }
        Ok((start, end))
    }
    fn value(&mut self, pointer: &str, depth: usize) -> io::Result<()> {
        if depth > MAX_DEPTH || pointer.len() > MAX_POINTER_BYTES {
            return Err(limit());
        }
        self.ws();
        match self.bytes.get(self.at).copied() {
            Some(b'"') => {
                self.string(Some(pointer))?;
            }
            Some(b'{') => {
                self.at += 1;
                self.ws();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(());
                }
                let mut keys = BTreeSet::new();
                loop {
                    self.fields += 1;
                    if self.fields > MAX_FIELDS {
                        return Err(limit());
                    }
                    let (start, end) = self.string(None)?;
                    if end - start > MAX_POINTER_BYTES * 6 {
                        return Err(limit());
                    }
                    let key =
                        decode_json_string_chunk(&self.bytes[start..end], true, MAX_POINTER_BYTES)?;
                    if !key.complete || !keys.insert(key.text.clone()) {
                        return Err(limit());
                    }
                    self.take(b':')?;
                    let next = format!(
                        "{pointer}/{}",
                        key.text.replace('~', "~0").replace('/', "~1")
                    );
                    self.value(&next, depth + 1)?;
                    self.ws();
                    match self.bytes.get(self.at) {
                        Some(b'}') => {
                            self.at += 1;
                            break;
                        }
                        Some(b',') => {
                            self.at += 1;
                        }
                        _ => return Err(invalid()),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                self.ws();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(());
                }
                let mut index = 0usize;
                loop {
                    self.value(&format!("{pointer}/{index}"), depth + 1)?;
                    index = index.checked_add(1).ok_or_else(limit)?;
                    self.ws();
                    match self.bytes.get(self.at) {
                        Some(b']') => {
                            self.at += 1;
                            break;
                        }
                        Some(b',') => {
                            self.at += 1;
                        }
                        _ => return Err(invalid()),
                    }
                }
            }
            Some(b't') => self.literal(b"true")?,
            Some(b'f') => self.literal(b"false")?,
            Some(b'n') => self.literal(b"null")?,
            Some(b'-' | b'0'..=b'9') => self.number()?,
            _ => return Err(invalid()),
        }
        Ok(())
    }
    fn literal(&mut self, literal: &[u8]) -> io::Result<()> {
        if self.bytes.get(self.at..self.at + literal.len()) != Some(literal) {
            return Err(invalid());
        }
        self.at += literal.len();
        Ok(())
    }
    fn digits(&mut self) -> bool {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        self.at != start
    }
    fn number(&mut self) -> io::Result<()> {
        if self.bytes.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        match self.bytes.get(self.at) {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(invalid()),
        }
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            if !self.digits() {
                return Err(invalid());
            }
        }
        if self
            .bytes
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b'e' | b'E'))
        {
            self.at += 1;
            if self
                .bytes
                .get(self.at)
                .is_some_and(|byte| matches!(byte, b'+' | b'-'))
            {
                self.at += 1;
            }
            if !self.digits() {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(raw: &[u8], budget: usize) -> String {
        let mut at = 0;
        let mut result = String::new();
        while at < raw.len() {
            let chunk = decode_json_string_chunk(&raw[at..], true, budget).unwrap();
            assert!(chunk.consumed > 0);
            assert!(chunk.text.len() <= budget);
            at += chunk.consumed;
            assert_eq!(chunk.complete, at == raw.len());
            result.push_str(&chunk.text);
        }
        result
    }

    fn decode_layered_all(raw: &[u8], budget: usize) -> String {
        let mut remaining = raw;
        let mut output = String::new();
        while !remaining.is_empty() {
            let chunk = decode_json_string_layered_chunk(remaining, 2, true, budget).unwrap();
            assert!(chunk.consumed > 0);
            output.push_str(&chunk.text);
            remaining = &remaining[chunk.consumed..];
        }
        output
    }

    #[test]
    fn layered_native_wrapper_spans_match_committed_parser_oracle() {
        let fixture =
            include_str!("../../../tests/fixtures/compatibility/v1/namespace-codex.jsonl");
        let mut qualified = 0;
        for line in fixture.lines() {
            let oracle = crate::tx::parse_native_record(line.as_bytes()).unwrap();
            if oracle
                .pointer("/payload/type")
                .and_then(serde_json::Value::as_str)
                != Some("function_call_output")
            {
                continue;
            }
            let outer = index_json_string_spans(line.as_bytes())
                .unwrap()
                .into_iter()
                .find(|span| span.json_pointer == "/payload/output")
                .unwrap();
            let inner: serde_json::Value =
                serde_json::from_str(oracle.pointer("/payload/output").unwrap().as_str().unwrap())
                    .unwrap();
            let spans = index_nested_json_string_spans(line.as_bytes(), &outer).unwrap();
            let body = spans
                .iter()
                .find(|span| span.json_pointer == "/output")
                .unwrap();
            assert_eq!(
                decode_layered_all(&line.as_bytes()[body.start as usize..body.end as usize], 16),
                inner["output"].as_str().unwrap()
            );
            qualified += 1;
        }
        assert_eq!(qualified, 1);
    }

    #[test]
    fn layered_decoder_preserves_fragment_boundaries_and_worst_escape_unit() {
        let inner = r#"{"output":"A\u00e9\uD83D\uDE80\n\\\"Z"}"#;
        let encoded: String = inner
            .bytes()
            .map(|byte| format!("\\u{:04x}", byte))
            .collect();
        let record = format!("{{\"payload\":\"{encoded}\"}}");
        let outer = index_json_string_spans(record.as_bytes())
            .unwrap()
            .remove(0);
        let span = index_nested_json_string_spans(record.as_bytes(), &outer)
            .unwrap()
            .remove(0);
        let raw = &record.as_bytes()[span.start as usize..span.end as usize];
        let oracle: serde_json::Value = serde_json::from_str(inner).unwrap();
        let expected = oracle["output"].as_str().unwrap();
        for split in 0..=raw.len() {
            let chunk = decode_json_string_layered_chunk(&raw[..split], 2, false, 1024).unwrap();
            assert!(expected.starts_with(&chunk.text));
            assert!(split - chunk.consumed <= JSON_STRING_LAYERED_SCALAR_SLACK);
            let rest =
                decode_json_string_layered_chunk(&raw[chunk.consumed..], 2, true, 1024).unwrap();
            assert!(rest.complete);
            assert_eq!(format!("{}{}", chunk.text, rest.text), expected);
        }
        assert_eq!(decode_layered_all(raw, 4), expected);
        assert!(decode_json_string_layered_chunk(raw, 3, true, 1024).is_err());
        assert!(decode_json_string_layered_chunk(b"\\\\uD800", 2, true, 1024).is_err());
    }

    #[test]
    fn decoded_slice_bounds_are_scalar_safe_and_record_relative() {
        let header = "Chunk ID: fixture\nFinal output:\n";
        let body = "é🚀\nquoted \"body\"\\";
        for record in [
            serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","output":format!("{header}{body}")}}),
            serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","content":format!("{header}{body}")}]}}),
        ] {
            let bytes = serde_json::to_vec(&record).unwrap();
            let oracle = crate::tx::parse_native_record(&bytes).unwrap();
            let pointer = if record["type"] == "user" {
                "/message/content/0/content"
            } else {
                "/payload/output"
            };
            let outer = index_json_string_spans(&bytes)
                .unwrap()
                .into_iter()
                .find(|span| span.json_pointer == pointer)
                .unwrap();
            assert_eq!(
                oracle.pointer(pointer).unwrap().as_str().unwrap(),
                format!("{header}{body}")
            );
            let selected =
                slice_json_string_span(&bytes, &outer, header.len()..header.len() + body.len())
                    .unwrap();
            assert_eq!(
                decode_all(&bytes[selected.start as usize..selected.end as usize], 8),
                body
            );
            assert!(
                slice_json_string_span(&bytes, &outer, header.len() + 1..header.len() + body.len())
                    .is_err()
            );
            assert!(
                slice_json_string_span(&bytes, &outer, header.len()..header.len() + 1).is_err()
            );
            assert!(
                slice_json_string_span(&bytes, &outer, 0..header.len() + body.len() + 1).is_err()
            );
            assert_eq!(
                slice_json_string_span(&bytes, &outer, header.len()..header.len())
                    .unwrap()
                    .start,
                selected.start
            );
        }
    }

    #[test]
    fn nested_late_chunks_are_bounded_as_native_record_size_grows() {
        for repetitions in [64 * 1024, 1024 * 1024] {
            let body = "x\\\n🚀".repeat(repetitions);
            let inner = serde_json::to_string(
                &serde_json::json!({"output":body,"metadata":{"exit_code":0}}),
            )
            .unwrap();
            let record = serde_json::to_vec(
                &serde_json::json!({"payload":{"type":"function_call_output","output":inner}}),
            )
            .unwrap();
            let outer = index_json_string_spans(&record)
                .unwrap()
                .into_iter()
                .find(|span| span.json_pointer == "/payload/output")
                .unwrap();
            let span = index_nested_json_string_spans(&record, &outer)
                .unwrap()
                .into_iter()
                .find(|span| span.json_pointer == "/output")
                .unwrap();
            for pair in span.checkpoints.windows(2) {
                assert!(pair[1] - pair[0] <= JSON_STRING_CHECKPOINT_BYTES);
            }
            let start = span.checkpoints[span.checkpoints.len() - 3] as usize;
            let end = (start + 4096 + JSON_STRING_LAYERED_SCALAR_SLACK).min(span.end as usize);
            assert_eq!(end - start, 4096 + JSON_STRING_LAYERED_SCALAR_SLACK);
            let chunk = decode_json_string_layered_chunk(
                &record[start..end],
                2,
                end == span.end as usize,
                4096,
            )
            .unwrap();
            assert!(
                chunk.consumed > 0 && chunk.consumed <= 4096 + JSON_STRING_LAYERED_SCALAR_SLACK
            );
            assert!(chunk.text.capacity() <= 4096);
            assert!(body.ends_with(&decode_layered_all(&record[start..span.end as usize], 4096)));
        }
    }

    #[test]
    fn escaped_unicode_chunks_match_json_oracle_at_every_fragment_boundary() {
        let raw = br#"A\"\\\/\b\f\n\r\t\u0000\u20ac\uD83D\uDE80Z"#;
        let encoded = [b"\"".as_slice(), raw, b"\""].concat();
        let expected: String = serde_json::from_slice(&encoded).unwrap();
        for budget in 4..=32 {
            assert_eq!(decode_all(raw, budget), expected);
        }
        for boundary in 0..=raw.len() {
            let first = decode_json_string_chunk(&raw[..boundary], false, 128).unwrap();
            let second = decode_json_string_chunk(&raw[first.consumed..], true, 128).unwrap();
            assert_eq!(format!("{}{}", first.text, second.text), expected);
            assert!(second.complete);
        }
        let raw = "é中🚀".as_bytes();
        for boundary in 0..=raw.len() {
            let first = decode_json_string_chunk(&raw[..boundary], false, 128).unwrap();
            let second = decode_json_string_chunk(&raw[first.consumed..], true, 128).unwrap();
            assert_eq!(format!("{}{}", first.text, second.text), "é中🚀");
        }
    }

    #[test]
    fn malformed_strings_and_nonprogress_budgets_are_explicit() {
        for raw in [
            b"\\x".as_slice(),
            b"\\uZZZZ",
            b"\\uD800",
            b"\\uDC00",
            b"\\uD800\\u0041",
            b"\n",
            b"\"",
            &[0xc0, 0x80],
            &[0xed, 0xa0, 0x80],
        ] {
            assert_eq!(
                decode_json_string_chunk(raw, true, 128).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        for raw in [b"\\".as_slice(), b"\\uD800\\u", &[0xf0, 0x9f]] {
            let chunk = decode_json_string_chunk(raw, false, 128).unwrap();
            assert_eq!(chunk.consumed, 0);
            assert!(!chunk.complete);
        }
        assert!(decode_json_string_chunk("🚀".as_bytes(), true, 3).is_err());
        assert!(decode_json_string_chunk(b"a", true, 0).is_err());
        assert!(decode_json_string_chunk(b"a", true, JSON_STRING_CHUNK_DECODED_MAX + 1).is_err());
        assert!(decode_json_string_chunk(b"", true, 4).unwrap().complete);
    }

    #[test]
    fn spans_match_native_claude_and_codex_string_fields() {
        let records = [
            serde_json::json!({"type":"user","uuid":"native-claude","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"line\n\"quoted\" 🚀"}]}}),
            serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"t2","output":"é\nresult"}}),
            serde_json::json!({"/~key":["", "unicode 中", {"body":{"nested":"structured"}}]}),
        ];
        for record in records {
            let bytes = serde_json::to_vec(&record).unwrap();
            let spans = index_json_string_spans(&bytes).unwrap();
            assert_eq!(spans, index_json_string_spans(&bytes).unwrap());
            for span in spans {
                assert_eq!(
                    decode_all(&bytes[span.start as usize..span.end as usize], 16),
                    record
                        .pointer(&span.json_pointer)
                        .unwrap()
                        .as_str()
                        .unwrap()
                );
            }
        }
        // Structured results require native qualification; no descriptor for
        // the object itself is silently interpreted as a text body.
        let structured = br#"{"payload":{"output":{"nested":"body"}}}"#;
        assert!(
            !index_json_string_spans(structured)
                .unwrap()
                .iter()
                .any(|span| span.json_pointer == "/payload/output")
        );
    }

    #[test]
    fn committed_native_fixture_spans_match_shared_transcript_parser() {
        for fixture in [
            include_str!("../../../tests/fixtures/compatibility/v1/claude-blocks.jsonl"),
            include_str!("../../../tests/fixtures/compatibility/v1/codex-legacy.jsonl"),
            include_str!("../../../tests/fixtures/compatibility/v1/codex-early-item.jsonl"),
        ] {
            for line in fixture.lines().filter(|line| !line.is_empty()) {
                let Some(oracle) = crate::tx::parse_native_record(line.as_bytes()) else {
                    assert!(index_json_string_spans(line.as_bytes()).is_err());
                    continue;
                };
                for span in index_json_string_spans(line.as_bytes()).unwrap() {
                    let decoded =
                        decode_all(&line.as_bytes()[span.start as usize..span.end as usize], 16);
                    assert_eq!(
                        Some(decoded.as_str()),
                        oracle.pointer(&span.json_pointer).unwrap().as_str()
                    );
                }
            }
        }
    }

    #[test]
    fn malformed_records_and_ambiguous_or_oversized_metadata_fail_explicitly() {
        for raw in [
            b"{\"a\":01}".as_slice(),
            b"[true,]",
            b"{\"a\":\"x\"}junk",
            b"[1e]",
            b"[.1]",
            b"{\"a\" \"b\"}",
        ] {
            assert_eq!(
                index_json_string_spans(raw).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        assert_eq!(
            index_json_string_spans(br#"{"a":"first","a":"second"}"#)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        let deep = format!(
            "{}0{}",
            "[".repeat(MAX_DEPTH + 1),
            "]".repeat(MAX_DEPTH + 1)
        );
        assert_eq!(
            index_json_string_spans(deep.as_bytes()).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        let large_key =
            serde_json::to_vec(&serde_json::json!({"a".repeat(MAX_POINTER_BYTES+1): "value"}))
                .unwrap();
        assert_eq!(
            index_json_string_spans(&large_key).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }

    #[test]
    fn large_result_checkpoints_bound_reads_independent_of_record_size() {
        for repeats in [100_000, 1_000_000] {
            let raw = "abcdef\\uD83D\\uDE80é".repeat(repeats);
            let record = format!("{{\"payload\":{{\"output\":\"{raw}\"}}}}");
            let span = index_json_string_spans(record.as_bytes())
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(span.json_pointer, "/payload/output");
            assert_eq!(span.checkpoints[0], span.start);
            assert_eq!(*span.checkpoints.last().unwrap(), span.end);
            let mut reconstructed = String::new();
            for pair in span.checkpoints.windows(2) {
                assert!(pair[1] - pair[0] <= JSON_STRING_CHECKPOINT_BYTES + 11);
                reconstructed.push_str(&decode_all(
                    &record.as_bytes()[pair[0] as usize..pair[1] as usize],
                    4096,
                ));
            }
            let value: serde_json::Value = serde_json::from_str(&record).unwrap();
            assert_eq!(
                reconstructed,
                value.pointer("/payload/output").unwrap().as_str().unwrap()
            );
            let start = span.checkpoints[span.checkpoints.len() - 2] as usize;
            let end = (start + 32 * 1024).min(span.end as usize);
            let chunk = decode_json_string_chunk(
                &record.as_bytes()[start..end],
                end == span.end as usize,
                4096,
            )
            .unwrap();
            assert_eq!(chunk.text.len(), 4096);
            assert!(chunk.consumed <= 12 * 4096);
            assert!(chunk.text.capacity() <= 4096);
        }
    }

    #[test]
    fn diagnostics_do_not_include_source_body_or_arbitrary_keys() {
        let span = index_json_string_spans(br#"{"private-key-sentinel":"private-body-sentinel"}"#)
            .unwrap()
            .pop()
            .unwrap();
        assert!(!format!("{span:?}").contains("private-key-sentinel"));
        let chunk = decode_json_string_chunk(b"private-body-sentinel", true, 128).unwrap();
        assert!(!format!("{chunk:?}").contains("private-body-sentinel"));
    }
}
