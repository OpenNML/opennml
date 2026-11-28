//! Feedback of the simulation to the player: floating credits above the
//! deliverer (`0x00493030`), text messages with arguments (`FUN_00492A40`/`FUN_00492C50`,
//! headhunter) and the event sounds; plus the tables `messages.ini` and `texte.deu`.
//! The message bar (`MessageBar_ShowMessage` `0x00494A60`) lives in
//! [`crate::game::hud::Hud`]. The text messages only get their `%s` filled in here ([`TextNotice`]); the host enqueues
//! them as screen texts of kind 1 of the message bar (`opennml_ui::hud::screen_text::ScreenTexts`, O-380). The
//! floating numbers run on the game time of the message bar ([`crate::game::hud::Hud::bar_ms`]) and stand still while
//! the clock is stopped, so that screenshots are repeatable. Without GPU (testable).

use std::collections::BTreeMap;

use crate::game::overlay::OverlayLine;
use glam::{Mat4, Vec3};
use opennml_formats::messages::{
    MESSAGES_PATH, Messages, TEXTS_PATH, decode_text_file, parse_messages, parse_text_table,
};
use opennml_formats::text::decode_cp1252;
use opennml_rda::GameVersion;
use opennml_sim::state::{SimEvent, TextArg};
use opennml_ui::dialog::UiContext;
use opennml_ui::draw::DrawCmd;
use opennml_ui::hud::SCREEN_TEXT_FONT;
use opennml_ui::hud::screen_text;
use opennml_ui::own_texts;
use opennml_ui::text::{LayoutParams, TextLayout, draw_runs, layout_text};

use crate::game::sim_host::SimHost;

/// Lifetime of a floating number in ms of message bar game time (`0x00492520`, last argument 2000).
pub const FLOAT_MS: i64 = 2000;
/// Rising speed of the number in world units per ms (`{0, 0.1, 0}`).
pub const FLOAT_RISE: f32 = 0.1;
/// Start point above the world position (`PropertyGetResource_DeliverLoad` `0x005155E0`: `y + 120`).
pub const FLOAT_HEIGHT: f32 = 120.0;
/// Color of the red damage number of the smith (`FUN_00493030(d, 0xDC0000, ...)`, re/special-attacks 3.6).
pub const DAMAGE_COLOR: u32 = 0xDC_0000;
/// Height of the damage number above the world position of the target (`target + 200` in y, re/special-attacks 3.6).
pub const DAMAGE_HEIGHT: f32 = 200.0;
/// Flags of the 2D event sound (`0x00552FB0(name, 0x14, 1, 0)`, re/headhunter-robbing 7).
pub const EVENT_2D_FLAGS: u32 = 0x14;
/// Sound of script action 7 and of signal `0x5C` (`FUN_0043EBE0` `0x0043EC10`, `0x0048D4D6`), as a 2D event
/// ([`SoundRequest::Event2d`]).
pub const SFX_SIGNAL: &str = "SFX_SIGNAL";
/// Color of the number per resource kind 0 gold, 1 wood, 2 food (`0x005155E0`), otherwise `0xFF`.
pub const CREDIT_COLORS: [u32; 3] = [0xFF_FF00, 0x00_FF00, 0xF6_94C0];

/// A text message with its `%s` already filled in (`FUN_00492A40` copies the text when enqueuing), for the screen
/// texts of the message bar (`ScreenTexts::push_text`).
#[derive(Debug, Clone, PartialEq)]
pub struct TextNotice {
    /// Kind of the screen text (`opennml_ui::hud::screen_text::kind`): always 1 (`LOW`, O-380).
    pub kind: u8,
    pub text: String,
    /// ARGB, opaque (`0xFF000000 | rgb`).
    pub color: u32,
    /// Duration in ms of the bar's game time.
    pub ms: i32,
}

/// A floating credit.
#[derive(Debug, Clone, PartialEq)]
pub struct FloatingNumber {
    /// Start point (world space, already with [`FLOAT_HEIGHT`]).
    pub position: Vec3,
    pub text: String,
    /// `0x00RRGGBB`.
    pub color: u32,
    /// Creation at the game time of the message bar ([`crate::game::hud::Hud::bar_ms`]).
    pub start_bar_ms: i64,
}

/// A sound that the application is to play.
#[derive(Debug, Clone, PartialEq)]
pub enum SoundRequest {
    /// Sound of a message: sound GUID from `soundguidlist.txt`, 2D (flags `0x12`, sound effects).
    Message(i32),
    /// Sound at an object: number of the simulation (action of the type's sound database or
    /// sound GUID, [`sound_kind`]), type and position.
    Object {
        guid: u32,
        sound: i32,
        position: Vec3,
    },
    /// Object sound of the combat simulation with the caller's flags (`SoundManager_PlayObjectSound3D`
    /// with action, flags, position and type GUID, re/combat-host 3): without fog check (C-116) and
    /// without `action_flags`.
    Object3d {
        action: i32,
        flags: u32,
        position: Vec3,
        guid: u32,
        /// Object at which the sound arose (only for the state snapshot `recent_sounds`).
        object: u32,
    },
    /// 2D event from `otherevents.ini` by name (`SoundManager_PlayEvent2D` `0x00552FB0` with
    /// flags [`EVENT_2D_FLAGS`], sound effects; re/headhunter-robbing 7, D-238).
    Event2d { name: String },
    /// 3D event from `otherevents.ini` by name at a position (`SoundManager_PlayEvent3D`
    /// `0x00553420`: distance filter, volume factor of the entry; wading sound
    /// `SFX_WATERWALK<n>` with flags `0x11`, re/world-display W.3.4). Without fog check (D-273).
    Event3d {
        name: String,
        flags: u32,
        position: Vec3,
    },
}

