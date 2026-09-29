//! Images attached to a prompt. `/api/tx` names each one by where it is (its
//! prompt's line and block) and says whether it can be shown; the bytes are
//! read back from that line one image at a time by `/api/attachment`. They
//! are never inlined into a page's JSON, and the index and its cache hold
//! none of them (risk:secret, as for text).
//!
//! An image is served only when its line carries it inline as base64, with a
//! declared type among [`TYPES`] (never SVG) whose signature its bytes start
//! with, and at most [`IMAGE_MAX`] bytes decoded. Anything else, such as a
//! Codex image given by a local path, or bytes a redacted copy of the logs
//! masked, is an image that isn't available.

use serde_json::{Value, json};

use crate::field;

/// The image types served, each the exact `Content-Type` it is served with.
pub(crate) const TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// The largest image served, decoded: above what either harness sends a
/// model (Claude's limit is 5 MB an image), well under the model's cap on a
/// line (64 MiB).
pub(crate) const IMAGE_MAX: usize = 8 * 1024 * 1024;

/// A prompt's content parts: a queued command's `prompt`, a Claude user
/// record's `message.content`, or a Codex user message's `content`.
pub(crate) fn parts(record: &Value) -> Option<&[Value]> {
    if let Some(attachment) = record.get("attachment") {
        if field(attachment, "type") != Some("queued_command") {
            return None;
        }
        return attachment.get("prompt")?.as_array().map(Vec::as_slice);
    }
    if let Some(message) = record.get("message") {
        if field(record, "type") != Some("user") {
            return None;
        }
        return message.get("content")?.as_array().map(Vec::as_slice);
    }
    let payload = record.get("payload")?;
    if field(payload, "type") != Some("message") || field(payload, "role") != Some("user") {
        return None;
    }
    payload.get("content")?.as_array().map(Vec::as_slice)
}

/// A Claude `image` block or a Codex `input_image` part.
pub(crate) fn is_image(part: &Value) -> bool {
    matches!(field(part, "type"), Some("image" | "input_image"))
}

/// Whether a prompt's parts attach an image. A Claude record that answers a
/// tool call is not a prompt: an image in it is the tool's.
pub(crate) fn has_image(parts: &[Value]) -> bool {
    parts.iter().any(is_image)
        && !parts
            .iter()
            .any(|part| field(part, "type") == Some("tool_result"))
}

/// A Codex `<image>` or `<image name=[Image #1]>` part, or the `</image>`
/// that closes one: the harness's frame around an image, not your text.
pub(crate) fn is_image_tag(text: &str) -> bool {
    let text = text.trim();
    if text == "</image>" || text == "<image>" {
        return true;
    }
    text.strip_prefix("<image name=")
        .and_then(|rest| rest.strip_suffix('>'))
        .is_some_and(|name| !name.contains(['<', '>', '\n']))
}

/// `text` without the `[Image #N]` placeholders the harness writes where an
/// image was attached (and one space after each), trimmed.
pub(crate) fn strip_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[Image #") {
        let after = &rest[start + "[Image #".len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with(']') {
            out.push_str(&rest[..start]);
            let tail = &after[digits + 1..];
            rest = tail.strip_prefix(' ').unwrap_or(tail);
        } else {
            out.push_str(&rest[..start + 1]);
            rest = &rest[start + 1..];
        }
    }
    out.push_str(rest);
    out.trim().to_owned()
}

/// An image part's inline bytes: the type it declares and its base64.
struct Inline<'a> {
    media: &'a str,
    data: &'a str,
}

fn inline(part: &Value) -> Option<Inline<'_>> {
    match field(part, "type")? {
        "image" => {
            let source = part.get("source")?;
            if field(source, "type")? != "base64" {
                return None;
            }
            Some(Inline {
                media: field(source, "media_type")?,
                data: field(source, "data")?,
            })
        }
        "input_image" => {
            let url = match part.get("image_url")? {
                Value::String(url) => url.as_str(),
                other => field(other, "url")?,
            };
            let (head, data) = url.strip_prefix("data:")?.split_once(',')?;
            Some(Inline {
                media: head.strip_suffix(";base64")?,
                data,
            })
        }
        _ => None,
    }
}

