//! `Engine.ini`: the settings file of the original (archive format `<key>value</>`).
//!
//! Format: `docs/formats/engine-ini.md`. The original reads the file in `EngineConfig_Construct`
//! (`0x00497010`) via `FUN_00496C00` onto the defaults of `FUN_00494FD0` and writes it with
//! `EngineConfig_Serialize` (`0x00497330`, caller `FUN_00496D40`). The reimplementation reads and writes
//! the same format losslessly; the file of the installation is never written to.
//!
//! The reimplementation does not read the old line-based text format (`FUN_004952D0`): an unreadable file
//! leads to the defaults and a warning.

use crate::FormatError;
use crate::text::decode_cp1252;

/// Deepest allowed nesting of an archive. Real files go four levels deep
/// (`InitFile` -> `Keyboard` -> `m_KeyMap` -> value); the limit protects against stack overflow.
const MAX_DEPTH: usize = 64;

/// A node of the archive.
#[derive(Debug, Clone, PartialEq)]
pub enum ArchiveNode {
    /// `<key>value</>`, the value raw (also empty: `<key></>`).
    Value { key: String, value: String },
    /// `<Name>` children `</>`.
    Block {
        key: String,
        children: Vec<ArchiveNode>,
    },
}

impl ArchiveNode {
    pub fn key(&self) -> &str {
        match self {
            ArchiveNode::Value { key, .. } | ArchiveNode::Block { key, .. } => key,
        }
    }
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    fn fail(&self, message: &str) -> FormatError {
        FormatError::Invalid(format!("archive, character {}: {message}", self.pos))
    }

    /// Nodes up to the end of the block (`</>`) or up to the end of the text at level 0.
    fn parse_nodes(&mut self, depth: usize) -> Result<Vec<ArchiveNode>, FormatError> {
        let mut nodes = Vec::new();
        loop {
            if depth == 0 {
                // The file of the original has no line ending; another editor may append one.
                let trimmed = self.rest().trim_start().len();
                self.pos = self.text.len() - trimmed;
            }
            let rest = self.rest();
            if rest.is_empty() {
                return if depth == 0 {
                    Ok(nodes)
                } else {
                    Err(self.fail("block not closed (end of file)"))
                };
            }
            if rest.starts_with("</>") {
                if depth == 0 {
                    return Err(self.fail("surplus block end \"</>\""));
                }
                self.pos += 3;
                return Ok(nodes);
            }
            if !rest.starts_with('<') {
                return Err(self.fail("tag \"<key>\" expected"));
            }
            let Some(close) = rest.find('>') else {
                return Err(self.fail("tag not terminated with \">\""));
            };
            let key = &rest[1..close];
            if key.is_empty() || key.contains('<') {
                return Err(self.fail("invalid key"));
            }
            let key = key.to_string();
            self.pos += close + 1;
            let rest = self.rest();
            if rest.starts_with('<') && !rest.starts_with("</>") {
                if depth + 1 > MAX_DEPTH {
                    return Err(self.fail("nesting too deep"));
                }
                let children = self.parse_nodes(depth + 1)?;
                nodes.push(ArchiveNode::Block { key, children });
            } else {
                let Some(end) = rest.find("</>") else {
                    return Err(self.fail("value not terminated with \"</>\""));
                };
                let value = rest[..end].to_string();
                self.pos += end + 3;
                nodes.push(ArchiveNode::Value { key, value });
            }
        }
    }
}

/// Reads an archive (format: `docs/formats/engine-ini.md`, section "Format"). Blocks and values follow each
/// other without whitespace; only before and after the root may whitespace appear.
pub fn parse_archive(text: &str) -> Result<Vec<ArchiveNode>, FormatError> {
    Parser { text, pos: 0 }.parse_nodes(0)
}

fn write_node(out: &mut String, node: &ArchiveNode) {
    match node {
        ArchiveNode::Value { key, value } => {
            out.push('<');
            out.push_str(key);
            out.push('>');
            out.push_str(value);
            out.push_str("</>");
        }
        ArchiveNode::Block { key, children } => {
            out.push('<');
            out.push_str(key);
            out.push('>');
            for child in children {
                write_node(out, child);
            }
            out.push_str("</>");
        }
    }
}

/// Writes an archive on a single line without a break (like `FUN_00496D40`).
pub fn write_archive(nodes: &[ArchiveNode]) -> String {
    let mut out = String::new();
    for node in nodes {
        write_node(&mut out, node);
    }
    out
}

pub(crate) fn value(key: &str, value: impl Into<String>) -> ArchiveNode {
    ArchiveNode::Value {
        key: key.to_string(),
        value: value.into(),
    }
}

pub(crate) fn block(key: &str, children: Vec<ArchiveNode>) -> ArchiveNode {
    ArchiveNode::Block {
        key: key.to_string(),
        children,
    }
}

/// Key of an element of a list or of a fixed field: `i%04d`.
pub(crate) fn element_key(prefix: &str, index: usize) -> String {
    format!("{prefix}{index:04}")
}

pub(crate) fn value_text(node: &ArchiveNode) -> Result<&str, String> {
    match node {
        ArchiveNode::Value { value, .. } => Ok(value),
        ArchiveNode::Block { .. } => Err("a value was expected, a block was found".into()),
    }
}

pub(crate) fn children_of(node: &ArchiveNode) -> Result<&[ArchiveNode], String> {
    match node {
        ArchiveNode::Block { children, .. } => Ok(children),
        ArchiveNode::Value { .. } => Err("a block was expected, a value was found".into()),
    }
}

pub(crate) fn parse_i32(text: &str) -> Result<i32, String> {
    text.trim()
        .parse()
        .map_err(|_| format!("\"{text}\" is not an integer"))
}

pub(crate) fn parse_u16(text: &str) -> Result<u16, String> {
    text.trim()
        .parse()
        .map_err(|_| format!("\"{text}\" is not a number from 0 to 65535"))
}

