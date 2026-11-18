//! Light presets `data/projects/lightdata.ini`, see `docs/formats/lightdata.md`.
//! Read like `GameEngine_LoadLightPresets` (`0x00439A80`) and `LightPreset_ParseLightBlock`
//! (`0x00436E50`): line by line, without bracket grammar.
/// Path of the file in the game.
pub const LIGHTDATA_PATH: &str = "data/projects/lightdata.ini";

/// A `[TERRAIN_LIGHT]` or `[OBJECT_LIGHT]` block; the same fields as `[TERRAIN] LIGHT_*` or
/// `[LIGHT]` of the `.wld`.
#[derive(Debug, Clone, PartialEq)]
pub struct LightBlock {
    /// Direction **to** the light (normalized when applied).
    pub position: [f32; 3],
    /// R G B; as in the original (`COLORREF`) only the low byte of each value.
    pub color: [i32; 3],
    pub intensity: i32,
}

/// A preset.
#[derive(Debug, Clone, PartialEq)]
pub struct LightPreset {
    /// Name as in the file (case preserved).
    pub name: String,
    pub terrain: LightBlock,
    pub object: LightBlock,
}

/// Reads all complete presets in file order. As in the original: lines are not trimmed, an entry
/// does not count until `[OBJECT_LIGHT]`, a later one of the same name (case-insensitive) replaces
/// the earlier one in its place.
///
/// Deviation from the original (broken files only, not present in the shipped data): a block that
/// ends before `LIGHT_INTENSITY` or contains an unreadable value discards the preset (the game
/// would take uninitialized values).
pub fn parse_lightdata(text: &str) -> Vec<LightPreset> {
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let mut presets: Vec<LightPreset> = Vec::new();
    let mut name: Option<String> = None;
    let mut terrain: Option<LightBlock> = None;
    while let Some(line) = lines.next() {
        if line.starts_with("LIGHT_NAME") {
            // `Mid(12)` without the last character: requires `LIGHT_NAME "` and `"`.
            let chars: Vec<char> = line.chars().collect();
            name = Some(if chars.len() > 12 {
                chars[12..chars.len() - 1].iter().collect()
            } else {
                String::new()
            });
            terrain = None;
        } else if line == "[TERRAIN_LIGHT]" {
            terrain = parse_block(&mut lines);
        } else if line == "[OBJECT_LIGHT]" {
            let object = parse_block(&mut lines);
            if let (Some(name), Some(terrain), Some(object)) = (&name, &terrain, object) {
                let preset = LightPreset {
                    name: name.clone(),
                    terrain: terrain.clone(),
                    object,
                };
                match presets
                    .iter_mut()
                    .find(|p| p.name.to_lowercase() == name.to_lowercase())
                {
                    Some(existing) => *existing = preset,
                    None => presets.push(preset),
                }
            }
        }
    }
    presets
}

/// Preset by name, case-insensitive (`GameEngine_ApplyLightPreset`
/// `0x00430C40` writes the name in lowercase).
pub fn find_preset<'a>(presets: &'a [LightPreset], name: &str) -> Option<&'a LightPreset> {
    let wanted = name.to_lowercase();
    presets.iter().find(|p| p.name.to_lowercase() == wanted)
}

/// Reads lines up to and including `LIGHT_INTENSITY`; other lines in between are ignored.
fn parse_block<'a>(lines: &mut impl Iterator<Item = &'a str>) -> Option<LightBlock> {
    let (mut position, mut color) = (None, None);
    for line in lines {
        if let Some(rest) = line.strip_prefix("LIGHT_POSITION") {
            position = numbers::<f32>(rest);
        } else if let Some(rest) = line.strip_prefix("LIGHT_COLOR") {
            color = numbers::<i32>(rest).map(|c| c.map(|v| v & 0xFF));
        } else if let Some(rest) = line.strip_prefix("LIGHT_INTENSITY") {
            let intensity = rest.split_whitespace().next()?.parse().ok()?;
            return Some(LightBlock {
                position: position?,
                color: color?,
                intensity,
            });
        }
    }
    None
}

