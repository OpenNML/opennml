//! Blood switch of the localisation (`0x006732BC`, byte 4 of the object `0x006732B8`; O-392).
//!
//! In the original the `BloodModeEnabled` line of `localisation.*` sets it (`FUN_00560420`), the cheat `bloodmode`
//! flips it (`FUN_0055FB90`), and `Localisation_IsBloodEnabled` (`FUN_0055FB80`) reads it at all three places that
//! attach blood: arrow (`ArrowProjectile_OnArrive` `0x004EDAD7`), rifle (`PropertyShootProjectile_FireGun`
//! `0x00526CDB`) and the killer's sniping (`0x0051BBAA`). Set: `blood.pef` (`0x00620F44`), otherwise `no_blood.pef`
//! (`0x00620F28`). There are no other readers of the two names.
//!
//! As in the original, the switch belongs to the program, not to the match: it is not in the savegame and applies
//! until the program ends, across new matches and loaded savegames. The reimplementation therefore keeps it
//! process-wide. The simulation always attaches `blood.pef` and reports the cheat as [`SimEvent::BloodToggle`]; the
//! host flips the switch ([`toggle`], in the game host's step) and replaces the path when attaching
//! ([`effect_path`]). Presentation only, the checksum stays the same.
//!
//! [`SimEvent::BloodToggle`]: opennml_sim::state::SimEvent::BloodToggle

use std::sync::atomic::{AtomicBool, Ordering};

use opennml_sim::combat::projectile::BLOOD;

/// Effect when blood is switched off (`0x00620F28`).
pub const NO_BLOOD: &str = "data/effects/no_blood.pef";

/// Initial value set: all three `localisation.{deu,eng,fra}` contain `BloodModeEnabled`. The programs set it at
/// start from the language file they read ([`init`]).
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Initial value from the language file (`Localisation::blood_mode`), once at program start.
pub fn init(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// [`init`] from the language file of the archives (program start of `opennml-game` and the viewer); `language_tag` is
/// the `LanguageTAG` of the engine settings. If it is missing, the initial value (set) stays and a warning goes to
/// the log.
pub fn init_from_localisation(vfs: &opennml_rda::Vfs, language_tag: &str) {
    match opennml_ui::assets::load_localisation(vfs, language_tag) {
        Ok(loc) => init(loc.blood_mode),
        Err(e) => log::warn!("Blood switch: language file not read, blood stays on: {e}"),
    }
}

/// `Localisation_IsBloodEnabled` (`FUN_0055FB80`).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// `Localisation_ToggleBlood` (`FUN_0055FB90`): `b = (b == 0)`.
pub fn toggle() {
    ENABLED.fetch_xor(true, Ordering::Relaxed);
}

/// Path of the effect to attach: with blood switched off, `blood.pef` becomes `no_blood.pef`; all others stay.
pub fn effect_path(path: &str, enabled: bool) -> &str {
    if !enabled && path == BLOOD {
        NO_BLOOD
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The choice of the three readers: set gives `blood.pef`, otherwise `no_blood.pef`; other effects (muzzle flash)
    /// stay.
    // QUIRK O-392: kept
    #[test]
    fn blood_switch_picks_no_blood_only_for_blood() {
        assert_eq!(effect_path(BLOOD, true), BLOOD);
        assert_eq!(effect_path(BLOOD, false), NO_BLOOD);
        let muzzle = "data/effects/normaler_schuss.pef";
        assert_eq!(effect_path(muzzle, false), muzzle);
        assert_eq!(effect_path(muzzle, true), muzzle);
    }

    /// Corpus: both effects are in the archives, and all language files switch blood on (initial value).
    // QUIRK O-392: kept
    #[test]
    fn corpus_has_both_blood_effects_and_blood_on() {
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        for path in [BLOOD, NO_BLOOD] {
            let pef = opennml_formats::pef::parse_pef(&opennml_formats::text::decode_cp1252(
                &vfs.read(path).unwrap(),
            ));
            assert!(!pef.components.is_empty(), "{path}");
        }
        assert!(
            opennml_ui::assets::load_localisation(&vfs, "DEU")
                .unwrap()
                .blood_mode
        );
    }
}
