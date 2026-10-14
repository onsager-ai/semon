//! Bounded continuation of the resolved native serde_json number producer.
//!
//! serde_json 1.0.151 has `float_roundtrip` enabled by semon-control's
//! serde_json_canonicalizer dependency. Its lexical f64 producer retains 768
//! significant decimal digits and one native sticky digit (lexical/bhcomp.rs and
//! lexical/num.rs, MAX_DIGITS = 769). Retain that bounded evidence, then delegate
//! conversion of an equivalent compact spelling to the same serde_json owner.
//!
//! Every f64 rounding midpoint is m * 2^q, m < 2^54 and q >= -1075. When q < 0,
//! its terminating decimal numerator m * 5^(-q) has at most 768 digits because
//! log10(2^54 * 5^1075) < 768. Thus a discarded nonzero digit after digit 768
//! can distinguish an exact midpoint from a value above it, but cannot cross
//! another midpoint. A single sticky 1 has the same rounding consequence. The
//! native parser additionally treats any discarded INTEGER digit as sticky,
//! even a zero: parse_truncated_float trims only fractional trailing zeroes,
//! while parse_mantissa unconditionally appends 1 for remaining digits. Preserve
//! that source-channel distinction, including the native result when a long
//! compensated integer spelling differs from its mathematical decimal twin.
//! This includes zero/min-subnormal and max-finite/overflow boundaries. No float
//! conversion or rounding is implemented here.
//!
//! Primary owner: https://github.com/serde-rs/json/tree/v1.0.151/src/lexical .
//! de.rs:623-636, 674-713, 740-833, 876-899; lexical/bhcomp.rs:21-75,
//! 192-222; lexical/num.rs:366-370, lexical/parse.rs:49-87 and
//! lexical/exponent.rs:25-49.
//! Changing that owner's version/features requires new derived metadata policy
//! and independent complete-parser qualification; old receipts cannot be reused.

use super::{JSON_STRING_CHUNK_DECODED_MAX, invalid, native_record::Number};
use serde::{Deserialize, Serialize};
use std::io;

