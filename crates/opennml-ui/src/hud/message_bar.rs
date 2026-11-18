//! Message bar of the game UI: one queue per display kind, enqueuing at the anchor, sound, ping and display duration
//! at the moment of appearing, tick in game time, drawing of the head per kind; plus the game-time clock of the bar and
//! the action stack of the ping locations. Without simulation: events arrive as message number, location and flag word,
//! game values via [`GameText`].
//!
//! Basis: re/game-ui N.7 (N.7.1-N.7.4); quirks O-42, O-210 ... O-217, O-219. Computation uses 32-bit integers as in the
//! original (wrapping, division toward zero).

use std::collections::{BTreeMap, VecDeque};

use opennml_formats::messages::{MessageTag, Messages, flags as def_flags};

use crate::coords::{game_x, game_y};
use crate::dialog::UiContext;
use crate::draw::DrawCmd;
use crate::text::{
    GameTags, GameText, LayoutParams, TextLayout, draw_runs, layout_flags, layout_text_game,
    resolve_game_tags,
};

/// Bits of the flag word of a message (`Msg[0]`, re/game-ui N.7.1/N.7.2).
pub mod bar_flags {
    /// Caller: enqueue; without the bit, a busy kind discards the message (O-42).
    pub const QUEUE: u32 = 0x1_0000;
    /// `PING` of the def (or the caller bit of the discard branch): ping and action stack, if a location is present.
    pub const PING: u32 = 0x800;
    /// `SOUND` of the def: sound at appearance.
    pub const SOUND: u32 = 0x400;
    /// `SOUNDONCE` of the def: sound at appearance, then a 15 s lock per kind.
    pub const SOUND_ONCE: u32 = 0x1000;
    /// Priority in the top byte, compared signed.
    pub const PRIORITY: u32 = 0xFF00_0000;
}

/// Fade remainder at appearance (`0x00492740`: `msg[3] = 1500`).
pub const FADE_MS: i32 = 1500;
/// SOUNDONCE lock per kind after a sound (`lock[kind] = 15000`).
pub const SOUND_ONCE_MS: i32 = 15_000;
/// Repeats of the ping at appearance (`GameWorld_Ping(..., 5)`).
pub const PING_REPEATS: i32 = 5;
/// Repeats of the ping in the discard branch (`0x00494B69`: only with caller bit `0x800`).
pub const IGNORED_PING_REPEATS: i32 = 1;
/// Location "no location" (`FUN_00415A90` compares with `(-1, -1)`).
pub const NO_TILE: (i32, i32) = (-1, -1);
/// Switches of the text layout of the messages (`0x56`, `MessageBar_ShowMessage` `0x00494CB5`).
pub const LAYOUT_FLAGS: u32 = layout_flags::START_FONT
    | layout_flags::START_COLOR
    | layout_flags::UNUSED_10
    | layout_flags::WIDTH;
/// Maximum number of locations in the action stack (`0x0047F1A0`: `RemoveTail` while more than 5).
pub const ACTION_STACK_LEN: usize = 5;

/// A message in the queue (`Msg`, `0x3C` bytes, re/game-ui N.7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarMessage {
    /// MSGID.
    pub id: i32,
    /// Flag word: caller | [`bar_flags::PING`] | [`bar_flags::SOUND`] | [`bar_flags::SOUND_ONCE`] of the def.
    pub flags: u32,
    /// Sound GUID from the sound field of the def (also inherited, without the sound bit; O-216).
    pub sound: i32,
    /// Display duration (ms of game time), set only at appearance.
    pub display_ms: i32,
    /// Fade remainder (ms of game time), [`FADE_MS`] at appearance.
    pub fade_ms: i32,
    /// Location (tile), [`NO_TILE`] = no location.
    pub tile: (i32, i32),
    /// Text ID of the def (key of the duplicate check instead of the text pointer).
    pub text_id: i32,
    /// Raw text with tags from the text table; if it is missing, the number (O-241).
    pub text: String,
    /// Player bytes of the parameter block (`[0xB]` = caller flags `& 0xFFFF`, before OR-ing in the def bits).
    pub players: u16,
}

/// Return value of `MessageBar_ShowMessage` (`0x00494A60`); names from the console `igmsg` (`0x0041A5D0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowResult {
    /// 0 "not exist": def missing, without `MSGTYPE`, or kind unknown.
    NotExist,
    /// 1 "Ok": the queue was empty, the message appears immediately.
    Shown,
    /// 2 "cached": enqueued behind the anchor.
    Cached,
    /// 3 "ignored": discarded (without enqueuing when the kind is busy, or equal at the anchor).
    Ignored,
}