/// How a sound number of the simulation is played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundKind {
    /// Action of the object type's sound database (`SoundManager_PlayObjectSound3D`), e.g. `0x65`
    /// destroyed, `0x67` while building, `0x68` finished, `0x69` chopping wood.
    Action,
    /// Sound GUID (`otherevents.ini`, e.g. 1176 tree falls).
    Guid,
}

/// Action if `actionidlist.txt` knows the number, otherwise sound GUID.
pub fn sound_kind(action_names: &BTreeMap<i32, String>, sound: i32) -> SoundKind {
    if action_names.contains_key(&sound) {
        SoundKind::Action
    } else {
        SoundKind::Guid
    }
}

/// Tables of the messages, text messages and credits of the map view.
#[derive(Debug, Default)]
pub struct Notices {
    /// `messages.ini` (the message bar in [`crate::game::hud::Hud`] gets a copy).
    pub messages: Messages,
    /// `texte.deu`.
    pub texts: BTreeMap<i32, String>,
    pub floats: Vec<FloatingNumber>,
    /// Own text "Player %d" of the edition for a player without a name ([`own_texts::PLAYER_N`]); empty (`Default`,
    /// tests without a `Vfs`): the English text.
    pub player_text: String,
}

impl Notices {
    /// Reads messages and texts; if a file is missing, its table stays empty (warning).
    pub fn load(vfs: &opennml_rda::Vfs) -> Self {
        let read = |path: &str| match vfs.read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                log::warn!("Messages: {path} not read: {e:#}");
                None
            }
        };
        Self {
            messages: read(MESSAGES_PATH)
                .map(|b| parse_messages(&decode_cp1252(&b)))
                .unwrap_or_default(),
            texts: read(TEXTS_PATH)
                .map(|b| parse_text_table(&decode_text_file(&b)))
                .unwrap_or_default(),
            player_text: own_texts::text(own_texts::PLAYER_N, own_texts::language(vfs))
                .unwrap_or_default()
                .to_string(),
            ..Self::default()
        }
    }

    /// Name of the player `slot` without a name, in the language of the edition.
    fn nameless_player(&self, slot: u8) -> String {
        let template = if self.player_text.is_empty() {
            own_texts::text(own_texts::PLAYER_N, GameVersion::English).unwrap_or_default()
        } else {
            &self.player_text
        };
        own_texts::fill(template, &[&slot.to_string()])
    }

    /// After loading or restart: show nothing anymore.
    pub fn clear(&mut self) {
        self.floats.clear();
    }

    /// Text of a message (`None`: unknown number or without text).
    pub fn message_text(&self, id: i32) -> Option<&str> {
        let def = self.messages.get(id)?;
        self.texts.get(&def.text_id()?).map(String::as_str)
    }

    /// Events of a logic step at `bar_ms` of the game time of the message bar (floating numbers,
    /// [`crate::game::hud::Hud::bar_ms`] before the bar's tick of this frame, as the original queues the number in the
    /// game tick and `FUN_00494A00` then ticks it with `dt`): credits only above own objects on ground that is
    /// currently seen; returns the sounds to play and the text messages for their receiver, filled in, in event order
    /// (the host enqueues them as screen texts). The message bar (`PlayerMessage`, `UnderAttack`, `TributeSent`,
    /// `CounterUpgradeStarted`) is served by [`crate::game::hud::Hud::handle`].
    pub fn handle(
        &mut self,
        events: &[SimEvent],
        sim: &SimHost,
        bar_ms: i64,
    ) -> (Vec<SoundRequest>, Vec<TextNotice>) {
        self.expire(bar_ms);
        let mut sounds = Vec::new();
        let mut notices = Vec::new();
        for event in events {
            match *event {
                SimEvent::Sound3d {
                    object,
                    action,
                    flags,
                    pos,
                    guid,
                } => {
                    sounds.push(SoundRequest::Object3d {
                        action,
                        flags,
                        position: Vec3::from(pos),
                        guid,
                        object: object.0,
                    });
                }
                SimEvent::Sound { object, sound } => {
                    if let Some(o) = sim.state.object(object, false) {
                        sounds.push(SoundRequest::Object {
                            guid: o.guid,
                            sound,
                            position: sim.display(o).0,
                        });
                    }
                }
                SimEvent::Credited {
                    object,
                    amount,
                    kind,
                } => {
                    let Some(o) = sim.state.object(object, false) else {
                        continue;
                    };
                    if o.owner() != sim.active_player {
                        continue;
                    }
                    // `0x00493030` checks the world position (`IsWorldPosVisibleToPlayer` without
                    // rectangle, including the view switch).
                    let seen = sim.fog.view(&sim.state).world_pos_visible(
                        &sim.rules.map,
                        o.world_pos,
                        i32::from(sim.active_player),
                        false,
                    );
                    if !seen {
                        continue;
                    }
                    let color = usize::try_from(kind)
                        .ok()
                        .and_then(|k| CREDIT_COLORS.get(k))
                        .copied()
                        .unwrap_or(0xFF);
                    self.floats.push(FloatingNumber {
                        position: sim.display(o).0 + Vec3::Y * FLOAT_HEIGHT,
                        text: amount.to_string(),
                        color,
                        start_bar_ms: bar_ms,
                    });
                }
                // Red damage number of the smith (`FUN_00493030(d, 0xDC0000, target + 200 y)`,
                // re/special-attacks 3.6): the same visibility check of the world position as for the
                // credits, but without owner filter - every viewer sees it.
                // `FUN_00492A40` (to everyone) or `FUN_00492C50` (only the local player):
                // re/headhunter-robbing 7, D-238.
                SimEvent::TextMessage {
                    to,
                    text,
                    ref args,
                    color,
                    duration_ms,
                } if to.is_none_or(|p| p == sim.active_player) => {
                    notices.extend(self.show_text(text, args, color, duration_ms, sim));
                }
                // `0x00552FB0(name, 0x14, 1, 0)` for every viewer (re/headhunter-robbing 7).
                SimEvent::Sound2d { ref name } => {
                    sounds.push(SoundRequest::Event2d { name: name.clone() });
                }
                SimEvent::DamageNumber { object, points } => {
                    let Some(o) = sim.state.object(object, false) else {
                        continue;
                    };
                    let seen = sim.fog.view(&sim.state).world_pos_visible(
                        &sim.rules.map,
                        o.world_pos,
                        i32::from(sim.active_player),
                        false,
                    );
                    if !seen {
                        continue;
                    }
                    self.floats.push(FloatingNumber {
                        position: sim.display(o).0 + Vec3::Y * DAMAGE_HEIGHT,
                        text: points.to_string(),
                        color: DAMAGE_COLOR,
                        start_bar_ms: bar_ms,
                    });
                }
                _ => {}
            }
        }
        (sounds, notices)
    }

    /// Text message `text` from `texte.deu` (`FUN_00492A40`/`FUN_00492C50`, re/headhunter-robbing 7):
    /// the `%s` replaced in order by the arguments ([`TextArg::PlayerName`] -> name of the
    /// slot, if empty "Player <slot>" in the language of the edition; [`TextArg::TypeName`] -> type name
    /// as in the window), color `0xRRGGBB` opaque, duration from the event, screen text kind 1 (centered at
    /// y = H - 200, O-380; the discard rule applies when enqueuing). If the text is missing, no message.
    fn show_text(
        &self,
        text: i32,
        args: &[TextArg],
        color: u32,
        duration_ms: i32,
        sim: &SimHost,
    ) -> Option<TextNotice> {
        let template = self.texts.get(&text)?;
        let arg = |a: &TextArg| match *a {
            TextArg::PlayerName(p) => sim
                .state
                .players
                .get(usize::from(p))
                .map(|pl| pl.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| self.nameless_player(p)),
            TextArg::TypeName(guid) => crate::game::economy::type_name(&sim.rules, guid),
        };
        Some(TextNotice {
            kind: screen_text::kind::LOW,
            text: fill_in(template, args.iter().map(arg)),
            color: 0xFF00_0000 | color,
            ms: duration_ms,
        })
    }

    /// Floating number from outside (`0x00493030`, e.g. the digits of the dynamite charge from
    /// `DynamiteEffect_Tick` `0x004F07A0`): `text` in `color` (`0x00RRGGBB`) from `position` (without
    /// height offset) at game time `bar_ms` of the message bar. The caller computes `seen` with
    /// the same query as for credit and damage number (`world_pos_visible` of the active
    /// player without rectangle); if unseen, no number arises.
    pub fn push_float(
        &mut self,
        position: Vec3,
        text: String,
        color: u32,
        bar_ms: i64,
        seen: bool,
    ) {
        if !seen {
            return;
        }
        self.floats.push(FloatingNumber {
            position,
            text,
            color,
            start_bar_ms: bar_ms,
        });
    }

    /// Remove expired numbers (game time of the bar `bar_ms`, gone from `t >= 2000` like the tick `FUN_004925D0`).
    pub fn expire(&mut self, bar_ms: i64) {
        self.floats.retain(|f| bar_ms - f.start_bar_ms < FLOAT_MS);
    }

    /// Floating numbers of a frame at their pixels at game time `bar_ms` of the message bar
    /// ([`crate::game::hud::Hud::bar_ms`]). Tick `FUN_004925D0` (the number is a screen text of the message bar with
    /// world position, construction `FUN_00492520`) with integer `t` (`+0x44`): position `start + {0, 0.1, 0} * t`,
    /// alpha `ftol(255 - t * step)` in `f32` with the step `255.0f / 2000` as for the screen texts, clamped to
    /// 0 ... 255, as ARGB `(color & 0xFFFFFF) | alpha << 24`; gone from `t >= 2000`. Pixel `FUN_004931A0`: projection
    /// of the position like `FUN_00572650` ([`device_project`]), both coordinates truncated (`ftol`), only for
    /// `0 <= x <= W` and `0 <= y <= H` (`Renderer+0xA4`, `+0xA8`). There the text is **left-aligned** (no center,
    /// x >= 0 does not set centering in `FUN_00492F20`).
    pub fn float_texts(&self, view_proj: Mat4, size: (u32, u32), bar_ms: i64) -> Vec<FloatText> {
        // `+0x38 = 255.0f / (float)2000` (`FUN_00492520`), like `opennml_ui::hud::screen_text::ScreenText`.
        let step = 255.0f32 / FLOAT_MS as f32;
        self.floats
            .iter()
            .filter_map(|f| {
                let t = bar_ms - f.start_bar_ms;
                if !(0..FLOAT_MS).contains(&t) {
                    return None;
                }
                let position = f.position + Vec3::Y * (FLOAT_RISE * t as f32);
                let [x, y] = device_project(view_proj, position, size)?;
                let (x, y) = (x as i32, y as i32);
                let inside = |v: i32, max: u32| v >= 0 && i64::from(v) <= i64::from(max);
                if !inside(x, size.0) || !inside(y, size.1) {
                    return None;
                }
                let alpha = ((255.0 - t as f32 * step) as i32).clamp(0, 255) as u32;
                Some(FloatText {
                    x,
                    y,
                    text: f.text.clone(),
                    color: (alpha << 24) | (f.color & 0x00FF_FFFF),
                })
            })
            .collect()
    }

    /// Floating numbers as draw commands for `opennml-game`: per number from
    /// [`Notices::float_texts`] (screen size = `ctx.screen`) the layout like `FUN_00492F20(x, y, text, color)` - flags
    /// [`opennml_ui::hud::screen_text::LAYOUT_FLAGS`] (`0xC6`) to the width `W - x`, font `InfoMessages` (`this+4` of the
    /// message bar), with shadow (`FUN_0055FD60(&(x, y), 1, 0)`). Called in the world pass with the screen texts
    /// (`GameSession::combat_cmds`).
    pub fn draw_floats(
        &self,
        view_proj: Mat4,
        bar_ms: i64,
        ctx: &UiContext<'_>,
        out: &mut Vec<DrawCmd>,
    ) {
        let s = ctx.screen;
        let size = (
            u32::try_from(s.width).unwrap_or(0),
            u32::try_from(s.height).unwrap_or(0),
        );
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        let font = ctx.fonts.by_name(SCREEN_TEXT_FONT);
        for f in self.float_texts(view_proj, size, bar_ms) {
            let params = LayoutParams {
                flags: screen_text::LAYOUT_FLAGS,
                // `float_texts` ensures `0 <= x <= W`.
                width: s.width - f.x,
                font,
                color: f.color,
            };
            let mut layout = TextLayout::default();
            layout_text(&f.text, &params, ctx.fonts, &mut layout);
            draw_runs(&layout, [f.x, f.y], true, None, ctx.fonts, out);
        }
    }

    /// The floating numbers as overlay lines of the viewer: the same pixels, colors and texts as
    /// [`Notices::float_texts`] at game time `bar_ms` of the message bar, drawn in the small font of the
    /// overlay.
    pub fn float_lines(&self, view_proj: Mat4, size: (u32, u32), bar_ms: i64) -> Vec<OverlayLine> {
        self.float_texts(view_proj, size, bar_ms)
            .into_iter()
            .map(|f| OverlayLine {
                x: f.x,
                y: f.y,
                text: f.text,
                color: f.color,
            })
            .collect()
    }
}

