//! `profiles.ini`: player profiles, progress and current campaign entry.
//!
//! See `docs/formats/profiles-ini.md` and re/singleplayer-menus 2.2. The original
//! writes the file in `FUN_0040E540` (reading `FUN_0040E5C0`, writing `FUN_0040E6C0`) in the
//! archive format of `Engine.ini`, but without a root block.

use crate::FormatError;
use crate::engine_ini::{
    ArchiveNode, block, children_of, counted, element_key, indexed_nodes, indexed_values,
    parse_archive, parse_i32, parse_u16, value, value_text, write_archive,
};

/// An unlocked entry of a campaign (level record, `FUN_0040D1A0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelRecord {
    pub campaign: i32,
    pub level: i32,
    pub trials: i32,
    pub complete: bool,
}

/// A profile (profile object `FUN_0040C9F0`, writer `0x0040E4E0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRecord {
    /// `m_Name`: `wchar_t[40]`, UTF-16 with null terminator and remainder.
    pub name: [u16; 40],
    pub guid: i32,
    pub levels: Vec<LevelRecord>,
    /// `m_VarMap` in file order.
    pub vars: Vec<(String, i32)>,
}

/// The content of `profiles.ini`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilesFile {
    /// `m_CurProfileID`: index into `profiles`, -1 = none.
    pub current: i32,
    pub next_guid: i32,
    pub profiles: Vec<ProfileRecord>,
    pub current_level: i32,
    pub current_campaign: i32,
}

impl Default for ProfilesFile {
    fn default() -> Self {
        Self {
            current: -1,
            next_guid: 0,
            profiles: Vec::new(),
            current_level: 0,
            current_campaign: 0,
        }
    }
}

/// Default name of a profile (`L"Jon Doe"`, constructor `FUN_0040C9F0`).
const DEFAULT_NAME: &str = "Jon Doe";

fn default_name() -> [u16; 40] {
    let mut name = [0u16; 40];
    for (slot, unit) in name.iter_mut().zip(DEFAULT_NAME.encode_utf16()) {
        *slot = unit;
    }
    name
}

/// Message of a rejected file: the prefix is fixed for the host and the tests.
fn broken(message: impl std::fmt::Display) -> FormatError {
    FormatError::Invalid(format!("corrupted profile file: {message}"))
}

fn int_of(node: &ArchiveNode) -> Result<i32, String> {
    value_text(node)
        .and_then(parse_i32)
        .map_err(|problem| format!("key \"{}\": {problem}", node.key()))
}

/// The `n` elements `i0000 ...` of a block with a counter header (`arraysize`/`listsize`), each as a block.
fn element_blocks<'a>(
    node: &'a ArchiveNode,
    count_key: &str,
) -> Result<Vec<&'a [ArchiveNode]>, String> {
    let (n, rest) = counted(children_of(node)?, count_key)?;
    if rest.len() < n {
        return Err(format!("{} elements instead of {n}", rest.len()));
    }
    let mut out = Vec::with_capacity(n);
    for (i, child) in rest.iter().take(n).enumerate() {
        let want = element_key("i", i);
        if child.key() != want {
            return Err(format!("\"{want}\" expected, found \"{}\"", child.key()));
        }
        out.push(children_of(child)?);
    }
    Ok(out)
}

fn parse_level(children: &[ArchiveNode]) -> Result<LevelRecord, String> {
    // Preassignment when reading: `{0, -1, -1, 0}` (`FUN_00497FB0`).
    let mut record = LevelRecord {
        campaign: -1,
        level: -1,
        trials: 0,
        complete: false,
    };
    for child in children {
        match child.key() {
            "m_CampaignID" => record.campaign = int_of(child)?,
            "m_LevelID" => record.level = int_of(child)?,
            "m_TrialCount" => record.trials = int_of(child)?,
            // `FUN_00498AC0`: the text "0" is false, everything else true.
            "m_LevelComplete" => record.complete = value_text(child)? != "0",
            _ => {}
        }
    }
    Ok(record)
}

