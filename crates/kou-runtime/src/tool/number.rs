use super::*;
use serde_json::value::RawValue;
use std::borrow::Cow;

pub(super) fn number(raw: &RawValue) -> Result<Option<f64>> {
    let encoded = raw.get().trim();
    if encoded == "null" {
        return Ok(None);
    }
    let string: String;
    let text = if encoded.starts_with('"') {
        string = serde_json::from_str(encoded).map_err(|error| Error(error.to_string()))?;
        string.trim()
    } else {
        encoded
    };
    if let Some(value) = parse(text).filter(|value| value.is_finite()) {
        return Ok(Some(value));
    }
    let bytes = encoded.as_bytes();
    let clipped = if bytes.len() <= 40 {
        encoded.to_owned()
    } else {
        format!("{}…", String::from_utf8_lossy(&bytes[..40]))
    };
    Err(Error(format!(
        "{}: want a number, such as 100, not {clipped}",
        type_error(encoded, "toolarg.Number")
    )))
}
fn parse(text: &str) -> Option<f64> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let hex = unsigned.starts_with("0x") || unsigned.starts_with("0X");
    for (index, byte) in unsigned.bytes().enumerate() {
        if byte != b'_' {
            continue;
        }
        let digit = |byte: u8| {
            if hex {
                byte.is_ascii_hexdigit()
            } else {
                byte.is_ascii_digit()
            }
        };
        if index + 1 == unsigned.len()
            || !digit(unsigned.as_bytes()[index + 1])
            || !(hex && index == 2 || index > 0 && digit(unsigned.as_bytes()[index - 1]))
        {
            return None;
        }
    }
    let clean = if text.contains('_') {
        Cow::Owned(text.replace('_', ""))
    } else {
        Cow::Borrowed(text)
    };
    if hex {
        hexadecimal(&clean)
    } else {
        clean.parse::<f64>().ok()
    }
}

// Keep the leading significand, a guard bit and sticky remainder. This performs
// one ties-to-even rounding, including the subnormal boundary; exact-only hex
// parsers reject values that Go's strconv.ParseFloat correctly rounds.
fn hexadecimal(text: &str) -> Option<f64> {
    let negative = text.starts_with('-');
    let text = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = text.get(2..)?.split_once(['p', 'P'])?;
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if integer.is_empty() && fraction.is_empty() {
        return None;
    }
    if !integer
        .bytes()
        .chain(fraction.bytes())
        .all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let exponent_negative = exponent.starts_with('-');
    let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let exponent = digits.bytes().fold(0i64, |n, b| {
        n.saturating_mul(10).saturating_add((b - b'0') as i64)
    });
    let exponent = if exponent_negative {
        -exponent
    } else {
        exponent
    };
    let nibble = |b: u8| {
        if b.is_ascii_digit() {
            b - b'0'
        } else {
            b.to_ascii_lowercase() - b'a' + 10
        }
    };
    let all = || integer.bytes().chain(fraction.bytes());
    let Some((first, digit)) = all().enumerate().find(|(_, b)| *b != b'0') else {
        return Some(if negative { -0.0 } else { 0.0 });
    };
    let bits = (integer.len() + fraction.len() - first - 1) as i64 * 4
        + (8 - nibble(digit).leading_zeros()) as i64;
    let power = exponent
        .saturating_sub((fraction.len() as i64).saturating_mul(4))
        .saturating_add(bits - 1);
    if power > 1023 {
        return None;
    }
    let sign = u64::from(negative) << 63;
    if power < -1075 {
        return Some(f64::from_bits(sign));
    }
    let precision = if power < -1022 {
        (power + 1075) as usize
    } else {
        53
    };
    let mut started = false;
    let mut count = 0;
    let mut significand = 0u64;
    let mut guard = false;
    let mut sticky = false;
    for digit in all().map(nibble) {
        for shift in (0..4).rev() {
            let bit = (digit >> shift) & 1;
            if !started && bit == 0 {
                continue;
            }
            started = true;
            if count < precision {
                significand = (significand << 1) | bit as u64;
            } else if count == precision {
                guard = bit != 0;
            } else {
                sticky |= bit != 0;
            }
            count += 1;
        }
    }
    if count < precision {
        significand <<= precision - count;
    }
    if guard && (sticky || significand & 1 != 0) {
        significand += 1;
    }
    if power < -1022 {
        return Some(f64::from_bits(sign | significand));
    }
    let mut power = power;
    if significand == 1 << 53 {
        significand >>= 1;
        power += 1;
    }
    if power > 1023 {
        return None;
    }
    Some(f64::from_bits(
        sign | ((power + 1023) as u64) << 52 | (significand & ((1 << 52) - 1)),
    ))
}
