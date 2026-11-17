//! Chat in the host (re/game-ui D5.7): builds the command `0x5D` from the text of a chat window ([`command`],
//! `FUN_004B9940`) and queues the simulation's chat lines as on-screen text of kind 10 ([`show`], `FUN_0042E980`).
//! Movement, fading and the waiting list of the lines live in `nml_ui::hud::screen_text`; here only "Name: Text"
//! and the color are produced.

use nml_sim::command::{Command, CommandKind};
use nml_sim::id::PLAYER_SLOTS;
use nml_sim::state::SimEvent;
use nml_ui::hud::screen_text::{CHAT_MS, ScreenTexts};

use crate::game::diplomacy::Masks;
use crate::game::sim_host::SimHost;

/// Recipients of the window `window` for the sender `local` (`FUN_004B9940`): 1 all (`0xFFFFFFFF`), 2 every slot
/// q = 0..7 with `GameWorld_HasFriendRelation(local, q)` (the sender is a friend of itself), 3 the sender's chat
/// mask (`FUN_004FAC60`). Other windows: `None`.
pub fn recipients(window: u8, local: u8, sim: &SimHost, masks: &Masks) -> Option<u32> {
    let l = usize::from(local);
    match window {
        1 => Some(0xFFFF_FFFF),
        2 => Some((0..PLAYER_SLOTS as i32).fold(0, |mask, q| {
            if sim.state.diplomacy.is_friend(i32::from(local), q) {
                mask | 1 << q
            } else {
                mask
            }
        })),
        3 => masks.chat.get(l).copied(),
        _ => None,
    }
}

/// `{0x5D, slot, mask, len, characters...}` from window `window` with `text` for the local slot `local`; `None`
/// without text (`wcslen == 0`) or for an unknown window. One UTF-16 unit per character (in the original a 32-bit
/// word from the `wchar_t`).
pub fn command(window: u8, local: u8, text: &str, sim: &SimHost, masks: &Masks) -> Option<Command> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.is_empty() {
        return None;
    }
    let mask = recipients(window, local, sim, masks)?;
    Some(Command {
        player: local,
        kind: CommandKind::Chat {
            player: i32::from(local),
            mask,
            text: units,
        },
    })
}

/// Text of the chat line (`FUN_0042E980`): `Name` (`Player+0xC`), `": "`, text - in the buffer of 1000 characters
/// (`wcscat`); game tags in the text take effect when drawing.
pub fn line(name: &str, text: &str) -> String {
    format!("{name}: {text}")
}

/// The chat lines of a step: for each [`SimEvent::Chat`] whose mask carries the bit of the local slot `local`,
/// "Name: Text" ([`line`], name from `name`) in the color `0x006189D0[Player_GetColor(sender)]` (`color`) for
/// [`CHAT_MS`] ms at (20, `height` - 200) ([`ScreenTexts::push_chat`]). A local slot from 32 on gets nothing.
pub fn show(
    events: &[SimEvent],
    local: u8,
    height: i32,
    name: impl Fn(u8) -> String,
    color: impl Fn(u8) -> u32,
    texts: &mut ScreenTexts,
) {
    for event in events {
        if let SimEvent::Chat { from, mask, text } = event
            && mask
                .checked_shr(u32::from(local))
                .is_some_and(|m| m & 1 != 0)
        {
            texts.push_chat(line(&name(*from), text), color(*from), CHAT_MS, height);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::sim_host::plain_testkit;

    /// Windows 1-3 (`FUN_004B9940`): all, friends including the sender, chat mask; empty text and window 4 give no
    /// command.
    // QUIRK O-325: kept
    #[test]
    fn recipients_of_the_three_windows() {
        let mut sim = plain_testkit::host();
        let d = &mut sim.state.diplomacy;
        for p in 0..8 {
            for q in 0..8 {
                d.set_friend(p, q, false);
            }
        }
        d.set_friend(1, 1, true);
        d.set_friend(1, 3, true);
        d.set_friend(3, 5, true);
        d.set_enemy(1, 5, true);
        let mut masks = Masks::initial();
        masks.chat[1] = 0b1010_0010;
        assert_eq!(recipients(1, 1, &sim, &masks), Some(0xFFFF_FFFF));
        assert_eq!(recipients(2, 1, &sim, &masks), Some(0b0000_1010));
        assert_eq!(recipients(3, 1, &sim, &masks), Some(0b1010_0010));
        assert_eq!(recipients(4, 1, &sim, &masks), None);
        assert_eq!(command(1, 1, "", &sim, &masks), None, "empty");
        assert_eq!(
            command(3, 1, "grüß", &sim, &masks),
            Some(Command {
                player: 1,
                kind: CommandKind::Chat {
                    player: 1,
                    mask: 0b1010_0010,
                    text: "grüß".encode_utf16().collect(),
                },
            })
        );
    }
}
