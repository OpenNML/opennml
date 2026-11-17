//! Reimplementation of the `sscanf` conversions of the MSVC runtime (VS 2003, `_sscanf` `0x005D30B5` ->
//! `__input` `0x005DB2D8`) as the map script reader uses them. Unlike `cscan`/`config`, these helpers do not
//! **saturate**: integers without `I64` run as 32-bit `int` without an overflow check
//! (`number * 10 + digit`, `<< 3`, `<< 4`), at the end the value is negated for `-` - so the result wraps
//! modulo 2^32 (`99999999999` -> `1215752191`).
//!
//! The functions take a word (without whitespace, as `scan_word` returns it) and read the longest number
//! prefix from it; the rest of the word is irrelevant, as with `sscanf`.

/// Whitespace of `%s`/`_isspace` in the C locale: `0x09...0x0D` and space.
fn is_c_space(c: char) -> bool {
    matches!(c, ' ' | '\t'..='\r')
}

/// `%s`: the `index`-th (0-based) word of the line, separated by whitespace.
pub fn scan_word(line: &str, index: usize) -> Option<&str> {
    line.split(is_c_space)
        .filter(|word| !word.is_empty())
        .nth(index)
}

/// Splits off the sign (`-`/`+`).
fn split_sign(word: &str) -> (bool, &str) {
    if let Some(rest) = word.strip_prefix('-') {
        (true, rest)
    } else {
        (false, word.strip_prefix('+').unwrap_or(word))
    }
}

/// Digits in base `radix` at the start; overflow modulo 2^32 (`__input`). Returns value and count.
fn digits(text: &str, radix: u32) -> (u32, usize) {
    let mut value = 0u32;
    let mut count = 0;
    for c in text.chars() {
        let Some(digit) = c.to_digit(radix) else {
            break;
        };
        value = value.wrapping_mul(radix).wrapping_add(digit);
        count += 1;
    }
    (value, count)
}

/// Negation after reading (`if (negative) number = -number`).
fn signed(negative: bool, value: u32) -> u32 {
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}

/// `%i`: sign, `0x`/`0X` -> hex, leading `0` -> octal (`8`/`9` end the number, the `0` counts as read:
/// `"09"` -> 0), otherwise decimal. `"0x"` without a hex digit fails.
pub fn scan_i(word: &str) -> Option<i32> {
    let (negative, rest) = split_sign(word);
    if let Some(hex) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
        let (value, count) = digits(hex, 16);
        return (count > 0).then(|| signed(negative, value) as i32);
    }
    if let Some(octal) = rest.strip_prefix('0') {
        let (value, _) = digits(octal, 8);
        return Some(signed(negative, value) as i32);
    }
    let (value, count) = digits(rest, 10);
    (count > 0).then(|| signed(negative, value) as i32)
}

/// `%d`: sign and decimal digits.
pub fn scan_d(word: &str) -> Option<i32> {
    scan_u(word).map(|value| value as i32)
}

/// `%u`: like `%d` (same branch `LAB_005dbc6c`), the sign is allowed: `"-5"` ->
/// `0xFFFFFFFB`.
pub fn scan_u(word: &str) -> Option<u32> {
    let (negative, rest) = split_sign(word);
    let (value, count) = digits(rest, 10);
    (count > 0).then(|| signed(negative, value))
}

