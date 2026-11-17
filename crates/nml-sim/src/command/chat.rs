//! Chat `0x5D` and the single-player cheats (re/commands 4.7, re/game-ui D5.7).
//!
//! The command `{0x5000005D, len + 3, slot, mask, len, chars[len]}` comes from the chat window (`FUN_004B9940`).
//! Execution checks the cheats first (`GameWorld_HandleChatCheat` `0x00484C30`); a recognized cheat suppresses the
//! chat line. Otherwise the simulation reports [`SimEvent::Chat`]; the host shows "name: text" if the bit of its
//! slot is set in the mask (`FUN_0042E980`). Stateless except for the cheats.

use super::reject;
use crate::id::PLAYER_SLOTS;
use crate::message::Message;
use crate::sim::Sim;
use crate::state::SimEvent;

/// Message "dying" (`{0x110}` without argument: `0x00484FC5` sets the code, `0x00484FCD` `argc` 0), posted with abort
/// (`GameObject_PostMessage(…, 1)`), like the dying path of `0x10` (`command::ai`).
const DIE: u32 = 0x110;
/// Module 1 (`CPropertyConstructBuilding`, construction worker) that `wheelofmayhem` checks
/// (`GameObject_HasModule(1, 0)`).
const CONSTRUCT_MODULE: usize = 1;

/// Execution of `0x5D` for the sender `sender` (slot field `player`, mask `mask`, text `text`). Deviation (A8): The
/// original takes the slot field unchecked (`GameEngine_GetPlayer(slot)`); the reimplementation requires the sender in
/// it because the cheats act on this slot.
pub(super) fn execute(sim: &mut Sim, sender: u8, player: i32, mask: u32, text: &[u16]) {
    if player != i32::from(sender) {
        reject(
            sim,
            sender,
            "Chat",
            format!("slot {player} is not the sender"),
        );
        return;
    }
    // In network play the cheats are off (`0x00484C30` checks `NetSession_IsActive`): the text goes out as a chat line.
    if !sim.state.network && cheat(sim, sender, text) {
        return;
    }
    sim.state.events.push(SimEvent::Chat {
        from: sender,
        mask,
        text: String::from_utf16_lossy(text),
    });
}

/// The text that `GameWorld_HandleChatCheat` compares: per character the **low byte** (`_Str1[i] = (uchar)word`),
/// read as a C string, i.e. up to before the first null byte (`__mbscmp`): a character like `U+0100` ends the
/// comparison.
pub fn cheat_bytes(text: &[u16]) -> Vec<u8> {
    text.iter()
        .map(|&u| (u & 0xFF) as u8)
        .take_while(|&b| b != 0)
        .collect()
}