/// Counter header of a block (`<listsize>n</>`, `<fixedsize>n</>`, `<mapsize>n</>`) and the
/// following children.
pub(crate) fn counted<'a>(
    children: &'a [ArchiveNode],
    count_key: &str,
) -> Result<(usize, &'a [ArchiveNode]), String> {
    let Some((head, rest)) = children.split_first() else {
        return Err(format!("\"{count_key}\" missing"));
    };
    if head.key() != count_key {
        return Err(format!(
            "\"{count_key}\" missing (found: \"{}\")",
            head.key()
        ));
    }
    let n = value_text(head)?;
    let n: usize = n
        .trim()
        .parse()
        .map_err(|_| format!("\"{count_key}\" \"{n}\" is not a count"))?;
    Ok((n, rest))
}

/// The values `i0000 ... i(n-1)` of a block with a counter header.
pub(crate) fn indexed_values<'a>(
    node: &'a ArchiveNode,
    count_key: &str,
    expected: Option<usize>,
) -> Result<Vec<&'a str>, String> {
    let (n, rest) = counted(children_of(node)?, count_key)?;
    if let Some(expected) = expected
        && n != expected
    {
        return Err(format!("\"{count_key}\" is {n} instead of {expected}"));
    }
    if rest.len() < n {
        return Err(format!("{} elements instead of {n}", rest.len()));
    }
    let mut values = Vec::with_capacity(n);
    for (i, child) in rest.iter().take(n).enumerate() {
        let want = element_key("i", i);
        if child.key() != want {
            return Err(format!("\"{want}\" expected, found \"{}\"", child.key()));
        }
        values.push(value_text(child)?);
    }
    Ok(values)
}

pub(crate) fn indexed_nodes<T>(
    key: &str,
    count_key: &str,
    items: impl ExactSizeIterator<Item = T>,
    text: impl Fn(T) -> String,
) -> ArchiveNode {
    let mut children = vec![value(count_key, items.len().to_string())];
    for (i, item) in items.enumerate() {
        children.push(value(&element_key("i", i), text(item)));
    }
    block(key, children)
}

/// A field of `EngineConfig`: reads itself from its node (warning instead of an error, the field
/// then stays unchanged) and writes itself as a node.
trait ArchiveField {
    fn read_into(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>);
    fn to_node(&self, key: &str) -> ArchiveNode;
}

fn warn_field(warnings: &mut Vec<String>, key: &str, problem: &str) {
    warnings.push(format!(
        "Engine.ini: key \"{key}\" invalid ({problem}), the default is kept"
    ));
}

/// Common path of the single values: read text, convert, warn on error.
fn read_scalar<T>(
    slot: &mut T,
    node: &ArchiveNode,
    warnings: &mut Vec<String>,
    parse: impl FnOnce(&str) -> Result<T, String>,
) {
    match value_text(node).and_then(parse) {
        Ok(v) => *slot = v,
        Err(problem) => warn_field(warnings, node.key(), &problem),
    }
}

impl ArchiveField for i32 {
    fn read_into(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>) {
        read_scalar(self, node, warnings, parse_i32);
    }
    fn to_node(&self, key: &str) -> ArchiveNode {
        value(key, self.to_string())
    }
}

/// `bool` is stored as `0`/`1` in the file; any other number counts as true.
impl ArchiveField for bool {
    fn read_into(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>) {
        read_scalar(self, node, warnings, |t| parse_i32(t).map(|n| n != 0));
    }
    fn to_node(&self, key: &str) -> ArchiveNode {
        value(key, if *self { "1" } else { "0" })
    }
}

/// `float` is stored as `%f` (six decimal places) in the file.
impl ArchiveField for f32 {
    fn read_into(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>) {
        read_scalar(self, node, warnings, |t| match t.trim().parse::<f32>() {
            Ok(v) if v.is_finite() => Ok(v),
            _ => Err(format!("\"{t}\" is not a finite number")),
        });
    }
    fn to_node(&self, key: &str) -> ArchiveNode {
        value(key, format!("{self:.6}"))
    }
}

impl ArchiveField for String {
    fn read_into(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>) {
        read_scalar(self, node, warnings, |t| Ok(t.to_string()));
    }
    fn to_node(&self, key: &str) -> ArchiveNode {
        value(key, self.clone())
    }
}

/// `wchar_t[N]`: N UTF-16 units unsigned decimal (`NetName` etc.).
impl<const N: usize> ArchiveField for [u16; N] {
    fn read_into(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>) {
        let parsed = indexed_values(node, "fixedsize", Some(N)).and_then(|values| {
            let mut out = [0u16; N];
            for (slot, text) in out.iter_mut().zip(values) {
                *slot = parse_u16(text)?;
            }
            Ok(out)
        });
        match parsed {
            Ok(v) => *self = v,
            Err(problem) => warn_field(warnings, node.key(), &problem),
        }
    }
    fn to_node(&self, key: &str) -> ArchiveNode {
        indexed_nodes(key, "fixedsize", self.iter(), |u| u.to_string())
    }
}

/// `TextureArrayPreLoad`: `<listsize>n</>` and `<i%04d>Name</>`.
impl ArchiveField for Vec<String> {
    fn read_into(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>) {
        match indexed_values(node, "listsize", None) {
            Ok(values) => *self = values.into_iter().map(str::to_string).collect(),
            Err(problem) => warn_field(warnings, node.key(), &problem),
        }
    }
    fn to_node(&self, key: &str) -> ArchiveNode {
        indexed_nodes(key, "listsize", self.iter(), |s| s.clone())
    }
}

