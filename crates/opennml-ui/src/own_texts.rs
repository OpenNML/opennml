//! Own texts of the reimplementation: IDs from 99 100 on for elements and rows that the original does not have - the
//! network rows in `LEVELINFO` (the selection field `TURNRASTER` in `LAN3` was dropped), the message for a missing
//! map after a map change, and the messages when hosting from a network savegame. One text per edition (German,
//! English); [`crate::assets::Texts::load`] inserts the texts of the detected edition into the table (only numbers
//! that the installation does not occupy itself), afterwards [`crate::assets::Texts::resolve`] resolves them like
//! texts from `localisation.*`. Without a detected edition the German ones apply.

use opennml_rda::testutil::GameVersion;

/// "Turn raster": row in `LEVELINFO`.
pub const TURN_RASTER: i32 = 99_100;
/// First stage of the raster: 99 101 "Original", 99 102 "LAN", 99 103 "LAN fast" (index 0 to 2 of `NetTurnRaster`).
pub const RASTER_STAGE: i32 = 99_101;
/// "Round trip: %d ms": greatest measured round trip in `LEVELINFO` at the host.
pub const ROUND_TRIP: i32 = 99_104;
/// Message of the client when the map is missing or differs after a map change of the host (deviation from E-322);
/// "OK" without a follow-up command, `LAN1` below it.
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

/// All own texts: ID, German, English.
const TABLE: [(i32, &str, &str); 10] = [
    (TURN_RASTER, "Rundenraster", "Turn raster"),
    (RASTER_STAGE, "Original", "Original"),
    (RASTER_STAGE + 1, "LAN", "LAN"),
    (RASTER_STAGE + 2, "LAN schnell", "LAN fast"),
    (ROUND_TRIP, "Laufzeit: %d ms", "Round trip: %d ms"),
    (
        MAP_MISSING,
        "[CENTER]Die Karte der Partie fehlt auf diesem Rechner oder weicht ab. Sie haben die Lobby verlassen.",
        "[CENTER]The game's map is missing on this computer or differs. You have left the lobby.",
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