/// The served type a declared one names, if it is one of [`TYPES`].
fn allowed(media: &str) -> Option<&'static str> {
    if media.eq_ignore_ascii_case("image/jpg") {
        return Some("image/jpeg");
    }
    TYPES
        .into_iter()
        .find(|served| served.eq_ignore_ascii_case(media))
}

/// Whether `content_type` is one an attachment is served with.
pub(crate) fn is_served_type(content_type: &str) -> bool {
    TYPES.contains(&content_type)
}

fn sextet(byte: u8) -> Option<u32> {
    let value = match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    };
    Some(u32::from(value))
}

/// Standard base64 (RFC 4648 §4, padding optional) without its padding, and
/// its decoded length; `None` when its length or padding can't be base64.
fn unpadded(data: &[u8]) -> Option<(&[u8], usize)> {
    let pad = data.iter().rev().take_while(|byte| **byte == b'=').count();
    if pad > 2 || (pad > 0 && !data.len().is_multiple_of(4)) {
        return None;
    }
    let body = &data[..data.len() - pad];
    let tail = match body.len() % 4 {
        0 => 0,
        2 => 1,
        3 => 2,
        _ => return None,
    };
    Some((body, body.len() / 4 * 3 + tail))
}

/// Decodes standard base64, refusing any other byte (whitespace included)
/// and anything that would decode to more than `max` bytes, before it
/// allocates.
fn decode(data: &[u8], max: usize) -> Option<Vec<u8>> {
    let (body, length) = unpadded(data)?;
    if length > max {
        return None;
    }
    let mut out = Vec::with_capacity(length);
    for chunk in body.chunks(4) {
        let mut n = 0;
        for byte in chunk {
            n = (n << 6) | sextet(*byte)?;
        }
        n <<= 6 * (4 - chunk.len()) as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..chunk.len() - 1]);
    }
    Some(out)
}

/// Whether `bytes` start with the signature of the image type `media`.
fn signature(media: &str, bytes: &[u8]) -> bool {
    match media {
        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => bytes.starts_with(&[0xFF, 0xD8, 0xFF]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()),
        _ => false,
    }
}

/// The decoded bytes read to find an image's size: a JPEG's frame header can
/// follow metadata segments (a phone's EXIF, a colour profile); the other
/// types have theirs in their first 30 bytes.
const JPEG_HEAD: usize = 256 * 1024;
const HEAD: usize = 30;

/// The first `want` decoded bytes of unpadded base64 `body`, or all of it.
fn head(body: &[u8], want: usize) -> Option<Vec<u8>> {
    let chars = want.div_ceil(3) * 4;
    decode(&body[..body.len().min(chars)], IMAGE_MAX)
}

fn be16(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes(
        bytes.get(at..at + 2)?.try_into().ok()?,
    )))
}

fn le16(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from(u16::from_le_bytes(
        bytes.get(at..at + 2)?.try_into().ok()?,
    )))
}

fn le24(bytes: &[u8], at: usize) -> Option<u32> {
    let b = bytes.get(at..at + 3)?;
    Some(u32::from(b[0]) | (u32::from(b[1]) << 8) | (u32::from(b[2]) << 16))
}

/// A JPEG's width and height, from its first frame header (SOFn), walking
/// the marker segments before it.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        match marker {
            // Fill bytes, and markers that stand alone.
            0xFF => at += 1,
            0x01 | 0xD0..=0xD8 => at += 2,
            // The scan or the end, before any frame header.
            0xD9 | 0xDA => return None,
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                return Some((be16(bytes, at + 7)?, be16(bytes, at + 5)?));
            }
            _ => at += 2 + usize::try_from(be16(bytes, at + 2)?).ok()?,
        }
    }
}