fn parse_vars(node: &ArchiveNode) -> Result<Vec<(String, i32)>, String> {
    let (n, rest) = counted(children_of(node)?, "mapsize")?;
    // Without multiplication: `n` comes from the file and can reach `usize::MAX`.
    if rest.len() / 2 < n {
        return Err(format!("{} entries instead of {n}", rest.len() / 2));
    }
    let mut vars = Vec::with_capacity(n);
    for (i, pair) in rest.chunks_exact(2).take(n).enumerate() {
        let (k, v) = (element_key("k", i), element_key("v", i));
        if pair[0].key() != k || pair[1].key() != v {
            return Err(format!("\"{k}\" and \"{v}\" expected"));
        }
        vars.push((value_text(&pair[0])?.to_string(), int_of(&pair[1])?));
    }
    Ok(vars)
}

/// The name only with `fixedsize == 40` (`FUN_00403A00`); otherwise the default stays.
fn parse_name(node: &ArchiveNode) -> Result<[u16; 40], String> {
    let (n, _) = counted(children_of(node)?, "fixedsize")?;
    if n != 40 {
        return Ok(default_name());
    }
    let mut name = [0u16; 40];
    for (slot, text) in name
        .iter_mut()
        .zip(indexed_values(node, "fixedsize", Some(40))?)
    {
        *slot = parse_u16(text)?;
    }
    Ok(name)
}

fn parse_profile(children: &[ArchiveNode]) -> Result<ProfileRecord, String> {
    let mut name = default_name();
    let mut guid = None;
    let mut levels = Vec::new();
    let mut vars = Vec::new();
    for child in children {
        match child.key() {
            "m_Name" => name = parse_name(child)?,
            "m_ProfileGUID" => guid = Some(int_of(child)?),
            "m_LevelList" => {
                levels = element_blocks(child, "listsize")?
                    .into_iter()
                    .map(parse_level)
                    .collect::<Result<_, _>>()?;
            }
            "m_VarMap" => vars = parse_vars(child)?,
            _ => {}
        }
    }
    // The original then leaves the GUID uninitialized (O-126).
    let guid = guid.ok_or("profile without \"m_ProfileGUID\"")?;
    Ok(ProfileRecord {
        name,
        guid,
        levels,
        vars,
    })
}

fn parse_inner(text: &str) -> Result<ProfilesFile, String> {
    let nodes = parse_archive(text).map_err(|e| match e {
        FormatError::Invalid(message) => message,
        other => other.to_string(),
    })?;
    let mut file = ProfilesFile::default();
    for node in &nodes {
        match node.key() {
            "m_CurProfileID" => file.current = int_of(node)?,
            "m_NextProfileGUID" => file.next_guid = int_of(node)?,
            "m_Profiles" => {
                file.profiles = element_blocks(node, "arraysize")?
                    .into_iter()
                    .map(parse_profile)
                    .collect::<Result<_, _>>()?;
            }
            "m_CurLevelID" => file.current_level = int_of(node)?,
            "m_CurCampaignID" => file.current_campaign = int_of(node)?,
            _ => {}
        }
    }
    check(&file)?;
    Ok(file)
}

/// The rejection rules (deviation from the original, O-126, `docs/formats/profiles-ini.md`).
fn check(file: &ProfilesFile) -> Result<(), String> {
    let count = file.profiles.len();
    let valid = usize::try_from(file.current).map_or(file.current == -1, |index| index < count);
    if !valid {
        return Err(format!(
            "\"m_CurProfileID\" {} is not in -1 ... {} (number of profiles: {count})",
            file.current,
            count as i64 - 1
        ));
    }
    for (i, profile) in file.profiles.iter().enumerate() {
        if file.profiles[..i].iter().any(|p| p.guid == profile.guid) {
            return Err(format!("the GUID {} occurs twice", profile.guid));
        }
    }
    if let Some(highest) = file.profiles.iter().map(|p| p.guid).max()
        && file.next_guid <= highest
    {
        return Err(format!(
            "\"m_NextProfileGUID\" {} is not greater than the highest GUID {highest}",
            file.next_guid
        ));
    }
    Ok(())
}

