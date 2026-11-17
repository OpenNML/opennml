//! Dispatcher `FUN_00545C10` (`0x00545C10`) and block readers of the map scripts. See re/random-map Z.5.4/Z.5.5.
use crate::FormatError;
use crate::rnd::reader::{ScriptReader, include_path};
use crate::rnd::scan::{scan_d, scan_f, scan_i, scan_u, scan_word};
use crate::rnd::{AreaDef, ConnectDef, ObjectDef, PlaceObjectDef, RandomMapScript, TextureDef};
use crate::wld::{ObjectLight, ShadowParams};

/// Keywords of the dispatcher in checking order (`FUN_00545C10`, table in re/random-map Z.5.4).
/// Each counts as a substring anywhere in the line; the first hit wins.
const TOP_KEYS: [(&str, Top); 21] = [
    ("#include", Top::Include),
    ("ProjectFile", Top::ProjectFile),
    ("SmoothFactor", Top::SmoothFactor),
    ("PathNormal", Top::PathNormal),
    ("BuildinglotXSize", Top::BuildinglotXSize),
    ("BuildinglotYSize", Top::BuildinglotYSize),
    ("BuildinglotTexture", Top::BuildinglotTexture),
    ("BaseTexture", Top::BaseTexture),
    ("#const", Top::Const),
    ("#define", Top::Define),
    ("#Texture", Top::Texture),
    ("#Object", Top::Object),
    ("TerrainLight", Top::TerrainLight),
    ("ObjectLight", Top::ObjectLight),
    ("ShadowLight", Top::ShadowLight),
    ("create_PlayerAreas", Top::PlayerAreas),
    ("create_LandArea", Top::LandArea),
    ("create_TerrainArea", Top::TerrainArea),
    ("place_Object", Top::PlaceObject),
    ("Water", Top::Water),
    ("connect_Players", Top::Connect),
];

#[derive(Clone, Copy)]
enum Top {
    Include,
    ProjectFile,
    SmoothFactor,
    PathNormal,
    BuildinglotXSize,
    BuildinglotYSize,
    BuildinglotTexture,
    BaseTexture,
    Const,
    Define,
    Texture,
    Object,
    TerrainLight,
    ObjectLight,
    ShadowLight,
    PlayerAreas,
    LandArea,
    TerrainArea,
    PlaceObject,
    Water,
    Connect,
}

/// Area block `FUN_00543CA0`, checking order (re/random-map Z.5.5).
const AREA_KEYS: [&str; 20] = [
    "ID",
    "Replace",
    "Texture",
    "minSize",
    "numTiles",
    "Percent",
    "LeftBorder",
    "RightBorder",
    "TopBorder",
    "BottomBorder",
    "BorderRoughness",
    "AvoidRoughness",
    "numClumps",
    "AvoidOther",
    "PutOnto",
    "AvoidPlayerAreas",
    "ScaleToPlayers",
    "ScaleToMapSize",
    "ScaleClumpsToMapSize",
    "SharePlayerLands",
];

/// `place_Object` block `FUN_00544510`, checking order (re/random-map Z.5.5).
const PLACE_KEYS: [&str; 18] = [
    "number",
    "numGroups",
    "maxGroupRadius",
    "minGroupDistance",
    "ForAllPlayers",
    "IsPlayer",
    "PutOnFlat",
    "minDistToPlayers",
    "maxDistToPlayers",
    "AvoidOther",
    "PutOnto",
    "Fill",
    "ScaleToPlayers",
    "ScaleToMapSize",
    "ScaleGroupsToMapSize",
    "NoGrid",
    "AvoidPlayerAreas",
    "GroundTexture",
];

/// `connect_Players` block `FUN_005449D0`.
const CONNECT_KEYS: [&str; 3] = ["Texture", "PutOnto", "radius"];

/// `Water` block `FUN_00544200`.
const WATER_KEYS: [&str; 8] = [
    "WaterLevel",
    "UseTexture",
    "BaseTextureGUID",
    "RefractionTextureGUID",
    "Tiling",
    "MaxDepth",
    "MinAlpha",
    "MaxAlpha",
];

/// Index of the first keyword that occurs as a substring in the line (`_mbsstr`).
fn first_key(line: &str, keys: &[&str]) -> Option<usize> {
    keys.iter().position(|key| line.contains(key))
}

/// Second word of the line (`sscanf("%s %...")`).
fn value(line: &str) -> Option<&str> {
    scan_word(line, 1)
}

/// `%i` on the second word; without a readable value the field stays.
fn set_i(line: &str, field: &mut i32) {
    if let Some(v) = value(line).and_then(scan_i) {
        *field = v;
    }
}

fn set_d(line: &str, field: &mut i32) {
    if let Some(v) = value(line).and_then(scan_d) {
        *field = v;
    }
}

