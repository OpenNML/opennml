//! Project file (catalog of the level editor): object types, ground textures, vertex colors, see
//! `docs/formats/prj.md`. Read like `ProjectFile_ParseObjects` (`0x004FBE50`).

use std::collections::BTreeMap;

use crate::FormatError;
use crate::text::{Node, find_all, parse_blocks};

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectObject {
    pub guid: u32,
    /// `OBJ_NAME` (display name in the editor).
    pub name: String,
    /// Model path by the game rule: `OBJ_GAME_FILENAME` if longer than 1 character, otherwise
    /// `OBJ_FILENAME` with extension `.rdo`. As written in the file (separator `\`).
    pub model: String,
    /// `OBJ_SHADOW_FILENAME` with extension `.rdo`; `None` if empty.
    pub shadow_model: Option<String>,
    /// `OBJ_GROUNDPLANE_FILENAME` as written (extension stays); `None` if empty.
    pub groundplane_model: Option<String>,
    /// `OBJ_SCAFFOLD_FILENAME` as written; `None` if empty (ship flag of the shadows,
    /// re/shadows).
    pub scaffold_model: Option<String>,
    /// `OBJ_ONWATER`.
    pub on_water: bool,
    /// `OBJ_XROT/YROT/ZROT` of the type (radians; not applied by the game when placing).
    pub default_rotation: [f32; 3],
    /// `OBJ_BLOCK_PATH == 1` (`GameObject+0x784`, `GameObject_LoadModel` `0x00469E60`): only the
    /// nature module evaluates it.
    pub block_path: bool,
    /// `OBJ_RANDOM_ROT != 0` (`%i` after project object `+0x3C`, `ProjectFile_ParseObjects`
    /// `0x004FC9AA`): the random map placer rotates the object randomly about the vertical axis
    /// (`FUN_0053FD00` `0x0053FD96`, `TEST EAX,EAX`, every value != 0). If the key is missing, the
    /// field stays uninitialized in the original (heap leftover); the reimplementation takes `false`.
    pub random_rot: bool,
    /// Selection marker on the ground (`OBJ_SELECTION_*`, re/renderer, ground markers).
    pub selection: SelectionMarker,
    /// `OBJ_UPGRADES { UPG_GUID n ... }`: upgrades that affect this type, in file order
    /// (re/upgrades 1.1, 10.1); empty without a block.
    pub upgrades: Vec<u32>,
}

/// `[UPGRADE]` (`UPG_NAME`, `UPG_GUID`): entry of the upgrade list of the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectUpgrade {
    pub guid: u32,
    pub name: String,
}