/// `%f`: `[+-] digits [. digits] [e|E [+-] digits]` with at least one mantissa digit, at most
/// 349 characters; the text is converted as `double` and stored as `float` (hence via `f64` and then
/// `as f32`, not `parse::<f32>()`). An `e` without digits is left over, no `inf`/`nan`, no hex.
pub fn scan_f(word: &str) -> Option<f32> {
    let bytes = &word.as_bytes()[..word.len().min(349)];
    let count = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut mantissa = count(end);
    end += mantissa;
    if bytes.get(end) == Some(&b'.') {
        let fraction = count(end + 1);
        mantissa += fraction;
        end += 1 + fraction;
    }
    if mantissa == 0 {
        return None;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let sign = usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let exponent = count(end + 1 + sign);
        if exponent > 0 {
            end += 1 + sign + exponent;
        }
    }
    word[..end].parse::<f64>().ok().map(|value| value as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_i_accepts_c_prefixes() {
        assert_eq!(scan_i("0x10"), Some(16));
        assert_eq!(scan_i("010"), Some(8));
        assert_eq!(scan_i("-7"), Some(-7));
        assert_eq!(scan_i("abc"), None);
    }

    #[test]
    fn scan_i_edge_cases_follow_msvc_input() {
        assert_eq!(scan_i("0X1f"), Some(31));
        assert_eq!(scan_i("-0x10"), Some(-16));
        assert_eq!(scan_i("+5"), Some(5));
        assert_eq!(scan_i("0"), Some(0));
        assert_eq!(scan_i("09"), Some(0), "8 and 9 end the octal number");
        assert_eq!(scan_i("078"), Some(7));
        assert_eq!(scan_i("12abc"), Some(12), "only the number prefix counts");
        assert_eq!(scan_i("0x"), None, "0x without a hex digit fails");
        assert_eq!(scan_i("0xg"), None);
        assert_eq!(scan_i("-"), None);
        assert_eq!(scan_i(""), None);
        assert_eq!(scan_i("0xFFFFFFFF"), Some(-1));
    }

    #[test]
    fn integers_overflow_modulo_2_pow_32() {
        assert_eq!(scan_i("99999999999"), Some(1_215_752_191));
        assert_eq!(scan_d("99999999999"), Some(1_215_752_191));
        assert_eq!(scan_d("-99999999999"), Some(-1_215_752_191));
        assert_eq!(scan_d("2147483648"), Some(i32::MIN));
        assert_eq!(scan_d("-2147483648"), Some(i32::MIN));
    }

    #[test]
    fn scan_d_is_decimal_only() {
        assert_eq!(scan_d("010"), Some(10));
        assert_eq!(scan_d("0x10"), Some(0), "x ends the number");
        assert_eq!(scan_d("-12.5"), Some(-12));
        assert_eq!(scan_d("+"), None);
        assert_eq!(scan_d("x1"), None);
    }

    #[test]
    fn scan_u_takes_the_sign_like_msvc() {
        assert_eq!(scan_u("-5"), Some(0xFFFF_FFFB));
        assert_eq!(scan_u("4294967295"), Some(u32::MAX));
        assert_eq!(scan_u("4294967296"), Some(0));
        assert_eq!(scan_u("30"), Some(30));
        assert_eq!(scan_u("a"), None);
    }

    #[test]
    fn scan_f_reads_the_msvc_grammar() {
        assert_eq!(scan_f("1e"), Some(1.0), "an e without digits is left over");
        assert_eq!(scan_f("1e+"), Some(1.0));
        assert_eq!(scan_f("2.5e2x"), Some(250.0));
        assert_eq!(scan_f("-0.8"), Some(-0.8));
        assert_eq!(scan_f(".5"), Some(0.5));
        assert_eq!(scan_f("3."), Some(3.0));
        assert_eq!(scan_f("+7"), Some(7.0));
        assert_eq!(scan_f("inf"), None);
        assert_eq!(scan_f("nan"), None);
        assert_eq!(scan_f("."), None);
        assert_eq!(scan_f("e5"), None);
        assert_eq!(scan_f("0x10"), Some(0.0), "no hex, the 0 counts");
    }

    #[test]
    fn scan_f_rounds_through_double_like_the_original() {
        // 16777217 lies between two f32; via f64 and then f32 the same rounding results as when storing
        // the double into a float (`as f32`), not a rounding of the text.
        assert_eq!(scan_f("16777217"), Some(16_777_216.0));
        assert_eq!(scan_f("0.1"), Some(0.1_f64 as f32));
    }

    #[test]
    fn scan_word_splits_at_c_whitespace() {
        assert_eq!(scan_word("a  b\tc", 0), Some("a"));
        assert_eq!(scan_word("a  b\tc", 1), Some("b"));
        assert_eq!(scan_word("a  b\tc", 2), Some("c"));
        assert_eq!(scan_word("a  b\tc", 3), None);
        assert_eq!(scan_word("  x ", 0), Some("x"));
        assert_eq!(scan_word("a\u{B}b\u{C}c", 2), Some("c"));
        assert_eq!(scan_word("", 0), None);
    }
}