fn set_f(line: &str, field: &mut f32) {
    if let Some(v) = value(line).and_then(scan_f) {
        *field = v;
    }
}

fn set_u(line: &str, field: &mut u32) {
    if let Some(v) = value(line).and_then(scan_u) {
        *field = v;
    }
}

/// `%s`: word into a text field.
fn set_s(line: &str, field: &mut String) {
    if let Some(v) = value(line) {
        *field = v.to_string();
    }
}

/// Name between the first and the last `"` (`FUN_00541D10`, `0x00541D10`). With fewer than two `"`,
/// `CString::Mid` clamps to the empty string.
fn quoted_name(line: &str) -> String {
    match (line.find('"'), line.rfind('"')) {
        (Some(first), Some(last)) if last > first => line[first + 1..last].to_string(),
        _ => String::new(),
    }
}

/// `POSITION`: at most three `%f`; whatever is not read stays (`"%s %f %f %f"` writes directly into
/// the target).
fn read_position(line: &str, position: &mut [f32; 3]) {
    for (i, slot) in position.iter_mut().enumerate() {
        match scan_word(line, i + 1).and_then(scan_f) {
            Some(v) => *slot = v,
            None => break,
        }
    }
}

/// `COLOR`: `"%s %i %i %i"` into stack variables, then `r | g<<8 | b<<16` - only the lower
/// byte per channel (`0x00543A9E ... 0x00543ABA`). The original writes without checking the result, for
/// missing channels it leaves undefined stack leftovers; here 0 (A7).
fn read_color(line: &str) -> [i32; 3] {
    let mut color = [0; 3];
    for (i, slot) in color.iter_mut().enumerate() {
        match scan_word(line, i + 1).and_then(scan_i) {
            Some(v) => *slot = v & 0xFF,
            None => break,
        }
    }
    color
}

/// `RandomMap_ParseLightBlock` (`0x005439B0`) and `RandomMap_ParseShadowLightBlock` (`0x00543B60`,
/// without `COLOR`) on the lines of a block.
fn apply_light(
    lines: &[String],
    position: &mut [f32; 3],
    mut color: Option<&mut [i32; 3]>,
    intensity: &mut i32,
) {
    for line in lines {
        if line.contains("POSITION") {
            read_position(line, position);
        } else if line.contains("COLOR") && color.is_some() {
            if let Some(color) = color.as_deref_mut() {
                *color = read_color(line);
            }
        } else if line.contains("INTENSITY") {
            set_i(line, intensity);
        }
    }
}

struct Parser<'r, 'a> {
    reader: &'r mut ScriptReader<'a>,
    script: RandomMapScript,
    /// `gen+0x10C`: texture counter.
    textures_seen: i32,
}

impl Parser<'_, '_> {
    /// Main loop of `FUN_00545C10` for the topmost open file.
    fn run(&mut self) -> Result<(), FormatError> {
        while let Some(line) = self.reader.next_line() {
            let Some(&(_, top)) = TOP_KEYS.iter().find(|(key, _)| line.contains(key)) else {
                // Unknown lines, `{`, `}`, orphaned block lines: silently ignored.
                continue;
            };
            self.dispatch(top, &line)?;
        }
        Ok(())
    }

    /// The lines of a block up to the line that is exactly `}` (`_mbscmp`, `0x00619360`). The
    /// original ignores the end of file and keeps running (D-324); here an error with location.
    fn read_block(&mut self, head: &str) -> Result<Vec<String>, FormatError> {
        let mut lines = Vec::new();
        loop {
            let Some(line) = self.reader.next_line() else {
                return Err(FormatError::Invalid(format!(
                    "Map script {}, line {}: end of file in block {}",
                    self.reader.current_path(),
                    self.reader.line_number(),
                    head.trim()
                )));
            };
            if line == "}" {
                return Ok(lines);
            }
            lines.push(line);
        }
    }