/// Reads `profiles.ini`. Missing keys keep the default ([`ProfilesFile::default`]); a
/// file that the original would load with crash consequences is rejected with an error message (a deviation from
/// the original): archive unreadable, number unreadable, profile without `m_ProfileGUID`, `m_CurProfileID`
/// outside -1 ... count-1, duplicate GUID, `m_NextProfileGUID` not above the highest GUID.
/// All messages begin with "corrupted profile file".
pub fn parse_profiles(text: &str) -> Result<ProfilesFile, FormatError> {
    parse_inner(text).map_err(broken)
}

fn profile_children(profile: &ProfileRecord) -> Vec<ArchiveNode> {
    let mut levels = vec![value("listsize", profile.levels.len().to_string())];
    for (i, level) in profile.levels.iter().enumerate() {
        levels.push(block(
            &element_key("i", i),
            vec![
                value("m_CampaignID", level.campaign.to_string()),
                value("m_LevelID", level.level.to_string()),
                value("m_TrialCount", level.trials.to_string()),
                value("m_LevelComplete", if level.complete { "1" } else { "0" }),
            ],
        ));
    }
    let mut vars = vec![value("mapsize", profile.vars.len().to_string())];
    for (i, (name, number)) in profile.vars.iter().enumerate() {
        vars.push(value(&element_key("k", i), name.clone()));
        vars.push(value(&element_key("v", i), number.to_string()));
    }
    vec![
        indexed_nodes("m_Name", "fixedsize", profile.name.iter(), |u| {
            u.to_string()
        }),
        value("m_ProfileGUID", profile.guid.to_string()),
        block("m_LevelList", levels),
        block("m_VarMap", vars),
    ]
}

