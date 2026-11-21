//! Own texts of the reimplementation: IDs from 99 100 on for elements and rows that the original does not have - the
//! network rows in `LEVELINFO` (the selection field `TURNRASTER` in `LAN3` was dropped), the messages when hosting
//! from a network savegame, and the messages of the game data check in the lobby and of a failed network start
//! (re/multiplayer-menus L.18). Placeholders `%s` and `%d` are filled in order when a text is shown ([`fill`]). One
//! text per edition (German, English) in `texts/de.txt` and `texts/en.txt`; [`crate::assets::Texts::load`] inserts
//! the texts of the detected edition into the table (only numbers that the installation does not occupy itself),
//! afterwards [`crate::assets::Texts::resolve`] resolves them like texts from `localisation.*`. Without a detected
//! edition the language of Windows applies ([`language`]).

use std::sync::LazyLock;

use opennml_rda::{GameVersion, Vfs, detect_version};

use crate::system_language::system_language;

/// "Turn raster": row in `LEVELINFO`.
pub const TURN_RASTER: i32 = 99_100;
/// First stage of the raster: 99 101 "Original", 99 102 "LAN", 99 103 "LAN fast" (index 0 to 2 of `NetTurnRaster`).
pub const RASTER_STAGE: i32 = 99_101;
/// "Round trip: %d ms": greatest measured round trip in `LEVELINFO` at the host.
pub const ROUND_TRIP: i32 = 99_104;
/// Message of a client when the fixed map of the session is missing here or has another map checksum (deviation from
/// E-322 in a new form): the client stays in the lobby until the host chooses another map; "OK" without a follow-up
/// command, the lobby below it.
pub const MAP_MISSING: i32 = 99_105;
/// Message of the host when the chosen network savegame is unreadable, corrupted or not a network savegame
/// (deviation from E-330); "OK" without a follow-up command, `LAN3` below it.
pub const SAVE_NOT_LOADED: i32 = 99_106;
/// Message of the host when fewer than two human slots are in the game in the network savegame.
pub const SAVE_ONE_PLAYER: i32 = 99_107;
/// Message of the host when its own slot in the network savegame has already dropped out.
pub const SAVE_SLOT_OUT: i32 = 99_108;
/// Message when a recording cannot be played (missing, corrupted, other edition or does not fit the game data;
/// deviation from E-343); "OK" without a follow-up command, `LOADAUTOPLAY` below it.
pub const REPLAY_NOT_PLAYABLE: i32 = 99_109;
/// Message of a client whose game data for the map of the session differ from the host's (deviation from E-354):
/// `%s` the file list ([`FILES_MORE`], [`FILE_UNKNOWN`]); "OK" without a follow-up command, the lobby below it.
pub const DATA_DIFFERS: i32 = 99_110;
/// Message of the host when the game data of a row differ: `%s` the player name, `%s` the file list.
pub const ROW_DIFFERS: i32 = 99_111;
/// Message of the host when the map of the session is missing for a row: `%s` the player name.
pub const ROW_MAP_MISSING: i32 = 99_112;
/// Message on every machine when a machine could not load the network match despite the check (deviation from
/// E-352): `%s` the player name; "OK" leads to the main menu.
pub const LOAD_FAILED: i32 = 99_113;
/// Tail of a file list beyond the files shown: `%d` the number of the others.
pub const FILES_MORE: i32 = 99_114;
/// File list of a differing row without a known file.
pub const FILE_UNKNOWN: i32 = 99_115;
/// End message of a network match that stopped on a checksum error: `%s` the name of the savegame.
pub const NET_STOPPED_SAVED: i32 = 99_116;
/// Like [`NET_STOPPED_SAVED`], but the savegame could not be written.
pub const NET_STOPPED_UNSAVED: i32 = 99_117;
/// End message when the host ended the match: `%s` the name of the savegame.
pub const NET_HOST_ENDED_SAVED: i32 = 99_118;
/// Like [`NET_HOST_ENDED_SAVED`], but the savegame could not be written.
pub const NET_HOST_ENDED_UNSAVED: i32 = 99_119;
/// End message when the connection to the host was lost: `%s` the name of the savegame.
pub const NET_HOST_LOST_SAVED: i32 = 99_120;
/// Like [`NET_HOST_LOST_SAVED`], but the savegame could not be written.
pub const NET_HOST_LOST_UNSAVED: i32 = 99_121;
/// Abort with the cause `opennml_net::HOST_LOST` (for example after a failed host migration), without a savegame.
pub const NET_HOST_LOST: i32 = 99_122;
/// Any other abort of the network match: `%s` the cause (foreign text, converted for the `MSGBOX`).
pub const NET_FAILED: i32 = 99_123;
/// Name of a player without a name (empty after cleaning), in the lobby and in game messages.
pub const PLAYER: i32 = 99_124;
/// Name of a player without a name in a game message: `%d` the slot (0-based like the messages).
pub const PLAYER_N: i32 = 99_125;
/// Start of the name of a network savegame (followed by `YYYY_MM_DD_hhmm`).
pub const NET_SAVE_PREFIX: i32 = 99_126;

