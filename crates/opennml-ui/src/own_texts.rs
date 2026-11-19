//! Own texts of the reimplementation: IDs from 99 100 on for elements and rows that the original does not have - the
//! network rows in `LEVELINFO` (the selection field `TURNRASTER` in `LAN3` was dropped), the messages when hosting
//! from a network savegame, and the messages of the game data check in the lobby and of a failed network start
//! (re/multiplayer-menus L.18). Placeholders `%s` and `%d` are filled in order when a text is shown ([`fill`]). One
//! text per edition (German, English); [`crate::assets::Texts::load`] inserts the texts of the detected edition into the table (only numbers
//! that the installation does not occupy itself), afterwards [`crate::assets::Texts::resolve`] resolves them like
//! texts from `localisation.*`. Without a detected edition the German ones apply.

use opennml_rda::testutil::GameVersion;

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

/// All own texts: ID, German, English.
const TABLE: [(i32, &str, &str); 16] = [
    (TURN_RASTER, "Rundenraster", "Turn raster"),
    (RASTER_STAGE, "Original", "Original"),
    (RASTER_STAGE + 1, "LAN", "LAN"),
    (RASTER_STAGE + 2, "LAN schnell", "LAN fast"),
    (ROUND_TRIP, "Laufzeit: %d ms", "Round trip: %d ms"),
    (
        MAP_MISSING,
        "[CENTER]Die Karte der Partie fehlt auf diesem Rechner. Sie können erst mitspielen, wenn der Host eine \
         andere Karte wählt.",
        "[CENTER]The game's map is missing on this computer. You can only take part once the host chooses another \
         map.",
    ),
    (
        SAVE_NOT_LOADED,
        "[CENTER]Der Netz-Spielstand lässt sich nicht laden.",
        "[CENTER]The network saved game cannot be loaded.",
    ),
    (
        SAVE_ONE_PLAYER,
        "[CENTER]In diesem Spielstand ist nur noch ein Spieler im Spiel.",
        "[CENTER]Only one player is still in this saved game.",
    ),
    (
        SAVE_SLOT_OUT,
        "[CENTER]Ihr Platz in diesem Spielstand ist schon ausgeschieden.",
        "[CENTER]Your place in this saved game is already out.",
    ),
    (
        REPLAY_NOT_PLAYABLE,
        "[CENTER]Die Aufnahme lässt sich nicht abspielen.",
        "[CENTER]The recording cannot be played back.",
    ),
    (
        DATA_DIFFERS,
        "[CENTER]Ihre Spieldaten für diese Karte weichen vom Host ab: %s. Sie können erst mitspielen, wenn die \
         Dateien übereinstimmen.",
        "[CENTER]Your game data for this map differs from the host's: %s. You can only take part once the files \
         match.",
    ),
    (
        ROW_DIFFERS,
        "[CENTER]Die Spieldaten von %s weichen ab: %s. Die Partie kann so nicht beginnen.",
        "[CENTER]%s's game data differs: %s. The game cannot start like this.",
    ),
    (
        ROW_MAP_MISSING,
        "[CENTER]%s fehlt die Karte der Partie. Die Partie kann so nicht beginnen.",
        "[CENTER]%s does not have the game's map. The game cannot start like this.",
    ),
    (
        LOAD_FAILED,
        "[CENTER]%s konnte die Partie nicht laden. Der Start wurde abgebrochen.",
        "[CENTER]%s could not load the game. The start was cancelled.",
    ),
    (FILES_MORE, " und %d weitere", " and %d more"),
    (FILE_UNKNOWN, "Datei unbekannt", "file unknown"),
];

/// The own text `id` of the edition `version`, otherwise `None` (no own ID).
pub fn text(id: i32, version: GameVersion) -> Option<&'static str> {
    TABLE
        .iter()
        .find(|(known, _, _)| *known == id)
        .map(|&(_, de, en)| match version {
            GameVersion::German => de,
            GameVersion::English => en,
        })
}

/// All own texts of the edition `version` as (ID, text), ascending.
pub fn all(version: GameVersion) -> impl Iterator<Item = (i32, &'static str)> {
    TABLE
        .iter()
        .filter_map(move |&(id, _, _)| text(id, version).map(|t| (id, t)))
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

    /// Both editions fill the same arguments in the same order.
    #[test]
    fn both_columns_have_the_same_placeholders() {
        assert_eq!(TABLE.len(), 16);
        for (id, de, en) in TABLE {
            assert_eq!(placeholders(de), placeholders(en), "text {id}");
        }
        let row = text(ROW_DIFFERS, GameVersion::German).unwrap();
        assert_eq!(placeholders(row), ["%s", "%s"]);
        let more = text(FILES_MORE, GameVersion::English).unwrap();
        assert_eq!(placeholders(more), ["%d"]);
    }

    /// The `MSGBOX` takes only the low byte of every UTF-16 unit as atlas code (O-61, E-296): every own text stays
    /// within U+00FF.
    #[test]
    fn own_texts_fit_the_msgbox() {
        for (id, de, en) in TABLE {
            for t in [de, en] {
                let wide: Vec<char> = t.chars().filter(|&c| u32::from(c) > 0xFF).collect();
                assert!(wide.is_empty(), "text {id}: {wide:?} in {t:?}");
            }
        }
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
