//! Bounded continuation of the complete native serde_json number producer.
//! Keep its default u64-significand/exponent components, then give a compact
//! equivalent spelling back to serde_json::Number for the final conversion.
//! Do not implement a second float converter or retain numeric source text.
//!
//! Native provenance: serde_json 1.0.151, default non-float_roundtrip numeric
//! paths parse_integer/parse_decimal/parse_exponent/parse_long_integer in
//! https://github.com/serde-rs/json/blob/v1.0.151/src/de.rs . The independent
//! oracle below must change with any dependency numeric policy/feature change.

use super::{JSON_STRING_CHUNK_DECODED_MAX, invalid, native_record::Number};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "NumberParts")]
pub(crate) enum NativeNumber {
    Unsigned(u64),
    Signed(i64),
    // Persist bits, because JSON serialization followed by the native default
    // float parser need not preserve every already-converted f64 bit pattern.
    Float(u64),
}
#[derive(Deserialize)]
enum NumberParts {
    Unsigned(u64),
    Signed(i64),
    Float(u64),
}
impl TryFrom<NumberParts> for NativeNumber {
    type Error = &'static str;
    fn try_from(value: NumberParts) -> Result<Self, Self::Error> {
        Ok(match value {
            NumberParts::Unsigned(value) => Self::Unsigned(value),
            NumberParts::Signed(value) => Self::Signed(value),
            NumberParts::Float(bits) if f64::from_bits(bits).is_finite() => Self::Float(bits),
            NumberParts::Float(_) => return Err("native number receipt is not finite"),
        })
    }
}
impl NativeNumber {
    pub(crate) fn number(self) -> serde_json::Number {
        match self {
            Self::Unsigned(value) => value.into(),
            Self::Signed(value) => value.into(),
            Self::Float(bits) => serde_json::Number::from_f64(f64::from_bits(bits))
                .expect("qualified native finite number"),
        }
    }
    fn from_native(number: serde_json::Number) -> Self {
        if number.is_f64() {
            Self::Float(number.as_f64().expect("native float").to_bits())
        } else if let Some(value) = number.as_u64() {
            Self::Unsigned(value)
        } else {
            Self::Signed(number.as_i64().expect("native signed integer"))
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Qualification {
    Finite(NativeNumber),
    Invalid,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    start: u64,
    end: u64,
    pub(crate) at: u64,
    state: Option<Number>,
    positive: bool,
    significand: u64,
    scale: i32,
    long_integer: bool,
    decimal_overflow: bool,
    float: bool,
    exponent_positive: bool,
    exponent: i32,
    exponent_overflow: bool,
    pub(crate) result: Option<Qualification>,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct Step {
    pub(crate) consumed: usize,
    pub(crate) complete: bool,
}
impl Cursor {
    pub(crate) fn new(start: u64, end: u64) -> io::Result<Self> {
        if start >= end {
            return Err(invalid());
        }
        Ok(Self {
            start,
            end,
            at: start,
            state: None,
            positive: true,
            significand: 0,
            scale: 0,
            long_integer: false,
            decimal_overflow: false,
            float: false,
            exponent_positive: true,
            exponent: 0,
            exponent_overflow: false,
            result: None,
        })
    }
    pub(crate) fn step(&mut self, bytes: &[u8], budget: usize) -> io::Result<Step> {
        if self.at < self.start
            || self.at > self.end
            || self.result.is_some()
            || budget == 0
            || budget > JSON_STRING_CHUNK_DECODED_MAX
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid native number continuation or byte budget",
            ));
        }
        let maximum = bytes
            .len()
            .min(budget)
            .min(usize::try_from(self.end - self.at).unwrap_or(usize::MAX));
        for (index, &byte) in bytes[..maximum].iter().enumerate() {
            if self.consume(byte).is_err() {
                self.result = Some(Qualification::Invalid);
                self.at = self.at.checked_add(index as u64 + 1).ok_or_else(invalid)?;
                return Ok(Step {
                    consumed: index + 1,
                    complete: true,
                });
            }
        }
        self.at = self.at.checked_add(maximum as u64).ok_or_else(invalid)?;
        if self.at == self.end {
            self.finish()
        }
        Ok(Step {
            consumed: maximum,
            complete: self.result.is_some(),
        })
    }
    fn consume(&mut self, byte: u8) -> io::Result<()> {
        let old = self.state;
        self.state = Some(match old {
            None => match byte {
                b'-' => {
                    self.positive = false;
                    Number::Sign
                }
                b'0' => Number::Zero,
                b'1'..=b'9' => Number::Integer,
                _ => return Err(invalid()),
            },
            Some(state) => state.next(byte)?.ok_or_else(invalid)?,
        });
        match byte {
            b'0'..=b'9' => {
                let digit = u64::from(byte - b'0');
                match old {
                    None | Some(Number::Sign) => self.significand = digit,
                    Some(Number::Integer) => {
                        if self.long_integer {
                            self.scale = self.scale.wrapping_add(1);
                        } else if let Some(value) = self
                            .significand
                            .checked_mul(10)
                            .and_then(|value| value.checked_add(digit))
                        {
                            self.significand = value;
                        } else {
                            self.long_integer = true;
                            self.float = true;
                            self.scale = self.scale.wrapping_add(1);
                        }
                    }
                    Some(Number::Dot | Number::Fraction) => {
                        if !self.decimal_overflow {
                            if let Some(value) = self
                                .significand
                                .checked_mul(10)
                                .and_then(|value| value.checked_add(digit))
                            {
                                self.significand = value;
                                self.scale = self.scale.wrapping_sub(1);
                            } else {
                                self.decimal_overflow = true
                            }
                        }
                    }
                    Some(Number::Exponent | Number::ExponentSign | Number::ExponentDigits) => {
                        if !self.exponent_overflow {
                            if let Some(value) = self
                                .exponent
                                .checked_mul(10)
                                .and_then(|value| value.checked_add(digit as i32))
                            {
                                self.exponent = value;
                            } else {
                                self.exponent_overflow = true
                            }
                        }
                    }
                    _ => return Err(invalid()),
                }
            }
            b'.' | b'e' | b'E' => self.float = true,
            b'-' if matches!(old, Some(Number::Exponent)) => self.exponent_positive = false,
            b'-' | b'+' => {}
            _ => return Err(invalid()),
        }
        Ok(())
    }
    fn finish(&mut self) {
        if self.state.is_none_or(|state| !state.complete()) {
            self.result = Some(Qualification::Invalid);
            return;
        }
        let sign = if self.positive { "" } else { "-" };
        let compact = if self.float {
            if self.exponent_overflow {
                if self.exponent_positive && self.significand != 0 {
                    self.result = Some(Qualification::Invalid);
                    return;
                }
                format!("{sign}0e0")
            } else {
                let scale = if self.exponent_positive {
                    self.scale.saturating_add(self.exponent)
                } else {
                    self.scale.saturating_sub(self.exponent)
                };
                format!("{sign}{}e{scale}", self.significand)
            }
        } else {
            format!("{sign}{}", self.significand)
        };
        self.result = Some(match serde_json::from_str::<serde_json::Number>(&compact) {
            Ok(value) => Qualification::Finite(NativeNumber::from_native(value)),
            Err(_) => Qualification::Invalid,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn qualify(text: &str, page: usize) -> Qualification {
        let mut cursor = Cursor::new(0, text.len() as u64).unwrap();
        while cursor.result.is_none() {
            let at = cursor.at as usize;
            let step = cursor.step(&text.as_bytes()[at..], page).unwrap();
            assert!(step.consumed <= page);
            let saved = serde_json::to_vec(&cursor).unwrap();
            assert!(saved.len() < 1024);
            cursor = serde_json::from_slice(&saved).unwrap();
        }
        cursor.result.unwrap()
    }
    fn assert_native(text: &str) {
        let native = serde_json::from_str::<serde_json::Number>(text);
        for page in [1, 7, 64, 4096] {
            let actual = qualify(text, page);
            match (native.as_ref(), actual) {
                (Ok(expected), Qualification::Finite(actual)) => {
                    let actual = actual.number();
                    assert_eq!(actual.is_f64(), expected.is_f64(), "{text}");
                    assert_eq!(actual.as_i64(), expected.as_i64(), "{text}");
                    assert_eq!(actual.as_u64(), expected.as_u64(), "{text}");
                    assert_eq!(
                        actual.as_f64().map(f64::to_bits),
                        expected.as_f64().map(f64::to_bits),
                        "{text}"
                    );
                }
                (Err(_), Qualification::Invalid) => {}
                _ => panic!("native number qualification disagrees for {text}"),
            }
        }
    }
    #[test]
    fn bounded_native_number_matches_complete_library_policy_and_bits() {
        for text in [
            "0",
            "-0",
            "0.0",
            "-0.0",
            "1",
            "-1",
            "9223372036854775807",
            "9223372036854775808",
            "-9223372036854775808",
            "-9223372036854775809",
            "18446744073709551615",
            "18446744073709551616",
            "-18446744073709551615",
            "-18446744073709551616",
            "1.2345678901234567890123456789",
            "0.00000000000000000000000001",
            "12345678901234567890123456789.0123456789e-20",
            "1e308",
            "1e309",
            "1e-400",
            "-1e-400",
            "0e2147483648",
            "-0e2147483648",
            "1e2147483648",
            "1e-2147483648",
            "1e2147483647",
            "1e-2147483647",
            "1e",
            "01",
            "1.",
        ] {
            assert_native(text)
        }
        for size in [0, 128, 1024, 8192] {
            assert_native(&format!("1{}e-{size}", "0".repeat(size)));
            assert_native(&format!("0.{}1", "0".repeat(size)));
            assert_native(&format!("1.{}e-7", "7".repeat(size)));
            assert_native(&format!("0e{}", "9".repeat(size + 1)));
            assert_native(&format!("1e-{}", "9".repeat(size + 1)));
        }
    }
    #[test]
    fn numeric_state_residency_and_step_bytes_stay_constant() {
        for size in [0, 128, 1024, 8192] {
            let text = format!("1.{}e-128", "3".repeat(size + 1));
            let mut cursor = Cursor::new(0, text.len() as u64).unwrap();
            while cursor.result.is_none() {
                let at = cursor.at as usize;
                let step = cursor.step(&text.as_bytes()[at..], 64).unwrap();
                assert!(step.consumed <= 64);
                assert!(serde_json::to_vec(&cursor).unwrap().len() < 1024);
            }
        }
    }
}