    fn dispatch(&mut self, top: Top, line: &str) -> Result<(), FormatError> {
        match top {
            Top::Include => self.include(line)?,
            Top::ProjectFile => {
                // The host calls `ProjectFile_ParseObjects` by name.
                self.script.project_file = Some(quoted_name(line));
            }
            Top::SmoothFactor => set_d(line, &mut self.script.smooth_factor),
            Top::PathNormal => {
                set_f(line, &mut self.script.path_normal);
                // The copy to `Terrain+0xA0` always happens, even without a value (`0x00545F07`).
                self.script.terrain.path_normal = self.script.path_normal;
            }
            Top::BuildinglotXSize => set_d(line, &mut self.script.buildinglot_size[0]),
            Top::BuildinglotYSize => set_d(line, &mut self.script.buildinglot_size[1]),
            Top::BuildinglotTexture => set_s(line, &mut self.script.buildinglot_texture),
            Top::BaseTexture => set_s(line, &mut self.script.base_texture),
            Top::Const => {
                // `sscanf("%s %s %s")`: name and value (only the first word); if one is missing,
                // the original reads stack leftovers, here "" (A7).
                let name = scan_word(line, 1).unwrap_or("").to_string();
                let text = scan_word(line, 2).unwrap_or("");
                self.reader.add_const(&name, text);
            }
            Top::Define => {
                let name = scan_word(line, 1).unwrap_or("").to_string();
                self.reader.add_define(&name);
            }
            Top::Texture => self.texture(line)?,
            Top::Object => self.object(line)?,
            Top::TerrainLight => {
                let lines = self.read_block(line)?;
                let terrain = &mut self.script.terrain;
                apply_light(
                    &lines,
                    &mut terrain.light_position,
                    Some(&mut terrain.light_color),
                    &mut terrain.light_intensity,
                );
            }
            Top::ObjectLight => {
                let lines = self.read_block(line)?;
                // The first block creates the value with 0 (D-345).
                let light = self.script.object_light.get_or_insert(ObjectLight {
                    position: [0.0; 3],
                    color: [0; 3],
                    intensity: 0,
                });
                apply_light(
                    &lines,
                    &mut light.position,
                    Some(&mut light.color),
                    &mut light.intensity,
                );
            }
            Top::ShadowLight => {
                let lines = self.read_block(line)?;
                let shadow = self.script.shadow.get_or_insert(ShadowParams {
                    light_position: [0.0; 3],
                    intensity: 0,
                });
                apply_light(
                    &lines,
                    &mut shadow.light_position,
                    None,
                    &mut shadow.intensity,
                );
            }
            Top::PlayerAreas => {
                let area = self.area(line)?;
                self.script.player_areas.push(area);
            }
            Top::LandArea => {
                let area = self.area(line)?;
                self.script.land_areas.push(area);
            }
            Top::TerrainArea => {
                let area = self.area(line)?;
                self.script.terrain_areas.push(area);
            }
            Top::PlaceObject => self.place_object(line)?,
            Top::Water => self.water(line)?,
            Top::Connect => self.connect(line)?,
        }
        Ok(())
    }

    /// `#include` (`0x00545C7F`): name between the `"`, path per `FUN_00542350`, recursive
    /// call `FUN_00546930`; a missing include is silently ignored.
    fn include(&mut self, line: &str) -> Result<(), FormatError> {
        let path = include_path(self.reader.main_path(), &quoted_name(line));
        if self.reader.open(&path)? {
            let result = self.run();
            self.reader.close();
            result
        } else {
            self.script.missing_includes.push(path);
            Ok(())
        }
    }

    /// `#Texture NAME { ... }` (`FUN_005422B0`): `HEIGHT`, then `GUID`.
    fn texture(&mut self, head: &str) -> Result<(), FormatError> {
        self.textures_seen = self.textures_seen.wrapping_add(1);
        let mut texture = TextureDef {
            number: self.textures_seen,
            name: scan_word(head, 1).unwrap_or("").to_string(),
            height: 0.0,
            guids: Vec::new(),
        };
        for line in self.read_block(head)? {
            if line.contains("HEIGHT") {
                set_f(&line, &mut texture.height);
            } else if line.contains("GUID") {
                // `"%s %u"`, AddTail: duplicates stay.
                if let Some(guid) = value(&line).and_then(scan_u) {
                    texture.guids.push(guid);
                }
            }
        }
        self.script.textures.push(texture);
        Ok(())
    }

    /// `#Object NAME { ... }` (`FUN_00541610`).
    fn object(&mut self, head: &str) -> Result<(), FormatError> {
        let mut object = ObjectDef {
            name: scan_word(head, 1).unwrap_or("").to_string(),
            guids: Vec::new(),
        };
        for line in self.read_block(head)? {
            if line.contains("GUID")
                && let Some(guid) = value(&line).and_then(scan_u)
            {
                object.guids.push(guid);
            }
        }
        self.script.objects.push(object);
        Ok(())
    }

    /// Area block (`FUN_00543CA0`); the generator computes borders and amounts.
    fn area(&mut self, head: &str) -> Result<AreaDef, FormatError> {
        let mut area = AreaDef::default();
        for line in self.read_block(head)? {
            let Some(key) = first_key(&line, &AREA_KEYS) else {
                continue;
            };
            match key {
                0 => set_i(&line, &mut area.id),
                1 => set_s(&line, &mut area.replace),
                2 => set_s(&line, &mut area.texture),
                3 => set_i(&line, &mut area.min_size),
                4 => set_i(&line, &mut area.num_tiles),
                5 => set_i(&line, &mut area.percent),
                6 => set_i(&line, &mut area.left),
                7 => set_i(&line, &mut area.right),
                8 => set_i(&line, &mut area.top),
                9 => set_i(&line, &mut area.bottom),
                10 => set_i(&line, &mut area.border_roughness),
                11 => set_i(&line, &mut area.avoid_roughness),
                12 => set_i(&line, &mut area.num_clumps),
                13 => set_i(&line, &mut area.avoid_other),
                14 => set_s(&line, &mut area.put_onto),
                // Flag: the word alone sets it, a value after it does not count.
                15 => area.avoid_player_areas = true,
                16 => area.scale_to_players = true,
                17 => area.scale_to_map_size = true,
                18 => area.scale_clumps_to_map_size = true,
                _ => area.share_player_lands = true,
            }
        }
        Ok(area)
    }

