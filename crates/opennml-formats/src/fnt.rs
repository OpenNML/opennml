//! Bitmap fonts (`FONTFILE V1.0`); the texture lies next to it as a `.tga` of the same name.

use crate::FormatError;
use crate::error::syntax;

/// A character: code, pixel size and texture coordinates (u0..u1 horizontal, v0..v1 vertical).
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub code: u32,
    pub width: u32,
    pub height: u32,
    pub u0: f32,
    pub u1: f32,
    pub v0: f32,
    pub v1: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Font {
    pub glyphs: Vec<Glyph>,
}

pub fn parse_fnt(text: &str) -> Result<Font, FormatError> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty());
    for expected in ["FONTFILE V1.0", "[CHARINFOS]"] {
        match lines.next() {
            Some((_, line)) if line == expected => {}
            Some((n, _)) => return Err(syntax(n, &format!("\"{expected}\" expected"))),
            None => return Err(FormatError::Invalid("font file is incomplete".into())),
        }
    }
    let mut glyphs = Vec::new();
    for (n, line) in lines {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 7 {
            return Err(syntax(n, "7 values per character expected"));
        }
        let int = |i: usize| {
            f[i].parse::<u32>()
                .map_err(|_| syntax(n, "integer expected"))
        };
        let flt = |i: usize| {
            f[i].parse::<f32>()
                .map_err(|_| syntax(n, "decimal number expected"))
        };
        glyphs.push(Glyph {
            code: int(0)?,
            width: int(1)?,
            height: int(2)?,
            u0: flt(3)?,
            u1: flt(4)?,
            v0: flt(5)?,
            v1: flt(6)?,
        });
    }
    Ok(Font { glyphs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_and_glyphs() {
        let text = "FONTFILE V1.0\r\n\r\n[CHARINFOS]\r\n0 9 15 0.000000 0.062500 0.000000 0.062500\r\n255 5 15 0.937500 1.000000 0.937500 1.000000\r\n";
        let font = parse_fnt(text).unwrap();
        assert_eq!(font.glyphs.len(), 2);
        assert_eq!(
            font.glyphs[1],
            Glyph {
                code: 255,
                width: 5,
                height: 15,
                u0: 0.9375,
                u1: 1.0,
                v0: 0.9375,
                v1: 1.0
            }
        );
    }

    #[test]
    fn rejects_wrong_header_and_bad_lines() {
        assert!(matches!(
            parse_fnt("FONTFILE V2.0\n[CHARINFOS]\n"),
            Err(FormatError::Syntax { line: 1, .. })
        ));
        assert!(matches!(
            parse_fnt("FONTFILE V1.0\n[CHARINFOS]\n1 2 3\n"),
            Err(FormatError::Syntax { line: 3, .. })
        ));
        assert!(matches!(
            parse_fnt("FONTFILE V1.0\n[CHARINFOS]\nx 9 15 0 0 0 0\n"),
            Err(FormatError::Syntax { line: 3, .. })
        ));
        assert!(matches!(parse_fnt(""), Err(FormatError::Invalid(_))));
    }
}