/// `Keyboard` -> `m_KeyMap`: `<mapsize>n</>` and per entry `<k%04d>ID</><v%04d>KEY</>`
/// (key as a combination name, also empty). The key of the field is that of the outer block.
///
/// Reading as in the original (`0x0049DCE0`, called via `0x0049DF70` with "m_KeyMap" from the
/// serializer of the keyboard object, `0x0049E0B0`): no clearing, every entry goes via
/// `CMap::operator[]` (`FUN_00485720`) into the existing layout - a known ID gets the
/// new key (its place stays), a new ID is appended. An error in the middle of the list leaves the
/// entries read up to then in place.
impl ArchiveField for Vec<(i32, String)> {
    fn read_into(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>) {
        let header = children_of(node).and_then(|children| {
            let Some(map) = children.iter().find(|c| c.key() == "m_KeyMap") else {
                return Err("\"m_KeyMap\" missing".to_string());
            };
            counted(children_of(map)?, "mapsize")
        });
        let (n, rest) = match header {
            Ok(header) => header,
            Err(problem) => return warn_field(warnings, node.key(), &problem),
        };
        let entries = rest.chunks_exact(2).take(n).enumerate();
        let mut read = 0;
        for (i, pair) in entries {
            let (k, v) = (element_key("k", i), element_key("v", i));
            let entry = if pair[0].key() == k && pair[1].key() == v {
                value_text(&pair[0])
                    .and_then(parse_i32)
                    .and_then(|id| Ok((id, value_text(&pair[1])?.to_string())))
            } else {
                Err(format!("\"{k}\" and \"{v}\" expected"))
            };
            match entry {
                Ok((id, key)) => {
                    match self.iter_mut().find(|(known, _)| *known == id) {
                        Some(slot) => slot.1 = key,
                        None => self.push((id, key)),
                    }
                    read += 1;
                }
                Err(problem) => return warn_field(warnings, node.key(), &problem),
            }
        }
        if read < n {
            warn_field(
                warnings,
                node.key(),
                &format!("{read} entries instead of {n}"),
            );
        }
    }
    fn to_node(&self, key: &str) -> ArchiveNode {
        let mut children = vec![value("mapsize", self.len().to_string())];
        for (i, (id, name)) in self.iter().enumerate() {
            children.push(value(&element_key("k", i), id.to_string()));
            children.push(value(&element_key("v", i), name.clone()));
        }
        block(key, vec![block("m_KeyMap", children)])
    }
}

/// The `SoundManager` block (`SoundManager_Serialize`, `0x00553870`): last in the file.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundConfig {
    pub output: i32,
    pub driver_id: i32,
    pub is_hardware_on: bool,
    pub revert_channels: bool,
    /// Music, sounds, speech (`Category` 0, 1, 2); default 349/599/499 (re/audio).
    pub volumes: [i32; 3],
}

impl Default for SoundConfig {
    fn default() -> Self {
        Self {
            output: 0,
            driver_id: 0,
            is_hardware_on: false,
            revert_channels: false,
            volumes: [349, 599, 499],
        }
    }
}

impl ArchiveField for [i32; 3] {
    fn read_into(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>) {
        let parsed = indexed_values(node, "fixedsize", Some(3)).and_then(|values| {
            let mut out = [0i32; 3];
            for (slot, text) in out.iter_mut().zip(values) {
                *slot = parse_i32(text)?;
            }
            Ok(out)
        });
        match parsed {
            Ok(v) => *self = v,
            Err(problem) => warn_field(warnings, node.key(), &problem),
        }
    }
    fn to_node(&self, key: &str) -> ArchiveNode {
        indexed_nodes(key, "fixedsize", self.iter(), |v| v.to_string())
    }
}

impl ArchiveField for SoundConfig {
    fn read_into(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>) {
        let children = match children_of(node) {
            Ok(children) => children,
            Err(problem) => {
                warn_field(warnings, node.key(), &problem);
                return;
            }
        };
        for child in children {
            match child.key() {
                "m_Output" => self.output.read_into(child, warnings),
                "m_DriverID" => self.driver_id.read_into(child, warnings),
                "m_IsHardwareOn" => self.is_hardware_on.read_into(child, warnings),
                "m_RevertChannels" => self.revert_channels.read_into(child, warnings),
                "m_Volumes" => self.volumes.read_into(child, warnings),
                other => warnings.push(format!(
                    "Engine.ini: unknown key \"{other}\" in \"SoundManager\" ignored"
                )),
            }
        }
    }
    fn to_node(&self, key: &str) -> ArchiveNode {
        block(
            key,
            vec![
                self.output.to_node("m_Output"),
                self.driver_id.to_node("m_DriverID"),
                self.is_hardware_on.to_node("m_IsHardwareOn"),
                self.revert_channels.to_node("m_RevertChannels"),
                self.volumes.to_node("m_Volumes"),
            ],
        )
    }
}

/// Generates `EngineConfig`, its defaults (`FUN_00494FD0`), reading a node and writing in a single
/// enumeration: the order is that of `EngineConfig_Serialize` (`0x00497330`, `docs/formats/engine-ini.md`,
/// section "Keys in write order"). Colors are `i32`. After the `;` the reimplementation's own keys
/// (`docs/formats/engine-ini.md`, section "Own keys"): read like the others, written only if the value differs
/// from the default, and then after the `SoundManager` block - so a file of the installation stays byte-identical
/// when read and written.
macro_rules! engine_config {
    ($($field:ident : $ty:ty = $default:expr => $key:literal,)*
     ; $($own:ident : $own_ty:ty = $own_default:expr => $own_key:literal,)*) => {
        /// All values of `Engine.ini` (one field per key). Defaults: `FUN_00494FD0`, without
        /// key layout (`key_map`, default from `defaultkeys.ini` via [`Self::with_default_keys`]).
        #[derive(Debug, Clone, PartialEq)]
        pub struct EngineConfig {
            $(pub $field: $ty,)*
            $(pub $own: $own_ty,)*
        }

        impl Default for EngineConfig {
            fn default() -> Self {
                Self { $($field: $default,)* $($own: $own_default,)* }
            }
        }

        impl EngineConfig {
            /// Reads a key of the root; `false` for an unknown key.
            fn read_key(&mut self, node: &ArchiveNode, warnings: &mut Vec<String>) -> bool {
                match node.key() {
                    $($key => self.$field.read_into(node, warnings),)*
                    $($own_key => self.$own.read_into(node, warnings),)*
                    _ => return false,
                }
                true
            }

            fn to_nodes(&self) -> Vec<ArchiveNode> {
                #[allow(unused_mut)]
                let mut nodes = vec![$(self.$field.to_node($key),)*];
                $(
                    if self.$own != $own_default {
                        nodes.push(self.$own.to_node($own_key));
                    }
                )*
                nodes
            }
        }
    };
}