/// `OBJ_SELECTION_FILENAME/RADIUS/USE_BBOX` (project object `+0x18/+0x1C/+0x20`,
/// `ProjectFile_ParseObjects` `0x004FC4E7...0x004FC5C6`). If a key is missing, 0 or empty applies (in the
/// original the field stays uninitialized; in the shipped data it is present for all 512 types).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelectionMarker {
    /// Texture of the ring or frame as written; `None` if at most 1 character long
    /// (`ObjectRenderer_Construct` `0x00549860` creates the decal only for length > 1), then the
    /// game draws a line rectangle.
    pub texture: Option<String>,
    /// Half edge length of the square (`%f`).
    pub radius: f32,
    /// `!= 0` (`%i`): frame around the model hull (`FUN_0054C2D0`) instead of a square.
    pub use_bbox: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectTexture {
    pub guid: u32,
    pub name: String,
    pub file: String,
    /// `TEX_BITS` (bit 0 tracks, bit 1 impassable).
    pub bits: u32,
    /// `TEX_GROUP_ID`, 0 = no group.
    pub group: u32,
    /// `TEX_MAP_COLOR` as COLORREF `0x00BBGGRR`.
    pub map_color: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectColor {
    pub guid: u32,
    pub name: String,
    /// `COLOR_DIFFUSE` as COLORREF `0x00BBGGRR`.
    pub diffuse: u32,
}

impl ProjectColor {
    pub fn rgb(&self) -> [u8; 3] {
        colorref_rgb(self.diffuse)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Project {
    pub objects: BTreeMap<u32, ProjectObject>,
    pub textures: BTreeMap<u32, ProjectTexture>,
    pub colors: BTreeMap<u32, ProjectColor>,
    /// `[UPGRADE]` blocks by `UPG_GUID`.
    pub upgrades: BTreeMap<u32, ProjectUpgrade>,
}

/// COLORREF `0x00BBGGRR` -> `[R, G, B]`.
pub fn colorref_rgb(colorref: u32) -> [u8; 3] {
    [
        colorref as u8,
        (colorref >> 8) as u8,
        (colorref >> 16) as u8,
    ]
}

/// Reads the project file. `[OBJECT]`, `[TEXTURE]`, `[COLOR]` and `[UPGRADE]` are searched at every depth;
/// a later GUID overwrites an earlier one (like `CMap::SetAt`). Editor blocks are
/// ignored.
pub fn parse_project(text: &str) -> Result<Project, FormatError> {
    let nodes = parse_blocks(text)?;
    let mut project = Project::default();
    for node in find_all(&nodes, "[OBJECT]") {
        let object = parse_object(node)?;
        project.objects.insert(object.guid, object);
    }
    for node in find_all(&nodes, "[TEXTURE]") {
        let texture = ProjectTexture {
            guid: required(node, "TEX_GUID")?,
            name: text_value(node, "TEX_NAME")?.unwrap_or_default(),
            file: text_value(node, "TEX_FILENAME")?.unwrap_or_default(),
            bits: optional(node, "TEX_BITS")?.unwrap_or(0),
            group: optional(node, "TEX_GROUP_ID")?.unwrap_or(0),
            map_color: optional(node, "TEX_MAP_COLOR")?.unwrap_or(0),
        };
        project.textures.insert(texture.guid, texture);
    }
    for node in find_all(&nodes, "[COLOR]") {
        let color = ProjectColor {
            guid: required(node, "COLOR_GUID")?,
            name: text_value(node, "COLOR_NAME")?.unwrap_or_default(),
            diffuse: optional(node, "COLOR_DIFFUSE")?.unwrap_or(0),
        };
        project.colors.insert(color.guid, color);
    }
    for node in find_all(&nodes, "[UPGRADE]") {
        let upgrade = ProjectUpgrade {
            guid: required(node, "UPG_GUID")?,
            name: text_value(node, "UPG_NAME")?.unwrap_or_default(),
        };
        project.upgrades.insert(upgrade.guid, upgrade);
    }
    Ok(project)
}

fn parse_object(node: &Node) -> Result<ProjectObject, FormatError> {
    let file = text_value(node, "OBJ_FILENAME")?.unwrap_or_default();
    let game_file = text_value(node, "OBJ_GAME_FILENAME")?.unwrap_or_default();
    // `0x004FC207`: replace the extension with `.rdo`; `0x004FC2F5`: OBJ_GAME_FILENAME overrides
    // unchanged if longer than 1 character.
    let model = if game_file.chars().count() > 1 {
        game_file
    } else if file.is_empty() {
        String::new()
    } else {
        replace_extension(&file, ".rdo")
    };
    let shadow = text_value(node, "OBJ_SHADOW_FILENAME")?.unwrap_or_default();
    let non_empty = |key: &str| -> Result<Option<String>, FormatError> {
        Ok(text_value(node, key)?.filter(|v| !v.is_empty()))
    };
    Ok(ProjectObject {
        guid: required(node, "OBJ_GUID")?,
        name: text_value(node, "OBJ_NAME")?.unwrap_or_default(),
        model,
        shadow_model: (!shadow.is_empty()).then(|| replace_extension(&shadow, ".rdo")),
        groundplane_model: non_empty("OBJ_GROUNDPLANE_FILENAME")?,
        scaffold_model: non_empty("OBJ_SCAFFOLD_FILENAME")?,
        on_water: optional::<i32>(node, "OBJ_ONWATER")?.unwrap_or(0) != 0,
        default_rotation: [
            optional(node, "OBJ_XROT")?.unwrap_or(0.0),
            optional(node, "OBJ_YROT")?.unwrap_or(0.0),
            optional(node, "OBJ_ZROT")?.unwrap_or(0.0),
        ],
        block_path: optional::<i32>(node, "OBJ_BLOCK_PATH")?.unwrap_or(0) == 1,
        random_rot: optional::<i32>(node, "OBJ_RANDOM_ROT")?.unwrap_or(0) != 0,
        selection: SelectionMarker {
            texture: text_value(node, "OBJ_SELECTION_FILENAME")?.filter(|t| t.chars().count() > 1),
            radius: optional(node, "OBJ_SELECTION_RADIUS")?.unwrap_or(0.0),
            use_bbox: optional::<i32>(node, "OBJ_SELECTION_USE_BBOX")?.unwrap_or(0) != 0,
        },
        upgrades: match node.child("OBJ_UPGRADES") {
            Some(block) => block
                .children_named("UPG_GUID")
                .map(|g| g.parse_arg(0))
                .collect::<Result<_, _>>()?,
            None => Vec::new(),
        },
    })
}

pub(crate) fn required<T: std::str::FromStr>(node: &Node, key: &str) -> Result<T, FormatError> {
    match node.child(key) {
        Some(child) => child.parse_arg(0),
        None => Err(crate::error::syntax(
            node.line,
            &format!("{}: {key} missing", node.name()),
        )),
    }
}

pub(crate) fn optional<T: std::str::FromStr>(
    node: &Node,
    key: &str,
) -> Result<Option<T>, FormatError> {
    node.child(key).map(|child| child.parse_arg(0)).transpose()
}

pub(crate) fn text_value(node: &Node, key: &str) -> Result<Option<String>, FormatError> {
    node.child(key)
        .map(|child| child.str_arg(0).map(str::to_string))
        .transpose()
}

/// `Path_ReplaceExtension` (`0x004FBBF0`): replace the extension after the last dot of the file name
/// or append one.
pub fn replace_extension(path: &str, ext: &str) -> String {
    let name_start = path.rfind(['\\', '/']).map_or(0, |i| i + 1);
    match path[name_start..].rfind('.') {
        Some(dot) => format!("{}{ext}", &path[..name_start + dot]),
        None => format!("{path}{ext}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "PROJECTFILE V1.08f\r\n\r\n[OBJECTFOLDER]\r\n{\r\n\tOBJ_FOLDER_NAME \"Alle\"\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_NAME \"Dragoner\"\r\n\tOBJ_GUID 3001\r\n\tOBJ_FILENAME \"data\\r3d\\eng\\dragoner_eng_zu_pferd.rdw\"\r\n\tOBJ_GAME_FILENAME \"data\\r3d\\eng\\dragoner_eng.rdo\"\r\n\tOBJ_SHADOW_FILENAME \"data\\r3d\\misc\\schatten.rdw\"\r\n\tOBJ_GROUNDPLANE_FILENAME \"data\\r3d\\ground\\dragoner.rdo\"\r\n\tOBJ_SCAFFOLD_FILENAME \"\"\r\n\tOBJ_SELECTION_FILENAME \"data\\textures\\selektion\\einheiten_kreis.tga\"\r\n\tOBJ_SELECTION_RADIUS 35.000000\r\n\tOBJ_SELECTION_USE_BBOX 0\r\n\tOBJ_YROT 1.5\r\n\tOBJ_ONWATER 0\r\n\tOBJ_UPGRADES\r\n\t{\r\n\t\tUPG_GUID 5\r\n\t}\r\n}\r\n[OBJECT]\r\n{\r\n\tOBJ_NAME \"Palme\"\r\n\tOBJ_GUID 3639\r\n\tOBJ_FILENAME \"data\\r3d\\natur\\palme.rdw\"\r\n\tOBJ_GAME_FILENAME \"\"\r\n\tOBJ_SHADOW_FILENAME \"\"\r\n\tOBJ_SELECTION_FILENAME \"\"\r\n\tOBJ_SELECTION_RADIUS 1.000000\r\n\tOBJ_SELECTION_USE_BBOX 2\r\n\tOBJ_ONWATER 1\r\n\tOBJ_BLOCK_PATH 1\r\n}\r\n[TEXTURE]\r\n{\r\n\tTEX_NAME \"Sand\"\r\n\tTEX_GUID 5\r\n\tTEX_FILENAME \"Data\\Textures\\Sand.tga\"\r\n\tTEX_GROUP_ID 12\r\n\tTEX_BITS 1\r\n\tTEX_MAP_COLOR 4160666\r\n}\r\n[COLOR]\r\n{\r\n\tCOLOR_NAME \"Blau\"\r\n\tCOLOR_GUID 16\r\n\tCOLOR_DIFFUSE 16213337\r\n}\r\n[UPGRADE]\r\n{\r\n\tUPG_NAME \"Holz\"\r\n\tUPG_GUID 2800\r\n}\r\n";

    #[test]
    fn random_rot_is_read_from_the_project() {
        let object = |extra: &str| {
            format!(
                "PROJECTFILE V1.08f
[OBJECT]
{{
	OBJ_NAME \"A\"
	OBJ_GUID 7
	OBJ_FILENAME \"a.rdw\"
{extra}}}
"
            )
        };
        let rot = |extra: &str| parse_project(&object(extra)).unwrap().objects[&7].random_rot;
        assert!(rot("	OBJ_RANDOM_ROT 1
"));
        assert!(rot("	OBJ_RANDOM_ROT 2
"));
        assert!(!rot("	OBJ_RANDOM_ROT 0
"));
        assert!(!rot(""));
    }

    #[test]
    fn parses_objects_textures_and_colors() {
        let p = parse_project(SAMPLE).unwrap();
        let dragoner = &p.objects[&3001];
        assert_eq!(dragoner.model, r"data\r3d\eng\dragoner_eng.rdo");
        assert_eq!(
            dragoner.shadow_model.as_deref(),
            Some(r"data\r3d\misc\schatten.rdo")
        );
        assert_eq!(dragoner.default_rotation, [0.0, 1.5, 0.0]);
        assert_eq!(
            dragoner.groundplane_model.as_deref(),
            Some(r"data\r3d\ground\dragoner.rdo")
        );
        assert_eq!(dragoner.scaffold_model, None);
        let palme = &p.objects[&3639];
        assert_eq!(palme.model, r"data\r3d\natur\palme.rdo");
        assert_eq!(palme.shadow_model, None);
        assert_eq!(palme.groundplane_model, None);
        assert!(palme.on_water);
        assert!(palme.block_path && !dragoner.block_path);
        assert_eq!(
            dragoner.selection,
            SelectionMarker {
                texture: Some(r"data\textures\selektion\einheiten_kreis.tga".into()),
                radius: 35.0,
                use_bbox: false,
            }
        );
        // Empty name: no texture (line rectangle); every value != 0 switches the frame on.
        assert_eq!(
            palme.selection,
            SelectionMarker {
                texture: None,
                radius: 1.0,
                use_bbox: true,
            }
        );
        let sand = &p.textures[&5];
        assert_eq!(
            (sand.group, sand.bits, sand.file.as_str()),
            (12, 1, r"Data\Textures\Sand.tga")
        );
        assert_eq!(p.colors[&16].rgb(), [89, 101, 247]);
    }

    #[test]
    fn parses_obj_upgrades_and_upgrade_sections() {
        // The Dragoner in `SAMPLE` carries a block with one GUID, the Palme none; plus an
        // object with two GUIDs in file order; `SAMPLE` ends with a `[UPGRADE]` block.
        let text = format!(
            "{SAMPLE}[OBJECT]\n{{\n\tOBJ_GUID 3002\n\tOBJ_UPGRADES\n\t{{\n\t\tUPG_GUID 2800\n\t\tUPG_GUID 2801\n\t}}\n}}\n"
        );
        let p = parse_project(&text).unwrap();
        assert_eq!(p.objects[&3002].upgrades, [2800, 2801]);
        assert_eq!(p.objects[&3001].upgrades, [5]);
        assert!(p.objects[&3639].upgrades.is_empty());
        assert_eq!(
            p.upgrades[&2800],
            ProjectUpgrade {
                guid: 2800,
                name: "Holz".into()
            }
        );
        assert_eq!(p.upgrades.len(), 1);
        // An empty block (`OBJ_UPGRADES { }`) also yields no upgrades.
        let empty = "[OBJECT]\n{\nOBJ_GUID 7\nOBJ_UPGRADES\n{\n}\n}\n";
        assert!(
            parse_project(empty).unwrap().objects[&7]
                .upgrades
                .is_empty()
        );
    }

    #[test]
    fn replace_extension_like_the_game() {
        assert_eq!(replace_extension(r"a\b.rdw", ".rdo"), r"a\b.rdo");
        assert_eq!(replace_extension(r"a.x\b", ".rdo"), r"a.x\b.rdo");
        assert_eq!(replace_extension("b.tar.gz", ".rdo"), "b.tar.rdo");
    }

    #[test]
    fn object_without_guid_is_an_error() {
        let text = "[OBJECT]\n{\nOBJ_NAME \"x\"\n}\n";
        assert!(matches!(
            parse_project(text),
            Err(FormatError::Syntax { line: 1, .. })
        ));
    }
}
