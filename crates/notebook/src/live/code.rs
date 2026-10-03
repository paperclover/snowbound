//! Live Share's codes in Crockford's base32 (0-9 and A-Z without I, L, O and U), shown
//! `7KQ-4MZ-9XR`: two symbols naming the code's room, its number on a relay; six of secret
//! (30 bits), which SPAKE2 meets through; and a check symbol, so a mistyped code is refused
//! here before it spends one of the relay's few tries. Reading ignores case, hyphens and
//! spaces, and takes I and L for 1 and O for 0, as Crockford's decoding does.
//!
//! The check is the symbols' values weighted 1 to 8, summed modulo 31, the prime under 32, so
//! it stays one of the code's own symbols: it catches any symbol mistyped and any two
//! neighbours swapped, but for 0 and Z, whose values differ by 31. Crockford's own check
//! symbol, modulo 37, adds `*~$=U`, which read badly aloud.

use std::io;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// The symbols naming a code's room, and how many rooms they name.
const NAMEPLATE: usize = 2;
pub const NAMEPLATES: u32 = 1 << (5 * NAMEPLATE);
/// The symbols of a code's secret.
pub const SECRET: usize = 6;

/// A new secret, `SECRET` random symbols.
pub fn secret() -> io::Result<String> {
    let mut bytes = [0; 4];
    getrandom::fill(&mut bytes).map_err(|_| io::Error::other("System random source failed"))?;
    let bits = u32::from_le_bytes(bytes);
    Ok((0..SECRET)
        .map(|at| symbol((bits >> (5 * at)) as u8))
        .collect())
}

fn symbol(value: u8) -> char {
    char::from(ALPHABET[usize::from(value & 31)])
}

/// A symbol's value as typed, reading I and L as 1 and O as 0.
fn value(typed: char) -> Option<u8> {
    let typed = match typed.to_ascii_uppercase() {
        'I' | 'L' => '1',
        'O' => '0',
        typed => typed,
    };
    ALPHABET
        .iter()
        .position(|symbol| char::from(*symbol) == typed)
        .map(|at| at as u8)
}

fn check(values: &[u8]) -> u8 {
    let sum: u32 = (values.iter().enumerate())
        .map(|(at, value)| (at as u32 + 1) * u32::from(*value))
        .sum();
    (sum % 31) as u8
}

/// The code for room `nameplate` and `secret`, as shown: `7KQ-4MZ-9XR`. A secret that isn't
/// `SECRET` symbols has no code.
pub fn format(nameplate: u32, secret: &str) -> Option<String> {
    let secret: Vec<u8> = secret.chars().map(value).collect::<Option<_>>()?;
    if nameplate >= NAMEPLATES || secret.len() != SECRET {
        return None;
    }
    let mut values = vec![(nameplate >> 5) as u8, (nameplate & 31) as u8];
    values.extend(secret);
    values.push(check(&values));
    let symbols: Vec<char> = values.into_iter().map(symbol).collect();
    Some(
        symbols
            .chunks(3)
            .map(|group| group.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("-"),
    )
}

/// A code as typed: its room's number and its secret, where it is a code whose check holds.
pub fn parse(typed: &str) -> Option<(u32, String)> {
    let values: Vec<u8> = typed
        .chars()
        .filter(|c| *c != '-' && !c.is_whitespace())
        .map(value)
        .collect::<Option<_>>()?;
    let (check_value, values) = values.split_last()?;
    if values.len() != NAMEPLATE + SECRET || check(values) != *check_value {
        return None;
    }
    let nameplate = (u32::from(values[0]) << 5) | u32::from(values[1]);
    let secret = values[NAMEPLATE..].iter().copied().map(symbol).collect();
    Some((nameplate, secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_read_back_however_they_are_typed() {
        let code = format(412, "4MZ9XR").unwrap();
        assert_eq!(code.len(), 11);
        assert_eq!(parse(&code), Some((412, "4MZ9XR".into())));
        let typed = code
            .to_lowercase()
            .replace('-', " ")
            .replace('1', "l")
            .replace('0', "o");
        assert_eq!(parse(&typed), Some((412, "4MZ9XR".into())));
        assert_eq!(parse(&format!("  {code} ")), Some((412, "4MZ9XR".into())));
        assert!(format(NAMEPLATES, "4MZ9XR").is_none() && format(1, "4MZ9X").is_none());
        assert!(parse("412-violet-otter").is_none() && parse("").is_none());
        for _ in 0..100 {
            let secret = secret().unwrap();
            assert_eq!(secret.len(), SECRET);
            assert_eq!(parse(&format(7, &secret).unwrap()), Some((7, secret)));
        }
    }

    /// The check refuses any one symbol mistyped, and any two neighbours swapped, but for 0
    /// and Z.
    #[test]
    fn the_check_catches_a_symbol_mistyped_or_two_swapped() {
        let code: Vec<char> = format(999, "Q4MZ9X")
            .unwrap()
            .replace('-', "")
            .chars()
            .collect();
        for at in 0..code.len() {
            for symbol in ALPHABET.iter().map(|byte| char::from(*byte)) {
                let mut typed = code.clone();
                if typed[at] != symbol && !matches!((typed[at], symbol), ('0', 'Z') | ('Z', '0')) {
                    typed[at] = symbol;
                    assert!(
                        parse(&typed.iter().collect::<String>()).is_none(),
                        "{typed:?}"
                    );
                }
            }
        }
        for at in 0..code.len() - 1 {
            let mut typed = code.clone();
            typed.swap(at, at + 1);
            if typed != code {
                assert!(
                    parse(&typed.iter().collect::<String>()).is_none(),
                    "{typed:?}"
                );
            }
        }
    }
}