impl ShowResult {
    /// Number of the original (0-3).
    pub fn code(self) -> i32 {
        match self {
            Self::NotExist => 0,
            Self::Shown => 1,
            Self::Cached => 2,
            Self::Ignored => 3,
        }
    }
}

/// Effect of the bar that the host carries out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarEffect {
    /// Sound GUID, 2D (`SoundManager_PlaySoundGuid2D(guid, 0x12, 1, 0)`).
    Sound(i32),
    /// `GameWorld_Ping(x, z, color, repeats)` (`0x00480C00`): location first onto the action stack, then minimap ping.
    /// Color = `COLOR` of the kind (full ARGB).
    Ping {
        tile: (i32, i32),
        color: u32,
        repeats: i32,
    },
}

/// The message bar (`this` of `MessageBar_ShowMessage`): defs, texts, kinds in file order, one queue per kind and one
/// SOUNDONCE lock counter.
#[derive(Debug, Clone)]
pub struct MessageBar {
    messages: Messages,
    texts: BTreeMap<i32, String>,
    /// `EngineConfig.TimePerChar` (`+0x50`, shipped data 55).
    time_per_char: i32,
    /// Kinds by index (file order, `+0x7C`).
    tags: Vec<MessageTag>,
    queues: Vec<VecDeque<BarMessage>>,
    /// SOUNDONCE lock counter per kind (`+0x84`, ms of game time).
    sound_lock: Vec<i32>,
    /// Cinemascope (`Engine+0x104`): messages run invisibly and silently (O-217).
    pub cinemascope: bool,
}

impl MessageBar {
    /// Bar from `messages.ini` and the text table (`texte.deu`): one empty queue per kind in file order, lock counters
    /// 0 (`0x00493BC0`).
    pub fn new(messages: &Messages, texts: &BTreeMap<i32, String>, time_per_char: i32) -> Self {
        let tags: Vec<MessageTag> = messages.tags_in_order().into_iter().cloned().collect();
        let n = tags.len();
        Self {
            messages: messages.clone(),
            texts: texts.clone(),
            time_per_char,
            tags,
            queues: vec![VecDeque::new(); n],
            sound_lock: vec![0; n],
            cinemascope: false,
        }
    }

    /// Position of the kind with index `index` in [`MessageBar::tags`] (the indices are dense, O-249).
    fn slot(&self, tag: &MessageTag) -> Option<usize> {
        self.tags.iter().position(|t| t.index == tag.index)
    }

    /// `MessageBar_ShowMessage(id, location, flags)` (`0x00494A60`, re/game-ui N.7.2). Effects (sound, ping) land in
    /// `out`; an enqueued message gets sound, ping and duration only at appearance (O-212).
    pub fn show(
        &mut self,
        id: i32,
        tile: (i32, i32),
        flags: u32,
        out: &mut Vec<BarEffect>,
    ) -> ShowResult {
        let Some(def) = self.messages.get(id) else {
            return ShowResult::NotExist;
        };
        let Some(tag) = self.messages.tag_of(def) else {
            return ShowResult::NotExist;
        };
        let Some(slot) = self.slot(tag) else {
            return ShowResult::NotExist;
        };
        let color = tag.color;
        let queue = &self.queues[slot];
        let mut anchor = None;
        if !queue.is_empty() {
            if flags & bar_flags::QUEUE == 0 {
                // `0x00494B69`: discarded without sound (O-42); a ping only with the caller bit 0x800 and a location.
                if flags & bar_flags::PING != 0 && tile != NO_TILE {
                    out.push(BarEffect::Ping {
                        tile,
                        color,
                        repeats: IGNORED_PING_REPEATS,
                    });
                }
                return ShowResult::Ignored;
            }
            // `0x00494BC9` (O-210): first node from the head with priority >= its own (signed), otherwise the last
            // one.
            let own = (flags & bar_flags::PRIORITY) as i32;
            anchor = Some(
                queue
                    .iter()
                    .position(|m| (m.flags & bar_flags::PRIORITY) as i32 >= own)
                    .unwrap_or(queue.len() - 1),
            );
        }
        let mut merker = flags;
        if def.flags & def_flags::PING != 0 {
            merker |= bar_flags::PING;
        }
        if def.flags & def_flags::SOUND != 0 {
            merker |= bar_flags::SOUND;
        }
        if def.flags & def_flags::SOUND_ONCE != 0 {
            merker |= bar_flags::SOUND_ONCE;
        }
        // `FUN_0055FFE0(TextID)` without checking the TEXT bit: the text field of the def (also inherited). If the text
        // is missing, the number (O-241). Deviation without effect in the shipped data (all message texts are present):
        // the original writes it into the shared buffer (`FUN_0055FFE0`, `+0x44`, O-4), which later misses overwrite;
        // the reimplementation keeps its own number and checks duplicates via the text ID.
        let text_id = def.text;
        let text = self
            .texts
            .get(&text_id)
            .cloned()
            .unwrap_or_else(|| text_id.to_string());
        let mut msg = BarMessage {
            id,
            flags: merker,
            sound: def.sound,
            display_ms: 0,
            fade_ms: 0,
            tile,
            text_id,
            text,
            players: (flags & 0xFFFF) as u16,
        };
        match anchor {
            None => {
                appear(
                    &mut msg,
                    &mut self.sound_lock[slot],
                    color,
                    self.time_per_char,
                    self.cinemascope,
                    out,
                );
                self.queues[slot].push_back(msg);
                ShowResult::Shown
            }
            Some(a) => {
                // `0x00494D25` (O-211): checked only against the anchor - whole flag word, text, location.
                let at = &self.queues[slot][a];
                if at.flags == msg.flags && at.text_id == msg.text_id && at.tile == msg.tile {
                    return ShowResult::Ignored;
                }
                self.queues[slot].insert(a + 1, msg);
                ShowResult::Cached
            }
        }
    }

