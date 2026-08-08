use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{StoreError, TraceId};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MIN_SAFE_INTEGER: i64 = -9_007_199_254_740_991;

pub(crate) fn canonicalize(value: &Value) -> Result<Vec<u8>, StoreError> {
    validate_number_domain(value)?;
    serde_json_canonicalizer::to_vec(value).map_err(StoreError::Canonicalization)
}

pub(crate) fn content_hash(canonical_json: &[u8]) -> TraceId {
    let digest = Sha256::digest(canonical_json);
    TraceId::from_digest(hex::encode(digest))
}

fn validate_number_domain(value: &Value) -> Result<(), StoreError> {
    match value {
        Value::Number(number) if number.is_i64() => {
            let integer = number
                .as_i64()
                .expect("an i64-classified JSON number must convert to i64");
            if !(MIN_SAFE_INTEGER..=MAX_SAFE_INTEGER as i64).contains(&integer) {
                return Err(StoreError::NumberOutsideCanonicalDomain(number.to_string()));
            }
        }
        Value::Number(number) if number.is_u64() => {
            let integer = number
                .as_u64()
                .expect("a u64-classified JSON number must convert to u64");
            if integer > MAX_SAFE_INTEGER {
                return Err(StoreError::NumberOutsideCanonicalDomain(number.to_string()));
            }
        }
        // serde_json classifies a number as f64 whenever the source spelling
        // carries a decimal point or exponent (e.g. `9007199254740993.0`),
        // even when the value it denotes is a whole number. IEEE-754 binary64
        // spacing means every finite f64 whose magnitude exceeds
        // MAX_SAFE_INTEGER is *already* integer-valued: from 2^52 upward the
        // gap between adjacent representable doubles is at least 1, so there
        // is no fractional double out here to worry about separately.
        // `1e300` and any other magnitude in this range (including the
        // `>= 1e21` band where JCS switches to exponential notation) is
        // therefore rejected by the same magnitude check, not exempted for
        // "not really being an integer literal". NaN and Infinity are not
        // reachable here: standard JSON text has no grammar for them, and
        // serde_json::Number::from_f64 refuses to construct a Number from a
        // non-finite f64, so no public API can hand this function one.
        Value::Number(number) if number.is_f64() => {
            let float = number
                .as_f64()
                .expect("an f64-classified JSON number must convert to f64");
            if float.abs() > MAX_SAFE_INTEGER as f64 {
                return Err(StoreError::NumberOutsideCanonicalDomain(number.to_string()));
            }
        }
        Value::Array(values) => {
            for value in values {
                validate_number_domain(value)?;
            }
        }
        Value::Object(object) => {
            for value in object.values() {
                validate_number_domain(value)?;
            }
        }
        // The `Value::Number(_)` fallback is defensive only: serde_json's
        // Number is documented to be classified as exactly one of i64, u64,
        // or f64, so every real Number is handled by a guarded arm above.
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{canonicalize, content_hash};
    use crate::StoreError;

    #[test]
    fn jcs_rules_are_visible_in_canonical_bytes() {
        let value = json!({
            "z": null,
            "escaped": "line\né",
            "number": 1.0,
            "a": {"second": true, "first": false}
        });

        let bytes = canonicalize(&value).unwrap();

        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "{\"a\":{\"first\":false,\"second\":true},\"escaped\":\"line\\né\",\"number\":1,\"z\":null}"
        );
    }

    #[test]
    fn rejects_integer_values_that_jcs_would_round() {
        let value = json!({"unsafe": 9_007_199_254_740_992_u64});

        assert!(matches!(
            canonicalize(&value),
            Err(StoreError::NumberOutsideCanonicalDomain(_))
        ));
    }

    #[test]
    fn rejects_unsafe_integers_spelled_as_floats_the_same_as_integer_literals() {
        // These are the exact repro values from the review finding: the
        // integer spelling was already rejected, but the float spelling
        // (which serde_json classifies as f64, not i64/u64) fell through the
        // catch-all arm unchecked and was silently accepted.
        let at_boundary = json!({"n": 9_007_199_254_740_992_u64});
        let one_past_boundary = json!({"n": 9_007_199_254_740_993_u64});
        let at_boundary_as_float: Value =
            serde_json::from_str(r#"{"n":9007199254740992.0}"#).expect("literal parses as JSON");
        let one_past_boundary_as_float: Value =
            serde_json::from_str(r#"{"n":9007199254740993.0}"#).expect("literal parses as JSON");

        for value in [
            &at_boundary,
            &one_past_boundary,
            &at_boundary_as_float,
            &one_past_boundary_as_float,
        ] {
            assert!(
                matches!(
                    canonicalize(value),
                    Err(StoreError::NumberOutsideCanonicalDomain(_))
                ),
                "expected {value:?} to be rejected"
            );
        }
    }

    #[test]
    fn accepts_the_max_safe_integer_in_both_integer_and_float_spellings_with_matching_ids() {
        let integer_spelling = json!({"n": 9_007_199_254_740_991_u64});
        let float_spelling: Value =
            serde_json::from_str(r#"{"n":9007199254740991.0}"#).expect("literal parses as JSON");

        let integer_bytes = canonicalize(&integer_spelling).expect("boundary value is accepted");
        let float_bytes = canonicalize(&float_spelling).expect("boundary value is accepted");

        assert_eq!(
            content_hash(&integer_bytes).as_str(),
            content_hash(&float_bytes).as_str()
        );
    }

    #[test]
    fn rejects_large_non_integer_shaped_float_literals_because_they_are_integer_valued_anyway() {
        // `1e300` reads as a "non-integer-valued float beyond 2^53" at a
        // glance, but IEEE-754 binary64 spacing makes every finite double
        // past that magnitude integer-valued: there is no fractional part
        // left to represent. So this is rejected by the same magnitude
        // check as any other number out here, including values (like this
        // one) that JCS would print in exponential notation.
        let value = json!({"n": 1e300});

        assert!(matches!(
            canonicalize(&value),
            Err(StoreError::NumberOutsideCanonicalDomain(_))
        ));
    }

    #[test]
    fn absent_and_explicit_null_are_distinct() {
        assert_ne!(
            canonicalize(&json!({})).unwrap(),
            canonicalize(&json!({"result": null})).unwrap()
        );
    }
}
