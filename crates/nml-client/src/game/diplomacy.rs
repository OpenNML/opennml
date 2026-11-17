//! Diplomacy in the host: the chat and tribute masks of the eight slots and the host part of the diplomacy dialog
//! `DIPLOMATIE` (re/game-ui D5.7): [`view`] builds the dialog's state, [`apply`] carries out its changes. The signal
//! and the chat read the chat mask.
//!
//! In the original the masks live in the player object (`Player+0x35C` chat mask `m_ChatMask`, `+0x360` tribute mask
//! `m_TributMask`; getter of the chat mask `0x004FAC60`) and in the player part of the savegame. They do not go over
//! the network and not into the simulation: the reimplementation keeps them in the host part (`HostPart::masks`, a
//! deviation from the original), without any effect on the checksum.

use serde::{Deserialize, Serialize};

use nml_sim::command::{Command, CommandKind};
use nml_sim::id::PLAYER_SLOTS;
use nml_ui::screens::diplomacy::{COLOR_TEXT, NATION_TEXT, TRIBUTE_AMOUNT};
use nml_ui::screens::{DiplomacyAction, DiplomacyRow, DiplomacyView};

use crate::game::sim_host::SimHost;

/// Chat and tribute mask per slot: bit q set means slot q receives chat lines or signals (chat) or is selected as
/// tribute recipient (tribute).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Masks {
    /// `m_ChatMask` (`Player+0x35C`) per slot.
    pub chat: [u32; PLAYER_SLOTS],
    /// `m_TributMask` (`Player+0x360`) per slot.
    pub tribute: [u32; PLAYER_SLOTS],
}

impl Masks {
    /// State at level start (`Player_InitSlot` `0x004FA960`, for all eight slots from `FUN_0043BB70`, also for empty
    /// ones): per slot the own bit, ORed onto the constructor's initial value 0 (`Player_Construct` `0x004FAF60`).
    /// Loading overwrites both masks with the savegame.
    pub fn initial() -> Self {
        let own: [u32; PLAYER_SLOTS] = std::array::from_fn(|slot| 1 << slot);
        Self {
            chat: own,
            tribute: own,
        }
    }
}

/// Set or clear bit `slot` in `mask` (`FUN_004FAC00` chat, `FUN_004FAC70` tribute); slots outside 0-7 do
/// nothing.
fn set_bit(mask: &mut u32, slot: usize, on: bool) {
    if slot < PLAYER_SLOTS {
        if on {
            *mask |= 1 << slot;
        } else {
            *mask &= !(1 << slot);
        }
    }
}

/// `FUN_0042ED20(l, N)`: enemy -> 1, otherwise friend -> 0, otherwise 2 (neutral).
fn relation(sim: &SimHost, local: i32, slot: i32) -> i32 {
    let d = &sim.state.diplomacy;
    if d.is_enemy(local, slot) {
        1
    } else if d.is_friend(local, slot) {
        0
    } else {
        2
    }
}

/// State of the dialog for the local slot `local` (table in re/game-ui D5.7): per slot N the display name `name(N)`
/// (the host passes `game_flow::player_display_name`, `Player+0xC`), color name 470 + color, nation 15120 + nation,
/// result `res(N)`, relation `FUN_0042ED20(l, N)`, vision `HasSharedVision(N, l)` (`FUN_0042EDB0`), the bits N of the
/// chat and tribute mask of `l`, and the fixed team choice (`Engine+0x20`, `m_IsLockTeams`).
pub fn view(
    sim: &SimHost,
    masks: &Masks,
    local: u8,
    locked_teams: bool,
    name: impl Fn(i32) -> String,
) -> DiplomacyView {
    let l = usize::from(local).min(PLAYER_SLOTS - 1);
    let rows = std::array::from_fn(|slot| {
        let n = slot as i32;
        let player = &sim.state.players[slot];
        DiplomacyRow {
            slot: slot as u8,
            name: name(n),
            color_text: COLOR_TEXT + player.color,
            nation_text: NATION_TEXT + player.nation,
            result: player.result,
            status: relation(sim, l as i32, n),
            view: sim.state.diplomacy.shares_vision(n, l as i32),
            chat: masks.chat[l] & (1 << slot) != 0,
            tribute: masks.tribute[l] & (1 << slot) != 0,
        }
    });
    DiplomacyView {
        local,
        rows,
        locked_teams,
    }
}