/// Pixel of a world position like `FUN_00572650` (for the floating numbers, `FUN_004931A0`): `clip = projection * view
/// * position` (world matrix identity), `x = (W >> 1) * (clip.x * (1/w) + 1)`, `y = (H >> 1) * (1 - clip.y * (1/w))`
/// with the screen size halved as integers. Deviation: for `w <= 0` (behind the camera) no position; the original does
/// not check `w` and drew a mirrored number there (a bug that the reimplementation does not adopt; whether the game
/// camera can reach it has not been checked). [`crate::game::grid::project`] and the combat display stay at `W/2`.
pub fn device_project(view_proj: Mat4, point: Vec3, size: (u32, u32)) -> Option<[f32; 2]> {
    let clip = view_proj * point.extend(1.0);
    if clip.w <= 0.0 {
        return None;
    }
    let inv = 1.0 / clip.w;
    Some([
        (size.0 >> 1) as f32 * (clip.x * inv + 1.0),
        (size.1 >> 1) as f32 * (1.0 - clip.y * inv),
    ])
}

/// A floating number at its pixel ([`Notices::float_texts`]).
#[derive(Debug, Clone, PartialEq)]
pub struct FloatText {
    /// Top left corner of the layout in pixels (truncated).
    pub x: i32,
    pub y: i32,
    pub text: String,
    /// ARGB with the alpha of the tick.
    pub color: u32,
}

