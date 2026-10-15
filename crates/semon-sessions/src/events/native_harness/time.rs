//! The native timestamp producer, with content-free resumable state. Fixed
//! numeric slices, ignored separators/tail and the first zone marker retain
//! the complete parser's established behavior, including signed components.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Number {
    value: i64,
    digits: u8,
    negative: bool,
    invalid: bool,
}
impl Number {
    fn byte(&mut self, byte: u8, first: bool) {
        if first && matches!(byte, b'+' | b'-') {
            self.negative = byte == b'-';
        } else if byte.is_ascii_digit() {
            self.value = self.value * 10 + i64::from(byte - b'0');
            self.digits += 1;
        } else {
            self.invalid = true;
        }
    }
    fn result(&self) -> Option<i64> {
        (!self.invalid && self.digits != 0).then_some(if self.negative {
            -self.value
        } else {
            self.value
        })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    at: u64,
    parts: [Number; 6],
    invalid: bool,
    fraction: bool,
    fraction_done: bool,
    fraction_digits: u8,
    fraction_value: i64,
    zone: Option<u8>,
    zone_at: u8,
    zone_parts: [Number; 2],
}
impl Cursor {
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.settled() {
                break;
            }
            if self.at < 19 {
                let component = match self.at {
                    0..=3 => Some((0, 0)),
                    5..=6 => Some((1, 5)),
                    8..=9 => Some((2, 8)),
                    11..=12 => Some((3, 11)),
                    14..=15 => Some((4, 14)),
                    17..=18 => Some((5, 17)),
                    _ => None,
                };
                if let Some((part, start)) = component {
                    self.parts[part].byte(byte, self.at == start);
                } else if !byte.is_ascii() {
                    // A multibyte separator crosses a fixed native slice's
                    // UTF-8 boundary. The complete str::get rejects it too.
                    self.invalid = true;
                }
            } else {
                if self.at == 19 {
                    self.fraction = byte == b'.';
                } else if self.fraction && !self.fraction_done {
                    if byte.is_ascii_digit() && self.fraction_digits < 3 {
                        self.fraction_value = self.fraction_value * 10 + i64::from(byte - b'0');
                        self.fraction_digits += 1;
                    } else {
                        self.fraction_done = true;
                    }
                }
                match self.zone {
                    None if matches!(byte, b'Z' | b'+' | b'-') => self.zone = Some(byte),
                    Some(b'+' | b'-') if self.zone_at < 5 => {
                        let part = match self.zone_at {
                            0..=1 => Some((0, 0)),
                            3..=4 => Some((1, 3)),
                            _ => None,
                        };
                        if let Some((part, start)) = part {
                            self.zone_parts[part].byte(byte, self.zone_at == start);
                        } else if !byte.is_ascii() {
                            self.invalid = true;
                        }
                        self.zone_at += 1;
                    }
                    _ => {}
                }
            }
            self.at = self.at.saturating_add(1);
        }
    }
    pub(crate) fn settled(&self) -> bool {
        self.invalid
            || self.parts.iter().any(|part| part.invalid)
            || self.zone == Some(b'Z')
            || self.zone_at == 5
    }
    pub(crate) fn seconds(&self) -> Option<u64> {
        if self.invalid || self.at < 19 {
            return None;
        }
        let parts = self.parts.each_ref().map(Number::result);
        let offset = match self.zone? {
            b'Z' => 0,
            sign if self.zone_at == 5 => {
                let hours = self.zone_parts[0].result()?;
                let minutes = self.zone_parts[1].result()?;
                if hours > 23 || minutes > 59 {
                    return None;
                }
                (hours * 3600 + minutes * 60) * if sign == b'+' { 1 } else { -1 }
            }
            _ => return None,
        };
        crate::native_time_from_parts(
            parts[0]?, parts[1]?, parts[2]?, parts[3]?, parts[4]?, parts[5]?, offset,
        )
    }
    pub(crate) fn millis(&self) -> Option<i64> {
        let seconds = i64::try_from(self.seconds()?).ok()?;
        let fraction = self.fraction_value * 10_i64.pow(u32::from(3 - self.fraction_digits));
        Some(seconds * 1000 + fraction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Independent frozen native behavior; this intentionally does not call the
    // new cursor/calculation adapter when comparing its grammar and rounding.
    fn reference(value: &str) -> Option<i64> {
        let date = value.get(..10)?;
        let time = value.get(11..19)?;
        let year: i64 = date.get(..4)?.parse().ok()?;
        let month: i64 = date.get(5..7)?.parse().ok()?;
        let day: i64 = date.get(8..10)?.parse().ok()?;
        let hour: i64 = time.get(..2)?.parse().ok()?;
        let minute: i64 = time.get(3..5)?.parse().ok()?;
        let second: i64 = time.get(6..8)?.parse().ok()?;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || hour > 23
            || minute > 59
            || second > 60
        {
            return None;
        }
        let tail = value.get(19..)?;
        let zone = tail.find(['Z', '+', '-'])?;
        let offset = match tail.as_bytes().get(zone)? {
            b'Z' => 0,
            sign => {
                let tz = tail.get(zone + 1..)?;
                let hours: i64 = tz.get(..2)?.parse().ok()?;
                let minutes: i64 = tz.get(3..5)?.parse().ok()?;
                if hours > 23 || minutes > 59 {
                    return None;
                }
                (hours * 3600 + minutes * 60) * if *sign == b'+' { 1 } else { -1 }
            }
        };
        let year = year - i64::from(month <= 2);
        let era = year.div_euclid(400);
        let yoe = year - era * 400;
        let mp = month + if month > 2 { -3 } else { 9 };
        let doy = (153 * mp + 2) / 5 + day - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let seconds = u64::try_from(
            (era * 146097 + doe - 719468) * 86400 + hour * 3600 + minute * 60 + second - offset,
        )
        .ok()?;
        let fraction = tail
            .strip_prefix('.')
            .map(|s| {
                s.chars()
                    .take_while(char::is_ascii_digit)
                    .chain("000".chars())
                    .take(3)
                    .collect::<String>()
                    .parse::<i64>()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        Some(i64::try_from(seconds).ok()? * 1000 + fraction)
    }
    #[test]
    fn streamed_time_equals_independent_native_parser_at_every_split() {
        for value in [
            "2026-10-15T01:02:03Z",
            "2026x10x15x-1x-2x-3.12Zsuffix",
            "+123-01-01T00:00:00Z",
            "9999-02-31T23:59:60+23:59ignored",
            "2026-10-15T01:02:03junk-01:-1rest",
            "2026-10-15T01:02:03.123456λZ",
            "2026-10-15T01:02:03.λ5Z",
            "2026-10-15T01:02:03+01λ12",
            "2026-10-15λ01:02:03Z",
            "2026-00-15T01:02:03Z",
            "1960-10-15T01:02:03Z",
            "2026-10-15T01:02:03",
        ] {
            for split in 0..=value.len() {
                let mut cursor = Cursor::default();
                cursor.push(&value.as_bytes()[..split]);
                cursor = serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap();
                cursor.push(&value.as_bytes()[split..]);
                assert_eq!(cursor.millis(), reference(value), "{value:?} at {split}");
                assert_eq!(super::super::super::parse_ms(value), reference(value));
            }
        }
        for size in [0, 128, 1024, 8192, 200_000] {
            let value = format!(
                "2026-10-15T01:02:03.12{}Z{}",
                "λ".repeat(size),
                "x".repeat(size)
            );
            let mut cursor = Cursor::default();
            for bytes in value.as_bytes().chunks(127) {
                cursor.push(bytes);
            }
            assert_eq!(cursor.millis(), reference(&value));
            assert!(serde_json::to_vec(&cursor).unwrap().len() < 2048);
        }
    }
}
