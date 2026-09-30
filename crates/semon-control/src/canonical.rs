//! The pinned canonical encoding of a payload: RFC 8785, the JSON
//! Canonicalization Scheme (JCS), restricted to the I-JSON safe number range.
//!
//! JCS sorts object keys by their UTF-16 code units, writes strings with the
//! minimal escapes, and writes every number the way ECMAScript's
//! `Number.prototype.toString` does, which goes through an IEEE 754 double.
//! An integer outside ±(2^53 − 1) can't pass through a double unchanged, so
//! two different payloads could encode, and hash, the same. Such a payload is
//! refused here instead ([`CanonicalError::UnsafeNumber`]), and a request
//! carrying one is shown read-only.
//!
//! The encoder is `serde_json_canonicalizer`, already used by semon-store;
//! the tests below pin it to the RFC's own vectors.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// The largest integer a double holds exactly, and so the edge of the range
/// this encoding accepts: 2^53 − 1.
pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Why a value has no canonical encoding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    /// A number outside ±(2^53 − 1), in any spelling.
    #[error("the number {0} is outside the I-JSON safe range, so its canonical form is ambiguous")]
    UnsafeNumber(String),
    /// The encoder refused the value.
    #[error("the value has no canonical encoding: {0}")]
    Encoding(String),
}

/// The canonical bytes of `value`.
pub fn canonical_bytes(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    check_numbers(value)?;
    jcs(value)
}

/// The lowercase hex SHA-256 of `value`'s canonical bytes.
pub fn sha256_hex(value: &Value) -> Result<String, CanonicalError> {
    canonical_bytes(value).map(|bytes| sha256_of(&bytes))
}