/// An image's width and height as its header states them, for a type whose
/// signature `bytes` already start with.
fn dimensions(media: &str, bytes: &[u8]) -> Option<(u32, u32)> {
    let size = match media {
        "image/png" if bytes.get(12..16) == Some(b"IHDR".as_slice()) => {
            let word =
                |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
            (word(16)?, word(20)?)
        }
        "image/gif" => (le16(bytes, 6)?, le16(bytes, 8)?),
        "image/webp" => match bytes.get(12..16)? {
            b"VP8 " if bytes.get(23..26) == Some([0x9D, 0x01, 0x2A].as_slice()) => {
                (le16(bytes, 26)? & 0x3FFF, le16(bytes, 28)? & 0x3FFF)
            }
            b"VP8L" if bytes.get(20) == Some(&0x2F) => {
                let b: [u8; 4] = bytes.get(21..25)?.try_into().ok()?;
                let bits = u32::from_le_bytes(b);
                (1 + (bits & 0x3FFF), 1 + ((bits >> 14) & 0x3FFF))
            }
            b"VP8X" => (1 + le24(bytes, 24)?, 1 + le24(bytes, 27)?),
            _ => return None,
        },
        "image/jpeg" => jpeg_size(bytes)?,
        _ => return None,
    };
    (size.0 > 0 && size.1 > 0).then_some(size)
}

