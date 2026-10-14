//! The native producer's deliberately limited JavaScript literal decoder.
//! Both complete code-mode parsing and qualified native views drive this owner.
//! Input/output are bounded request-local chunks; a durable cursor saves flags.
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Quote {
    Single,
    Double,
    Template,
}
impl Quote {
    pub(crate) fn of(value: char) -> Option<Self> {
        match value {
            '\'' => Some(Self::Single),
            '"' => Some(Self::Double),
            '`' => Some(Self::Template),
            _ => None,
        }
    }
    pub(crate) fn character(self) -> char {
        match self {
            Self::Single => '\'',
            Self::Double => '"',
            Self::Template => '`',
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    Scalar {
        value: Option<char>,
        consumed: usize,
    },
    Close {
        consumed: usize,
    },
    Template {
        consumed: usize,
    },
}

/// One producer literal unit. At most twelve input characters are inspected.
/// An incomplete prefix is replayed, never persisted as source text.
pub(crate) fn unit(input: &str, quote: Quote, final_input: bool) -> Option<Unit> {
    let mut chars = input.char_indices();
    let (_, first) = chars.next()?;
    if first == quote.character() {
        return Some(Unit::Close {
            consumed: first.len_utf8(),
        });
    }
    if first == '$' && quote == Quote::Template {
        match chars.next() {
            Some((_, '{')) => return Some(Unit::Template { consumed: 1 }),
            None if !final_input => return None,
            _ => {}
        }
    }
    if first != '\\' {
        return Some(Unit::Scalar {
            value: Some(first),
            consumed: first.len_utf8(),
        });
    }
    let (_, escaped) = chars.next()?;
    let prefix = first.len_utf8() + escaped.len_utf8();
    let rest = &input[prefix..];
    let hex = |digits: &str| {
        (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| u32::from_str_radix(digits, 16).ok())
            .flatten()
    };
    let (value, extra) = match escaped {
        'n' => (Some('\n'), 0),
        't' => (Some('\t'), 0),
        'r' => (Some('\r'), 0),
        'b' => (Some('\u{8}'), 0),
        'f' => (Some('\u{c}'), 0),
        'v' => (Some('\u{b}'), 0),
        '0' => (Some('\0'), 0),
        '\n' => (None, 0),
        'x' => {
            if rest.len() < 2 && !final_input {
                return None;
            }
            match rest.get(..2).and_then(hex).and_then(char::from_u32) {
                Some(value) => (Some(value), 2),
                None => (Some('x'), 0),
            }
        }
        'u' if rest.starts_with('{') => {
            // A non-hex digit already makes this producer escape invalid,
            // regardless of a later brace. Do not needlessly buffer Unicode
            // lookahead from the rest of an invalid escape.
            if rest[1..]
                .chars()
                .take_while(|value| *value != '}')
                .take(7)
                .any(|value| !value.is_ascii_hexdigit())
            {
                return Some(Unit::Scalar {
                    value: Some('u'),
                    consumed: prefix,
                });
            }
            let close = rest
                .char_indices()
                .take(8)
                .find(|(_, c)| *c == '}')
                .map(|(at, _)| at);
            if close.is_none() && rest.chars().take(8).count() < 8 && !final_input {
                return None;
            }
            match close.and_then(|at| hex(&rest[1..at]).map(|code| (at, code))) {
                Some((at, code)) => (Some(char::from_u32(code).unwrap_or('\u{fffd}')), at + 1),
                None => (Some('u'), 0),
            }
        }
        'u' => {
            if rest.len() < 4 && !final_input {
                return None;
            }
            match rest.get(..4).and_then(hex) {
                Some(code) => (Some(char::from_u32(code).unwrap_or('\u{fffd}')), 4),
                None => (Some('u'), 0),
            }
        }
        other => (Some(other), 0),
    };
    Some(Unit::Scalar {
        value,
        consumed: prefix + extra,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Cursor {
    pub(crate) quote: Quote,
    pub(crate) plain: bool,
    pub(crate) complete: bool,
}
pub(crate) struct Step {
    pub(crate) consumed: usize,
    pub(crate) output: String,
}
impl Cursor {
    pub(crate) fn new(quote: Quote) -> Self {
        Self {
            quote,
            plain: true,
            complete: false,
        }
    }
    pub(crate) fn step(
        &mut self,
        input: &str,
        final_input: bool,
        maximum: usize,
    ) -> io::Result<Step> {
        if self.complete || maximum < 4 || maximum > super::JSON_STRING_CHUNK_DECODED_MAX {
            return Err(io::Error::other(
                "invalid native JavaScript literal continuation",
            ));
        }
        let mut consumed = 0;
        let mut output = String::with_capacity(input.len().min(maximum));
        while consumed < input.len() {
            let Some(next) = unit(&input[consumed..], self.quote, final_input) else {
                break;
            };
            match next {
                Unit::Close { consumed: width } => {
                    consumed += width;
                    self.complete = true;
                    break;
                }
                Unit::Template { consumed: width } => {
                    if output.len() == maximum {
                        break;
                    }
                    self.plain = false;
                    output.push('$');
                    consumed += width;
                }
                Unit::Scalar {
                    value,
                    consumed: width,
                } => {
                    if value.is_some_and(|value| output.len() + value.len_utf8() > maximum) {
                        break;
                    }
                    if let Some(value) = value {
                        output.push(value);
                    }
                    consumed += width;
                }
            }
        }
        Ok(Step { consumed, output })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn undersized_scalar_output_budgets_refuse_instead_of_deferring_forever() {
        for maximum in 0..4 {
            let mut cursor = Cursor::new(Quote::Single);
            assert!(cursor.step("é😀'", true, maximum).is_err());
        }
        let mut cursor = Cursor::new(Quote::Single);
        let first = cursor.step("é😀'", true, 4).unwrap();
        assert_eq!(first.output, "é");
        assert_eq!(first.consumed, 2);
        let second = cursor.step("😀'", true, 4).unwrap();
        assert_eq!(second.output, "😀");
        assert!(cursor.complete);
    }
    #[test]
    fn literal_units_preserve_native_escape_and_template_rules_across_splits() {
        for (input, expected, plain) in [
            ("a\\x62\\u0063\\u{1f600}'", "abc😀", true),
            ("\\uD800\\xZZ\\u{}'", "�xZZu{}", true),
            ("a\\\nb'", "ab", true),
            ("a${x}`", "a${x}", false),
        ] {
            let quote = if input.ends_with('`') {
                Quote::Template
            } else {
                Quote::Single
            };
            for split in input
                .char_indices()
                .map(|(at, _)| at)
                .chain(std::iter::once(input.len()))
            {
                let mut cursor = Cursor::new(quote);
                let first = cursor
                    .step(&input[..split], split == input.len(), 1024)
                    .unwrap();
                let state = serde_json::to_vec(&cursor).unwrap();
                let mut cursor: Cursor = serde_json::from_slice(&state).unwrap();
                let mut output = first.output;
                if !cursor.complete {
                    output.push_str(
                        &cursor
                            .step(&input[first.consumed..], true, 1024)
                            .unwrap()
                            .output,
                    );
                }
                assert!(cursor.complete);
                assert_eq!(cursor.plain, plain);
                assert_eq!(output, expected);
            }
        }
    }
}