pub(crate) fn sha256_of(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// RFC 8785 alone, without the number-range check.
fn jcs(value: &Value) -> Result<Vec<u8>, CanonicalError> {
    serde_json_canonicalizer::to_vec(value)
        .map_err(|error| CanonicalError::Encoding(error.to_string()))
}

fn check_numbers(value: &Value) -> Result<(), CanonicalError> {
    match value {
        Value::Number(number) => {
            let safe = if let Some(integer) = number.as_u64() {
                integer <= MAX_SAFE_INTEGER
            } else if let Some(integer) = number.as_i64() {
                integer.unsigned_abs() <= MAX_SAFE_INTEGER
            } else {
                // Spelled with a fraction or an exponent. Every double past
                // 2^53 is already a whole number, so the magnitude check is
                // the whole test.
                number
                    .as_f64()
                    .is_some_and(|float| float.abs() <= MAX_SAFE_INTEGER as f64)
            };
            if safe {
                Ok(())
            } else {
                Err(CanonicalError::UnsafeNumber(number.to_string()))
            }
        }
        Value::Array(values) => values.iter().try_for_each(check_numbers),
        Value::Object(object) => object.values().try_for_each(check_numbers),
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{CanonicalError, canonical_bytes, jcs, sha256_hex};

    fn text(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("canonical JSON is UTF-8")
    }

    /// RFC 8785 appendix B: an IEEE 754 double and its canonical spelling.
    /// NaN and the infinities have no JSON form and are left out.
    const APPENDIX_B: &[(u64, &str)] = &[
        (0x0000_0000_0000_0000, "0"),
        (0x8000_0000_0000_0000, "0"),
        (0x0000_0000_0000_0001, "5e-324"),
        (0x8000_0000_0000_0001, "-5e-324"),
        (0x7fef_ffff_ffff_ffff, "1.7976931348623157e+308"),
        (0xffef_ffff_ffff_ffff, "-1.7976931348623157e+308"),
        (0x4340_0000_0000_0000, "9007199254740992"),
        (0xc340_0000_0000_0000, "-9007199254740992"),
        (0x4430_0000_0000_0000, "295147905179352830000"),
        (0x44b5_2d02_c7e1_4af5, "9.999999999999997e+22"),
        (0x44b5_2d02_c7e1_4af6, "1e+23"),
        (0x44b5_2d02_c7e1_4af7, "1.0000000000000001e+23"),
        (0x444b_1ae4_d6e2_ef4e, "999999999999999700000"),
        (0x444b_1ae4_d6e2_ef4f, "999999999999999900000"),
        (0x444b_1ae4_d6e2_ef50, "1e+21"),
        (0x3eb0_c6f7_a0b5_ed8c, "9.999999999999997e-7"),
        (0x3eb0_c6f7_a0b5_ed8d, "0.000001"),
        (0x41b3_de43_5555_5553, "333333333.3333332"),
        (0x41b3_de43_5555_5554, "333333333.33333325"),
        (0x41b3_de43_5555_5555, "333333333.3333333"),
        (0x41b3_de43_5555_5556, "333333333.3333334"),
        (0x41b3_de43_5555_5557, "333333333.33333343"),
        (0xbecb_f647_612f_3696, "-0.0000033333333333333333"),
        (0x4314_3ff3_c1cb_0959, "1424953923781206.2"),
    ];

    #[test]
    fn rfc_8785_appendix_b_numbers() {
        for (bits, expected) in APPENDIX_B {
            let value = Value::from(f64::from_bits(*bits));
            assert_eq!(text(jcs(&value).unwrap()), *expected, "bits {bits:#018x}");
        }
    }

    #[test]
    fn rfc_8785_section_3_2_2_example() {
        let input: Value = serde_json::from_str(
            r#"{
              "numbers": [333333333.33333329, 1E30, 4.50,
                          2e-3, 0.000000000000000000000000001],
              "string": "\u20ac$\u000F\u000aA'\u0042\u0022\u005c\\\"\/",
              "literals": [null, true, false]
            }"#,
        )
        .unwrap();
        assert_eq!(
            text(jcs(&input).unwrap()),
            "{\"literals\":[null,true,false],\"numbers\":[333333333.3333333,1e+30,4.5,0.002,1e-27],\"string\":\"\u{20ac}$\\u000f\\nA'B\\\"\\\\\\\\\\\"/\"}"
        );
    }

    #[test]
    fn rfc_8785_section_3_2_3_sorts_keys_by_utf16_code_units() {
        let input: Value = serde_json::from_str(
            r#"{
              "\u20ac": "Euro Sign",
              "\r": "Carriage Return",
              "\ufb33": "Hebrew Letter Dalet With Dagesh",
              "1": "One",
              "\ud83d\ude00": "Emoji: Grinning Face",
              "\u0080": "Control",
              "\u00f6": "Latin Small Letter O With Diaeresis"
            }"#,
        )
        .unwrap();
        // U+1F600 sorts before U+FB33 because its first UTF-16 unit is
        // 0xD83D; in code-point or UTF-8 order it would come last.
        assert_eq!(
            text(canonical_bytes(&input).unwrap()),
            "{\"\\r\":\"Carriage Return\",\"1\":\"One\",\"\u{80}\":\"Control\",\"\u{f6}\":\"Latin Small Letter O With Diaeresis\",\"\u{20ac}\":\"Euro Sign\",\"\u{1f600}\":\"Emoji: Grinning Face\",\"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}"
        );
    }

    #[test]
    fn the_hash_ignores_key_order_whitespace_and_number_spelling() {
        let spellings = [
            r#"{"command":"cargo test","timeout":120000,"flags":[1,2.5],"cwd":{"a":true,"b":null}}"#,
            r#"{ "cwd": {"b": null, "a": true}, "flags": [1.0, 25e-1], "timeout": 1.2e5, "command": "cargo test" }"#,
            r#"{"timeout":120000.000,"flags":[10e-1,2.50],"command":"cargo test","cwd":{"a":true,"b":null}}"#,
        ];
        let hashes: Vec<String> = spellings
            .iter()
            .map(|spelling| sha256_hex(&serde_json::from_str(spelling).unwrap()).unwrap())
            .collect();
        assert!(hashes.iter().all(|hash| *hash == hashes[0]), "{hashes:?}");
        assert_eq!(hashes[0].len(), 64);
        assert!(
            hashes[0]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        );

        let different = sha256_hex(&json!({"command": "cargo test ", "timeout": 120000, "flags": [1, 2.5], "cwd": {"a": true, "b": null}})).unwrap();
        assert_ne!(different, hashes[0]);
    }

    #[test]
    fn numbers_outside_the_safe_range_are_refused_in_every_spelling() {
        for spelling in [
            "9007199254740992",
            "-9007199254740992",
            "9007199254740993",
            "18446744073709551615",
            "9007199254740992.0",
            "1e30",
            "-1e300",
        ] {
            let value: Value = serde_json::from_str(&format!("{{\"n\":[{spelling}]}}")).unwrap();
            assert!(
                matches!(
                    canonical_bytes(&value),
                    Err(CanonicalError::UnsafeNumber(_))
                ),
                "{spelling} should be refused"
            );
        }
        for spelling in [
            "9007199254740991",
            "-9007199254740991",
            "9007199254740991.0",
            "0.5",
            "-1e-300",
        ] {
            let value: Value = serde_json::from_str(&format!("{{\"n\":{spelling}}}")).unwrap();
            assert!(
                canonical_bytes(&value).is_ok(),
                "{spelling} should be accepted"
            );
        }
    }
}