engine_config! {
    render_debug_block: i32 = 0 => "RenderDebugBlock",
    is_menu_level: bool = true => "IsMenuLevel",
    is_intro: bool = true => "IsIntro",
    hardware_cursor: bool = true => "HardwareCursor",
    double_click_timeout: i32 = 200 => "DoubleClickTimeout",
    check_sum_enabled: i32 = 100 => "CheckSumEnabled",
    campaign_difficulty: i32 = 1 => "CampaignDifficulty",
    time_per_char: i32 = 55 => "TimePerChar",
    console_key: i32 = 1 => "ConsoleKey",
    activate_alt_f4: i32 = 1 => "ActivateAltF4",
    texture_quality: i32 = 0 => "TextureQuality",
    compressed_textures: bool = true => "CompressedTextures",
    screen_saver_time: i32 = 0 => "ScreenSaverTime",
    screen_saver_file: String = String::new() => "ScreenSaverFile",
    is_auto_record: i32 = 0 => "IsAutoRecord",
    terrain_software_processing: i32 = 0 => "TerrainSoftwareProcessing",
    refresh_rate: i32 = 75 => "RefreshRate",
    direct3d_driver: i32 = 0 => "Direct3DDriver",
    direct3d_device: i32 = 1 => "Direct3DDevice",
    net_host_adr: String = String::new() => "NetHostAdr",
    net_is_host: i32 = 0 => "NetIsHost",
    net_provider: i32 = 0 => "NetProvider",
    net_port: i32 = 0 => "NetPort",
    net_client_count: i32 = 1 => "NetClientCount",
    net_auto_start: i32 = 0 => "NetAutoStart",
    stop_on_check_sum_error: i32 = 0 => "StopOnCheckSumError",
    texture_array_preload: Vec<String> = Vec::new() => "TextureArrayPreLoad",
    // O-375: `IsShowInfo` is read and written but evaluated nowhere - as in the original
    // (no reader in the tip, area and setup functions, re/campaign-ui D6.8); the tips stay on.
    // QUIRK O-375: kept
    is_show_info: i32 = 0 => "IsShowInfo",
    is_map: i32 = 1 => "IsMap",
    is_status_bar: i32 = 1 => "IsStatusBar",
    is_tool_bar: i32 = 1 => "IsToolBar",
    is_log_file_on: i32 = 0 => "IsLogFileOn",
    screen_x_size: i32 = 1024 => "ScreenXSize",
    screen_y_size: i32 = 768 => "ScreenYSize",
    screen_depth: i32 = 16 => "ScreenDepth",
    screen_format: i32 = 0 => "ScreenFormat",
    wait_for_ki_debugger: i32 = 0 => "WaitForKIDebugger",
    script_filename: String = "Console.ini".to_string() => "ScriptFilename",
    intro_filename: String = String::new() => "IntroFilename",
    logo_filename: String = String::new() => "LogoFilename",
    level_file_name: String = String::new() => "LevelFileName",
    language_tag: String = "DEU".to_string() => "LanguageTAG",
    ki_server_port: i32 = 5479 => "KIServerPort",
    version: f32 = 0.0 => "Version",
    show_occluded: i32 = 0 => "ShowOccluded",
    back_color: i32 = 0 => "BackColor",
    ambient_light: i32 = 0xFF_FFFF => "AmbientLight",
    render_specular: bool = false => "RenderSpecular",
    render_dither: bool = true => "RenderDither",
    render_solid: bool = true => "RenderSolid",
    render_gouraud: bool = true => "RenderGouraud",
    render_perspective: bool = true => "RenderPerspective",
    render_subpixel: bool = true => "RenderSubpixel",
    render_fog: bool = false => "RenderFog",
    mip_map_level: i32 = 1 => "MipMapLevel",
    terrain_quality: i32 = 0 => "TerrainQuality",
    fog_color: i32 = 0x1_0000 => "FogColor",
    fog_start: f32 = 950.0 => "FogStart",
    fog_end: f32 = 1100.0 => "FogEnd",
    gamma: f32 = 1.0 => "Gamma",
    ambient_sound: i32 = 1 => "AmbientSound",
    unit_response: i32 = 1 => "UnitResponse",
    shadow_quality: i32 = 2 => "ShadowQuality",
    particles_quality: i32 = 2 => "ParticlesQuality",
    mouse_scroll: i32 = 10 => "MouseScroll",
    enable_mouse_scroll: i32 = 1 => "EnableMouseScroll",
    keyboard_scroll: i32 = 10 => "KeyboardScroll",
    game_speed: i32 = 14 => "GameSpeed",
    enhanced_tooltips: i32 = 1 => "EnhancedTooltips",
    mouse_speed: i32 = 10 => "MouseSpeed",
    lock_camera: i32 = 1 => "LockCamera",
    enable_zoom: i32 = 1 => "EnableZoom",
    nation: i32 = 0 => "Nation",
    handicap: i32 = 0 => "Handicap",
    color: i32 = 0 => "Color",
    net_name: [u16; 40] = [0; 40] => "NetName",
    net_session: [u16; 40] = [0; 40] => "NetSession",
    random_map_seed: [u16; 40] = [0; 40] => "RandomMapSeed",
    random_map_size: i32 = 0 => "RandomMapSize",
    random_players: i32 = 0 => "RandomPlayers",
    random_terrain: i32 = 0 => "RandomTerrain",
    random_landscape: i32 = 0 => "RandomLandscape",
    random_daytime: i32 = 0 => "RandomDaytime",
    level_type: i32 = 0 => "LevelType",
    save_replay: i32 = 0 => "SaveReplay",
    lock_teams: i32 = 0 => "LockTeams",
    map_explored: i32 = 0 => "MapExplored",
    game_type: i32 = 0 => "GameType",
    unit_limit: i32 = 2 => "UnitLimit",
    resourcen: i32 = 0 => "Resourcen",
    time_limit: i32 = 20 => "TimeLimit",
    initial_attack_status: i32 = 2 => "InitialAttackStatus",
    key_map: Vec<(i32, String)> = Vec::new() => "Keyboard",
    sound: SoundConfig = SoundConfig::default() => "SoundManager",
    ;
    // Stage of the turn raster (0 "Original" 8/4, 1 "LAN" 4/2, 2 "LAN fast" 2/1), the last choice in
    // `LAN3`; a value outside 0 to 2 counts as 0 when reading ([`parse_engine_ini_onto`]).
    net_turn_raster: i32 = 0 => "NetTurnRaster",
}