    /// `place_Object NAME { ... }` (`FUN_00544510`).
    fn place_object(&mut self, head: &str) -> Result<(), FormatError> {
        let mut def = PlaceObjectDef {
            name: scan_word(head, 1).unwrap_or("").to_string(),
            ..PlaceObjectDef::default()
        };
        for line in self.read_block(head)? {
            let Some(key) = first_key(&line, &PLACE_KEYS) else {
                continue;
            };
            match key {
                0 => set_i(&line, &mut def.number),
                1 => set_i(&line, &mut def.num_groups),
                2 => set_i(&line, &mut def.max_group_radius),
                3 => set_i(&line, &mut def.min_group_distance),
                4 => def.for_all_players = true,
                5 => def.is_player = true,
                6 => def.put_on_flat = true,
                7 => set_i(&line, &mut def.min_dist_to_players),
                8 => set_i(&line, &mut def.max_dist_to_players),
                9 => set_i(&line, &mut def.avoid_other),
                10 => set_s(&line, &mut def.put_onto),
                11 => def.fill = true,
                12 => def.scale_to_players = true,
                13 => def.scale_to_map_size = true,
                14 => def.scale_groups_to_map_size = true,
                15 => def.no_grid = true,
                16 => def.avoid_player_areas = true,
                _ => set_s(&line, &mut def.ground_texture),
            }
        }
        self.script.placements.push(def);
        Ok(())
    }

    /// `connect_Players { ... }` (`FUN_005449D0`).
    fn connect(&mut self, head: &str) -> Result<(), FormatError> {
        let mut def = ConnectDef::default();
        for line in self.read_block(head)? {
            match first_key(&line, &CONNECT_KEYS) {
                Some(0) => set_s(&line, &mut def.texture),
                Some(1) => set_s(&line, &mut def.put_onto),
                Some(2) => set_i(&line, &mut def.radius),
                _ => {}
            }
        }
        self.script.connections.push(def);
        Ok(())
    }

    /// `Water { ... }` (`FUN_00544200`) writes `Terrain+0x78 ...` (the host loads the texture handles
    /// for `UseTexture != 0`).
    fn water(&mut self, head: &str) -> Result<(), FormatError> {
        let lines = self.read_block(head)?;
        let terrain = &mut self.script.terrain;
        for line in lines {
            match first_key(&line, &WATER_KEYS) {
                Some(0) => set_f(&line, &mut terrain.water_level),
                Some(1) => {
                    let mut flag = i32::from(terrain.water_use_texture);
                    set_d(&line, &mut flag);
                    terrain.water_use_texture = flag != 0;
                }
                Some(2) => set_u(&line, &mut terrain.water_base_texture),
                Some(3) => set_u(&line, &mut terrain.water_refraction_texture),
                Some(4) => set_d(&line, &mut terrain.water_texture_tiling),
                Some(5) => set_d(&line, &mut terrain.water_max_depth),
                Some(6) => set_d(&line, &mut terrain.water_min_alpha),
                Some(7) => set_d(&line, &mut terrain.water_max_alpha),
                _ => {}
            }
        }
        Ok(())
    }
}