/// A change of the dialog for the local slot `local`. Commands go through [`SimHost::submit`] (while the dialog is
/// open the match stands still, they take effect immediately and appear in the command log); the host sets the masks
/// locally without a command, like the original:
/// - `Status` -> `{0x64, l, N, choice}` (`FUN_0042ED70`, one-sided; message 231-233 to all, even without a change,
///   O-247);
/// - `View` -> `{0x63, N, l, on}` (`FUN_0042EDD0`: N sees what I see; O-322);
/// - `Chat`/`Tribute` -> bit N of the mask of `l`;
/// - `ChatBy` (`FUN_004BA3F0`) -> chat mask for the slots `!= l` with `res == 0` by relation (-1 all, 0 friends,
///   1 enemies, 2 neutral = neither); the other bits stay, also those of eliminated players (C-231);
/// - `SendTribute` (`FUN_004BA4D0`) -> `{0x65, l, tribute mask, resource, 100}` without a pre-check (that is done by
///   `GameWorld_CmdSendTribute` per recipient);
/// - `Close` is only a report.
pub fn apply(action: DiplomacyAction, local: u8, sim: &mut SimHost, masks: &mut Masks) {
    let l = usize::from(local);
    if l >= PLAYER_SLOTS {
        return;
    }
    let me = i32::from(local);
    let submit = |sim: &mut SimHost, kind| {
        sim.submit(Command {
            player: local,
            kind,
        })
    };
    match action {
        DiplomacyAction::Status { slot, mode } => submit(
            sim,
            CommandKind::Diplomacy {
                p: me,
                q: i32::from(slot),
                mode,
            },
        ),
        DiplomacyAction::View { slot, on } => submit(
            sim,
            CommandKind::SharedVision {
                p: i32::from(slot),
                q: me,
                on,
            },
        ),
        DiplomacyAction::Chat { slot, on } => set_bit(&mut masks.chat[l], usize::from(slot), on),
        DiplomacyAction::Tribute { slot, on } => {
            set_bit(&mut masks.tribute[l], usize::from(slot), on)
        }
        DiplomacyAction::ChatBy { relation: mode } => {
            for q in 0..PLAYER_SLOTS {
                if q == l || sim.state.players[q].result != 0 {
                    continue;
                }
                let qi = q as i32;
                let d = &sim.state.diplomacy;
                let on = match mode {
                    0 => d.is_friend(me, qi),
                    1 => d.is_enemy(me, qi),
                    2 => !d.is_friend(me, qi) && !d.is_enemy(me, qi),
                    _ => true,
                };
                set_bit(&mut masks.chat[l], q, on);
            }
        }
        DiplomacyAction::SendTribute { resource } => submit(
            sim,
            CommandKind::Tribute {
                from: me,
                recipients: masks.tribute[l],
                resource,
                amount: TRIBUTE_AMOUNT,
            },
        ),
        DiplomacyAction::Close => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::sim_host::plain_testkit;
    use nml_sim::state::SimEvent;

    /// `Player_InitSlot`: per slot exactly the own bit in both masks.
    #[test]
    fn initial_masks_hold_the_own_bit() {
        let m = Masks::initial();
        assert_eq!(m.chat, [1, 2, 4, 8, 0x10, 0x20, 0x40, 0x80]);
        assert_eq!(m.tribute, m.chat);
    }

    /// Match of the fake with `n` occupied slots (result 0), halted as with an open dialog: commands take effect
    /// immediately.
    fn halted(n: usize) -> SimHost {
        let mut host = plain_testkit::host();
        for p in host.state.players.iter_mut().take(n) {
            p.slot_active = true;
            p.result = 0;
        }
        host.halted = true;
        host
    }

    fn last_command(host: &SimHost) -> CommandKind {
        host.log.last().expect("a command").command.kind.clone()
    }

    /// Tribute mask `0x05` (slots 0 and 2), "Transfer gold" -> `{0x65, 0, 0x05, 0, 100}`; with gold >= 100 slot 2
    /// gets 100 gold (message 230 via `TributeSent`), with gold 50 message 2 and nothing booked.
    #[test]
    fn tribute_d1() {
        for (gold, paid) in [(150, true), (50, false)] {
            let mut host = halted(3);
            host.state.players[0].set_resource(0, gold);
            host.state.players[2].set_resource(0, 0);
            let mut masks = Masks::initial();
            masks.tribute[0] = 0x05;
            apply(
                DiplomacyAction::SendTribute { resource: 0 },
                0,
                &mut host,
                &mut masks,
            );
            assert_eq!(
                last_command(&host),
                CommandKind::Tribute {
                    from: 0,
                    recipients: 0x05,
                    resource: 0,
                    amount: 100
                }
            );
            assert!(host.pending_commands().is_empty(), "executed immediately");
            let events = &host.state.events;
            let sent = events.contains(&SimEvent::TributeSent {
                giver: 0,
                recipient: 2,
            });
            let lack = events
                .iter()
                .any(|e| matches!(e, SimEvent::PlayerMessage { id: 2, .. }));
            assert_eq!((sent, lack), (paid, !paid), "Gold {gold}");
            let (giver, recipient) = (host.state.resource(0, 0), host.state.resource(2, 0));
            if paid {
                assert_eq!((giver, recipient), (gold - 100, 100));
            } else {
                assert_eq!((giver, recipient), (gold, 0), "nothing booked");
            }
        }
    }

    /// Slot 0, row 3 "Enemy" -> "Allied" (0): `{0x64, 0, 3, 0}`, message 231 to all with
    /// `(3 | 0x100) << 8 | 0 = 0x10300`.
    #[test]
    fn status_d2() {
        let mut host = halted(4);
        assert!(host.state.diplomacy.is_enemy(0, 3));
        let mut masks = Masks::initial();
        apply(
            DiplomacyAction::Status { slot: 3, mode: 0 },
            0,
            &mut host,
            &mut masks,
        );
        assert_eq!(
            last_command(&host),
            CommandKind::Diplomacy {
                p: 0,
                q: 3,
                mode: 0
            }
        );
        let d = &host.state.diplomacy;
        assert!(d.is_friend(0, 3) && !d.is_enemy(0, 3));
        assert!(d.is_enemy(3, 0), "one-sided");
        assert!(host.state.events.iter().any(|e| matches!(
            e,
            SimEvent::PlayerMessage {
                to: None,
                id: 231,
                flags,
                ..
            } if flags & 0xFFFFF == 0x10300
        )));
        assert_eq!(masks, Masks::initial(), "no mask");
    }

    /// Vision check mark in row 3 -> `{0x63, 3, 0, 1}` -> `View[3]` bit 0 (N sees what I see).
    #[test]
    fn view_d3() {
        let mut host = halted(4);
        let mut masks = Masks::initial();
        apply(
            DiplomacyAction::View { slot: 3, on: true },
            0,
            &mut host,
            &mut masks,
        );
        assert_eq!(
            last_command(&host),
            CommandKind::SharedVision {
                p: 3,
                q: 0,
                on: true
            }
        );
        assert!(host.state.diplomacy.shares_vision(3, 0));
        assert!(!host.state.diplomacy.shares_vision(0, 3));
        let v = view(&host, &masks, 0, false, |_| String::new());
        assert!(v.rows[3].view && v.rows[0].view && !v.rows[1].view);
    }

    /// 7 occupied slots: slot 0; 2 allied, 5 enemy, 6 neutral, 4 eliminated. "Allied players" -> bit 2 on, 5 and 6
    /// off, bit 0 (own) and bit 4 (eliminated) unchanged; the other buttons by relation.
    #[test]
    fn chat_by_relation_d4() {
        let scene = || {
            let mut host = halted(7);
            let d = &mut host.state.diplomacy;
            for q in 1..8 {
                d.set_enemy(0, q, false);
            }
            d.set_friend(0, 2, true);
            d.set_enemy(0, 5, true);
            host.state.players[4].result = 1;
            host
        };
        let base = Masks {
            chat: [0x10 | 0x20 | 0x40 | 0x01, 2, 4, 8, 0x10, 0x20, 0x40, 0x80],
            ..Masks::initial()
        };
        let chat_after = |mode| {
            let mut masks = base;
            let mut host = scene();
            apply(
                DiplomacyAction::ChatBy { relation: mode },
                0,
                &mut host,
                &mut masks,
            );
            assert!(host.log.is_empty(), "no command");
            masks.chat[0]
        };
        // Bit 7 is unoccupied (result 3): stays like bit 4.
        assert_eq!(chat_after(0), 0x01 | 0x04 | 0x10);
        assert_eq!(chat_after(1), 0x01 | 0x10 | 0x20);
        assert_eq!(chat_after(2), 0x01 | 0x02 | 0x08 | 0x10 | 0x40);
        assert_eq!(
            chat_after(-1),
            0x01 | 0x02 | 0x04 | 0x08 | 0x10 | 0x20 | 0x40
        );
        // Single bits locally; slots outside 0-7 do nothing.
        let mut host = scene();
        let mut masks = base;
        apply(
            DiplomacyAction::Chat { slot: 4, on: false },
            0,
            &mut host,
            &mut masks,
        );
        apply(
            DiplomacyAction::Tribute { slot: 3, on: true },
            0,
            &mut host,
            &mut masks,
        );
        apply(
            DiplomacyAction::Tribute { slot: 9, on: true },
            0,
            &mut host,
            &mut masks,
        );
        assert_eq!(masks.chat[0], 0x01 | 0x20 | 0x40);
        assert_eq!(masks.tribute[0], 0x01 | 0x08);
        assert_eq!(masks.tribute[1..], Masks::initial().tribute[1..]);
        assert!(host.log.is_empty());
    }

    /// State of the dialog: name, color 470 + color, nation 15120 + nation, result, relation, masks, fixed teams.
    #[test]
    fn view_reads_players_relations_and_masks() {
        let mut host = halted(3);
        host.state.players[1].color = 5;
        host.state.players[1].nation = 2;
        host.state.players[1].name = "Hugo".into();
        host.state.diplomacy.set_enemy(0, 2, false);
        let mut masks = Masks::initial();
        masks.chat[0] = 0x03;
        masks.tribute[0] = 0x05;
        let v = view(&host, &masks, 0, true, |n| {
            host.state.players[n as usize].name.clone()
        });
        assert_eq!((v.local, v.locked_teams), (0, true));
        let r = &v.rows[1];
        assert_eq!(
            (r.name.as_str(), r.color_text, r.nation_text, r.result),
            ("Hugo", 475, 15122, 0)
        );
        assert_eq!(
            [v.rows[0].status, v.rows[1].status, v.rows[2].status],
            [0, 1, 2]
        );
        assert_eq!(v.rows[5].result, 3, "unoccupied");
        let chat: Vec<bool> = v.rows.iter().map(|r| r.chat).collect();
        let tribute: Vec<bool> = v.rows.iter().map(|r| r.tribute).collect();
        assert_eq!(&chat[..3], [true, true, false]);
        assert_eq!(&tribute[..3], [true, false, true]);
    }
}