/// Highest stage of `NetTurnRaster` (0 to 2).
pub const NET_TURN_RASTER_MAX: i32 = 2;

impl EngineConfig {
    /// Sets the key layout to the defaults from `defaultkeys.ini` (`FUN_00494FD0` loads it
    /// as well). The order is that of the file; the original keeps the layout in a `CMap` and
    /// writes it in its order (`docs/formats/engine-ini.md`, section "Implementation"). Intended as the
    /// base for [`parse_engine_ini_onto`].
    pub fn with_default_keys(mut self, keys: &[(i32, String)]) -> Self {
        self.key_map = keys.to_vec();
        self
    }
}

/// Reads `Engine.ini` onto the defaults (`FUN_00496C00`). Missing keys and blocks keep
/// the default, unknown ones are ignored, an invalid value or a fixed field with a wrong
/// `fixedsize` leaves the field unchanged - each with a warning. If the file as a whole is
/// unreadable, the defaults and a warning come back. The key layout is empty here;
/// with `defaultkeys.ini` as the default: [`parse_engine_ini_onto`].
pub fn parse_engine_ini(text: &str) -> (EngineConfig, Vec<String>) {
    parse_engine_ini_onto(EngineConfig::default(), text)
}

/// Like [`parse_engine_ini`], but onto a given base as the default - this is how the original starts
/// (`EngineConfig_Construct` `0x00497010`: `FUN_00494FD0` also loads `defaultkeys.ini`, then
/// the reading). What the file does not contain or cannot be read keeps the value of the base; the
/// key layout of the file is inserted into that of the base (`key_map` above).
pub fn parse_engine_ini_onto(base: EngineConfig, text: &str) -> (EngineConfig, Vec<String>) {
    let mut cfg = base;
    let mut warnings = Vec::new();
    let nodes = match parse_archive(text) {
        Ok(nodes) => nodes,
        Err(e) => {
            warnings.push(format!("Engine.ini unreadable ({e}), the defaults apply"));
            return (cfg, warnings);
        }
    };
    let Some(ArchiveNode::Block { children, .. }) = nodes
        .iter()
        .find(|n| matches!(n, ArchiveNode::Block { key, .. } if key == "InitFile"))
    else {
        warnings.push(
            "Engine.ini unreadable (root block \"InitFile\" missing), the defaults apply".into(),
        );
        return (cfg, warnings);
    };
    for child in children {
        if !cfg.read_key(child, &mut warnings) {
            warnings.push(format!(
                "Engine.ini: unknown key \"{}\" ignored",
                child.key()
            ));
        }
    }
    // Own key of the reimplementation: only the stages 0 to 2.
    if !(0..=NET_TURN_RASTER_MAX).contains(&cfg.net_turn_raster) {
        warnings.push(format!(
            "Engine.ini: NetTurnRaster {} unknown (only 0 to {NET_TURN_RASTER_MAX}), 0 applies",
            cfg.net_turn_raster
        ));
        cfg.net_turn_raster = 0;
    }
    (cfg, warnings)
}

/// Writes `Engine.ini` in the order of `EngineConfig_Serialize` (`0x00497330`).
pub fn write_engine_ini(cfg: &EngineConfig) -> String {
    write_archive(&[block("InitFile", cfg.to_nodes())])
}

/// Like [`parse_engine_ini`] on the bytes of the file (Windows-1252, as `NetHostAdr` etc. are
/// ANSI text in the original).
pub fn parse_engine_ini_bytes(bytes: &[u8]) -> (EngineConfig, Vec<String>) {
    parse_engine_ini(&decode_cp1252(bytes))
}

/// Like [`parse_engine_ini_onto`] on the bytes of the file (Windows-1252).
pub fn parse_engine_ini_bytes_onto(
    base: EngineConfig,
    bytes: &[u8],
) -> (EngineConfig, Vec<String>) {
    parse_engine_ini_onto(base, &decode_cp1252(bytes))
}

