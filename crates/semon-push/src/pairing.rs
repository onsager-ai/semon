//! Checked one-time pairing codes.
use crate::{credentials::Secret, enrollment::Error};
use zeroize::Zeroizing;
type Result<T> = std::result::Result<T, Error>;
pub const WINDOW: u64 = 600;
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const WEIGHTS: [u8; 9] = [1, 3, 5, 7, 9, 11, 13, 15, 17];

fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>> {
    let mut bytes = Zeroizing::new([0; N]);
    getrandom::fill(bytes.as_mut()).map_err(|_| Error::Random)?;
    Ok(bytes)
}
fn check(data: &[u8]) -> u8 {
    data.iter()
        .zip(WEIGHTS)
        .fold(0u16, |sum, (v, w)| sum + u16::from(*v) * u16::from(w)) as u8
        & 31
}
/// Pure local input validation. Only one optional middle hyphen is accepted.
/// Aliases are normalized before checking; U and non-ASCII input are refused.
pub fn parse_code(input: &str) -> Result<Secret> {
    let input = input.trim();
    if input.len() != 10 && !(input.len() == 11 && input.as_bytes()[5] == b'-') {
        return Err(Error::Invalid);
    }
    let mut values = Zeroizing::new(Vec::with_capacity(10));
    for (i, byte) in input.bytes().enumerate() {
        if input.len() == 11 && i == 5 {
            continue;
        }
        let byte = match byte.to_ascii_uppercase() {
            b'O' => b'0',
            b'I' | b'L' => b'1',
            b => b,
        };
        values.push(
            ALPHABET
                .iter()
                .position(|b| *b == byte)
                .ok_or(Error::Invalid)? as u8,
        );
    }
    if check(&values[..9]) != values[9] {
        return Err(Error::Invalid);
    }
    Ok(Secret::new(
        values
            .iter()
            .map(|v| ALPHABET[*v as usize] as char)
            .collect(),
    ))
}
pub(crate) fn generate_code() -> Result<Secret> {
    // Masking uniform independent bytes yields unbiased five-bit symbols.
    let mut values = random::<9>()?;
    for v in values.iter_mut() {
        *v &= 31;
    }
    let mut code: String = values
        .iter()
        .map(|v| ALPHABET[*v as usize] as char)
        .collect();
    code.push(ALPHABET[check(values.as_ref()) as usize] as char);
    Ok(Secret::new(code))
}
/// Explicit local-only display operation; the returned wrapper still redacts.
pub fn display_code(code: &Secret) -> Secret {
    Secret::new(format!("{}-{}", &code.expose()[..5], &code.expose()[5..]))
}