/// Reads the map script `main_path` including includes. `read` returns the bytes of a file (`None`
/// = missing), `random` the raw value of `rand()` (only the preprocessor draws randomness). `defines`
/// are the defines before parsing (map size, define string).
///
/// Errors: main script missing (D-327, silent failure in the original), end of file inside a block
/// (D-324), include depth above 16 (D-344).
pub fn parse_random_map_script(
    main_path: &str,
    defines: Vec<String>,
    read: &mut dyn FnMut(&str) -> Option<Vec<u8>>,
    random: &mut dyn FnMut() -> i32,
) -> Result<RandomMapScript, FormatError> {
    let mut reader = ScriptReader::new(read, random, defines);
    if !reader.open(main_path)? {
        return Err(FormatError::Invalid(format!(
            "Map script {main_path} is missing"
        )));
    }
    let mut parser = Parser {
        reader: &mut reader,
        script: RandomMapScript::default(),
        textures_seen: 0,
    };
    parser.run()?;
    let mut script = parser.script;
    reader.close();
    script.defines = reader.defines().to_vec();
    script.consts = reader.consts().to_vec();
    Ok(script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wld::{ObjectLight, ShadowParams};
    use std::collections::BTreeMap;

    const MAIN: &str = "data/randommapscripts/main.rnd";

    /// Script including includes in memory; `rolls` are the rolls of `rand()` for `#random`.
    fn parse_with(
        main: &str,
        extra: &[(&str, &str)],
        defines: &[&str],
        rolls: &[i32],
    ) -> Result<RandomMapScript, FormatError> {
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        files.insert(MAIN.to_string(), main.as_bytes().to_vec());
        for (path, text) in extra {
            files.insert(path.to_string(), text.as_bytes().to_vec());
        }
        let mut read = |path: &str| files.get(path).cloned();
        let mut rolls = rolls.iter().copied();
        let mut random = || rolls.next().unwrap_or(0);
        parse_random_map_script(
            MAIN,
            defines.iter().map(|d| d.to_string()).collect(),
            &mut read,
            &mut random,
        )
    }

    fn parse(main: &str) -> RandomMapScript {
        parse_with(main, &[], &[], &[]).unwrap()
    }

    // QUIRK D-323, D-324: kept
    #[test]
    fn dispatcher_takes_the_first_substring_match() {
        // Value = second word of the line (`sscanf("%s %s")`), keyword anywhere.
        assert_eq!(parse("BaseTexture WATER x").base_texture, "WATER");
        assert_eq!(parse("x BaseTexture WATER").base_texture, "BaseTexture");
        // Lowercase `o`: not a keyword, the block lines fall through (D-323).
        let script = parse("place_object PALM2\n{\nnumber 5\n}\n");
        assert!(script.placements.is_empty());
        // Orphaned block lines and braces have no effect.
        let script = parse("number 5\n{\n}\nsomething\n");
        assert_eq!(script, RandomMapScript::default());
        // With an uppercase `O` a record is created.
        let script = parse("place_Object PALM2\n{\nnumber 5\n}\n");
        assert_eq!(script.placements.len(), 1);
        assert_eq!(script.placements[0].name, "PALM2");
        assert_eq!(script.placements[0].number, 5);
        // The order decides: `ProjectFile` comes before `#const`.
        let script = parse("ProjectFile \"a.prj\" #const X 1\n");
        assert_eq!(script.project_file.as_deref(), Some("a.prj"));
        assert!(script.consts.is_empty());
    }

    #[test]
    fn simple_keys_write_the_generator_fields() {
        let script = parse(
            "SmoothFactor 4\nPathNormal 0.5\nBuildinglotXSize 3\nBuildinglotYSize 5\n\
             BuildinglotTexture GRAS\nBaseTexture WATER\n",
        );
        assert_eq!(script.smooth_factor, 4);
        assert_eq!(script.path_normal, 0.5);
        assert_eq!(script.terrain.path_normal, 0.5);
        assert_eq!(script.buildinglot_size, [3, 5]);
        assert_eq!(script.buildinglot_texture, "GRAS");
        assert_eq!(script.base_texture, "WATER");
        // Without lines the defaults of the constructor.
        let script = parse("");
        assert_eq!(script.smooth_factor, 1);
        assert_eq!(script.path_normal, 0.0);
        assert_eq!(script.buildinglot_size, [2, 2]);
        // No value: field unchanged; PathNormal is copied into the terrain anyway (0x00545F07).
        let script = parse("SmoothFactor\nSmoothFactor x\nPathNormal 0.25\nPathNormal\n");
        assert_eq!(script.smooth_factor, 1);
        assert_eq!(script.terrain.path_normal, 0.25);
        // Overflow wraps modulo 2^32.
        assert_eq!(parse("SmoothFactor 99999999999").smooth_factor, 1215752191);
    }

    // QUIRK D-350: deviation
    #[test]
    fn const_define_and_include_are_read_by_the_dispatcher() {
        let script = parse_with(
            "#const SIZE 5 extra\n#define CARIBBEAN\nBuildinglotXSize $SIZE\n",
            &[],
            &["A"],
            &[],
        )
        .unwrap();
        assert_eq!(script.consts, vec![("SIZE".to_string(), "5".to_string())]);
        assert_eq!(
            script.defines,
            vec!["A".to_string(), "CARIBBEAN".to_string()]
        );
        assert_eq!(script.buildinglot_size[0], 5);
        // Include relative to the main script, also from within an include.
        let script = parse_with(
            "#include \"sub.inc\"\nBaseTexture A\n",
            &[
                (
                    "data/randommapscripts/sub.inc",
                    "SmoothFactor 7\n#include \"deep.inc\"\n",
                ),
                ("data/randommapscripts/deep.inc", "BuildinglotYSize 9\n"),
            ],
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(script.smooth_factor, 7);
        assert_eq!(script.buildinglot_size[1], 9);
        assert_eq!(script.base_texture, "A");
        assert!(script.missing_includes.is_empty());
        // Missing includes (also without two quotation marks) are silently ignored by the original.
        let script = parse("#include \"nope.inc\"\n#include nope.inc\nBaseTexture B\n");
        assert_eq!(script.base_texture, "B");
        assert_eq!(
            script.missing_includes,
            vec![
                "data/randommapscripts/nope.inc".to_string(),
                "data/randommapscripts/".to_string()
            ]
        );
    }

    // QUIRK D-350: deviation
    #[test]
    fn project_file_name_sits_between_first_and_last_quote() {
        assert_eq!(
            parse("ProjectFile \"Data\\Projects\\x.prj\"")
                .project_file
                .as_deref(),
            Some("Data\\Projects\\x.prj")
        );
        // Fewer than two `"`: empty name (`CString::Mid` clamps, FUN_00541D10).
        assert_eq!(
            parse("ProjectFile Data\\x.prj").project_file.as_deref(),
            Some("")
        );
        assert_eq!(
            parse("ProjectFile \"Data\\x.prj").project_file.as_deref(),
            Some("")
        );
        assert_eq!(parse("ProjectFile \"\"").project_file.as_deref(), Some(""));
        // Without the line it stays `None`; the last line counts.
        assert_eq!(parse("").project_file, None);
        assert_eq!(
            parse("ProjectFile \"a\"\nProjectFile \"b\"\n")
                .project_file
                .as_deref(),
            Some("b")
        );
    }

    // QUIRK D-323, D-324: kept
    #[test]
    fn area_block_reads_keys_in_order_and_flags_without_value() {
        let script = parse(
            "create_LandArea\n{\nID 3\nReplace GRASS\nTexture WATER\nminSize 4\nnumTiles 0x10\n\
             Percent 60\nLeftBorder 1\nRightBorder 2\nTopBorder 3\nBottomBorder 4\n\
             BorderRoughness 5\nAvoidRoughness 6\nAvoidOther 7\nPutOnto SAND\n\
             AvoidPlayerAreas\nScaleToPlayers 0\nScaleToMapSize\nScaleClumpsToMapSize\n\
             SharePlayerLands\nunknown 9\n}\n",
        );
        assert_eq!(script.land_areas.len(), 1);
        assert_eq!(
            script.land_areas[0],
            AreaDef {
                id: 3,
                replace: "GRASS".into(),
                texture: "WATER".into(),
                put_onto: "SAND".into(),
                min_size: 4,
                num_tiles: 16,
                percent: 60,
                left: 1,
                right: 2,
                top: 3,
                bottom: 4,
                border_roughness: 5,
                avoid_roughness: 6,
                // `numClumps` is missing: default 1.
                num_clumps: 1,
                avoid_other: 7,
                avoid_player_areas: true,
                // A value after the flag does not count: the word alone sets it (D-324).
                scale_to_players: true,
                scale_to_map_size: true,
                scale_clumps_to_map_size: true,
                share_player_lands: true,
            }
        );
        // Comment remainders are dropped; an unreadable value leaves the field alone.
        let script = parse("create_TerrainArea\nnumClumps 3 ;x\nID abc\n}\n");
        assert_eq!(script.terrain_areas[0].num_clumps, 3);
        assert_eq!(script.terrain_areas[0].id, 0);
        // In the `place_Object` block the typo has no effect (D-323).
        let script = parse("place_Object P\nminGroupDisatnce 5\nmaxGroupRadius 4\n}\n");
        assert_eq!(script.placements[0].min_group_distance, 0);
        assert_eq!(script.placements[0].max_group_radius, 4);
    }

    // QUIRK D-324: kept
    #[test]
    fn block_ends_only_at_exactly_a_closing_brace() {
        // `}// x` becomes `}` and ends; `} ;x` becomes `} ` and does not end.
        let script = parse("create_LandArea\nnumTiles 5\n}// x\nnumTiles 9\n");
        assert_eq!(script.land_areas[0].num_tiles, 5);
        let script = parse("create_LandArea\nnumTiles 5\n} ;x\nnumTiles 9\n}\n");
        assert_eq!(script.land_areas[0].num_tiles, 9);
        // A `{` is meaningless, and the head line itself does not count as an end.
        let script = parse("create_LandArea {\n{\nnumTiles 2\n}\n");
        assert_eq!(script.land_areas[0].num_tiles, 2);
    }

    // QUIRK D-324: kept
    #[test]
    fn player_area_block_is_kept_as_template() {
        let script = parse(
            "create_PlayerAreas\n{\nTexture A\n}\ncreate_PlayerAreas\n{\nTexture B\n}\n\
             create_LandArea\n{\nTexture C\n}\n",
        );
        assert_eq!(script.player_areas.len(), 2);
        assert_eq!(script.player_areas[0].texture, "A");
        assert_eq!(script.player_areas[1].texture, "B");
        assert_eq!(script.land_areas.len(), 1);
        assert!(script.terrain_areas.is_empty());
    }

    #[test]
    fn texture_numbers_count_in_definition_order() {
        let script = parse(
            "#Texture A\n{\nHEIGHT -12.5\nGUID 30\nGUID 31\nGUID 30\n}\n\
             #Texture B\n{\n}\n#Texture A\n{\nGUID 7\n}\n\
             #Object TREE\n{\nGUID 100\nGUID 100\n}\n",
        );
        let numbers: Vec<(i32, &str)> = script
            .textures
            .iter()
            .map(|t: &TextureDef| (t.number, t.name.as_str()))
            .collect();
        assert_eq!(numbers, [(1, "A"), (2, "B"), (3, "A")]);
        assert_eq!(script.textures[0].height, -12.5);
        // Duplicate GUIDs stay.
        assert_eq!(script.textures[0].guids, vec![30, 31, 30]);
        assert!(script.textures[1].guids.is_empty());
        assert_eq!(script.textures[1].height, 0.0);
        assert_eq!(script.objects.len(), 1);
        assert_eq!(script.objects[0].name, "TREE");
        assert_eq!(script.objects[0].guids, vec![100, 100]);
    }

    #[test]
    fn connect_block_reads_texture_put_onto_and_radius() {
        let script = parse("connect_Players\n{\nTexture ROAD\nPutOnto GRASS\nradius 3\n}\n");
        assert_eq!(
            script.connections,
            vec![ConnectDef {
                texture: "ROAD".into(),
                put_onto: "GRASS".into(),
                radius: 3
            }]
        );
    }

    #[test]
    fn place_object_block_reads_all_keys() {
        let script = parse(
            "place_Object TREE\n{\nnumber 0x10\nnumGroups 3\nmaxGroupRadius 4\nminGroupDistance 5\n\
             ForAllPlayers\nIsPlayer\nPutOnFlat\nminDistToPlayers 6\nmaxDistToPlayers 7\n\
             AvoidOther 8\nPutOnto SAND\nFill\nScaleToPlayers\nScaleToMapSize\n\
             ScaleGroupsToMapSize\nNoGrid\nAvoidPlayerAreas\nGroundTexture MOSS\n}\n",
        );
        assert_eq!(
            script.placements,
            vec![PlaceObjectDef {
                name: "TREE".into(),
                number: 16,
                num_groups: 3,
                max_group_radius: 4,
                min_group_distance: 5,
                for_all_players: true,
                is_player: true,
                put_on_flat: true,
                min_dist_to_players: 6,
                max_dist_to_players: 7,
                avoid_other: 8,
                put_onto: "SAND".into(),
                fill: true,
                scale_to_players: true,
                scale_to_map_size: true,
                scale_groups_to_map_size: true,
                no_grid: true,
                avoid_player_areas: true,
                ground_texture: "MOSS".into(),
            }]
        );
        // Defaults: number 1, numGroups 1.
        let script = parse("place_Object X\n}\n");
        assert_eq!(script.placements[0].number, 1);
        assert_eq!(script.placements[0].num_groups, 1);
    }

    // QUIRK D-26: kept
    // QUIRK D-345: deviation
    #[test]
    fn light_and_water_blocks_write_terrain_params() {
        let script = parse(
            "TerrainLight\n{\nPOSITION 0.5 -1 2\nCOLOR 300 2 3\nINTENSITY 80\n}\n\
             Water\n{\nWaterLevel -10\nUseTexture 1\nBaseTextureGUID 34\n\
             RefractionTextureGUID 35\nTiling 8\nMaxDepth 300\nMinAlpha 1\nMaxAlpha 220\n}\n",
        );
        let t = &script.terrain;
        assert_eq!(t.light_position, [0.5, -1.0, 2.0]);
        // COLOR: only the lower byte per channel (0x00543A9E...).
        assert_eq!(t.light_color, [44, 2, 3]);
        assert_eq!(t.light_intensity, 80);
        assert_eq!(t.water_level, -10.0);
        assert!(t.water_use_texture);
        assert_eq!(t.water_base_texture, 34);
        assert_eq!(t.water_refraction_texture, 35);
        assert_eq!(t.water_texture_tiling, 8);
        assert_eq!(t.water_max_depth, 300);
        assert_eq!(t.water_min_alpha, 1);
        assert_eq!(t.water_max_alpha, 220);
        // Without a block the defaults; missing object/shadow block: None (D-345).
        let script = parse("");
        assert_eq!(script.terrain, crate::wld::TerrainParams::default());
        assert_eq!(script.object_light, None);
        assert_eq!(script.shadow, None);
        // Missing channels of COLOR: 0 (A7); negative values: lower byte.
        assert_eq!(
            parse("TerrainLight\nCOLOR 5\n}\n").terrain.light_color,
            [5, 0, 0]
        );
        assert_eq!(
            parse("TerrainLight\nCOLOR\n}\n").terrain.light_color,
            [0, 0, 0]
        );
        assert_eq!(
            parse("TerrainLight\nCOLOR -1 256 255\n}\n")
                .terrain
                .light_color,
            [255, 0, 255]
        );
        // Missing numbers in POSITION leave the remainders alone.
        assert_eq!(
            parse("TerrainLight\nPOSITION 3 4\n}\n")
                .terrain
                .light_position,
            [3.0, 4.0, 0.0]
        );
    }

    // QUIRK D-345: deviation
    #[test]
    fn object_and_shadow_light_start_at_zero_when_the_block_exists() {
        let script = parse(
            "ObjectLight\n{\nPOSITION 1 2 3\nCOLOR 255 128 0\nINTENSITY 90\n}\n\
             ShadowLight\n{\nPOSITION 1 2 3\n}\n",
        );
        assert_eq!(
            script.object_light,
            Some(ObjectLight {
                position: [1.0, 2.0, 3.0],
                color: [255, 128, 0],
                intensity: 90
            })
        );
        // Missing key: intensity 0 (D-345).
        assert_eq!(
            script.shadow,
            Some(ShadowParams {
                light_position: [1.0, 2.0, 3.0],
                intensity: 0
            })
        );
        // An empty block creates the value with 0.
        let script = parse("ObjectLight\n}\nShadowLight\n}\n");
        assert_eq!(
            script.object_light,
            Some(ObjectLight {
                position: [0.0; 3],
                color: [0; 3],
                intensity: 0
            })
        );
        assert_eq!(
            script.shadow,
            Some(ShadowParams {
                light_position: [0.0; 3],
                intensity: 0
            })
        );
        // A second block writes into the same value.
        let script = parse("ShadowLight\nINTENSITY 5\n}\nShadowLight\nPOSITION 1 1 1\n}\n");
        assert_eq!(
            script.shadow,
            Some(ShadowParams {
                light_position: [1.0; 3],
                intensity: 5
            })
        );
    }

    #[test]
    fn preprocessor_decides_what_the_dispatcher_sees() {
        let script = parse_with(
            "#ifdef A\nBaseTexture X\n#else\nBaseTexture Y\n#endif\n",
            &[],
            &["A"],
            &[],
        )
        .unwrap();
        assert_eq!(script.base_texture, "X");
        let script = parse("#ifdef A\nBaseTexture X\n#else\nBaseTexture Y\n#endif\n");
        assert_eq!(script.base_texture, "Y");
    }

    #[test]
    fn random_hook_reaches_the_preprocessor() {
        // Roll 170 -> 70 (`% 100`): `#chance 50` misses, the second branch hits.
        let text = "#random
#chance 50
BaseTexture A
#chance 50
BaseTexture B
#endrandom
";
        let script = parse_with(text, &[], &[], &[170]).unwrap();
        assert_eq!(script.base_texture, "B");
        // Roll 30 hits the first branch; afterwards the roll is 101 and the second one misses.
        let script = parse_with(text, &[], &[], &[30]).unwrap();
        assert_eq!(script.base_texture, "A");
    }

    // QUIRK D-324: kept
    #[test]
    fn end_of_file_inside_a_block_is_an_error() {
        let error = parse_with("create_LandArea\n{\nID 3\n", &[], &[], &[]).unwrap_err();
        let text = error.to_string();
        assert!(text.contains(MAIN), "{text}");
        assert!(text.contains("line 3"), "{text}");
        assert!(
            text.contains("end of file in block create_LandArea"),
            "{text}"
        );
        // Also in the include.
        let error = parse_with(
            "#include \"sub.inc\"\n",
            &[("data/randommapscripts/sub.inc", "Water\nWaterLevel 1\n")],
            &[],
            &[],
        )
        .unwrap_err();
        assert!(error.to_string().contains("sub.inc"), "{error}");
        assert!(
            error.to_string().contains("end of file in block Water"),
            "{error}"
        );
        // All block kinds report it.
        for head in [
            "#Texture A",
            "#Object A",
            "TerrainLight",
            "ObjectLight",
            "ShadowLight",
            "create_PlayerAreas",
            "create_TerrainArea",
            "place_Object A",
            "connect_Players",
        ] {
            let text = parse_with(&format!("{head}\n{{\n"), &[], &[], &[])
                .unwrap_err()
                .to_string();
            assert!(text.contains("end of file in block"), "{head}: {text}");
        }
    }

    #[test]
    fn missing_main_script_is_an_error() {
        let mut read = |_: &str| None;
        let mut random = || 0;
        let error =
            parse_random_map_script("data/x/none.rnd", vec![], &mut read, &mut random).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Map script data/x/none.rnd is missing"),
            "{error}"
        );
    }
}