/// Like [`write_engine_ini`] as Windows-1252 bytes; characters without a counterpart become `?`
/// (never `&#...;`).
pub fn write_engine_ini_bytes(cfg: &EngineConfig) -> Vec<u8> {
    let text = write_engine_ini(cfg);
    let (bytes, _, unmappable) = encoding_rs::WINDOWS_1252.encode(&text);
    if !unmappable {
        return bytes.into_owned();
    }
    let mut out = Vec::with_capacity(text.len());
    let mut buf = [0u8; 4];
    for ch in text.chars() {
        let (b, _, bad) = encoding_rs::WINDOWS_1252.encode(ch.encode_utf8(&mut buf));
        if bad {
            out.push(b'?');
        } else {
            out.extend_from_slice(&b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(key: &str, value: &str) -> ArchiveNode {
        ArchiveNode::Value {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    fn block(key: &str, children: Vec<ArchiveNode>) -> ArchiveNode {
        ArchiveNode::Block {
            key: key.to_string(),
            children,
        }
    }

    #[test]
    fn archive_round_trips_blocks_lists_and_fixed_fields() {
        let text = "<InitFile><A>1</><ScreenSaverFile></><L><listsize>2</><i0000>x</><i0001>y z</></>\
                    <F><fixedsize>2</><i0000>7</><i0001>0</></><N><M><deep>ü</></></></>";
        let nodes = parse_archive(text).unwrap();
        let expected = vec![block(
            "InitFile",
            vec![
                node("A", "1"),
                node("ScreenSaverFile", ""),
                block(
                    "L",
                    vec![
                        node("listsize", "2"),
                        node("i0000", "x"),
                        node("i0001", "y z"),
                    ],
                ),
                block(
                    "F",
                    vec![
                        node("fixedsize", "2"),
                        node("i0000", "7"),
                        node("i0001", "0"),
                    ],
                ),
                block("N", vec![block("M", vec![node("deep", "ü")])]),
            ],
        )];
        assert_eq!(nodes, expected);
        assert_eq!(write_archive(&nodes), text);
    }

    #[test]
    fn archive_rejects_broken_nesting_without_panicking() {
        for bad in [
            "<A>1",
            "<A><B>1</>",
            "</>",
            "<A>1</></>",
            "text",
            "<>1</>",
            "<A",
            "<A><B>1</>x</>",
        ] {
            assert!(parse_archive(bad).is_err(), "{bad:?}");
        }
        let deep = "<a>".repeat(10_000);
        assert!(parse_archive(&deep).is_err());
        assert_eq!(parse_archive("").unwrap(), vec![]);
        assert_eq!(parse_archive("<A>1</>\r\n").unwrap(), vec![node("A", "1")]);
    }

    #[test]
    fn defaults_follow_fun_00494fd0() {
        let c = EngineConfig::default();
        assert!(c.is_menu_level);
        assert!(c.is_intro);
        assert!(c.hardware_cursor);
        assert!(c.compressed_textures);
        assert!(!c.render_specular && c.render_dither && !c.render_fog);
        assert_eq!(c.screen_x_size, 1024);
        assert_eq!(c.screen_y_size, 768);
        assert_eq!(c.screen_depth, 16);
        assert_eq!(c.gamma, 1.0);
        assert_eq!(c.game_speed, 14);
        assert_eq!(c.time_per_char, 55);
        assert_eq!(c.double_click_timeout, 200);
        assert_eq!(c.script_filename, "Console.ini");
        assert_eq!(c.language_tag, "DEU");
        assert_eq!(c.ambient_light, 0xFF_FFFF);
        assert_eq!(c.fog_color, 0x1_0000);
        assert_eq!((c.fog_start, c.fog_end), (950.0, 1100.0));
        assert_eq!(c.ki_server_port, 5479);
        assert_eq!(c.unit_limit, 2);
        assert_eq!(c.time_limit, 20);
        assert_eq!(c.initial_attack_status, 2);
        assert_eq!(c.net_name, [0; 40]);
        assert!(c.texture_array_preload.is_empty());
        assert!(c.key_map.is_empty());
        assert_eq!(c.sound.volumes, [349, 599, 499]);
        assert_eq!(c.sound.output, 0);
        assert!(!c.sound.is_hardware_on && !c.sound.revert_channels);
    }

    #[test]
    fn default_config_round_trips_without_warnings() {
        let cfg = EngineConfig::default().with_default_keys(&[
            (0, String::new()),
            (113, "+".to_string()),
            (573, "ALT N".to_string()),
        ]);
        let text = write_engine_ini(&cfg);
        let (back, warnings) = parse_engine_ini(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back, cfg);
        assert_eq!(write_engine_ini(&back), text);
    }

    #[test]
    fn written_file_follows_the_serialize_order() {
        let text = write_engine_ini(&EngineConfig::default());
        assert!(text.starts_with(
            "<InitFile><RenderDebugBlock>0</><IsMenuLevel>1</><IsIntro>1</><HardwareCursor>1</>"
        ));
        assert!(text.contains("<TextureArrayPreLoad><listsize>0</></>"));
        assert!(text.contains("<ScreenSaverFile></>"));
        assert!(text.contains("<Version>0.000000</><ShowOccluded>0</>"));
        assert!(text.contains("<FogStart>950.000000</><FogEnd>1100.000000</><Gamma>1.000000</>"));
        assert!(text.contains("<AmbientLight>16777215</>"));
        assert!(text.contains("<InitialAttackStatus>2</><Keyboard><m_KeyMap><mapsize>0</></></>"));
        assert!(text.ends_with(
            "<SoundManager><m_Output>0</><m_DriverID>0</><m_IsHardwareOn>0</><m_RevertChannels>0</>\
             <m_Volumes><fixedsize>3</><i0000>349</><i0001>599</><i0002>499</></></></>"
        ));
        assert!(!text.contains('\n'));
    }

    #[test]
    fn engine_ini_keeps_net_turn_raster() {
        // Own key `NetTurnRaster`, not written with the default 0 (installation byte-identical),
        // otherwise after `SoundManager`; read like the others, an unknown value counts as 0 (with a warning).
        assert_eq!(EngineConfig::default().net_turn_raster, 0);
        assert!(!write_engine_ini(&EngineConfig::default()).contains("NetTurnRaster"));
        for stage in [1, 2] {
            let cfg = EngineConfig {
                net_turn_raster: stage,
                ..EngineConfig::default()
            };
            let text = write_engine_ini(&cfg);
            assert!(
                text.ends_with(&format!("</></></><NetTurnRaster>{stage}</></>")),
                "{text}"
            );
            let (back, warnings) = parse_engine_ini(&text);
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(back, cfg);
        }
        for bad in ["3", "-1", "99"] {
            let text = write_engine_ini(&EngineConfig::default()).replace(
                "</></></></>",
                &format!("</></></><NetTurnRaster>{bad}</></>"),
            );
            let (back, warnings) = parse_engine_ini(&text);
            assert_eq!(back.net_turn_raster, 0, "{bad}");
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].contains("NetTurnRaster"), "{warnings:?}");
        }
        // On a base with stage 2 and no key in the file the base stays.
        let base = EngineConfig {
            net_turn_raster: 2,
            ..EngineConfig::default()
        };
        let (back, _) = parse_engine_ini_onto(base, &write_engine_ini(&EngineConfig::default()));
        assert_eq!(back.net_turn_raster, 2);
    }

    #[test]
    fn floats_are_written_like_printf_f() {
        let cfg = EngineConfig {
            gamma: 0.52,
            fog_start: -3.5,
            ..EngineConfig::default()
        };
        let text = write_engine_ini(&cfg);
        assert!(text.contains("<Gamma>0.520000</>"));
        assert!(text.contains("<FogStart>-3.500000</>"));
        let (back, warnings) = parse_engine_ini(&text);
        assert!(warnings.is_empty());
        assert_eq!(back.gamma, 0.52);
    }

    #[test]
    fn net_name_is_unsigned_utf16() {
        let mut cfg = EngineConfig::default();
        let units: Vec<u16> = "Jürgen 😀".encode_utf16().collect();
        cfg.net_name[..units.len()].copy_from_slice(&units);
        cfg.net_name[39] = 0xFFFF;
        let text = write_engine_ini(&cfg);
        assert!(text.contains("<NetName><fixedsize>40</><i0000>74</><i0001>252</>"));
        assert!(text.contains("<i0039>65535</></><NetSession>"));
        // 40 units per field, also for the empty one.
        assert_eq!(text.matches("<fixedsize>40</>").count(), 3);
        let (back, warnings) = parse_engine_ini(&text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(back.net_name, cfg.net_name);
        assert_eq!(back.net_name[1], 252);
        // A negative number is not a valid unit: the field stays.
        let bad = write_engine_ini(&EngineConfig::default()).replacen(
            "<NetName><fixedsize>40</><i0000>0</>",
            "<NetName><fixedsize>40</><i0000>-4</>",
            1,
        );
        let (cfg, warnings) = parse_engine_ini(&bad);
        assert_eq!(cfg.net_name, [0; 40]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    // QUIRK D-245, D-247: deviation
    #[test]
    fn broken_archive_keeps_defaults() {
        for bad in [
            "",
            "<InitFile>",
            "<InitFile><A>1</>",
            "Garbage without tags",
            "<A>1</>",
        ] {
            let (cfg, warnings) = parse_engine_ini(bad);
            assert_eq!(cfg, EngineConfig::default(), "{bad:?}");
            assert_eq!(warnings.len(), 1, "{bad:?}: {warnings:?}");
            assert!(warnings[0].starts_with("Engine.ini"), "{warnings:?}");
            assert!(warnings[0].contains("defaults"), "{warnings:?}");
        }
    }

    #[test]
    fn missing_keys_and_blocks_keep_the_default() {
        let (cfg, warnings) = parse_engine_ini("<InitFile><GameSpeed>3</></>");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.game_speed, 3);
        assert_eq!(cfg.screen_x_size, 1024);
        assert_eq!(cfg.sound.volumes, [349, 599, 499]);
    }

    #[test]
    fn bad_values_keep_the_default_with_a_warning() {
        let text = "<InitFile><ScreenXSize>abc</><Gamma>nan</><GameSpeed>99</>\
                    <HardwareCursor>2</><TimeLimit>99999999999</></>";
        let (cfg, warnings) = parse_engine_ini(text);
        assert_eq!(cfg.screen_x_size, 1024);
        assert_eq!(cfg.gamma, 1.0);
        assert_eq!(cfg.time_limit, 20);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        // Numbers outside the menu ranges are valid for the file (the menus catch them).
        assert_eq!(cfg.game_speed, 99);
        assert!(cfg.hardware_cursor);
    }

    #[test]
    fn wrong_fixedsize_keeps_the_field() {
        let text = "<InitFile><NetName><fixedsize>7</><i0000>65</></>\
                    <RandomMapSeed><fixedsize>40</><i0000>66</><i0001>67</></>\
                    <SoundManager><m_Volumes><fixedsize>2</><i0000>1</><i0001>2</></></>\
                    <GameSpeed>5</></>";
        let (cfg, warnings) = parse_engine_ini(text);
        assert_eq!(cfg.net_name, [0; 40]);
        assert_eq!(cfg.random_map_seed, [0; 40]);
        assert_eq!(cfg.sound.volumes, [349, 599, 499]);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings.iter().all(|w| w.starts_with("Engine.ini")));
        // The remaining keys are read anyway.
        assert_eq!(cfg.game_speed, 5);
    }

    #[test]
    fn unknown_keys_are_ignored_with_a_warning() {
        let text = "<InitFile><GameSpeed>4</><Foreign>1</><Block><x>1</></><MouseSpeed>6</></>";
        let (cfg, warnings) = parse_engine_ini(text);
        assert_eq!(cfg.game_speed, 4);
        assert_eq!(cfg.mouse_speed, 6);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].contains("Foreign") && warnings[0].contains("unknown"));
        assert!(warnings[1].contains("Block"));
    }

    fn keys(list: &[(i32, &str)]) -> Vec<(i32, String)> {
        list.iter().map(|(id, k)| (*id, k.to_string())).collect()
    }

    // QUIRK D-246: deviation
    #[test]
    fn keyboard_block_is_inserted_into_the_base_map() {
        // 0x0049DCE0: no clearing; a known ID gets the new key (place stays), a new ID goes at the end.
        let base = EngineConfig::default().with_default_keys(&keys(&[
            (8, "A"),
            (559, "B"),
            (573, "ALT N"),
        ]));
        let text = "<InitFile><Keyboard><m_KeyMap><mapsize>3</><k0000>559</><v0000>X</>\
                    <k0001>1</><v0001></><k0002>8</><v0002>SHIFT J</></></></>";
        let (cfg, warnings) = parse_engine_ini_onto(base, text);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            cfg.key_map,
            keys(&[(8, "SHIFT J"), (559, "X"), (573, "ALT N"), (1, "")])
        );
        // Without a base the file order results.
        let (cfg, _) = parse_engine_ini(text);
        assert_eq!(cfg.key_map, keys(&[(559, "X"), (1, ""), (8, "SHIFT J")]));
    }

    #[test]
    fn missing_or_broken_keyboard_keeps_the_base_map() {
        let base = EngineConfig::default().with_default_keys(&keys(&[(1, "A"), (2, "B")]));
        // No block, no root block, unreadable file: the base stays.
        let (cfg, warnings) = parse_engine_ini_onto(base.clone(), "<InitFile><GameSpeed>3</></>");
        assert!(warnings.is_empty());
        assert_eq!(cfg.key_map, base.key_map);
        assert_eq!(cfg.game_speed, 3);
        for bad in ["", "<InitFile>", "Garbage", "<A>1</>"] {
            let (cfg, warnings) = parse_engine_ini_onto(base.clone(), bad);
            assert_eq!(cfg, base, "{bad:?}");
            assert_eq!(warnings.len(), 1, "{bad:?}");
        }
        // Block without m_KeyMap or without mapsize: the base stays, warning.
        for bad in [
            "<InitFile><Keyboard></></>",
            "<InitFile><Keyboard><m_KeyMap></></></>",
            "<InitFile><Keyboard><m_KeyMap><mapsize>x</></></></>",
        ] {
            let (cfg, warnings) = parse_engine_ini_onto(base.clone(), bad);
            assert_eq!(cfg.key_map, base.key_map, "{bad:?}");
            assert_eq!(warnings.len(), 1, "{bad:?}: {warnings:?}");
        }
        // mapsize larger than the list: the entries read apply (as in the original), warning.
        let text = "<InitFile><Keyboard><m_KeyMap><mapsize>5</><k0000>2</><v0000>Z</></></></>";
        let (cfg, warnings) = parse_engine_ini_onto(base.clone(), text);
        assert_eq!(cfg.key_map, keys(&[(1, "A"), (2, "Z")]));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        // Second entry with a wrong name: the first applies, the rest does not.
        let text = "<InitFile><Keyboard><m_KeyMap><mapsize>2</><k0000>1</><v0000>Q</>\
                    <k0007>2</><v0007>R</></></></>";
        let (cfg, warnings) = parse_engine_ini_onto(base, text);
        assert_eq!(cfg.key_map, keys(&[(1, "Q"), (2, "B")]));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn bytes_onto_a_base_keep_its_keys() {
        let base = EngineConfig::default().with_default_keys(&keys(&[(1, "A")]));
        let (cfg, warnings) =
            parse_engine_ini_bytes_onto(base.clone(), b"<InitFile><LevelFileName>\xe4</></>");
        assert!(warnings.is_empty());
        assert_eq!(cfg.key_map, base.key_map);
        assert_eq!(cfg.level_file_name, "ä");
    }

    #[test]
    fn sound_block_reads_each_entry_on_its_own() {
        let text = "<InitFile><SoundManager><m_Output>2</><m_IsHardwareOn>1</><m_Volumes>\
                    <fixedsize>3</><i0000>1</><i0001>2</><i0002>3</></><m_New>1</></></>";
        let (cfg, warnings) = parse_engine_ini(text);
        assert_eq!(cfg.sound.output, 2);
        assert!(cfg.sound.is_hardware_on);
        assert_eq!(cfg.sound.driver_id, 0);
        assert_eq!(cfg.sound.volumes, [1, 2, 3]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn texture_array_preload_round_trips() {
        let cfg = EngineConfig {
            texture_array_preload: vec!["a.tga".to_string(), "b c.tga".to_string()],
            ..EngineConfig::default()
        };
        let text = write_engine_ini(&cfg);
        assert!(
            text.contains("<TextureArrayPreLoad><listsize>2</><i0000>a.tga</><i0001>b c.tga</></>")
        );
        let (back, warnings) = parse_engine_ini(&text);
        assert!(warnings.is_empty());
        assert_eq!(back.texture_array_preload, cfg.texture_array_preload);
        // List shorter than `listsize`: the default stays.
        let (cfg, warnings) =
            parse_engine_ini("<InitFile><TextureArrayPreLoad><listsize>3</><i0000>a</></></>");
        assert!(cfg.texture_array_preload.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn bytes_are_windows_1252_both_ways() {
        let bytes = b"<InitFile><ScreenSaverFile>Sch\xf6n\x80\x81</><LevelFileName>\xe4.lvl</></>";
        let (cfg, warnings) = parse_engine_ini_bytes(bytes);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(cfg.screen_saver_file, "Schön\u{20ac}\u{81}");
        assert_eq!(cfg.level_file_name, "ä.lvl");
        let out = write_engine_ini_bytes(&cfg);
        assert!(
            out.windows(10).any(|w| w == b"Sch\xf6n\x80\x81</>"),
            "{}",
            String::from_utf8_lossy(&out)
        );
        // Characters outside cp1252 become "?", never `&#...;`.
        let cfg = EngineConfig {
            screen_saver_file: "a😀b€".to_string(),
            ..EngineConfig::default()
        };
        let out = write_engine_ini_bytes(&cfg);
        let expected = b"a?b\x80</><IsAutoRecord>";
        assert!(out.windows(expected.len()).any(|w| w == expected));
    }
}
