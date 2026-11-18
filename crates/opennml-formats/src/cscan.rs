//! Small reimplementations of the C runtime and of the line readers of the original that several text
//! formats share (`campaign.ini`, `levels%04d.ini`, `randommaptypes.ini`, `.ldg`).

/// Whitespace in the sense of `isspace` of the C runtime.
pub(crate) const C_SPACE: [char; 6] = [' ', '\t', '\n', '\x0b', '\x0c', '\r'];

/// Lines like `GameFile_ReadLine` (`0x00574C00`): the original splits at `\n` and drops **every** `\r`, not just
/// the one at the line end. It no longer returns an empty last line at the end of the file - without effect for the
/// readers because empty lines do nothing.
pub(crate) fn game_file_lines(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split('\n').map(|line| line.replace('\r', ""))
}

/// `atol` of the C runtime: whitespace, sign, digits; anything else ends the number, without
/// digits the result is 0. Values that are too large are clamped to the `i32` range.
pub(crate) fn atol(text: &str) -> i32 {
    scan_int(text).unwrap_or(0)
}

/// `sscanf("%d")`: like [`atol`], but without digits there is no result.
pub(crate) fn scan_int(text: &str) -> Option<i32> {
    scan_int_rest(text).map(|(value, _)| value)
}

/// Like [`scan_int`], additionally returns the rest after the number (for several `%d` in one call).
pub(crate) fn scan_int_rest(text: &str) -> Option<(i32, &str)> {
    let text = text.trim_start_matches(C_SPACE);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let end = digits.bytes().take_while(u8::is_ascii_digit).count();
    if end == 0 {
        return None;
    }
    let mut value: i64 = 0;
    for digit in digits[..end].bytes() {
        value = (value * 10 + i64::from(digit - b'0')).min(1 << 40);
    }
    let value = if negative { -value } else { value };
    let value = value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    Some((value, &digits[end..]))
}

/// `sscanf("%d %d ...")` with `N` numbers: reads until a number is missing; the remaining fields stay 0.
/// The original leaves them undetermined (no return value checked, stack or heap content);
/// 0 is the fixed choice of the reimplementation, see `docs/formats/gifcon-misc.md`, section "statusbar.ini".
pub(crate) fn scan_ints<const N: usize>(text: &str) -> [i32; N] {
    let mut out = [0; N];
    let mut rest = text;
    for slot in &mut out {
        let Some((value, tail)) = scan_int_rest(rest) else {
            break;
        };
        *slot = value;
        rest = tail;
    }
    out
}

/// `sscanf("%X")`: whitespace, an optional `0x`/`0X`, hex digits in any letter case. Values that are too large
/// result in `u32::MAX`. Without digits there is no result.
pub(crate) fn scan_hex(text: &str) -> Option<u32> {
    let text = text.trim_start_matches(C_SPACE);
    let digits = match text.get(..2) {
        Some("0x" | "0X") if text[2..].starts_with(|c: char| c.is_ascii_hexdigit()) => &text[2..],
        _ => text,
    };
    let end = digits.bytes().take_while(u8::is_ascii_hexdigit).count();
    if end == 0 {
        return None;
    }
    let value = digits[..end].bytes().fold(0u64, |acc, byte| {
        let digit = (byte as char).to_digit(16).unwrap_or(0);
        (acc * 16 + u64::from(digit)).min(u64::from(u32::MAX))
    });
    Some(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_drop_every_carriage_return() {
        let lines: Vec<String> = game_file_lines("a\r\nb\rc\r\n\r\nd").collect();
        assert_eq!(lines, ["a", "bc", "", "d"]);
    }

    #[test]
    fn decimal_scan_returns_the_rest() {
        assert_eq!(scan_int_rest("  12 34"), Some((12, " 34")));
        assert_eq!(scan_int_rest("-5x"), Some((-5, "x")));
        assert_eq!(scan_int_rest("x"), None);
        assert_eq!(atol("7abc"), 7);
        assert_eq!(atol("abc"), 0);
    }

    #[test]
    fn counted_scan_stops_at_the_first_missing_number() {
        assert_eq!(scan_ints::<4>(" 1 -2 3 4 5"), [1, -2, 3, 4]);
        assert_eq!(scan_ints::<4>("7 8"), [7, 8, 0, 0]);
        assert_eq!(scan_ints::<3>("7 x 9"), [7, 0, 0]);
        assert_eq!(scan_ints::<2>(""), [0, 0]);
    }

    #[test]
    fn hex_scan_ignores_case_and_accepts_a_prefix() {
        assert_eq!(scan_hex("ffff00ff"), Some(0xFFFF_00FF));
        assert_eq!(scan_hex(" AA463910"), Some(0xAA46_3910));
        assert_eq!(scan_hex("0x80"), Some(0x80));
        assert_eq!(scan_hex("0xg"), Some(0));
        assert_eq!(scan_hex("123456789"), Some(u32::MAX));
        assert_eq!(scan_hex("zz"), None);
    }
}