/// German texts, one line per text (`<ID>=<text>`).
const DE: &str = include_str!("../texts/de.txt");
/// English texts, same format and IDs as [`DE`].
const EN: &str = include_str!("../texts/en.txt");

/// Parses a text file: `<ID>=<text>` per line, the text runs to the end of the line and is not trimmed. Empty lines
/// and lines starting with `#` are skipped. `None` for a line of another form.
fn parse(file: &str) -> Option<Vec<(i32, &str)>> {
    file.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (id, text) = line.split_once('=')?;
            let well_formed = !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
            well_formed.then_some(())?;
            Some((id.parse().ok()?, text))
        })
        .collect()
}

/// Parsed lines of one file: (ID, text).
type Entries = Vec<(i32, &'static str)>;

/// The parsed files (German, English), read once.
fn tables() -> &'static (Entries, Entries) {
    static TABLES: LazyLock<(Entries, Entries)> = LazyLock::new(|| {
        (
            parse(DE).expect("texts/de.txt has the form <ID>=<text>"),
            parse(EN).expect("texts/en.txt has the form <ID>=<text>"),
        )
    });
    &TABLES
}

/// The entries of the edition `version`.
fn entries(version: GameVersion) -> &'static [(i32, &'static str)] {
    match version {
        GameVersion::German => &tables().0,
        GameVersion::English => &tables().1,
    }
}

/// The edition whose own texts apply for the installation `vfs`: the detected edition, otherwise the language of
/// Windows ([`system_language`]).
pub fn language(vfs: &Vfs) -> GameVersion {
    detect_version(vfs).unwrap_or_else(|_| system_language())
}

/// The own text `id` of the edition `version`, otherwise `None` (no own ID).
pub fn text(id: i32, version: GameVersion) -> Option<&'static str> {
    entries(version)
        .iter()
        .find(|(known, _)| *known == id)
        .map(|&(_, text)| text)
}

/// All own texts of the edition `version` as (ID, text), in file order (ascending).
pub fn all(version: GameVersion) -> impl Iterator<Item = (i32, &'static str)> {
    entries(version).iter().copied()
}