/// The first three numbers after the key.
fn numbers<T: std::str::FromStr>(rest: &str) -> Option<[T; 3]> {
    let mut values = rest.split_whitespace().map(|v| v.parse::<T>().ok());
    Some([values.next()??, values.next()??, values.next()??])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[LIGHT]\r\nLIGHT_NAME \"Sommer Tag\"\r\n[TERRAIN_LIGHT]\r\nLIGHT_POSITION 800.000000 400.000000 -1300.000000\r\nLIGHT_COLOR 200 200 200\r\nLIGHT_INTENSITY 20\r\n[OBJECT_LIGHT]\r\nLIGHT_POSITION 800.000000 400.000000 -1300.000000\r\nLIGHT_COLOR 255 241 193\r\nLIGHT_INTENSITY 40\r\n\r\n[LIGHT]\r\nLIGHT_NAME \"Spa 5 Nacht\"\r\n[TERRAIN_LIGHT]\r\nLIGHT_POSITION 800.000000 400.000000 -1300.000000\r\nLIGHT_COLOR 69 135 233\r\nLIGHT_INTENSITY 70\r\n[OBJECT_LIGHT]\r\nLIGHT_POSITION 800.000000 600.000000 -1300.000000\r\nLIGHT_COLOR 0 128 255\r\nLIGHT_INTENSITY 75\r\n";

    #[test]
    fn presets_are_read_in_file_order() {
        let presets = parse_lightdata(SAMPLE);
        assert_eq!(presets.len(), 2);
        let tag = &presets[0];
        assert_eq!(tag.name, "Sommer Tag");
        assert_eq!(
            tag.terrain,
            LightBlock {
                position: [800.0, 400.0, -1300.0],
                color: [200, 200, 200],
                intensity: 20,
            }
        );
        assert_eq!(tag.object.color, [255, 241, 193]);
        assert_eq!(tag.object.intensity, 40);
        assert_eq!(presets[1].object.position, [800.0, 600.0, -1300.0]);
    }

    #[test]
    fn names_are_found_without_case() {
        let presets = parse_lightdata(SAMPLE);
        assert_eq!(
            find_preset(&presets, "spa 5 NACHT").map(|p| p.terrain.intensity),
            Some(70)
        );
        assert!(find_preset(&presets, "Spa 5").is_none());
    }

    #[test]
    fn incomplete_entries_are_dropped_and_duplicates_replace_in_place() {
        // Without [OBJECT_LIGHT] no entry; block without LIGHT_INTENSITY at the end of the file discarded.
        let no_object = SAMPLE.replace("[OBJECT_LIGHT]\r\nLIGHT_POSITION 800.000000 400", "x\r\n");
        assert_eq!(parse_lightdata(&no_object).len(), 1);
        let cut = &SAMPLE[..SAMPLE.rfind("LIGHT_INTENSITY 75").unwrap()];
        assert_eq!(parse_lightdata(cut).len(), 1);
        // Leading spaces prevent recognition, as in the original.
        let indented = SAMPLE.replace("[OBJECT_LIGHT]", " [OBJECT_LIGHT]");
        assert!(parse_lightdata(&indented).is_empty());
        // The same name in a different spelling replaces the first entry in its place.
        let twice = format!("{SAMPLE}{}", SAMPLE.replace("Sommer Tag", "SOMMER TAG"));
        let presets = parse_lightdata(&twice);
        assert_eq!(presets.len(), 2);
        assert_eq!(presets[0].name, "SOMMER TAG");
        // Color values above 255: only the low byte (COLORREF).
        let wide = SAMPLE.replace("LIGHT_COLOR 200 200 200", "LIGHT_COLOR 456 200 200");
        assert_eq!(parse_lightdata(&wide)[0].terrain.color, [200, 200, 200]);
    }
}