    /// Tick `0x00494570(dt)` with `dt` ms of game time, per kind (re/game-ui N.7.3): lock `max(0, s - dt)`; head with
    /// duration >= 1 -> duration - dt; otherwise with `dt < remainder` remainder - dt, if another message is waiting
    /// immediately 0; otherwise remove the head and the next one appears. No carry-over between the phases.
    pub fn tick(&mut self, dt: i32, out: &mut Vec<BarEffect>) {
        for slot in 0..self.queues.len() {
            let lock = &mut self.sound_lock[slot];
            *lock = lock.wrapping_sub(dt).max(0);
            let queue = &mut self.queues[slot];
            let waiting = queue.len() >= 2;
            let Some(head) = queue.front_mut() else {
                continue;
            };
            if head.display_ms >= 1 {
                head.display_ms = head.display_ms.wrapping_sub(dt);
            } else if dt < head.fade_ms {
                head.fade_ms = if waiting {
                    0
                } else {
                    head.fade_ms.wrapping_sub(dt)
                };
            } else {
                queue.pop_front();
                if let Some(next) = queue.front_mut() {
                    appear(
                        next,
                        &mut self.sound_lock[slot],
                        self.tags[slot].color,
                        self.time_per_char,
                        self.cinemascope,
                        out,
                    );
                }
            }
        }
    }

    /// The head of the queue per kind in file order (kinds with an empty queue are missing).
    pub fn heads(&self) -> impl Iterator<Item = (&MessageTag, &BarMessage)> {
        self.tags
            .iter()
            .zip(&self.queues)
            .filter_map(|(tag, q)| Some((tag, q.front()?)))
    }

    /// The kinds in file order.
    pub fn tags(&self) -> &[MessageTag] {
        &self.tags
    }

    /// Queue of the kind at position `tag` (file order, like [`MessageBar::tags`]); empty for an unknown one.
    pub fn queue(&self, tag: usize) -> &VecDeque<BarMessage> {
        static EMPTY: VecDeque<BarMessage> = VecDeque::new();
        self.queues.get(tag).unwrap_or(&EMPTY)
    }

    /// SOUNDONCE lock counter of the kind at position `tag` (ms of game time).
    pub fn sound_lock(&self, tag: usize) -> i32 {
        self.sound_lock.get(tag).copied().unwrap_or(0)
    }

    /// After loading or restart: all queues and locks empty (not in the savegame, N.7.4).
    pub fn clear(&mut self) {
        self.queues.iter_mut().for_each(VecDeque::clear);
        self.sound_lock.iter_mut().for_each(|s| *s = 0);
    }

    /// Color of the head (`0x00493570`): `(COLOR & 0xFFFFFF) | (remainder * 255 / 1500) << 24` (O-214).
    fn head_color(tag: &MessageTag, head: &BarMessage) -> u32 {
        let alpha = head.fade_ms.wrapping_mul(255) / FADE_MS;
        (tag.color & 0x00FF_FFFF) | (alpha as u32).wrapping_shl(24)
    }

