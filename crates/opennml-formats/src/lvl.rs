//! Level `.lvl`: project file and players, see `docs/formats/lvl.md`. Starting resources and
//! object configuration (`[RESOURCES]`, `[CONFIGDATA]`, `[UPGRADES]`) are not read here.

use crate::FormatError;
use crate::text::{Node, parse_blocks};

#[derive(Debug, Clone, PartialEq)]
pub struct Player {
    /// `PLAYER_NAME` as stored (e.g. `29081 RED`).
    pub name: String,
    /// Nation: `WID`, `PRI`, `SPA`, `SID`, `ENG`, `PAT`.
    pub nation: String,
    /// `PLAYER_TYPE`: 0 human, 1 AI opponent, 2 passive AI (derived from the shipped data).
    pub kind: u32,
    /// `PLAYER_COLOR`: player slot 0-7, index for `OBJ_COLOR` of the `.wld`.
    pub color: i32,
}

impl Player {
    /// Text ID if the name begins with a number >= 1 other than 29320 (`PlayerSetup_ParseLvl`).
    pub fn name_text_id(&self) -> Option<u32> {
        name_text_id(&self.name)
    }
}

/// Text ID of a `PLAYER_NAME`: `PlayerSetup_ParseLvl` (`0x00486640`) reads the leading number (`_atol`) and replaces
/// the whole name with its text (`FUN_0055FFE0`) if it is >= 1 and not 29320 (placeholder).
pub fn name_text_id(name: &str) -> Option<u32> {
    let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|&id| id >= 1 && id != 29320)
}

#[derive(Debug, Clone, PartialEq)]
pub struct LevelFile {
    /// `PROJECT_FILE` as stored (e.g. `Data\Projects\nml-wiese.prj`).
    pub project_file: String,
    pub players: Vec<Player>,
}

/// Reads the `.lvl` (top level: `PROJECT_FILE`, `[PLAYER]` blocks).
pub fn parse_level(text: &str) -> Result<LevelFile, FormatError> {
    use crate::prj::{optional, required, text_value};
    let nodes = parse_blocks(text)?;
    let project_file = nodes
        .iter()
        .find(|n| n.name().eq_ignore_ascii_case("PROJECT_FILE"))
        .map(|n| n.str_arg(0).map(str::to_string))
        .transpose()?
        .ok_or_else(|| FormatError::Invalid("PROJECT_FILE missing".into()))?;
    let players = nodes
        .iter()
        .filter(|n| n.name().eq_ignore_ascii_case("[PLAYER]"))
        .map(|node: &Node| {
            Ok(Player {
                name: text_value(node, "PLAYER_NAME")?.unwrap_or_default(),
                nation: text_value(node, "PLAYER_PLAYER")?.unwrap_or_default(),
                kind: optional(node, "PLAYER_TYPE")?.unwrap_or(0),
                color: required(node, "PLAYER_COLOR")?,
            })
        })
        .collect::<Result<Vec<_>, FormatError>>()?;
    Ok(LevelFile {
        project_file,
        players,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_file_and_players() {
        let text = "LEVELFILE V1.08e\r\nPROJECT_FILE \"Data\\Projects\\nml-wiese.prj\"\r\n[RESOURCES]\r\n{\r\n}\r\n[PLAYER]\r\n{\r\n\tPLAYER_NAME \"29081 RED\"\r\n\tPLAYER_PLAYER \"SPA\"\r\n\tPLAYER_TYPE 0\r\n\tPLAYER_COLOR 3\r\n\tPLAYER_LINETOOL 0 128 128 0\r\n\t[RESOURCES]\r\n\t{\r\n\t}\r\n}\r\n[PLAYER]\r\n{\r\n\tPLAYER_NAME \"Dummy\"\r\n\tPLAYER_PLAYER \"PAT\"\r\n\tPLAYER_TYPE 2\r\n\tPLAYER_COLOR 7\r\n}\r\n";
        let level = parse_level(text).unwrap();
        assert_eq!(level.project_file, r"Data\Projects\nml-wiese.prj");
        assert_eq!(level.players.len(), 2);
        assert_eq!(level.players[0].color, 3);
        assert_eq!(level.players[0].name_text_id(), Some(29081));
        assert_eq!(level.players[1].kind, 2);
        assert_eq!(level.players[1].name_text_id(), None);
        let placeholder = Player {
            name: "29320".into(),
            ..level.players[0].clone()
        };
        assert_eq!(placeholder.name_text_id(), None);
    }

    #[test]
    fn missing_project_file_is_an_error() {
        assert!(matches!(
            parse_level("LEVELFILE V1.08e\n"),
            Err(FormatError::Invalid(_))
        ));
    }
}