/// Writes `profiles.ini` in the order and form of the original (`FUN_0040E540`): one line without a
/// root block.
pub fn write_profiles(file: &ProfilesFile) -> String {
    let mut profiles = vec![value("arraysize", file.profiles.len().to_string())];
    for (i, profile) in file.profiles.iter().enumerate() {
        profiles.push(block(&element_key("i", i), profile_children(profile)));
    }
    write_archive(&[
        value("m_CurProfileID", file.current.to_string()),
        value("m_NextProfileGUID", file.next_guid.to_string()),
        block("m_Profiles", profiles),
        value("m_CurLevelID", file.current_level.to_string()),
        value("m_CurCampaignID", file.current_campaign.to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Name as a fixed-size field: `<m_Name><fixedsize>40</><i0000>...</>...</>`.
    fn name_node(name: &str) -> String {
        let mut units: Vec<u16> = name.encode_utf16().collect();
        units.resize(40, 0);
        let mut out = String::from("<m_Name><fixedsize>40</>");
        for (i, u) in units.iter().enumerate() {
            out.push_str(&format!("<i{i:04}>{u}</>"));
        }
        out.push_str("</>");
        out
    }

    fn name_units(name: &str) -> [u16; 40] {
        let mut units = [0u16; 40];
        for (slot, u) in units.iter_mut().zip(name.encode_utf16()) {
            *slot = u;
        }
        units
    }

    fn profile_text(name: &str, guid: i32, levels: &str, vars: &str) -> String {
        format!("{}<m_ProfileGUID>{guid}</>{levels}{vars}", name_node(name))
    }

    const LEVELS_A: &str = "<m_LevelList><listsize>2</>\
        <i0000><m_CampaignID>0</><m_LevelID>0</><m_TrialCount>0</><m_LevelComplete>1</></>\
        <i0001><m_CampaignID>1</><m_LevelID>3</><m_TrialCount>2</><m_LevelComplete>0</></></>";
    const VARS_A: &str =
        "<m_VarMap><mapsize>2</><k0000>alpha</><v0000>5</><k0001>b ü</><v0001>-7</></>";
    const NO_LEVELS: &str = "<m_LevelList><listsize>0</></>";
    const NO_VARS: &str = "<m_VarMap><mapsize>0</></>";

    fn file_text(cur: i32, next: i32, profiles: &[String], level: i32, campaign: i32) -> String {
        let mut out = format!("<m_CurProfileID>{cur}</><m_NextProfileGUID>{next}</>");
        out.push_str(&format!("<m_Profiles><arraysize>{}</>", profiles.len()));
        for (i, p) in profiles.iter().enumerate() {
            out.push_str(&format!("<i{i:04}>{p}</>"));
        }
        out.push_str(&format!(
            "</><m_CurLevelID>{level}</><m_CurCampaignID>{campaign}</>"
        ));
        out
    }

    fn two_profiles() -> Vec<String> {
        vec![
            profile_text("Anna", 0, LEVELS_A, VARS_A),
            profile_text("Jörg", 5, NO_LEVELS, NO_VARS),
        ]
    }

    fn rejection(text: &str) -> String {
        match parse_profiles(text) {
            Err(FormatError::Invalid(m)) => m,
            other => panic!("rejection expected, got: {other:?}"),
        }
    }

    #[test]
    fn profiles_round_trip_is_lossless() {
        let text = file_text(1, 6, &two_profiles(), 4, 2);
        let file = parse_profiles(&text).unwrap();
        assert_eq!(file.current, 1);
        assert_eq!(file.next_guid, 6);
        assert_eq!((file.current_level, file.current_campaign), (4, 2));
        assert_eq!(file.profiles.len(), 2);
        let anna = &file.profiles[0];
        assert_eq!(anna.name, name_units("Anna"));
        assert_eq!(anna.guid, 0);
        assert_eq!(
            anna.levels,
            vec![
                LevelRecord {
                    campaign: 0,
                    level: 0,
                    trials: 0,
                    complete: true
                },
                LevelRecord {
                    campaign: 1,
                    level: 3,
                    trials: 2,
                    complete: false
                },
            ]
        );
        assert_eq!(
            anna.vars,
            vec![("alpha".to_string(), 5), ("b ü".to_string(), -7)]
        );
        assert_eq!(file.profiles[1].name, name_units("Jörg"));
        assert_eq!(file.profiles[1].guid, 5);
        assert!(file.profiles[1].levels.is_empty() && file.profiles[1].vars.is_empty());
        assert_eq!(write_profiles(&file), text);
    }

    #[test]
    fn missing_file_defaults() {
        let file = ProfilesFile::default();
        assert_eq!(file.current, -1);
        assert_eq!(file.next_guid, 0);
        assert!(file.profiles.is_empty());
        assert_eq!((file.current_level, file.current_campaign), (0, 0));
    }

    #[test]
    fn original_empty_file_parses() {
        let text = "<m_CurProfileID>-1</><m_NextProfileGUID>1</><m_Profiles><arraysize>0</></>\
                    <m_CurLevelID>1</><m_CurCampaignID>0</>";
        let file = parse_profiles(text).unwrap();
        assert_eq!(
            file,
            ProfilesFile {
                current: -1,
                next_guid: 1,
                profiles: Vec::new(),
                current_level: 1,
                current_campaign: 0,
            }
        );
        assert_eq!(write_profiles(&file), text);
        // A line ending that a foreign editor appends does not matter.
        assert_eq!(parse_profiles(&format!("{text}\r\n")).unwrap(), file);
    }

    #[test]
    fn missing_keys_keep_their_defaults() {
        let file = parse_profiles("<m_NextProfileGUID>9</>").unwrap();
        assert_eq!(
            file,
            ProfilesFile {
                next_guid: 9,
                ..ProfilesFile::default()
            }
        );
        assert_eq!(parse_profiles("").unwrap(), ProfilesFile::default());
    }

    #[test]
    fn missing_level_keys_use_the_read_defaults() {
        // When reading, the level record is preassigned with {0, -1, -1, 0} (re/singleplayer-menus 2.1).
        let level = "<m_LevelList><listsize>1</><i0000><m_TrialCount>3</></></>";
        let text = file_text(-1, 1, &[profile_text("A", 0, level, NO_VARS)], 0, 0);
        let file = parse_profiles(&text).unwrap();
        assert_eq!(
            file.profiles[0].levels,
            vec![LevelRecord {
                campaign: -1,
                level: -1,
                trials: 3,
                complete: false
            }]
        );
    }

    #[test]
    fn level_complete_is_true_for_anything_but_the_text_zero() {
        let level = "<m_LevelList><listsize>2</>\
            <i0000><m_LevelComplete>7</></><i0001><m_LevelComplete>0</></></>";
        let text = file_text(-1, 1, &[profile_text("A", 0, level, NO_VARS)], 0, 0);
        let file = parse_profiles(&text).unwrap();
        assert!(file.profiles[0].levels[0].complete);
        assert!(!file.profiles[0].levels[1].complete);
    }

    // QUIRK O-126: deviation
    #[test]
    fn wrong_name_size_keeps_the_default_name() {
        // `FUN_00403A00`: only with `fixedsize == 40` is it read, otherwise "Jon Doe" stays.
        let name = "<m_Name><fixedsize>2</><i0000>65</><i0001>66</></>";
        let p = format!("{name}<m_ProfileGUID>3</>{NO_LEVELS}{NO_VARS}");
        let file = parse_profiles(&file_text(0, 4, &[p], 0, 0)).unwrap();
        assert_eq!(file.profiles[0].name, name_units("Jon Doe"));
        assert_eq!(file.profiles[0].guid, 3);
    }

    // QUIRK O-126: deviation
    #[test]
    fn profiles_reject_broken_files() {
        let ok = |guid: i32| profile_text("A", guid, NO_LEVELS, NO_VARS);
        let no_guid = format!("{}{NO_LEVELS}{NO_VARS}", name_node("A"));
        let cases: Vec<(&str, String)> = vec![
            ("profile without GUID", file_text(-1, 1, &[no_guid], 0, 0)),
            ("index too large", file_text(2, 9, &[ok(0), ok(1)], 0, 0)),
            ("index equal to count", file_text(1, 9, &[ok(0)], 0, 0)),
            ("index below -1", file_text(-2, 9, &[ok(0)], 0, 0)),
            ("index without profiles", file_text(0, 1, &[], 0, 0)),
            ("duplicate GUID", file_text(0, 9, &[ok(4), ok(4)], 0, 0)),
            (
                "counter equal to highest GUID",
                file_text(0, 5, &[ok(2), ok(5)], 0, 0),
            ),
            (
                "counter below highest GUID",
                file_text(0, 1, &[ok(2), ok(5)], 0, 0),
            ),
            ("archive unreadable", "<m_CurProfileID>-1".to_string()),
            ("number unreadable", "<m_CurProfileID>x</>".to_string()),
            (
                "too few profiles",
                "<m_Profiles><arraysize>2</><i0000></></></>".to_string(),
            ),
            // Counter from the file up to `usize::MAX`: no overflows, no giant allocation.
            (
                "mapsize 2^63",
                file_text(
                    -1,
                    1,
                    &[profile_text(
                        "A",
                        0,
                        NO_LEVELS,
                        "<m_VarMap><mapsize>9223372036854775808</></>",
                    )],
                    0,
                    0,
                ),
            ),
            (
                "mapsize usize::MAX",
                file_text(
                    -1,
                    1,
                    &[profile_text(
                        "A",
                        0,
                        NO_LEVELS,
                        "<m_VarMap><mapsize>18446744073709551615</></>",
                    )],
                    0,
                    0,
                ),
            ),
            (
                "listsize usize::MAX",
                file_text(
                    -1,
                    1,
                    &[profile_text(
                        "A",
                        0,
                        "<m_LevelList><listsize>18446744073709551615</></>",
                        NO_VARS,
                    )],
                    0,
                    0,
                ),
            ),
            (
                "arraysize usize::MAX",
                "<m_Profiles><arraysize>18446744073709551615</></>".to_string(),
            ),
        ];
        for (what, text) in cases {
            let message = rejection(&text);
            assert!(
                message.starts_with("corrupted profile file"),
                "{what}: message begins differently: {message}"
            );
        }
        // The error text carries the prefix in the display too.
        let shown = parse_profiles("<m_CurProfileID>x</>")
            .unwrap_err()
            .to_string();
        assert!(shown.contains("corrupted profile file"), "{shown}");
    }

    // QUIRK O-126: deviation
    #[test]
    fn counter_just_above_the_highest_guid_is_accepted() {
        let ok = |guid: i32| profile_text("A", guid, NO_LEVELS, NO_VARS);
        let file = parse_profiles(&file_text(1, 6, &[ok(5), ok(2)], 0, 0)).unwrap();
        assert_eq!(file.profiles.len(), 2);
    }
}
