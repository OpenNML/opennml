//! Object `.cfg`: animations and sub-objects of a model, see `docs/formats/object-cfg.md`.
//! The game reads it line by line with a substring search (`ObjectCfg_ParseText` `0x0059C960`);
//! here the first word of the line is compared - equivalent for the shipped data.

use crate::FormatError;
use crate::text::{Node, parse_blocks};

/// Sub-object that runs along (at object level always, at animation level only in this animation).
#[derive(Debug, Clone, PartialEq)]
pub struct SubObject {
    /// Model path as in the file, lowercase (`CString_MakeLowerGerman`).
    pub file: String,
    /// Animation in the `.cfg` of the sub-object that runs in parallel (`-1` = none).
    pub index: i32,
    pub start: i32,
    pub looping: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnimationEntry {
    /// `.rdm` path, lowercase; `None` if the block has no `FILE`.
    pub file: Option<String>,
    pub guid: Option<i32>,
    /// `min.x min.y min.z max.x max.y max.z`; tool artifact, not reliable in object space.
    pub max_bounding_box: Option<[f32; 6]>,
    pub sub_objects: Vec<SubObject>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectConfig {
    /// `OBJECT`/`FILE` - **not** read by the game, only for checks.
    pub model_file: Option<String>,
    pub sub_objects: Vec<SubObject>,
    /// Animations in file order; the index is the animation index of the game.
    pub animations: Vec<AnimationEntry>,
}

/// Reads the `OBJECT` block of an object `.cfg`. Unknown keys are ignored;
/// `LODENABLED`, `CASTSHADOW`, `UPGRADEID`, `TEXTUREUPGRADE` and `TIMELABEL` are not needed here.
/// Without an `OBJECT` block the configuration is empty as in the game: `ObjectCfg_ParseText`
/// (`0x0059C960`) skips all lines outside `OBJECT` and reports success (e.g. the
/// exporter companion file `data/rdm/kanone_spa_stehen.cfg`).
pub fn parse_object_cfg(text: &str) -> Result<ObjectConfig, FormatError> {
    let nodes = parse_blocks(text)?;
    let Some(object) = nodes
        .iter()
        .find(|n| n.name().eq_ignore_ascii_case("OBJECT"))
    else {
        return Ok(ObjectConfig::default());
    };
    let mut config = ObjectConfig {
        model_file: file_arg(object)?,
        ..ObjectConfig::default()
    };
    for node in object.children() {
        if node.name().eq_ignore_ascii_case("SUBOBJECT") {
            config.sub_objects.extend(parse_sub_object(node)?);
        } else if node.name().eq_ignore_ascii_case("ANIMATION") {
            config.animations.push(parse_animation(node)?);
        }
    }
    Ok(config)
}

/// `FILE "..."` of a block, lowercase.
fn file_arg(node: &Node) -> Result<Option<String>, FormatError> {
    node.child("FILE")
        .map(|f| f.str_arg(0).map(str::to_lowercase))
        .transpose()
}

/// The game creates the link only if `FILE` can be loaded; without `FILE` -> `None`.
fn parse_sub_object(node: &Node) -> Result<Option<SubObject>, FormatError> {
    let Some(file) = file_arg(node)? else {
        return Ok(None);
    };
    let int = |key: &str, default: i32| -> Result<i32, FormatError> {
        node.child(key).map_or(Ok(default), |n| n.parse_arg(0))
    };
    Ok(Some(SubObject {
        file,
        index: int("INDEX", -1)?,
        start: int("START", 0)?,
        looping: int("LOOP", 1)? != 0,
    }))
}

fn parse_animation(node: &Node) -> Result<AnimationEntry, FormatError> {
    let mut sub_objects = Vec::new();
    for child in node.children_named("SUBOBJECT") {
        sub_objects.extend(parse_sub_object(child)?);
    }
    Ok(AnimationEntry {
        file: file_arg(node)?,
        guid: node.child("GUID").map(|n| n.parse_arg(0)).transpose()?,
        max_bounding_box: node
            .child("MAXBOUNDINGBOX")
            .map(|n| n.parse_args::<f32, 6>())
            .transpose()?,
        sub_objects,
    })
}

/// Path of the object `.cfg` for an `.rdo` like `Model_LoadObjectConfig` (`0x0059DFF0`): same
/// directory, same name with extension `.cfg`, a prefix `lod_` in the file name is dropped.
/// Only for `.rdo` (the game checks the extension); result lowercase, `/` as separator.
pub fn object_cfg_path(rdo_path: &str) -> Option<String> {
    let lower = rdo_path.replace('\\', "/").to_lowercase();
    let stem = lower.strip_suffix(".rdo")?;
    let (dir, name) = match stem.rfind('/') {
        Some(i) => (&stem[..=i], &stem[i + 1..]),
        None => ("", stem),
    };
    let name = name.strip_prefix("lod_").unwrap_or(name);
    Some(format!("{dir}{name}.cfg"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "OBJECT\r\n{\r\n\tFILE \".\\data\\r3d\\wid\\Tierbeschwoerer.rdo\"\r\n\tLODENABLED 0\r\n\tSUBOBJECT\r\n\t{\r\n\t\tFILE \".\\data\\r3d\\wid\\Hut.rdo\"\r\n\t}\r\n\tANIMATION\r\n\t{\r\n\t\tFILE \".\\data\\rdm\\Laufen.rdm\"\r\n\t\tMAXBOUNDINGBOX -1 0 -2 1 137 2\r\n\t\tSUBOBJECT\r\n\t\t{\r\n\t\t\tFILE \".\\data\\r3d\\wid\\Stab.rdo\"\r\n\t\t\tINDEX 1\r\n\t\t\tSTART 0\r\n\t\t\tLOOP 0\r\n\t\t}\r\n\t}\r\n\tANIMATION\r\n\t{\r\n\t\tGUID 7\r\n\t}\r\n}\r\n";

    #[test]
    fn parses_animations_and_sub_objects() {
        let cfg = parse_object_cfg(SAMPLE).unwrap();
        assert_eq!(
            cfg.model_file.as_deref(),
            Some(r".\data\r3d\wid\tierbeschwoerer.rdo")
        );
        assert_eq!(
            cfg.sub_objects,
            [SubObject {
                file: r".\data\r3d\wid\hut.rdo".into(),
                index: -1,
                start: 0,
                looping: true
            }]
        );
        assert_eq!(cfg.animations.len(), 2);
        let laufen = &cfg.animations[0];
        assert_eq!(laufen.file.as_deref(), Some(r".\data\rdm\laufen.rdm"));
        assert_eq!(
            laufen.max_bounding_box,
            Some([-1.0, 0.0, -2.0, 1.0, 137.0, 2.0])
        );
        assert_eq!(laufen.sub_objects[0].index, 1);
        assert!(!laufen.sub_objects[0].looping);
        assert_eq!(cfg.animations[1].file, None);
        assert_eq!(cfg.animations[1].guid, Some(7));
    }

    #[test]
    fn missing_object_block_gives_an_empty_config() {
        // Exporter companion file like `data/rdm/kanone_spa_stehen.cfg`: the game does not read `ANIMATION`
        // outside `OBJECT` (`ObjectCfg_ParseText` `0x0059C960`).
        let exporter = "ANIMATION\r\n{\r\n\tFILE \".\\data\\rdm\\kanone_spa_stehen.rdm\"\r\n\tSPEED 1.0\r\n\tSTARTFRAME 0\r\n\tLOOP 1\r\n\tINTERPFIRSTLAST 1\r\n}\r\n";
        assert_eq!(parse_object_cfg(exporter).unwrap(), ObjectConfig::default());
        assert_eq!(parse_object_cfg("").unwrap(), ObjectConfig::default());
    }

    #[test]
    fn bad_numbers_are_errors() {
        let bad = "OBJECT\n{\nANIMATION\n{\nMAXBOUNDINGBOX 1 2\n}\n}\n";
        assert!(matches!(
            parse_object_cfg(bad),
            Err(FormatError::Syntax { line: 5, .. })
        ));
    }

    #[test]
    fn cfg_path_follows_the_rdo_and_strips_lod_prefix() {
        assert_eq!(
            object_cfg_path("data/r3d/wid/units/lod_bauer.rdo").as_deref(),
            Some("data/r3d/wid/units/bauer.cfg")
        );
        assert_eq!(
            object_cfg_path("Data/R3D/Farm.RDO").as_deref(),
            Some("data/r3d/farm.cfg")
        );
        assert_eq!(object_cfg_path("data/r3d/x.rdw"), None);
    }
}