pub(crate) const PRODUCER_POLICY: &str = "serde-json-1.0.151-float-roundtrip-prefix768-v1";
const SIGNIFICANT_DIGITS: usize = 768;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "NumberParts")]
pub(crate) enum NativeNumber {
    Unsigned(u64),
    Signed(i64),
    // Persist already-qualified finite bits, independently of JSON spellings.
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
    // Compact field names preserve the existing 1 KiB fixture checkpoint
    // assertion even after retaining the necessary rounding evidence. Version 1
    // explicitly refuses previous u64-only checkpoints.
    #[serde(rename = "v")]
    policy: u8,
    #[serde(rename = "0")]
    start: u64,
    #[serde(rename = "1")]
    end: u64,
    #[serde(rename = "2")]
    pub(crate) at: u64,
    #[serde(rename = "3")]
    state: Option<Number>,
    #[serde(rename = "4")]
    positive: bool,
    // Bounded significant source evidence, never the full numeric token.
    #[serde(rename = "5")]
    prefix: String,
    #[serde(rename = "6")]
    sticky: bool,
    #[serde(rename = "7")]
    integer_digits: u64,
    #[serde(rename = "8")]
    integer_nonzero: bool,
    #[serde(rename = "9")]
    fraction_leading_zeros: u64,
    #[serde(rename = "10")]
    significand: u64,
    #[serde(rename = "11")]
    scale: i32,
    #[serde(rename = "12")]
    long_integer: bool,
    #[serde(rename = "13")]
    decimal_overflow: bool,
    #[serde(rename = "14")]
    float: bool,
    #[serde(rename = "15")]
    exponent_positive: bool,
    #[serde(rename = "16")]
    exponent: i32,
    #[serde(rename = "17")]
    exponent_overflow: bool,
    #[serde(rename = "18")]
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
            policy: 1,
            start,
            end,
            at: start,
            state: None,
            positive: true,
            prefix: String::with_capacity(SIGNIFICANT_DIGITS),
            sticky: false,
            integer_digits: 0,
            integer_nonzero: false,
            fraction_leading_zeros: 0,
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
        if self.policy != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("numeric continuation requires {PRODUCER_POLICY}"),
            ));
        }
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
        if self.prefix.len() > SIGNIFICANT_DIGITS
            || self.prefix.bytes().any(|byte| !byte.is_ascii_digit())
            || self.prefix.starts_with('0')
        {
            return Err(invalid());
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
                if matches!(
                    old,
                    None | Some(Number::Sign | Number::Integer | Number::Dot | Number::Fraction)
                ) {
                    if matches!(old, None | Some(Number::Sign | Number::Integer)) {
                        self.integer_digits =
                            self.integer_digits.checked_add(1).ok_or_else(invalid)?;
                        self.integer_nonzero |= digit != 0;
                    } else if self.prefix.is_empty() && digit == 0 {
                        self.fraction_leading_zeros = self
                            .fraction_leading_zeros
                            .checked_add(1)
                            .ok_or_else(invalid)?;
                    }
                    if !self.prefix.is_empty() || digit != 0 {
                        if self.prefix.len() < SIGNIFICANT_DIGITS {
                            self.prefix.push(char::from(byte));
                        } else {
                            // Native lexical trims trailing zeros only from
                            // the fraction. Integer digits left after its 768
                            // digit prefix always cause the appended sticky 1.
                            self.sticky |= digit != 0 || matches!(old, Some(Number::Integer));
                        }
                    }
                }
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
                if self.exponent_positive && !self.prefix.is_empty() {
                    self.result = Some(Qualification::Invalid);
                    return;
                }
                format!("{sign}0e0")
            } else if self.long_integer || self.decimal_overflow {
                // Mirror lexical::scientific_exponent's saturation order, not
                // a mathematical cancellation across comically large counts.
                let exponent = if self.exponent_positive {
                    self.exponent
                } else {
                    -self.exponent
                };
                let scientific = if self.integer_nonzero {
                    exponent
                        .saturating_add(i32::try_from(self.integer_digits - 1).unwrap_or(i32::MAX))
                } else {
                    exponent
                        .saturating_sub(
                            i32::try_from(self.fraction_leading_zeros).unwrap_or(i32::MAX),
                        )
                        .saturating_sub(1)
                };
                let mut digits = self.prefix.clone();
                if self.sticky {
                    digits.push('1');
                }
                // A decimal marker preserves the native F64 variant, including
                // long all-integer spellings and their negative counterpart.
                let (first, rest) = digits.split_at(1);
                let rest = if rest.is_empty() { "0" } else { rest };
                format!("{sign}{first}.{rest}e{scientific}")
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
            assert!(std::mem::size_of::<Cursor>() + cursor.prefix.capacity() < 2048);
            assert!(cursor.prefix.len() <= SIGNIFICANT_DIGITS);
            assert_eq!(step.complete, cursor.result.is_some());
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
                assert!(std::mem::size_of::<Cursor>() + cursor.prefix.capacity() < 2048);
            }
        }
    }
    // Exact dyadic spelling constructed using decimal integer arithmetic, not
    // either number parser. Midpoint tests still compare against the complete
    // resolved native serde producer, independently of this continuation.
    fn midpoint(mut mantissa: u64, exponent: i32) -> String {
        let mut digits = Vec::new();
        while mantissa != 0 {
            digits.push((mantissa % 10) as u8);
            mantissa /= 10;
        }
        for _ in 0..exponent.unsigned_abs() {
            let mut carry = 0;
            for digit in &mut digits {
                let value = *digit * if exponent < 0 { 5 } else { 2 } + carry;
                *digit = value % 10;
                carry = value / 10;
            }
            if carry != 0 {
                digits.push(carry);
            }
        }
        let integer: String = digits
            .iter()
            .rev()
            .map(|digit| char::from(b'0' + digit))
            .collect();
        if exponent >= 0 {
            return format!("{integer}.0");
        }
        let places = exponent.unsigned_abs() as usize;
        if integer.len() <= places {
            format!("0.{}{integer}", "0".repeat(places - integer.len()))
        } else {
            let split = integer.len() - places;
            format!("{}.{}", &integer[..split], &integer[split..])
        }
    }
    #[test]
    fn bounded_roundtrip_preserves_midpoints_and_arbitrarily_late_sticky_digits() {
        // Tie at 1, zero/min-subnormal, max-subnormal/min-normal, a
        // normal/subnormal midpoint, and the finite/infinite boundary.
        for (mantissa, exponent) in [
            ((1u64 << 53) + 1, -53),
            (1, -1075),
            ((1u64 << 53) - 1, -1075),
            ((1u64 << 53) + 1, -1075),
            ((1u64 << 54) - 1, 970),
        ] {
            let tie = midpoint(mantissa, exponent);
            assert_native(&tie);
            assert_native(&format!("-{tie}"));
            for zeros in [0, 768, 8192] {
                let above = format!("{tie}{}1", "0".repeat(zeros));
                assert_native(&above);
                assert_native(&format!("-{above}"));
            }
        }
        let tie = midpoint((1u64 << 53) + 1, -53);
        assert_eq!(
            serde_json::from_str::<serde_json::Number>(&tie)
                .unwrap()
                .as_f64()
                .unwrap()
                .to_bits(),
            1f64.to_bits()
        );
        let above = format!("{tie}{}1", "0".repeat(8192));
        assert_eq!(
            serde_json::from_str::<serde_json::Number>(&above)
                .unwrap()
                .as_f64()
                .unwrap()
                .to_bits(),
            1f64.to_bits() + 1
        );
    }
    #[test]
    fn long_mantissas_compensated_exponents_and_zero_keep_native_results() {
        for size in [0, 128, 1024, 8192, 100_000] {
            for text in [
                format!("123{}e-{size}", "0".repeat(size)),
                format!("0.{}123e{size}", "0".repeat(size)),
                format!("0.{}e2147483648", "0".repeat(size + 1)),
                format!("-0.{}e2147483648", "0".repeat(size + 1)),
                format!("1.{}e-{}", "7".repeat(size + 1), "9".repeat(128)),
            ] {
                let native = serde_json::from_str::<serde_json::Number>(&text);
                for page in [4096, JSON_STRING_CHUNK_DECODED_MAX] {
                    match (native.as_ref(), qualify(&text, page)) {
                        (Ok(expected), Qualification::Finite(actual)) => {
                            assert_eq!(actual, NativeNumber::from_native(expected.clone()))
                        }
                        (Err(_), Qualification::Invalid) => {}
                        _ => panic!("native long number result differs at size {size}"),
                    }
                }
            }
        }
    }
    #[test]
    fn native_integer_suffix_sticky_differs_from_fractional_zero_trimming() {
        for (mantissa, exponent) in [
            ((1u64 << 53) + 1, -53),
            (1, -1075),
            ((1u64 << 53) - 1, -1075),
            ((1u64 << 53) + 1, -1075),
            ((1u64 << 54) - 1, 970),
        ] {
            let tie = midpoint(mantissa, exponent);
            let (integer, fraction) = tie.split_once('.').unwrap();
            let digits = format!("{integer}{fraction}");
            let digits = digits.trim_start_matches('0');
            for width in [767, 768, 769, 8192] {
                // Widths below the exact midpoint's significant length use
                // its downward decimal prefix; the native complete parser
                // still independently owns the expected result.
                let kept = digits.len().min(width);
                let zeros = width.saturating_sub(digits.len());
                let scale =
                    digits.len() as i64 - kept as i64 - fraction.len() as i64 - zeros as i64;
                let integer_form = format!("{}{}e{scale}", &digits[..kept], "0".repeat(zeros));
                assert_native(&integer_form);
                assert_native(&format!("-{integer_form}"));
                let decimal_form = format!("{tie}{}", "0".repeat(width));
                assert_native(&decimal_form);
                assert_native(&format!("-{decimal_form}"));
            }
        }
        // Preserve the primary owner's surprising integer-channel behavior,
        // rather than replacing it with an independently rounded decimal.
        let tie = midpoint((1u64 << 53) + 1, -53);
        let digits = tie.replace('.', "");
        let zeros = "0".repeat(8192);
        let scale = digits.len() + zeros.len() - 1;
        let integer_form = format!("{digits}{zeros}e-{scale}");
        assert_eq!(
            serde_json::from_str::<serde_json::Number>(&integer_form)
                .unwrap()
                .as_f64()
                .unwrap()
                .to_bits(),
            1f64.to_bits() + 1
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Number>(&format!("{tie}{zeros}"))
                .unwrap()
                .as_f64()
                .unwrap()
                .to_bits(),
            1f64.to_bits()
        );
    }
    #[test]
    fn restart_rejects_a_different_numeric_producer_policy_and_nonfinite_receipts() {
        let mut cursor = Cursor::new(0, 2).unwrap();
        assert!(!cursor.step(b"1", 1).unwrap().complete);
        cursor.policy = 0;
        let mut resumed: Cursor =
            serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap();
        assert!(resumed.step(b"2", 1).is_err());
        let old = br#"{"start":0,"end":2,"at":1,"state":"Integer","positive":true,"significand":1,"scale":0,"long_integer":false,"decimal_overflow":false,"float":false,"exponent_positive":true,"exponent":0,"exponent_overflow":false,"result":null}"#;
        assert!(serde_json::from_slice::<Cursor>(old).is_err());
        let mut current = Cursor::new(0, 2).unwrap();
        assert!(
            current
                .step(b"12", JSON_STRING_CHUNK_DECODED_MAX + 1)
                .is_err()
        );
        assert_eq!(current.at, 0);
        for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            let receipt = serde_json::to_vec(&NativeNumber::Float(value.to_bits())).unwrap();
            assert!(serde_json::from_slice::<NativeNumber>(&receipt).is_err());
        }
    }
}