/// Fills the placeholders `%s` and `%d` of `text` in order with `args` (numbers formatted by the caller).
/// Placeholders beyond `args` stay as they are, surplus arguments are dropped; an inserted argument is not searched
/// for placeholders again.
pub fn fill(text: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut args = args.iter();
    let mut rest = text;
    while let Some(at) = rest.find('%') {
        let (head, tail) = rest.split_at(at);
        out.push_str(head);
        let placeholder = tail.starts_with("%s") || tail.starts_with("%d");
        match placeholder.then(|| args.next()).flatten() {
            Some(arg) => {
                out.push_str(arg);
                rest = &tail[2..];
            }
            None => {
                out.push('%');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The placeholders `%s`/`%d` of a text in order of appearance.
    fn placeholders(text: &str) -> Vec<&str> {
        let mut found: Vec<(usize, &str)> = text
            .match_indices("%s")
            .chain(text.match_indices("%d"))
            .collect();
        found.sort_unstable();
        found.into_iter().map(|(_, p)| p).collect()
    }

    /// Both files have the same IDs in the same order (ascending, no duplicates, from 99 100 on) and the same
    /// placeholders per text.
    #[test]
    fn both_columns_have_the_same_placeholders() {
        let (de, en) = tables();
        assert_eq!(de.len(), 27);
        assert_eq!(en.len(), de.len());
        for (&(id, de), &(en_id, en)) in de.iter().zip(en) {
            assert_eq!(id, en_id, "same order");
            assert!(id >= 99_100, "text {id}");
            assert_eq!(placeholders(de), placeholders(en), "text {id}");
        }
        assert!(de.windows(2).all(|w| w[0].0 < w[1].0), "ascending, unique");
        let row = text(ROW_DIFFERS, GameVersion::German).unwrap();
        assert_eq!(placeholders(row), ["%s", "%s"]);
        let more = text(FILES_MORE, GameVersion::English).unwrap();
        assert_eq!(placeholders(more), ["%d"]);
    }

    /// The `MSGBOX` takes only the low byte of every UTF-16 unit as atlas code (O-61, E-296): every own text stays
    /// within U+00FF.
    #[test]
    fn own_texts_fit_the_msgbox() {
        for version in [GameVersion::German, GameVersion::English] {
            for (id, t) in all(version) {
                let wide: Vec<char> = t.chars().filter(|&c| u32::from(c) > 0xFF).collect();
                assert!(wide.is_empty(), "text {id}: {wide:?} in {t:?}");
            }
        }
    }

    /// Every line of both files is a comment, empty or `<ID>=...`; the text is not trimmed.
    #[test]
    fn files_parse() {
        for file in [DE, EN] {
            for line in file.lines() {
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let (id, _) = line.split_once('=').expect(line);
                assert!(
                    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
                    "{line}"
                );
            }
            assert!(parse(file).is_some());
        }
        assert_eq!(text(99_114, GameVersion::German), Some(" und %d weitere"));
        assert!(parse("abc").is_none());
        assert!(parse("1 =x").is_none());
        assert_eq!(
            parse(
                "# c

5= x 
"
            ),
            Some(vec![(5, " x ")])
        );
    }

    /// The edition decides the language; without an edition Windows does.
    #[test]
    fn language_follows_the_edition() {
        let dir = tempfile::tempdir().unwrap();
        for (german, expected) in [(true, GameVersion::German), (false, GameVersion::English)] {
            let sub = dir.path().join(if german { "de" } else { "en" });
            opennml_rda::testutil::write_fake_install(&sub, 11, german);
            let vfs = Vfs::open_game_dir(&sub).unwrap();
            assert_eq!(language(&vfs), expected);
        }
        let sub = dir.path().join("none");
        opennml_rda::testutil::write_fake_install(&sub, 3, true);
        let vfs = Vfs::open_game_dir(&sub).unwrap();
        // Without an edition the OS decides, so the test can only check that the result is `system_language()`.
        assert_eq!(language(&vfs), system_language());
    }

    #[test]
    fn fill_replaces_in_order() {
        assert_eq!(
            fill("%s's data: %s.", &["Bert", "a.ini"]),
            "Bert's data: a.ini."
        );
        assert_eq!(fill(" and %d more", &["2"]), " and 2 more");
        // Mixed in order of appearance; an argument is not searched again.
        assert_eq!(fill("%d / %s", &["%s", "x"]), "%s / x");
        // Placeholders beyond the arguments stay, a lone percent sign too; surplus arguments are dropped.
        assert_eq!(fill("%s and %s", &["a"]), "a and %s");
        assert_eq!(fill("100% %s", &["sure"]), "100% sure");
        assert_eq!(fill("none", &["x"]), "none");
        assert_eq!(fill("%", &[]), "%");
        let de = text(LOAD_FAILED, GameVersion::German).unwrap();
        assert_eq!(
            fill(de, &["Bert"]),
            "[CENTER]Bert konnte die Partie nicht laden. Der Start wurde abgebrochen."
        );
        let en = text(DATA_DIFFERS, GameVersion::English).unwrap();
        assert_eq!(
            fill(en, &["data/gifcon/bonus.ini"]),
            "[CENTER]Your game data for this map differs from the host's: data/gifcon/bonus.ini. You can only \
             take part once the files match."
        );
    }
}