    /// Drawing `0x00493B80` -> `0x00493570` (re/game-ui N.7.4): only without cinemascope, per kind in file order the
    /// head; text layout with [`LAYOUT_FLAGS`] to the full screen width W, font `FONT` of the kind, player bytes of the
    /// head; `x = POS.x < 0 ? W - width : W * POS.x / 1280`, `y = H * POS.y / 1024` (truncating), with a shadow.
    pub fn draw(&self, ctx: &UiContext<'_>, game: &dyn GameText, out: &mut Vec<DrawCmd>) {
        if self.cinemascope {
            return;
        }
        let screen = ctx.screen;
        for (tag, head) in self.heads() {
            let params = LayoutParams {
                flags: LAYOUT_FLAGS,
                width: screen.width,
                font: ctx.fonts.by_name(&tag.font),
                color: Self::head_color(tag, head),
            };
            let tags = GameTags::new(game, head.players);
            let mut layout = TextLayout::default();
            layout_text_game(
                &head.text,
                &params,
                ctx.fonts,
                &mut layout,
                None,
                Some(&tags),
            );
            let (px, py) = tag.position;
            let x = if px < 0 {
                screen.width.wrapping_sub(layout.width)
            } else {
                game_x(screen, px)
            };
            let y = game_y(screen, py);
            draw_runs(&layout, [x, y], true, None, ctx.fonts, out);
        }
    }

    /// The heads like [`MessageBar::draw`] as readable lines with their color (viewer overlay): game tags substituted
    /// ([`resolve_game_tags`]); none in cinemascope.
    pub fn lines(&self, game: &dyn GameText) -> Vec<(String, u32)> {
        if self.cinemascope {
            return Vec::new();
        }
        self.heads()
            .map(|(tag, head)| {
                let tags = GameTags::new(game, head.players);
                (
                    resolve_game_tags(&head.text, &tags),
                    Self::head_color(tag, head),
                )
            })
            .collect()
    }
}

/// `MessageSystem_PlayMessageSound` (`0x00492740`, N.7.3): sound (SOUNDONCE with a lock per kind, otherwise SOUND;
/// both not in cinemascope), duration `wcslen(raw text) * TimePerChar`, ping with PING and a location, remainder
/// [`FADE_MS`].
fn appear(
    msg: &mut BarMessage,
    lock: &mut i32,
    color: u32,
    time_per_char: i32,
    cinemascope: bool,
    out: &mut Vec<BarEffect>,
) {
    if msg.flags & bar_flags::SOUND_ONCE != 0 && !cinemascope {
        if *lock < 1 {
            out.push(BarEffect::Sound(msg.sound));
            *lock = SOUND_ONCE_MS;
        }
    } else if msg.flags & bar_flags::SOUND != 0 && !cinemascope {
        out.push(BarEffect::Sound(msg.sound));
    }
    // `wcslen`: UTF-16 units up to the first null character, including tags.
    let len = msg.text.encode_utf16().take_while(|&u| u != 0).count() as i32;
    msg.display_ms = len.wrapping_mul(time_per_char);
    if msg.flags & bar_flags::PING != 0 && msg.tile != NO_TILE {
        out.push(BarEffect::Ping {
            tile: msg.tile,
            color,
            repeats: PING_REPEATS,
        });
    }
    msg.fade_ms = FADE_MS;
}

/// Game-time clock of the bar (deviation): `dt_game = floor((remainder + dt * (GameSpeed + 10) * 5) / 100)` with a
/// carried remainder like `GameEngine_Update` (`0x00445650`). The remainder is an `f64` from the frame time of the
/// session; with integer frame times (virtual clock) it equals the original.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GameClock {
    rest: f64,
}

impl GameClock {
    /// Game time of a frame of `dt_ms` ms at `game_speed`.
    pub fn tick(&mut self, dt_ms: f32, game_speed: u8) -> i32 {
        let total = self.rest + f64::from(dt_ms) * (f64::from(game_speed) + 10.0) * 5.0;
        let dt = (total / 100.0).floor();
        self.rest = total - dt * 100.0;
        dt as i32
    }

    /// Remainder to 0 (loading, restart).
    pub fn reset(&mut self) {
        self.rest = 0.0;
    }
}

/// Action stack `GameWorld+0x48` (`0x0047F1A0`, O-219): the most recent [`ACTION_STACK_LEN`] ping locations, the most
/// recent at the front. Button/key 570 (`0x0047F1E0`) takes one off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionStack {
    tiles: VecDeque<(i32, i32)>,
}

impl ActionStack {
    /// Adds a location at the front, then at most [`ACTION_STACK_LEN`] locations (`AddHead`, `RemoveTail`).
    pub fn push(&mut self, tile: (i32, i32)) {
        self.tiles.push_front(tile);
        self.tiles.truncate(ACTION_STACK_LEN);
    }

    /// Takes off the most recent location (`MessageQueue_PopFront` `0x0047C060`).
    pub fn pop(&mut self) -> Option<(i32, i32)> {
        self.tiles.pop_front()
    }

    /// The locations, the most recent at the front.
    pub fn tiles(&self) -> &VecDeque<(i32, i32)> {
        &self.tiles
    }

    /// Clears (loading, restart).
    pub fn clear(&mut self) {
        self.tiles.clear();
    }
}

#[cfg(test)]
mod tests;