/// `GameWorld_HandleChatCheat` (`0x00484C30`) for the sender `player`: `true` if the text is a cheat (then no chat
/// line). Exact comparison on the low bytes ([`cheat_bytes`]), case-sensitive (`__mbscmp`), in the order of the
/// original; the effect always hits the sender, `wheelofmayhem` all slots. Without a slot
/// (`GameEngine_GetPlayer == 0`) no cheat.
///
/// Before the comparison the original checks `NetSession_IsActive()` and allows cheats only without a network session;
/// the reimplementation checks that at the call in [`execute`] (`SimState::network`). This function itself has no lock.
pub fn cheat(sim: &mut Sim, player: u8, text: &[u16]) -> bool {
    let slot = usize::from(player);
    if slot >= PLAYER_SLOTS {
        return false;
    }
    let max_living_space = sim.state.economy.max_living_space;
    let p = &mut sim.state.players[slot];
    match cheat_bytes(text).as_slice() {
        // `0x00484D46`: food (resource 2) + 1000, without limit (`Player_AddResource` `0x004FAE30`).
        b"hoppingbonus" => p.add_resource(2, 1000),
        // `0x00484D61`: wood + 1000.
        b"backwardsbonus" => p.add_resource(1, 1000),
        // `0x00484DB9`: gold + 1000.
        b"goldenmonkey" => p.add_resource(0, 1000),
        // `0x00484E10`: food, wood, gold each + 10000.
        b"wickedwango" => {
            p.add_resource(2, 10000);
            p.add_resource(1, 10000);
            p.add_resource(0, 10000);
        }
        // `0x00484E84`: living space (resource 4) := `GameEngine_GetMaxLivingSpace` (`Engine+0x5EC`).
        b"mudhut" => p.set_resource(4, max_living_space),
        // `0x00484EC7`: `GameEngine_SetPlayerResult(slot, 2)` - victory; a result that is already set stays.
        b"angelpass" => crate::game_end::set_player_result(sim, player, crate::game_end::VICTORY),
        // `0x00484F0A`: damage factor `GameWorld+0x2250[slot]` < 1 -> 100, otherwise 0
        // (`GameWorld_SetDefenseDamageFactor`).
        b"bamboozle!" => {
            let f = &mut sim.state.combat.hit_factor[slot];
            *f = if *f < 1 { 100 } else { 0 };
        }
        // `0x00484F67`: for each slot 0-7 the object table (`GameWorld+0x234`, up to the capacity `+0x2B8`) in index
        // order, every object with module 1 (construction worker) switched on gets `{0x110}` (without argument) with
        // abort - own ones included.
        b"wheelofmayhem" => {
            let ids: Vec<_> = sim.state.tables[..PLAYER_SLOTS]
                .iter()
                .flat_map(|t| t.objects.iter().flatten())
                .filter(|o| o.has_module(CONSTRUCT_MODULE, false))
                .map(|o| o.id)
                .collect();
            for id in ids {
                crate::messaging::post_message(sim, id, Message::new(DIE, &[]), true);
            }
        }
        // `0x0048500E`: `FUN_00476860(−1)` toggles `m_FogShowMode` 0 <-> 7. The switch lives in the host
        // (`FogDisplay::show_mode`, deviation from the original): only the event.
        b"rainbowring" => sim.state.events.push(SimEvent::FogToggle),
        // `0x0048504C`: `FUN_0055FB90` toggles the blood switch `0x006732BC` (`blood.pef`/`no_blood.pef`). The switch
        // lives in the host like the fog switch (O-392): only the event.
        b"bloodmode" => sim.state.events.push(SimEvent::BloodToggle),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::super::testkit;
    use super::super::{Command, CommandKind, Outcome, execute};
    use super::*;
    use crate::message::Posted;
    use crate::save::checksum;
    use crate::state::SimState;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    fn chat(player: i32, mask: u32, text: &str) -> CommandKind {
        CommandKind::Chat {
            player,
            mask,
            text: units(text),
        }
    }

    fn run(sim: &mut Sim, sender: u8, kind: CommandKind) -> Outcome {
        execute(
            sim,
            &Command {
                player: sender,
                kind,
            },
            true,
        )
    }

    /// Stock of the slot in whole units (gold, wood, food, population, living space raw).
    fn stock(state: &SimState, slot: usize) -> [i32; 5] {
        state.players[slot].resources_fx.map(|r| r >> 12)
    }

    /// `goldenmonkey` gives the sender 1000 gold and shows no chat line; an ordinary text reports the line with sender,
    /// mask and text.
    #[test]
    fn goldenmonkey_adds_1000_gold_without_line() {
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        sim.state.events.clear();
        let before = stock(sim.state, 2);
        assert_eq!(
            run(&mut sim, 2, chat(2, 0xFFFF_FFFF, "goldenmonkey")),
            Outcome::Accepted
        );
        let after = stock(sim.state, 2);
        assert_eq!(after[0], before[0] + 1000);
        assert_eq!(after[1..], before[1..]);
        assert!(sim.state.events.is_empty(), "no chat line");
        assert_eq!(run(&mut sim, 2, chat(2, 0b101, "hello")), Outcome::Accepted);
        assert_eq!(
            sim.state.events,
            [SimEvent::Chat {
                from: 2,
                mask: 0b101,
                text: "hello".into(),
            }]
        );
        // A8: slot field != sender -> rejected, without a line and without the cheat.
        sim.state.events.clear();
        let hash = checksum(sim.state);
        assert!(matches!(
            run(&mut sim, 1, chat(2, 1, "goldenmonkey")),
            Outcome::Rejected(why) if why.contains("is not the sender")
        ));
        assert_eq!(checksum(sim.state), hash);
        assert!(sim.state.events.is_empty());
    }

    /// In network play (`SimState::network`) a cheat text is a chat line without a cheat effect.
    // QUIRK E-276, O-325: kept
    #[test]
    fn network_turns_cheats_into_chat() {
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        state.network = true;
        let mut sim = Sim::new(&mut state, &rules);
        sim.state.events.clear();
        let before = stock(sim.state, 2);
        assert_eq!(
            run(&mut sim, 2, chat(2, 0xFFFF_FFFF, "goldenmonkey")),
            Outcome::Accepted
        );
        assert_eq!(stock(sim.state, 2), before, "no cheat effect");
        assert_eq!(
            sim.state.events,
            [SimEvent::Chat {
                from: 2,
                mask: 0xFFFF_FFFF,
                text: "goldenmonkey".into(),
            }]
        );
    }

    /// Every cheat with its effect on the sender, without a chat line.
    // QUIRK O-390, O-392: kept
    #[test]
    fn all_cheats_by_table() {
        let rules = testkit::rules();
        let fresh = || {
            let mut s = testkit::state(&rules);
            for p in &mut s.players {
                p.result = 0;
                p.resources_fx = [100 << 12, 200 << 12, 300 << 12, 7 << 12, 3 << 12];
            }
            s.economy.max_living_space = 250;
            s.events.clear();
            s
        };
        // (text, expected stock of slot 4 afterwards)
        let table: [(&str, [i32; 5]); 5] = [
            ("hoppingbonus", [100, 200, 1300, 7, 3]),
            ("backwardsbonus", [100, 1200, 300, 7, 3]),
            ("goldenmonkey", [1100, 200, 300, 7, 3]),
            ("wickedwango", [10100, 10200, 10300, 7, 3]),
            ("mudhut", [100, 200, 300, 7, 250]),
        ];
        for (text, expected) in table {
            let mut state = fresh();
            let mut sim = Sim::new(&mut state, &rules);
            assert_eq!(run(&mut sim, 4, chat(4, 1, text)), Outcome::Accepted);
            assert_eq!(stock(sim.state, 4), expected, "{text}");
            assert_eq!(
                stock(sim.state, 3),
                [100, 200, 300, 7, 3],
                "{text}: only the sender"
            );
            assert!(sim.state.events.is_empty(), "{text}: no chat line");
        }
        // angelpass: victory of the sender (`set_player_result`); a result that is set stays.
        let mut state = fresh();
        let mut sim = Sim::new(&mut state, &rules);
        run(&mut sim, 4, chat(4, 1, "angelpass"));
        assert_eq!(sim.state.players[4].result, crate::game_end::VICTORY);
        assert!(
            !sim.state
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::Chat { .. })),
            "angelpass: no chat line"
        );
        let mut state = fresh();
        state.players[4].result = crate::game_end::DEFEAT;
        let mut sim = Sim::new(&mut state, &rules);
        run(&mut sim, 4, chat(4, 1, "angelpass"));
        assert_eq!(
            sim.state.players[4].result,
            crate::game_end::DEFEAT,
            "stays"
        );
        // bamboozle!: 100 -> 0 -> 100; only the sender.
        let mut state = fresh();
        let mut sim = Sim::new(&mut state, &rules);
        run(&mut sim, 4, chat(4, 1, "bamboozle!"));
        assert_eq!(sim.state.combat.hit_factor[4], 0);
        run(&mut sim, 4, chat(4, 1, "bamboozle!"));
        assert_eq!(sim.state.combat.hit_factor[4], 100);
        sim.state.combat.hit_factor[4] = 1;
        run(&mut sim, 4, chat(4, 1, "bamboozle!"));
        assert_eq!(sim.state.combat.hit_factor[4], 0, ">= 1 -> 0");
        assert_eq!(sim.state.combat.hit_factor[3], 100);
        assert!(sim.state.events.is_empty());
        // rainbowring: only the event to the host, state unchanged.
        let mut state = fresh();
        let mut sim = Sim::new(&mut state, &rules);
        let hash = checksum(sim.state);
        run(&mut sim, 4, chat(4, 1, "rainbowring"));
        assert_eq!(sim.state.events, [SimEvent::FogToggle]);
        assert_eq!(checksum(sim.state), hash);
        // bloodmode: only the event to the host, without a line, state unchanged (O-392).
        let mut state = fresh();
        let mut sim = Sim::new(&mut state, &rules);
        let hash = checksum(sim.state);
        run(&mut sim, 4, chat(4, 1, "bloodmode"));
        assert_eq!(sim.state.events, [SimEvent::BloodToggle]);
        assert_eq!(checksum(sim.state), hash);
        // No cheat: chat line.
        let mut state = fresh();
        let mut sim = Sim::new(&mut state, &rules);
        run(&mut sim, 4, chat(4, 1, "goldenmonkeys"));
        assert_eq!(stock(sim.state, 4), [100, 200, 300, 7, 3]);
        assert_eq!(sim.state.events.len(), 1);
    }

    /// `wheelofmayhem` sends `{0x110}` with abort to every object with module 1 switched on (construction worker) of
    /// the slots 0-7 in table order, own ones included; buildings without module 1 and objects with module 1 switched
    /// off stay. The dying itself is the path of `0x10` (`{0x110}` with abort, tests in `command::ai`); the colonists
    /// of the test world have no dying module.
    // QUIRK O-391: kept
    #[test]
    fn wheelofmayhem_kills_all_builders_too() {
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        let own = testkit::spawn(&mut sim, testkit::COLONIST, 0, (5, 5));
        let foreign = testkit::spawn(&mut sim, testkit::COLONIST, 3, (9, 9));
        let off = testkit::spawn(&mut sim, testkit::COLONIST, 3, (12, 9));
        let hq = testkit::spawn(&mut sim, testkit::HQ, 0, (20, 20));
        assert!(sim.object(own).unwrap().has_module(CONSTRUCT_MODULE, false));
        assert!(!sim.object(hq).unwrap().has_module(CONSTRUCT_MODULE, true));
        sim.object_mut(off).unwrap().modules[CONSTRUCT_MODULE]
            .as_mut()
            .unwrap()
            .base
            .enabled = false;
        let fifo = |sim: &Sim, id| sim.object(id).unwrap().fifo.clone();
        let before_hq = fifo(&sim, hq);
        let before_off = fifo(&sim, off);
        sim.state.events.clear();
        assert_eq!(
            run(&mut sim, 0, chat(0, 1, "wheelofmayhem")),
            Outcome::Accepted
        );
        // The whole message: `{0x110}` without argument like `0x00484F67` and the dying path of `0x10` (`command::ai`).
        for id in [own, foreign] {
            assert_eq!(
                fifo(&sim, id).back(),
                Some(&Posted::Message {
                    msg: Message::new(DIE, &[]),
                    abort: true
                }),
                "{id:?}"
            );
        }
        assert_eq!(fifo(&sim, hq), before_hq, "building without module 1");
        assert_eq!(fifo(&sim, off), before_off, "module 1 switched off");
        assert!(sim.state.events.is_empty(), "no chat line");
    }

    /// Comparison on the low bytes and up to before the first null byte, case-sensitive. `U+0167` has the low byte
    /// `0x67` ("g"), `U+0100` ends the comparison as a null byte; umlauts stay preserved in the chat text.
    // QUIRK O-390: kept
    #[test]
    fn chat_text_limits_tags_and_low_byte_cheats_in_the_sim() {
        assert_eq!(cheat_bytes(&units("Goldenmonkey")), b"Goldenmonkey");
        let mut low: Vec<u16> = units("goldenmonkey");
        low[0] = 0x0167;
        assert_eq!(cheat_bytes(&low), b"goldenmonkey");
        let mut cut = units("goldenmonkey");
        cut.extend([0x0100, u16::from(b'x')]);
        assert_eq!(cheat_bytes(&cut), b"goldenmonkey");
        assert_eq!(cheat_bytes(&units("mädchen")), b"m\xE4dchen");
        let rules = testkit::rules();
        let mut state = testkit::state(&rules);
        let mut sim = Sim::new(&mut state, &rules);
        sim.state.events.clear();
        let gold = |s: &SimState| s.players[0].resources_fx[0] >> 12;
        let g0 = gold(sim.state);
        // Uppercase: no cheat, but a chat line with the text.
        run(&mut sim, 0, chat(0, 1, "GOLDENMONKEY"));
        assert_eq!(gold(sim.state), g0);
        assert_eq!(sim.state.events.len(), 1);
        // Low byte: cheat.
        sim.state.events.clear();
        run(
            &mut sim,
            0,
            CommandKind::Chat {
                player: 0,
                mask: 1,
                text: low,
            },
        );
        assert_eq!(gold(sim.state), g0 + 1000);
        // Null byte ends the comparison: also a cheat.
        run(
            &mut sim,
            0,
            CommandKind::Chat {
                player: 0,
                mask: 1,
                text: cut,
            },
        );
        assert_eq!(gold(sim.state), g0 + 2000);
        assert!(sim.state.events.is_empty());
        // Umlauts and tags go unchanged into the line.
        run(&mut sim, 0, chat(0, 1, "[COLOR FF00FF00]grün"));
        assert_eq!(
            sim.state.events,
            [SimEvent::Chat {
                from: 0,
                mask: 1,
                text: "[COLOR FF00FF00]grün".into(),
            }]
        );
    }
}