/// `%s` of the template replaced in order by `args`; inserted texts are not replaced again,
/// surplus `%s` stay, surplus arguments are dropped.
fn fill_in(template: &str, mut args: impl Iterator<Item = String>) -> String {
    let mut pieces = template.split("%s");
    let mut out = pieces.next().unwrap_or_default().to_string();
    for piece in pieces {
        match args.next() {
            Some(a) => out.push_str(&a),
            None => out.push_str("%s"),
        }
        out.push_str(piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::options::DEFAULT_GAME_SPEED;
    use opennml_sim::id::ObjectId;
    use opennml_sim::rules::Rules;
    use opennml_sim::setup::MatchSetup;
    use opennml_sim::world::{ObjectDesc, create_object};

    const MESSAGES: &str = "MSGTAG ECONOMY\nCOLOR E000FF00\n/MSGTAG\nMSGTAG MILITARY\nCOLOR E0FF0000\n/MSGTAG\nMSGID 0\nMSGTYPE ECONOMY\nSOUND 877\nTEXT 17000\n/MSGID\nMSGID 100\nMSGTYPE MILITARY\nSOUNDONCE 872\nTEXT 17100\n/MSGID\nMSGID 9\nTEXT 99999\n/MSGID\n";
    const TYPES: &str =
        "GUID 1100\nNAME Kolonist\nVALUEID 100 6 1 0\nPROPID 0\nPROPID 2\nVALUEID 30001 1 1 0\n";

    fn notices() -> Notices {
        Notices {
            messages: parse_messages(MESSAGES),
            texts: [
                (17000, "WIR HABEN NICHT GENUG NAHRUNG!".to_string()),
                (17100, "Angriff!".to_string()),
            ]
            .into_iter()
            .collect(),
            ..Notices::default()
        }
    }

    #[test]
    fn message_texts_resolve_through_the_definition() {
        let n = notices();
        assert_eq!(n.message_text(0), Some("WIR HABEN NICHT GENUG NAHRUNG!"));
        assert_eq!(n.message_text(9), None, "text missing from the table");
        assert_eq!(n.message_text(5), None, "unknown message");
    }

    /// Host with a colonist of player 0 and one of player 1.
    fn host() -> (SimHost, ObjectId, ObjectId) {
        let rules = Rules::synthetic(TYPES, 16);
        let setup = MatchSetup {
            mode: -1,
            seed: 0,
            lobby: Vec::new(),
            resource_setting: 0,
            living_space: 2,
            map_explored: false,
            ki: opennml_sim::ki::KiSetup::default(),
            script: opennml_sim::script::ScriptSetup::default(),
            ..MatchSetup::default()
        };
        let mut host = SimHost::new(rules, setup, DEFAULT_GAME_SPEED).unwrap();
        host.active_player = 0;
        let rules = host.rules.clone();
        let mut sim = opennml_sim::sim::Sim::new(&mut host.state, &rules);
        let own =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 0, [64.0, 0.0, 64.0])).unwrap();
        let foreign =
            create_object(&mut sim, &mut ObjectDesc::new(1100, 1, [320.0, 0.0, 64.0])).unwrap();
        (host, own, foreign)
    }

    #[test]
    fn credits_float_upwards_and_fade_only_over_own_objects() {
        let (mut host, own, foreign) = host();
        // Credits need ground that is currently seen (`floating_numbers_need_a_seen_tile`).
        opennml_sim::fog::reveal_map(&mut host.state, -1);
        let mut n = notices();
        let credit = |object, kind| SimEvent::Credited {
            object,
            amount: 17,
            kind,
        };
        n.handle(
            &[credit(foreign, 1), credit(own, 1), credit(own, 7)],
            &host,
            500,
        );
        assert_eq!(n.floats.len(), 2);
        let wood = &n.floats[0];
        assert_eq!((wood.text.as_str(), wood.color), ("17", 0x00_FF00));
        assert_eq!(n.floats[1].color, 0xFF, "unknown kind blue");
        let o = host.state.object(own, false).unwrap();
        assert_eq!(wood.position, host.display(o).0 + Vec3::Y * 120.0);
        // An orthogonal view from above is unnecessary here: a projection that maps y onto the screen.
        let view_proj = Mat4::from_cols(
            glam::Vec4::new(1.0 / 1000.0, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 1.0 / 1000.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, 0.5, 1.0),
        );
        let at = |now| n.float_lines(view_proj, (2000, 2000), now);
        let start = at(500);
        let later = at(1500);
        assert_eq!(start.len(), 2);
        // After one second 100 units higher (screen y smaller) and half transparent.
        assert_eq!(start[0].y - later[0].y, 100);
        assert_eq!(start[0].color, 0xFF00_FF00);
        assert_eq!(later[0].color, 0x7F00_FF00);
        assert!(at(500 + FLOAT_MS).is_empty());
        n.expire(500 + FLOAT_MS);
        assert!(n.floats.is_empty());
    }

    #[test]
    fn object_sounds_are_actions_of_the_type_or_sound_guids() {
        let (host, own, _) = host();
        let mut n = notices();
        let (sounds, _) = n.handle(
            &[SimEvent::Sound {
                object: own,
                sound: 0x69,
            }],
            &host,
            0,
        );
        let o = host.state.object(own, false).unwrap();
        assert_eq!(
            sounds,
            [SoundRequest::Object {
                guid: 1100,
                sound: 0x69,
                position: host.display(o).0,
            }]
        );
        let names: BTreeMap<i32, String> = [(105, "Holzhacken".to_string())].into();
        assert_eq!(sound_kind(&names, 105), SoundKind::Action);
        assert_eq!(sound_kind(&names, 1176), SoundKind::Guid);
    }

    #[test]
    fn combat_sounds_pass_through_and_attacks_belong_to_the_hud() {
        let (host, own, _) = host();
        let mut n = notices();
        let (sounds, shown) = n.handle(
            &[
                SimEvent::Sound3d {
                    object: own,
                    action: 0x6A,
                    flags: 0x42,
                    pos: [1.0, 2.0, 3.0],
                    guid: 1100,
                },
                SimEvent::UnderAttack {
                    object: own,
                    owner: 1,
                    guid: 1100,
                    building: false,
                    tile: (1, 1),
                },
                SimEvent::UnderAttack {
                    object: own,
                    owner: 0,
                    guid: 1100,
                    building: false,
                    tile: (1, 1),
                },
            ],
            &host,
            0,
        );
        assert_eq!(
            sounds,
            [SoundRequest::Object3d {
                action: 0x6A,
                flags: 0x42,
                position: Vec3::new(1.0, 2.0, 3.0),
                guid: 1100,
                object: own.0,
            },]
        );
        assert!(
            shown.is_empty(),
            "attack messages are shown by the message bar (`Hud::handle`)"
        );
    }

    /// Messages without `messages.ini`, only the `texte.deu` templates of the headhunter
    /// (recognizably artificial templates with the same `%s` structure as the original, re/headhunter-robbing 1.4).
    fn hunter_notices() -> Notices {
        Notices {
            texts: [
                (10002, "TESTTEXT Gold missing".to_string()),
                (10010, "TESTTEXT %s hires %s".to_string()),
            ]
            .into_iter()
            .collect(),
            ..Notices::default()
        }
    }

    // QUIRK D-238, C-310: kept
    #[test]
    fn text_messages_fill_in_names_color_and_duration() {
        use crate::game::testworld::{HQ_SID, HQ_SPA, SID_SHERIFF, hunter_host, spawn};
        use opennml_sim::command::CommandKind;
        use opennml_sim::command::headhunter::{COLOR_RED, TEXT_HIRED, TEXT_NO_GOLD, TEXT_SUCCESS};
        use opennml_sim::state::TextArg;
        let mut host = hunter_host();
        host.state.players[0].resources_fx[0] = 1000 << 12;
        host.state.players[0].name = "Anna".into();
        let hq = spawn(&mut host, HQ_SPA, 0, (20, 20));
        spawn(&mut host, HQ_SID, 1, (40, 40));
        spawn(&mut host, SID_SHERIFF, 1, (44, 30));
        let hire = CommandKind::HireHeadhunter {
            art: 2,
            target_player: 1,
            target_guid: SID_SHERIFF as i32,
            building: hq,
        };
        host.submit_own(hire.clone());
        let events = host.step_once();
        // 10010 to everyone (D-238), here at slot 1: name of the payer and type name of the hunter in the
        // `%s`, green, 6000 ms.
        let mut n = hunter_notices();
        host.active_player = 1;
        let (sounds, shown) = n.handle(&events, &host, 1000);
        let events_2d: Vec<&SoundRequest> = sounds
            .iter()
            .filter(|s| matches!(s, SoundRequest::Event2d { .. }))
            .collect();
        assert_eq!(
            events_2d,
            [&SoundRequest::Event2d {
                name: "SFX_HEADHUNTERSTART".into()
            }]
        );
        assert_eq!(
            shown,
            [TextNotice {
                kind: screen_text::kind::LOW,
                text: "TESTTEXT Anna hires Kopfgeldjaeger_SID".into(),
                color: 0xFF00_FF00,
                ms: 6000,
            }]
        );
        // C-310, display part: the hunter (slot 8) carries the color of the building.
        let hunter = host.state.headhunters[2].expect("hunter hired");
        let (h, b) = (
            host.state.object(hunter, false).unwrap(),
            host.state.object(hq, false).unwrap(),
        );
        assert_eq!(h.owner(), 8);
        assert_eq!(
            SimHost::base_look(h).color,
            Some(opennml_world::markers::player_rgb(b.color))
        );
        // 10002 only for the payer: red, 4000 ms (the same hunter again, without gold).
        host.active_player = 0;
        host.state.players[0].resources_fx[0] = 0;
        host.submit_own(hire);
        let events = host.step_once();
        assert!(events.iter().any(|e| matches!(
            e,
            SimEvent::TextMessage {
                to: Some(0),
                text: TEXT_NO_GOLD,
                color: COLOR_RED,
                ..
            }
        )));
        host.active_player = 1;
        let mut other = hunter_notices();
        let (_, shown) = other.handle(&events, &host, 2000);
        assert!(shown.is_empty(), "slot 1 does not pay");
        host.active_player = 0;
        let mut payer = hunter_notices();
        let (_, shown) = payer.handle(&events, &host, 2000);
        let line = shown.last().expect("message for the payer");
        assert_eq!(
            (line.kind, line.text.as_str(), line.color, line.ms),
            (
                screen_text::kind::LOW,
                "TESTTEXT Gold missing",
                0xFFFF_0000,
                4000
            )
        );
        // Empty name: "Player <slot>" (0-based like the other messages);
        // surplus `%s` stay, surplus arguments are dropped; unknown text: no
        // line.
        let message = |text, args: Vec<TextArg>| SimEvent::TextMessage {
            to: None,
            text,
            args,
            color: 0x00FF00,
            duration_ms: 3000,
        };
        let mut n = hunter_notices();
        let (_, shown) = n.handle(
            &[
                message(TEXT_HIRED, vec![TextArg::PlayerName(1)]),
                message(TEXT_NO_GOLD, vec![TextArg::TypeName(4100)]),
                message(4711, Vec::new()),
            ],
            &host,
            0,
        );
        let lines: Vec<String> = shown.into_iter().map(|l| l.text).collect();
        assert_eq!(
            lines,
            ["TESTTEXT Player 1 hires %s", "TESTTEXT Gold missing"]
        );
        // The templates of the original carry as many `%s` as the simulation sends arguments.
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        let texts = Notices::load(&vfs).texts;
        let marks = |id: i32| texts.get(&id).map(|t| t.matches("%s").count());
        assert_eq!(
            [marks(TEXT_NO_GOLD), marks(TEXT_HIRED), marks(TEXT_SUCCESS)],
            [Some(0), Some(2), Some(1)]
        );
    }

    /// A player without a name is shown in the language of the edition: "Player n" or "Spieler n"; without an edition
    /// text (`Default`) the English one.
    #[test]
    fn player_fallback_follows_the_edition() {
        use crate::game::testworld::hunter_host;
        use opennml_rda::GameVersion::{English, German};
        let host = hunter_host();
        let event = SimEvent::TextMessage {
            to: None,
            text: 10002,
            args: vec![TextArg::PlayerName(2)],
            color: 0,
            duration_ms: 1000,
        };
        let line = |player_text: String| {
            let mut n = Notices {
                texts: [(10002, "%s".to_string())].into_iter().collect(),
                player_text,
                ..Notices::default()
            };
            n.handle(std::slice::from_ref(&event), &host, 0)
                .1
                .remove(0)
                .text
        };
        let own = |version| {
            own_texts::text(own_texts::PLAYER_N, version)
                .unwrap()
                .to_string()
        };
        assert_eq!(line(own(English)), "Player 2");
        assert_eq!(line(own(German)), "Spieler 2");
        assert_eq!(line(String::new()), "Player 2");
    }

    // QUIRK D-238: kept
    #[test]
    fn sound_2d_requests_the_named_event() {
        let (host, _, _) = host();
        let mut n = Notices::default();
        // For every viewer (`0x00552FB0(name, 0x14, 1, 0)`, re/headhunter-robbing 7).
        let (sounds, _) = n.handle(
            &[SimEvent::Sound2d {
                name: "SFX_HEADHUNTERSUCCESS".into(),
            }],
            &host,
            0,
        );
        assert_eq!(
            sounds,
            [SoundRequest::Event2d {
                name: "SFX_HEADHUNTERSUCCESS".into()
            }]
        );
        // The event names are in `otherevents.ini` (sound GUID 923/924).
        let Some(dir) = opennml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = opennml_rda::Vfs::open_game_dir(&dir).unwrap();
        let db = opennml_audio::db::SoundDb::load(&vfs);
        assert_eq!(db.event("SFX_HEADHUNTERSTART"), Some(923));
        assert_eq!(db.event("SFX_HEADHUNTERSUCCESS"), Some(924));
    }

    #[test]
    fn floating_numbers_need_a_seen_tile() {
        let (mut host, a, _) = host();
        host.state.fog = opennml_sim::fog::Fog::new(host.rules.map.dim_x, host.rules.map.dim_z);
        let mut n = Notices::default();
        let credited = [SimEvent::Credited {
            object: a,
            amount: 20,
            kind: 1,
        }];
        n.handle(&credited, &host, 0);
        assert!(
            n.float_lines(Mat4::IDENTITY, (800, 600), 0).is_empty(),
            "no number in fog"
        );
        opennml_sim::fog::reveal_map(&mut host.state, -1);
        n.handle(&credited, &host, 0);
        assert_eq!(n.floats.len(), 1);
    }

    /// `0x00493030` from outside (digits of the dynamite charge): the caller computes visibility; only
    /// seen positions get a number, without height offset, with color and clock time.
    // QUIRK D-244: kept
    #[test]
    fn pushed_floats_need_a_seen_position() {
        let mut n = Notices::default();
        let at = Vec3::new(10.0, 20.0, 30.0);
        n.push_float(at, "7".into(), 0xC8_C8C8, 1500, false);
        assert!(n.floats.is_empty(), "unseen: no number");
        n.push_float(at, "7".into(), 0xC8_C8C8, 1500, true);
        assert_eq!(
            n.floats,
            [FloatingNumber {
                position: at,
                text: "7".into(),
                color: 0xC8_C8C8,
                start_bar_ms: 1500,
            }]
        );
        n.expire(1500 + FLOAT_MS);
        assert!(n.floats.is_empty(), "gone after two seconds");
    }

    /// Font `InfoMessages` (16 x 16 grid, 10 px per character, line height 15).
    fn info_messages_install() -> (tempfile::TempDir, opennml_rda::Vfs) {
        use crate::testutil::{fnt, install, tga};
        let loc = "Font InfoMessages\nFontSize 15\nTextureFont\nFontName Test\n";
        install(&[
            ("data/txt/localisation.deu", loc.as_bytes().to_vec()),
            ("data/fonts/Test_15_0_0_0_0.fnt", fnt()),
            ("data/fonts/Test_15_0_0_0_0.tga", tga(256, 256)),
        ])
    }

    /// Projection for 1024 x 768 that maps x and y of the world onto the screen: pixel `(512 + x, 384 - y)`.
    fn flat_view_proj() -> Mat4 {
        Mat4::from_cols(
            glam::Vec4::new(1.0 / 512.0, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 1.0 / 384.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, 0.5, 1.0),
        )
    }

    /// The font cells as `(x, y, color)`.
    fn cells(cmds: &[DrawCmd]) -> Vec<(i32, i32, u32)> {
        cmds.iter()
            .map(|c| match *c {
                DrawCmd::Quad {
                    texture: Some(_),
                    dst,
                    color,
                    ..
                } => (dst[0], dst[1], color),
                ref other => panic!("only font cells expected: {other:?}"),
            })
            .collect()
    }

    /// Floating numbers (`FUN_004926D0` branch `+0x54 & 1` -> `FUN_004931A0` -> `FUN_00492F20`): the number
    /// is left-aligned at the truncated pixel of its position (no center), in `InfoMessages` with shadow (shadow
    /// at (x, y + 2) in `color & 0xFF000000`, text at (x + 2, y)), color `0xC8C8C8` with the alpha of the tick
    /// `FUN_004925D0`; rises 0.1 units per ms; after 2,000 ms nothing anymore. Pixels outside 0 ... W, 0 ... H are
    /// dropped.
    // QUIRK D-244: kept
    #[test]
    fn floats_draw_left_aligned_in_info_messages_and_fade() {
        let (_dir, vfs) = info_messages_install();
        let mut layer = crate::ui_layer::UiLayer::new(&vfs, "DEU", 1024, None).unwrap();
        let ctx = layer.context(
            &vfs,
            opennml_ui::coords::Screen {
                width: 1024,
                height: 768,
            },
        );
        let mut n = Notices::default();
        n.push_float(
            Vec3::new(100.25, 0.0, 0.0),
            "17".into(),
            0xC8_C8C8,
            500,
            true,
        );
        // Outside to the left (x < 0) and outside at the bottom (y > H): not drawn.
        n.push_float(
            Vec3::new(-600.0, 0.0, 0.0),
            "1".into(),
            0xC8_C8C8,
            500,
            true,
        );
        n.push_float(
            Vec3::new(0.0, -600.0, 0.0),
            "2".into(),
            0xC8_C8C8,
            500,
            true,
        );
        let at = |now| {
            let mut out = Vec::new();
            n.draw_floats(flat_view_proj(), now, &ctx, &mut out);
            cells(&out)
        };
        // Pixel (612.25 -> 612, 384); "17" two characters of 10 px next to each other from x.
        assert_eq!(
            at(500),
            [
                (612, 386, 0xFF00_0000),
                (622, 386, 0xFF00_0000),
                (614, 384, 0xFFC8_C8C8),
                (624, 384, 0xFFC8_C8C8),
            ]
        );
        // After 1,000 ms 100 units higher, alpha `ftol(255 - 1000 * 255/2000)` = 127.
        assert_eq!(
            at(1500),
            [
                (612, 286, 0x7F00_0000),
                (622, 286, 0x7F00_0000),
                (614, 284, 0x7FC8_C8C8),
                (624, 284, 0x7FC8_C8C8),
            ]
        );
        assert!(at(500 + FLOAT_MS).is_empty(), "nothing after 2,000 ms");
        // The same point in the overlay lines of the viewer.
        let lines = n.float_lines(flat_view_proj(), (1024, 768), 1500);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            (lines[0].x, lines[0].y, lines[0].color),
            (612, 284, 0x7FC8_C8C8)
        );
    }

    /// Device projection (`FUN_00572650`): the half screen size is an integer (`>> 1`): at 1025 x 769 the screen center
    /// lies at (512, 384), not at (512.5, 384.5) as in [`crate::game::grid::project`]; behind the camera (`w <= 0`)
    /// no position.
    // QUIRK D-244: kept
    #[test]
    fn device_projection_halves_the_size_as_integer() {
        let size = (1025, 769);
        assert_eq!(
            device_project(Mat4::IDENTITY, Vec3::ZERO, size),
            Some([512.0, 384.0])
        );
        assert_eq!(
            crate::game::grid::project(Mat4::IDENTITY, Vec3::ZERO, size),
            Some([512.5, 384.5])
        );
        let behind = Mat4::from_cols(
            glam::Vec4::X,
            glam::Vec4::Y,
            glam::Vec4::Z,
            glam::Vec4::new(0.0, 0.0, 0.0, -1.0),
        );
        assert_eq!(device_project(behind, Vec3::ZERO, size), None);
    }

    #[test]
    fn damage_numbers_float_in_red() {
        let (mut host, own, foreign) = host();
        let mut n = notices();
        let number = |object, points| SimEvent::DamageNumber { object, points };
        // Occluded victim (fog): no number.
        host.state.fog = opennml_sim::fog::Fog::new(host.rules.map.dim_x, host.rules.map.dim_z);
        n.handle(&[number(foreign, -30)], &host, 0);
        assert!(n.floats.is_empty(), "no number in fog");
        // Visible: red, 200 above the object, also above a foreign one (no owner filter), with
        // the value of the return of `0x27F`.
        opennml_sim::fog::reveal_map(&mut host.state, -1);
        n.handle(
            &[
                number(foreign, -30),
                number(own, 0),
                number(ObjectId::NONE, -5),
            ],
            &host,
            500,
        );
        assert_eq!(n.floats.len(), 2, "a missing object shows nothing");
        let f = &n.floats[0];
        assert_eq!(
            (f.text.as_str(), f.color, f.start_bar_ms),
            ("-30", 0xDC_0000, 500)
        );
        let o = host.state.object(foreign, false).unwrap();
        assert_eq!(f.position, host.display(o).0 + Vec3::Y * 200.0);
        assert_eq!(n.floats[1].text, "0");
        assert_eq!(f.color >> 24, 0, "the color carries no alpha");
    }
}
