//! "Handle Effect" `.hef` (circle/radius indicators from `.pef` effects) and the list of the
//! continuous effects on helper nodes `data/effects/effectlist.ini`, see `docs/formats/hef.md` and
//! re/effects.

use crate::pef::c_float;
use crate::property::atol;

/// Path of the continuous effect list in the game.
pub const EFFECT_LIST_PATH: &str = "data/effects/effectlist.ini";

/// A record `PARTICLEMOVE ... /PARTICLEMOVE` (`0x28` bytes).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HefRecord {
    /// `ARCRANGE` was read (only then does the record set effects).
    pub arc: bool,
    /// Arc from/to in degrees.
    pub arc_from: i32,
    pub arc_to: i32,
    /// `COUNT`: count mode (otherwise time mode).
    pub count_mode: bool,
    /// `HALFVISIONRANGE`: radius * 2/3.
    pub half_vision_range: bool,
    /// `CENTERPOS`: radius 0.
    pub center: bool,
    /// `DURATION` (ms, revolution time) or `COUNT` (number) - the same field.
    pub duration_or_count: i32,
    /// `TIMEOUT` (ms).
    pub timeout: i32,
    /// `EFFECT`: `.pef` path as in the file.
    pub effect: String,
    /// `EFFECTDURATION` (ms).
    pub effect_duration: i32,
}

/// Reads a `.hef` like `HandleEffect_ParseHef` (`0x004220E0`): comparison from column 0 without
/// trimming, `PARTICLEMOVE`, `HALFVISIONRANGE`, `CENTERPOS` as a whole line, the other
/// keys as a prefix including the space; numbers with `atoi`. `PARTICLEMOVE` only resets
/// flags (and the run-time counters), the other fields of the working record stay from the
/// previous record (original: the same stack memory).
pub fn parse_hef(text: &str) -> Vec<HefRecord> {
    let mut records = Vec::new();
    let mut current = HefRecord::default();
    let int = |s: &str| atol(s) as i32;
    for line in text.split('\n').map(|l| l.trim_end_matches('\r')) {
        if line == "PARTICLEMOVE" {
            current.arc = false;
            current.count_mode = false;
            current.half_vision_range = false;
            current.center = false;
        } else if line == "HALFVISIONRANGE" {
            current.half_vision_range = true;
        } else if line == "CENTERPOS" {
            current.center = true;
        } else if let Some(rest) = line.strip_prefix("ARCRANGE ") {
            let mut parts = rest.split_whitespace();
            current.arc = true;
            current.arc_from = parts.next().map_or(0, int);
            current.arc_to = parts.next().map_or(0, int);
        } else if let Some(rest) = line.strip_prefix("COUNT ") {
            current.count_mode = true;
            current.duration_or_count = int(rest);
        } else if let Some(rest) = line.strip_prefix("DURATION ") {
            current.duration_or_count = int(rest);
        } else if let Some(rest) = line.strip_prefix("TIMEOUT ") {
            current.timeout = int(rest);
        } else if let Some(rest) = line.strip_prefix("EFFECT ") {
            current.effect = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("EFFECTDURATION ") {
            current.effect_duration = int(rest);
        } else if line.starts_with("/PARTICLEMOVE") {
            records.push(current.clone());
        }
    }
    records
}

/// An entry of `effectlist.ini`: helper node prefix and effect.
#[derive(Debug, Clone, PartialEq)]
pub struct DummyEffect {
    /// `DUMMYOBJECT` name (node `<name>01`, `<name>02`, ...).
    pub node: String,
    /// Scale from the file - stored but never used, as in the original.
    pub scale: f32,
    /// `.pef` path as in the file.
    pub effect: String,
}

/// Reads `effectlist.ini` like `GameWorld_LoadEffectList` (`0x00489D10`): `DUMMYOBJECT`
/// (`Left(11)`, name from column 12) remembers the name, `EFFECT` (`Left(6)`, `sscanf("%f %s")` from
/// column 7) forms an entry with it.
pub fn parse_effect_list(text: &str) -> Vec<DummyEffect> {
    let mut entries = Vec::new();
    let mut node = String::new();
    for line in text.split('\n').map(|l| l.trim_end_matches('\r')) {
        if line.starts_with("DUMMYOBJECT") {
            node = line.get(12..).unwrap_or("").to_string();
        } else if line.starts_with("EFFECT") {
            let rest = line.get(7..).unwrap_or("");
            let mut parts = rest.split_whitespace();
            let scale = parts.next().and_then(c_float).unwrap_or(0.0);
            entries.push(DummyEffect {
                node: node.clone(),
                scale,
                effect: parts.next().unwrap_or("").to_string(),
            });
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_follow_the_original_line_rules() {
        let text = "#\r\n# Comment\r\nPARTICLEMOVE\r\nARCRANGE 0 180\r\nHALFVISIONRANGE\r\nDURATION 2000\r\nTIMEOUT 10\r\nEFFECT data/effects/circle_effect_1.pef\r\nEFFECTDURATION 2000\r\n/PARTICLEMOVE\r\n\r\nPARTICLEMOVE\r\nARCRANGE 180\r\nCENTERPOS\r\nCOUNT 20\r\n  TIMEOUT 99\r\nEFFECT wolf.pef\r\n/PARTICLEMOVE\r\n";
        let records = parse_hef(text);
        assert_eq!(records.len(), 2);
        let a = &records[0];
        assert!(a.arc && a.half_vision_range && !a.center && !a.count_mode);
        assert_eq!((a.arc_from, a.arc_to), (0, 180));
        assert_eq!((a.duration_or_count, a.timeout), (2000, 10));
        assert_eq!(a.effect, "data/effects/circle_effect_1.pef");
        assert_eq!(a.effect_duration, 2000);
        let b = &records[1];
        // Missing second arc value = 0; flags new, other fields from the previous record;
        // indented keys do not apply.
        assert_eq!((b.arc_from, b.arc_to), (180, 0));
        assert!(b.center && b.count_mode && !b.half_vision_range);
        assert_eq!((b.duration_or_count, b.timeout), (20, 10));
        assert_eq!(b.effect, "wolf.pef");
        assert_eq!(b.effect_duration, 2000);
    }

    #[test]
    fn effect_list_pairs_effects_with_the_last_dummy_object() {
        let text = "DUMMYOBJECT Feuer\r\nEFFECT 0.6 data/effects/feuer.pef\r\n\r\nDUMMYOBJECT Rauch\r\nEFFECT 1.0 data/effects/rauch_gerb.pef\r\nEFFECT 2 data/effects/b.pef\r\n";
        let list = parse_effect_list(text);
        assert_eq!(list.len(), 3);
        assert_eq!(
            list[0],
            DummyEffect {
                node: "Feuer".into(),
                scale: 0.6,
                effect: "data/effects/feuer.pef".into()
            }
        );
        assert_eq!(list[2].node, "Rauch");
        assert_eq!(list[2].scale, 2.0);
    }
}