/// FNV-1a, 64 bits: names an image's exact base64 in its URL, so the URL
/// names that content, and a line rewritten under the same offset is a
/// different URL.
fn version(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// An image that can be served, checked without decoding it whole: its type,
/// length, every byte of its base64, and the signature its first bytes
/// decode to. Its size, as its header states it, when that is found.
struct Servable {
    media: &'static str,
    length: usize,
    version: u64,
    size: Option<(u32, u32)>,
}

fn servable(part: &Value) -> Option<Servable> {
    let found = inline(part)?;
    let media = allowed(found.media)?;
    let data = found.data.as_bytes();
    let (body, length) = unpadded(data)?;
    if length > IMAGE_MAX || !body.iter().all(|byte| sextet(*byte).is_some()) {
        return None;
    }
    let want = if media == "image/jpeg" {
        JPEG_HEAD
    } else {
        HEAD
    };
    let first = head(body, want)?;
    signature(media, &first).then(|| Servable {
        media,
        length,
        version: version(data),
        size: dimensions(media, &first),
    })
}

/// The images a prompt's line attaches, in order, as `/api/tx` gives them:
/// each one's block `b`, and, when it can be served, its type, decoded size,
/// the version its URL names (`v`, [`version`]) and, when its header states
/// them, its width and height (`w`, `h`), so a thumbnail's box is right
/// before it loads; `na` when it can't be served. Empty for a line that
/// isn't a prompt.
pub(crate) fn refs(record: &Value) -> Vec<Value> {
    let Some(parts) = parts(record).filter(|parts| has_image(parts)) else {
        return Vec::new();
    };
    parts
        .iter()
        .enumerate()
        .filter(|(_, part)| is_image(part))
        .map(|(block, part)| match servable(part) {
            Some(found) => {
                let mut image = json!({"b": block, "type": found.media, "size": found.length, "v": format!("{:016x}", found.version)});
                if let Some((width, height)) = found.size {
                    image["w"] = json!(width);
                    image["h"] = json!(height);
                }
                image
            }
            None => json!({"b": block, "na": true}),
        })
        .collect()
}

/// Image `block` of a prompt's line, decoded, and the type it is served
/// with; `None` when that block is no image that can be served, or its
/// base64 isn't the content `version` names.
pub(crate) fn image(record: &Value, block: usize, want: u64) -> Option<(&'static str, Vec<u8>)> {
    let parts = parts(record).filter(|parts| has_image(parts))?;
    let part = parts.get(block).filter(|part| is_image(part))?;
    let found = inline(part)?;
    let media = allowed(found.media)?;
    if version(found.data.as_bytes()) != want {
        return None;
    }
    let bytes = decode(found.data.as_bytes(), IMAGE_MAX)?;
    signature(media, &bytes).then_some((media, bytes))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub(crate) fn encode(bytes: &[u8]) -> String {
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for index in 0..4 {
                if index <= chunk.len() {
                    out.push(ALPHABET[((n >> (18 - 6 * index)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[test]
    fn base64_round_trips_with_or_without_padding_and_refuses_the_rest() {
        let all: Vec<u8> = (0..=255).collect();
        for length in 0..40 {
            let raw = &all[..length];
            let encoded = encode(raw);
            assert_eq!(decode(encoded.as_bytes(), usize::MAX).unwrap(), raw);
            let bare = encoded.trim_end_matches('=');
            assert_eq!(decode(bare.as_bytes(), usize::MAX).unwrap(), raw);
        }
        for bad in [
            "A", "AAAAA", "AA=A", "A===", "AA AA", "AA\nAA", "AA*A", "AAA-", "AA_A", "====",
        ] {
            assert!(decode(bad.as_bytes(), usize::MAX).is_none(), "{bad:?}");
        }
        assert_eq!(decode(b"AAAA", 3).unwrap().len(), 3);
        assert!(
            decode(b"AAAA", 2).is_none(),
            "the cap is checked before decoding"
        );
    }

    #[test]
    fn placeholders_and_codex_image_tags() {
        assert_eq!(
            strip_placeholders("[Image #1] [Image #2] The top bar"),
            "The top bar"
        );
        assert_eq!(
            strip_placeholders("Look at [Image #12] here"),
            "Look at here"
        );
        assert_eq!(
            strip_placeholders("[Image #] and [Image #x] stay"),
            "[Image #] and [Image #x] stay"
        );
        assert_eq!(strip_placeholders("[Image #1]"), "");
        assert!(is_image_tag("<image name=[Image #1]>"));
        assert!(is_image_tag("</image>"));
        assert!(is_image_tag("<image>"));
        assert!(!is_image_tag("<image name=<b>>"));
        assert!(!is_image_tag("<images>"));
    }

    /// The `v` a URL names for base64 `data`.
    pub(crate) fn version_of(data: &str) -> String {
        format!("{:016x}", version(data.as_bytes()))
    }

    #[test]
    fn only_the_four_raster_types_are_served() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        let part = |media: &str, bytes: &[u8]| json!({"type":"image","source":{"type":"base64","media_type":media,"data":encode(bytes)}});
        let record =
            |parts: Vec<Value>| json!({"type":"user","message":{"role":"user","content":parts}});
        let v = |bytes: &[u8]| version(encode(bytes).as_bytes());
        let served = |parts: Vec<Value>, bytes: &[u8]| image(&record(parts), 0, v(bytes));
        assert_eq!(
            served(vec![part("image/png", png)], png).unwrap().0,
            "image/png"
        );
        let jpeg = b"\xFF\xD8\xFF\xE0";
        assert_eq!(
            served(vec![part("IMAGE/JPG", jpeg)], jpeg).unwrap().0,
            "image/jpeg"
        );
        // The version must name this block's content.
        assert!(served(vec![part("image/png", png)], jpeg).is_none());
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'/>";
        assert!(served(vec![part("image/svg+xml", svg)], svg).is_none());
        assert!(
            served(vec![part("image/png", svg)], svg).is_none(),
            "an SVG declared as PNG"
        );
        assert!(served(vec![part("text/html", png)], png).is_none());
        assert_eq!(
            refs(&record(vec![
                part("image/svg+xml", b"<svg/>"),
                json!({"type":"text","text":"x"}),
                part("image/png", png)
            ])),
            vec![
                json!({"b":0,"na":true}),
                json!({"b":2,"type":"image/png","size":png.len(),"v":format!("{:016x}", v(png))})
            ]
        );
        // A tool's result is not a prompt: an image in it is not an attachment.
        let answered = record(vec![
            json!({"type":"tool_result","tool_use_id":"t","content":"x"}),
            part("image/png", png),
        ]);
        assert!(refs(&answered).is_empty());
        assert!(image(&answered, 1, v(png)).is_none());
        // A Codex data URL; a local path or a remote URL isn't in the log.
        let codex = |url: &str| json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_image","image_url":url}]}});
        let url = format!("data:image/png;base64,{}", encode(png));
        assert_eq!(image(&codex(&url), 0, v(png)).unwrap().1, png);
        for url in [
            "/home/me/shot.png",
            "https://example.com/a.png",
            "data:image/png,raw",
        ] {
            assert!(
                image(&codex(url), 0, version(url.as_bytes())).is_none(),
                "{url}"
            );
            assert_eq!(refs(&codex(url)), vec![json!({"b":0,"na":true})], "{url}");
        }
    }

    #[test]
    fn sizes_are_read_from_each_types_header() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\x04\x92\0\0\x09\xe4\x08\x02";
        assert_eq!(dimensions("image/png", png), Some((1170, 2532)));
        assert_eq!(
            dimensions("image/gif", b"GIF89a\x40\x01\xf0\0"),
            Some((320, 240))
        );
        let riff = |chunk: &[u8]| [b"RIFF\0\0\0\0WEBP".as_slice(), chunk].concat();
        // Lossy: a key frame's start code, then 14-bit width and height.
        let lossy = riff(b"VP8 \0\0\0\0\x30\x01\0\x9d\x01\x2a\x80\x02\xe0\x01");
        assert_eq!(dimensions("image/webp", &lossy), Some((640, 480)));
        // Lossless: 0x2f, then width - 1 and height - 1 in 14 bits each.
        let bits: u32 = (640 - 1) | ((480 - 1) << 14);
        let lossless = riff(&[b"VP8L\0\0\0\0\x2f".as_slice(), &bits.to_le_bytes()].concat());
        assert_eq!(dimensions("image/webp", &lossless), Some((640, 480)));
        // Extended: the canvas's width - 1 and height - 1 in 24 bits each.
        let extended = riff(b"VP8X\x0a\0\0\0\x10\0\0\0\x7f\x02\0\xdf\x01\0");
        assert_eq!(dimensions("image/webp", &extended), Some((640, 480)));
        // A JPEG's frame header after an EXIF segment and a fill byte; a
        // scan before any frame header, or a truncated one, has no size.
        let mut jpeg = b"\xFF\xD8\xFF\xE1\x01\x02".to_vec();
        jpeg.resize(jpeg.len() + 0x100, b'x');
        jpeg.extend_from_slice(b"\xFF\xFF\xC2\0\x11\x08\x09\xe4\x04\x92\x03");
        assert_eq!(dimensions("image/jpeg", &jpeg), Some((1170, 2532)));
        assert_eq!(dimensions("image/jpeg", b"\xFF\xD8\xFF\xDA\0\x02"), None);
        assert_eq!(
            dimensions("image/jpeg", b"\xFF\xD8\xFF\xC0\0\x11\x08"),
            None
        );
        assert_eq!(
            dimensions("image/gif", b"GIF89a\0\0\x01\0"),
            None,
            "no zero sizes"
        );
        // The size of a whole image in a line comes out in its reference.
        let record = json!({"type":"user","message":{"role":"user","content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":encode(png)}}]}});
        let found = &refs(&record)[0];
        assert_eq!(
            (found["w"].as_u64(), found["h"].as_u64()),
            (Some(1170), Some(2532))
        );
    }
}
