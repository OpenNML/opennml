//! Mouse pointer per context (re/game-ui D5.2, re/pointer-3d).
//!
//! The original sets the pointer only on events (`Cursor3D` `0x00671B00`: `SetPointer(id)` `0x004CD4C0`,
//! `SetVariant(f)` `0x004CD5D0`); the variant is 0 normal, 1 red, 2 green. [`PointerState`] holds the ID and variant
//! of the match and evaluates the mouse movements of a frame once in the frame step (`GameSession::pointer_step`,
//! after the frame step of the bar), in the order in which the window tree distributes the movement (`0x004528F0`):
//!
//! 1. **Minimap** (slot 2 `0x0044A4C0`, within the rectangle, also hidden): 7, variant stays.
//! 2. **Targeting mode** (`TargetMode_OnMouse` `0x004515B0`, takes every movement): `MAUSICONID` of the first row of
//!    the full `targets.ini` list with the `BUTTONMSG` (default 0), valid -> variant 2, otherwise 1. Ground targets and
//!    `MM2_POSITIONALLOWED` are always valid, also over the bar. Also on opening. End (`0x00451540`): 7/0.
//! 3. **Wall cursor** (`0x004513B0`): takes the movement without a pointer change. **Defense radius**
//!    (`0x00450190`): 7, variant stays.
//! 4. **Command bar** (slot 2 `0x00453790`, also hidden): within the rectangle 7/0; otherwise, while a right drag of
//!    the world is open (`+0x5BC`), nothing; otherwise the pointer of the world ([`SimHost::hover_pointer`]:
//!    `MAUSICONID` of the row that the right click would choose, otherwise 7; variant stays). The build cursor does not
//!    take the movement, so the world pointer applies below it. With the software pointer (`HardwareCursor 0`) it does
//!    tint the pointer green or red in every frame according to the verdict of its preview
//!    (`BuildCursor_UpdatePlacement` `0x0044FFBC` -> `FUN_004CCB20`, writes only the tint of the 3D pointer), also over
//!    the minimap and the bar; with the hardware pointer the variant stays 0 (O-394).
//!
//! The right press in the world looks for the pointer like a movement; sending a right click with a found
//! row (`0x00457C50`, [`SimHost::right_click_sends`]) sets 7 until the next movement. The buttons set 7/0
//! only in their rectangle and are overwritten by the bar afterwards; the variant is
//! 0 outside the targeting mode and the build cursor anyway, so they are missing here.
//!
//! Deviations: the movements of a frame count as one (the last position); the world pointer checks the right-click
//! target of the frame (one ray test per frame with movement instead of per event). Menus and open dialogs show 7/0
//! (`MenuFrame` `0x00460599`); the flow chooses that (`GameFlow::pointer_choice`). The tint ([`PointerTint`]) follows
//! the original: every `SetPointer` sets white, the targeting mode tints afterwards (0.5/1/0.5 or 1/0.5/0.5, opacity
//! 0.8), the build cursor with the software pointer per frame green or red; wall cursor and right drag leave it in
//! place, so does the end of the build cursor (O-464), menus and dialogs are white. The variant still chooses the TGA
//! of the 2D pointer.
//!
//! [`SimHost::hover_pointer`]: crate::game::sim_host::SimHost::hover_pointer
//! [`SimHost::right_click_sends`]: crate::game::sim_host::SimHost::right_click_sends

use nml_formats::gifcon::POINTER_NORMAL;

/// Variant normal (`Cursor3D+0x2C = 0`).
pub const VARIANT_NORMAL: u8 = 0;
/// Variant red (invalid target in the targeting mode).
pub const VARIANT_RED: u8 = 1;
/// Variant green (valid target in the targeting mode).
pub const VARIANT_GREEN: u8 = 2;
/// Number of pointers in `mousepointer.ini` (space for 12 in the object, `+0x84 + 12*0x3C = +0x354`).
pub const POINTER_COUNT: i32 = 12;
/// Number of variants (normal, red, green).
pub const VARIANT_COUNT: u8 = 3;

/// Tint of the 3D pointer (`Cursor3D+0x68…+0x74`, RGBA; re/pointer-3d MZ.6). Every `SetPointer` (`0x004CD4C0`) sets
/// white (re/pointer-3d MZ.9); afterwards the targeting mode (`0x004515B0`) or the build cursor
/// (`0x0044FFBC` -> `FUN_004CCB20`) tints it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PointerTint {
    /// Untinted (1, 1, 1, 1).
    #[default]
    White,
    /// Build cursor, valid site (0, 1, 0, 1).
    BuildValid,
    /// Build cursor, invalid site (1, 0, 0, 1).
    BuildInvalid,
    /// Targeting mode, valid target (0.5, 1, 0.5, 0.8).
    TargetValid,
    /// Targeting mode, invalid target (1, 0.5, 0.5, 0.8).
    TargetInvalid,
}

impl PointerTint {
    /// The color with which the 3D pointer is drawn.
    pub fn rgba(self) -> [f32; 4] {
        match self {
            Self::White => [1.0, 1.0, 1.0, 1.0],
            Self::BuildValid => [0.0, 1.0, 0.0, 1.0],
            Self::BuildInvalid => [1.0, 0.0, 0.0, 1.0],
            Self::TargetValid => [0.5, 1.0, 0.5, 0.8],
            Self::TargetInvalid => [1.0, 0.5, 0.5, 0.8],
        }
    }
}

/// Pointer ID, variant and tint (`Cursor3D+0x38`, `+0x2C`, `+0x68…`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerChoice {
    pub id: i32,
    /// 0 normal, 1 red, 2 green.
    pub variant: u8,
    pub tint: PointerTint,
}

impl PointerChoice {
    /// Pointer 7 in variant 0: menus, command bar, buttons, end of the targeting mode.
    pub const NORMAL: Self = Self {
        id: POINTER_NORMAL,
        variant: VARIANT_NORMAL,
        tint: PointerTint::White,
    };

    /// `SetPointer(id)` without `SetVariant`: the variant stays, the tint becomes white (re/pointer-3d MZ.9).
    pub fn with_id(self, id: i32) -> Self {
        Self {
            id,
            tint: PointerTint::White,
            ..self
        }
    }

    /// Do this ID and variant exist in the shipped pointers (0..11, 0..2)?
    pub fn in_range(self) -> bool {
        (0..POINTER_COUNT).contains(&self.id) && self.variant < VARIANT_COUNT
    }
}

impl Default for PointerChoice {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// Who takes a mouse movement first (children of the root in list order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerArea {
    /// Rectangle of the minimap (`0x0044A4C0`).
    Minimap,
    /// Rectangle of the command bar (`0x00453790` `PtInRect`).
    Toolbar,
    /// Everything else (also the status bar and the gap between command bar and minimap).
    World,
}

/// Open cursor of the command bar, as far as it touches the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerCursor {
    /// No cursor.
    None,
    /// Build or tent cursor: does not take the movement. With the software pointer (`software`, `HardwareCursor 0`)
    /// it tints the pointer with the verdict of its preview (`valid`; `None` without preview), otherwise the variant
    /// stays 0.
    Build { valid: Option<bool>, software: bool },
    /// Wall cursor: takes the movement without a pointer change.
    Wall,
    /// Defense radius: 7, variant stays.
    Defense,
    /// Targeting mode with `MAUSICONID` and the validity of the target under the pointer.
    Target { icon: i32, valid: bool },
}

/// Situation of a frame for the pointer ([`PointerState::step`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerFrame {
    pub area: PointerArea,
    pub cursor: PointerCursor,
    /// A right drag of the world is open (`+0x5BC`): movements look for no pointer.
    pub right_drag: bool,
}

/// What happened since the last frame step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerEvent {
    Move,
    RightDown,
}

/// Pointer of the match (see module header). Not in the savegame: a loaded match starts with 7/0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PointerState {
    /// The set pointer.
    pub choice: PointerChoice,
    pending: Option<PointerEvent>,
    target_open: bool,
    /// The build cursor was open in the last frame (its tint is in the variant).
    build_open: bool,
}

impl PointerState {
    /// The mouse has moved (event 1).
    pub fn moved(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(PointerEvent::Move);
        }
    }

    /// Right press in the world (event 5): looks for the pointer, even if a right drag is open afterwards.
    pub fn right_down(&mut self) {
        self.pending = Some(PointerEvent::RightDown);
    }

    /// A right click with a found row was sent (`0x00457C50` -> `SetPointer(7)`): 7 until the next
    /// movement, the variant stays. Earlier movements of this frame are thereby obsolete.
    pub fn right_click_sent(&mut self) {
        self.choice = self.choice.with_id(POINTER_NORMAL);
        self.pending = None;
    }

    /// The game menu has the mouse (`MenuFrame` `0x00460599`): 7/0.
    pub fn menu(&mut self) {
        self.choice = PointerChoice::NORMAL;
        self.pending = None;
    }

    /// Frame step: open targeting mode, its end, otherwise the movements of the frame (see module header). `world` is
    /// the pointer of the world at the mouse position; called only when it is needed.
    pub fn step(&mut self, frame: &PointerFrame, world: impl FnOnce() -> i32) {
        let pending = self.pending.take();
        if let PointerCursor::Target { icon, valid } = frame.cursor {
            let opened = !std::mem::replace(&mut self.target_open, true);
            if opened || pending.is_some() {
                self.choice = if frame.area == PointerArea::Minimap {
                    self.choice.with_id(POINTER_NORMAL)
                } else {
                    PointerChoice {
                        id: icon,
                        variant: if valid { VARIANT_GREEN } else { VARIANT_RED },
                        tint: if valid {
                            PointerTint::TargetValid
                        } else {
                            PointerTint::TargetInvalid
                        },
                    }
                };
            }
            return;
        }
        if std::mem::take(&mut self.target_open) {
            self.choice = PointerChoice::NORMAL;
            return;
        }
        if let Some(event) = pending {
            match (frame.area, frame.cursor) {
                (PointerArea::Minimap, _) | (_, PointerCursor::Defense) => {
                    self.choice = self.choice.with_id(POINTER_NORMAL);
                }
                (_, PointerCursor::Wall) => {}
                (PointerArea::Toolbar, _) => self.choice = PointerChoice::NORMAL,
                (PointerArea::World, _) if frame.right_drag && event == PointerEvent::Move => {}
                (PointerArea::World, _) => self.choice = self.choice.with_id(world()),
            }
        }
        // The build cursor tints the software pointer in every frame according to the verdict of its preview
        // (`0x0044FFBC`), also over the minimap and the command bar (reference run: an arrow tinted red there); the
        // hardware pointer stays untinted. With the end of the cursor the variant is 0 again (O-394); the tint stays
        // until the next `SetPointer` (the destructor `0x0044FB80` does not write it, O-464).
        match frame.cursor {
            PointerCursor::Build { valid, software } => {
                self.build_open = true;
                if !software {
                    self.choice.variant = VARIANT_NORMAL;
                } else if let Some(valid) = valid {
                    self.choice.variant = if valid { VARIANT_GREEN } else { VARIANT_RED };
                    self.choice.tint = if valid {
                        PointerTint::BuildValid
                    } else {
                        PointerTint::BuildInvalid
                    };
                }
            }
            _ if std::mem::take(&mut self.build_open) => self.choice.variant = VARIANT_NORMAL,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::economy::testkit::*;
    use crate::game::selection::RightClickTarget::Object;
    use crate::game::sim_host::SimHost;
    use nml_formats::targets::{TargetEntry, parse_target_defines, parse_targets};
    use nml_sim::module::gather::{load, set_carried_load};
    use nml_sim::save::checksum;
    use nml_sim::sim::Sim;

    const WORLD: PointerFrame = PointerFrame {
        area: PointerArea::World,
        cursor: PointerCursor::None,
        right_drag: false,
    };

    fn at(area: PointerArea) -> PointerFrame {
        PointerFrame { area, ..WORLD }
    }

    fn with(cursor: PointerCursor) -> PointerFrame {
        PointerFrame { cursor, ..WORLD }
    }

    fn choice(id: i32, variant: u8) -> PointerChoice {
        PointerChoice {
            id,
            variant,
            tint: PointerTint::White,
        }
    }

    /// Compares only ID and variant (the old tests do not know the tint).
    fn idv(p: &PointerState) -> (i32, u8) {
        (p.choice.id, p.choice.variant)
    }

    /// Rows in the manner of `targets.ini` with the values of the test world: 444 deliver wood (load wood, own
    /// building; the filter is simplified) and 411 attack (enemy).
    fn extra_rows() -> Vec<TargetEntry> {
        let text = concat!(
            "BEGIN\nBUTTONMSG 444\nMY_MM\nMM_MODE MM_CHECKLOADWOOD\nMM_END\n",
            "TARGET_MM\nMM_MODE MM_ANDALL\nMM_MODE MM_HL_MYLIST\nMM_MODE MM_CHECKPROPERTY\nMM_PROPID PROP_BUILDING\n",
            "MM_END\nMAUSICONID 8\nEND\n",
            "BEGIN\nBUTTONMSG 411\nMY_MM\nMM_MODE MM_HL_MYLIST\nMM_END\n",
            "TARGET_MM\nMM_MODE MM_HL_ENEMYLIST\nMM_END\nMAUSICONID 6\nEND\n",
        );
        parse_targets(text, &parse_target_defines(DEFINES)).entries
    }

    /// The test world with the `MAUSICONID` of the shipped data (re/game-ui D5.2: 400 -> 5, 412/413 -> 1, 414 -> 2,
    /// 447/448 -> 7) and the rows 444 (before 412, as in the file before the build rows) and 411 (before 447).
    fn pointer_host() -> SimHost {
        let mut host = host();
        for e in &mut host.rules.targets {
            e.mouse_icon = match e.button_msg {
                400 => 5,
                412 | 413 => 1,
                414 => 2,
                _ => 7,
            };
        }
        let [deliver, attack] = <[TargetEntry; 2]>::try_from(extra_rows()).unwrap();
        let at = |host: &SimHost, msg| {
            host.rules
                .targets
                .iter()
                .position(|e| e.button_msg == msg)
                .unwrap()
        };
        let i = at(&host, 412);
        host.rules.targets.insert(i, deliver);
        let i = at(&host, 447);
        host.rules.targets.insert(i, attack);
        host
    }

    /// The pointer over the world is the `MAUSICONID` of the row that the right click would choose (active list),
    /// otherwise 7; the query changes nothing.
    // QUIRK O-296: kept
    #[test]
    fn world_pointer_follows_the_right_click_list() {
        let mut host = pointer_host();
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let store = spawn(&mut host, STORE, 0, (30, 30));
        let enemy = spawn(&mut host, HOUSE, 1, (40, 40));
        host.step_once();
        let before = checksum(&host.state);
        // Without selection 7 (active list empty).
        assert_eq!(host.hover_pointer(&[], Some(Object(tree))), 7);
        assert_eq!(host.hover_pointer(&[], None), 7);
        // Wood cutter over tree 5; fighter over enemy 6; over ground 7 ("Go to" 448).
        assert_eq!(host.hover_pointer(&[worker], Some(Object(tree))), 5);
        assert_eq!(host.hover_pointer(&[worker], Some(Object(enemy))), 6);
        assert_eq!(host.hover_pointer(&[worker], None), 7);
        // Over the own storage without load "Repair" (1), with load wood "Deliver" (8).
        assert_eq!(host.hover_pointer(&[worker], Some(Object(store))), 1);
        let rules = host.rules.clone();
        set_carried_load(
            &mut Sim::new(&mut host.state, &rules),
            worker,
            20,
            load::WOOD,
        );
        let loaded = checksum(&host.state);
        assert_eq!(host.hover_pointer(&[worker], Some(Object(store))), 8);
        // Queries change nothing.
        assert_eq!(checksum(&host.state), loaded);
        assert_ne!(before, loaded, "the load is state");
        assert!(host.state.events.is_empty(), "{:?}", host.state.events);
    }

    /// With a foreign selection: the pointer does not check ownership (`0x00480E80`), sending does
    /// (`0x00457C50`), and only a found row sets 7 afterwards.
    // QUIRK O-296, O-336: kept
    #[test]
    fn foreign_selection_shows_its_pointer_but_sends_nothing() {
        let mut host = pointer_host();
        let tree = spawn(&mut host, TREE, 9, (14, 10));
        let foreign = spawn(&mut host, COLONIST, 1, (12, 10));
        let worker = spawn(&mut host, COLONIST, 0, (10, 10));
        assert_eq!(host.hover_pointer(&[foreign], Some(Object(tree))), 5);
        assert!(!host.right_click_sends(&[foreign], Some(Object(tree))));
        assert!(host.right_click_sends(&[worker], Some(Object(tree))));
        assert!(host.right_click_sends(&[worker], None));
        assert!(!host.right_click_sends(&[], Some(Object(tree))));
    }

    /// Command bar 7/0, minimap 7 with the variant staying, after sending a right click 7 until the next
    /// movement; without movement the pointer stays, even if something changes underneath.
    // QUIRK O-296, O-337: kept
    #[test]
    fn toolbar_and_minimap_pointer() {
        let mut p = PointerState::default();
        assert_eq!(p.choice, PointerChoice::NORMAL);
        p.moved();
        p.step(&WORLD, || 5);
        assert_eq!(idv(&p), (5, 0));
        // No movement: no new pointer (the world is not asked).
        p.step(&WORLD, || unreachable!("no search without movement"));
        assert_eq!(idv(&p), (5, 0));
        p.moved();
        p.step(&at(PointerArea::Toolbar), || unreachable!());
        assert_eq!(p.choice, PointerChoice::NORMAL);
        // Minimap: 7, the variant stays (here the red one from a targeting mode over the minimap).
        p.choice = choice(9, VARIANT_RED);
        p.moved();
        p.step(&at(PointerArea::Minimap), || unreachable!());
        assert_eq!(idv(&p), (7, VARIANT_RED));
        // Right click sent: 7 until the next movement; a movement before it in the same frame is obsolete.
        p.choice = choice(5, 0);
        p.moved();
        p.right_click_sent();
        p.step(&WORLD, || unreachable!());
        assert_eq!(idv(&p), (7, 0));
        p.moved();
        p.step(&WORLD, || 5);
        assert_eq!(idv(&p), (5, 0));
        // Right drag: the press searches, movements afterwards keep the pointer from the press.
        let drag = PointerFrame {
            right_drag: true,
            ..WORLD
        };
        p.right_down();
        p.step(&drag, || 6);
        assert_eq!(idv(&p), (6, 0));
        p.moved();
        p.step(&drag, || unreachable!("right drag does not search"));
        assert_eq!(idv(&p), (6, 0));
        // Build cursor: pointer of the world; wall cursor: unchanged; defense radius: 7.
        p.moved();
        p.step(
            &with(PointerCursor::Build {
                valid: None,
                software: true,
            }),
            || 1,
        );
        assert_eq!(idv(&p), (1, 0));
        p.moved();
        p.step(&with(PointerCursor::Wall), || unreachable!());
        assert_eq!(idv(&p), (1, 0));
        p.moved();
        p.step(&with(PointerCursor::Defense), || unreachable!());
        assert_eq!(idv(&p), (7, 0));
        // The game menu takes the mouse: 7/0.
        p.choice = choice(5, 0);
        p.menu();
        assert_eq!(p.choice, PointerChoice::NORMAL);
    }

    /// Targeting mode red/green with the `MAUSICONID` of the first row, also over the command bar; over the minimap 7
    /// with the variant; end 7/0. With the installation: the values of the first rows and which targets are always
    /// valid.
    // QUIRK O-295: kept
    #[test]
    fn target_mode_pointer_is_red_or_green() {
        let mut p = PointerState::default();
        let target = |icon, valid| with(PointerCursor::Target { icon, valid });
        // Opening sets immediately (`0x004517B0` -> `0x004515B0`), without movement.
        p.step(&target(9, true), || unreachable!());
        assert_eq!(idv(&p), (9, VARIANT_GREEN));
        // Without movement the variant stays, even if the verdict changes.
        p.step(&target(9, false), || unreachable!());
        assert_eq!(idv(&p), (9, VARIANT_GREEN));
        p.moved();
        p.step(&target(9, false), || unreachable!());
        assert_eq!(idv(&p), (9, VARIANT_RED));
        // Over the command bar the targeting mode takes the movement (a ground target stays green).
        p.moved();
        let over_bar = PointerFrame {
            area: PointerArea::Toolbar,
            ..target(7, true)
        };
        p.step(&over_bar, || unreachable!());
        assert_eq!(idv(&p), (7, VARIANT_GREEN));
        // Over the minimap 7 with the last variant.
        p.moved();
        p.step(&target(9, false), || unreachable!());
        p.moved();
        let over_map = PointerFrame {
            area: PointerArea::Minimap,
            ..target(9, true)
        };
        p.step(&over_map, || unreachable!());
        assert_eq!(idv(&p), (7, VARIANT_RED));
        // End: 7/0, also over the world without movement.
        p.step(&WORLD, || unreachable!());
        assert_eq!(p.choice, PointerChoice::NORMAL);

        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return;
        };
        let vfs = nml_rda::Vfs::open_game_dir(&dir).unwrap();
        let text = |path: &str| nml_formats::text::decode_cp1252(&vfs.read(path).unwrap());
        let defines = parse_target_defines(&text(nml_formats::targets::TARGETDEFINES_PATH));
        let rows = parse_targets(&text(nml_formats::targets::TARGETS_PATH), &defines).entries;
        let first = |msg| rows.iter().find(|e| e.button_msg == msg).unwrap();
        // Always valid: ground target (TARGET mode 0) or `MM2_POSITIONALLOWED` (`0x1000`).
        let always = |e: &TargetEntry| e.target.mode == 0 || e.target.mode2 & 0x1000 != 0;
        for (msg, icon, always_green) in [
            (457, 9, false),
            (448, 7, true),
            (447, 7, true),
            (484, 7, true),
            (427, 6, true),
            (414, 2, false),
        ] {
            let e = first(msg);
            assert_eq!((e.mouse_icon, always(e)), (icon, always_green), "{msg}");
        }
        // 414 has three rows (2, 11, 2); the targeting mode takes the first.
        let icons: Vec<i32> = rows
            .iter()
            .filter(|e| e.button_msg == 414)
            .map(|e| e.mouse_icon)
            .collect();
        assert_eq!(icons, [2, 11, 2]);
    }

    /// With the software pointer (`HardwareCursor 0`) the build cursor tints the pointer with the verdict of the build
    /// preview (`BuildCursor_UpdatePlacement` `0x0044FFBC`, green or red) in every frame, also over the minimap (in the
    /// reference run an arrow 7 tinted red there, because the verdict there was red; O-394). With the hardware pointer
    /// the variant stays 0. With the end of the build cursor it is 0 again. This test sets the verdict by hand; for the
    /// real position of `toolbar_step` see
    /// `game_session::tests::build_pointer_tint_follows_the_preview_verdict_over_the_minimap`.
    // QUIRK O-394: kept
    #[test]
    fn build_cursor_tints_the_pointer_also_over_the_minimap() {
        let mut p = PointerState::default();
        let build = |area, valid| PointerFrame {
            area,
            cursor: PointerCursor::Build {
                valid,
                software: true,
            },
            right_drag: false,
        };
        // Over the world: pointer of the world, tinted according to the verdict.
        p.moved();
        p.step(&build(PointerArea::World, Some(true)), || 1);
        assert_eq!(idv(&p), (1, VARIANT_GREEN));
        p.moved();
        p.step(&build(PointerArea::World, Some(false)), || 1);
        assert_eq!(idv(&p), (1, VARIANT_RED));
        // Over the minimap: arrow 7, tinted according to the verdict of the preview (here red by hand; in the
        // reimplementation the verdict on the ground behind the pointer, O-394, re/game-ui D5.1).
        p.moved();
        p.step(&build(PointerArea::Minimap, Some(false)), || unreachable!());
        assert_eq!(idv(&p), (7, VARIANT_RED));
        // The verdict also changes without movement (the preview is checked in every frame).
        p.step(&build(PointerArea::Minimap, Some(true)), || unreachable!());
        assert_eq!(idv(&p), (7, VARIANT_GREEN));
        // Over the command bar the pointer sets 7/0, the build cursor tints again afterwards in the same frame.
        p.moved();
        p.step(&build(PointerArea::Toolbar, Some(false)), || unreachable!());
        assert_eq!(idv(&p), (7, VARIANT_RED));
        // Without preview no tint.
        p.moved();
        p.step(&build(PointerArea::World, None), || 1);
        assert_eq!(idv(&p), (1, VARIANT_RED));
        // End of the build cursor: variant 0, ID stays.
        p.step(&WORLD, || unreachable!());
        assert_eq!(idv(&p), (1, VARIANT_NORMAL));
        // Hardware pointer: no tint, also not over the minimap.
        let hardware = |area| PointerFrame {
            area,
            cursor: PointerCursor::Build {
                valid: Some(false),
                software: false,
            },
            right_drag: false,
        };
        p.moved();
        p.step(&hardware(PointerArea::World), || 1);
        assert_eq!(idv(&p), (1, VARIANT_NORMAL));
        p.moved();
        p.step(&hardware(PointerArea::Minimap), || unreachable!());
        assert_eq!(idv(&p), (7, VARIANT_NORMAL));
        p.step(&WORLD, || unreachable!());
        // Without build cursor the variant stays 0 (movement over the minimap).
        p.moved();
        p.step(&at(PointerArea::Minimap), || unreachable!());
        assert_eq!(idv(&p), (7, VARIANT_NORMAL));
    }

    fn build_frame(area: PointerArea, valid: Option<bool>, software: bool) -> PointerFrame {
        PointerFrame {
            area,
            cursor: PointerCursor::Build { valid, software },
            right_drag: false,
        }
    }

    fn target_frame(area: PointerArea, valid: bool) -> PointerFrame {
        PointerFrame {
            area,
            cursor: PointerCursor::Target { icon: 9, valid },
            right_drag: false,
        }
    }

    /// `TargetMode_OnMouse` `0x004515B0`: the targeting mode tints after the `SetPointer`; end and minimap are white.
    #[test]
    fn target_mode_tints_after_set_pointer() {
        let mut p = PointerState::default();
        p.moved();
        p.step(&target_frame(PointerArea::World, true), || unreachable!());
        assert_eq!(p.choice.tint, PointerTint::TargetValid);
        p.moved();
        p.step(&target_frame(PointerArea::World, false), || unreachable!());
        assert_eq!(p.choice.tint, PointerTint::TargetInvalid);
        // Over the minimap this calls `SetPointer(7)`: white arrow (reference run re/pointer-3d MZ.13).
        p.moved();
        p.step(&target_frame(PointerArea::Minimap, true), || unreachable!());
        assert_eq!((p.choice.id, p.choice.tint), (7, PointerTint::White));
        p.moved();
        p.step(&target_frame(PointerArea::World, true), || unreachable!());
        assert_eq!(p.choice.tint, PointerTint::TargetValid);
        // End (`0x00451540`): 7/0, white.
        p.step(&WORLD, || unreachable!());
        assert_eq!(p.choice, PointerChoice::NORMAL);
    }

    /// O-394: only with the software pointer does the build cursor write the tint; without a verdict it stays.
    // QUIRK O-394: kept
    #[test]
    fn build_cursor_tints_only_with_software() {
        let mut p = PointerState::default();
        p.moved();
        p.step(&build_frame(PointerArea::World, Some(true), true), || 1);
        assert_eq!(p.choice.tint, PointerTint::BuildValid);
        p.step(&build_frame(PointerArea::World, Some(false), true), || 1);
        assert_eq!(p.choice.tint, PointerTint::BuildInvalid);
        p.step(&build_frame(PointerArea::World, None, true), || 1);
        assert_eq!(p.choice.tint, PointerTint::BuildInvalid);
        // Over the minimap: arrow 7 sets white, the build cursor tints afterwards.
        p.moved();
        p.step(
            &build_frame(PointerArea::Minimap, Some(true), true),
            || unreachable!(),
        );
        assert_eq!((p.choice.id, p.choice.tint), (7, PointerTint::BuildValid));
        // Hardware pointer: no tint (the movement sets white).
        p.moved();
        p.step(&build_frame(PointerArea::World, Some(false), false), || 1);
        assert_eq!(p.choice.tint, PointerTint::White);
        p.step(&build_frame(PointerArea::World, Some(false), false), || 1);
        assert_eq!(p.choice.tint, PointerTint::White);
    }

    /// The build cursor writes the tint per frame; without events it stays.
    // QUIRK O-394: kept
    #[test]
    fn build_tint_survives_frames_without_events() {
        let mut p = PointerState::default();
        p.moved();
        p.step(&build_frame(PointerArea::World, Some(true), true), || 1);
        for _ in 0..10 {
            p.step(
                &build_frame(PointerArea::World, Some(true), true),
                || unreachable!(),
            );
            assert_eq!(p.choice.tint, PointerTint::BuildValid);
        }
    }

    /// O-464 (reference run re/pointer-3d MZ.13): after the end of the build cursor the tint stays until the next
    /// `SetPointer`; the variant goes to 0.
    // QUIRK O-464: kept
    #[test]
    fn build_cursor_end_keeps_the_tint_until_the_next_move() {
        let mut p = PointerState::default();
        p.moved();
        p.step(&build_frame(PointerArea::World, Some(false), true), || 1);
        p.step(&WORLD, || unreachable!());
        assert_eq!(p.choice.variant, VARIANT_NORMAL);
        assert_eq!(p.choice.tint, PointerTint::BuildInvalid);
        p.step(&WORLD, || unreachable!());
        assert_eq!(p.choice.tint, PointerTint::BuildInvalid);
        p.moved();
        p.step(&WORLD, || 1);
        assert_eq!(p.choice.tint, PointerTint::White);
    }

    /// Wall cursor and right drag call no `SetPointer`: the tint stays.
    #[test]
    fn wall_cursor_and_right_drag_keep_the_tint() {
        let mut p = PointerState::default();
        p.moved();
        p.step(&build_frame(PointerArea::World, Some(true), true), || 1);
        p.moved();
        p.step(&with(PointerCursor::Wall), || unreachable!());
        assert_eq!(p.choice.tint, PointerTint::BuildValid);
        p.right_down();
        p.step(
            &PointerFrame {
                right_drag: true,
                ..WORLD
            },
            || 1,
        );
        p.moved();
        p.step(
            &PointerFrame {
                right_drag: true,
                ..WORLD
            },
            || unreachable!(),
        );
        assert_eq!(p.choice.tint, PointerTint::White);
        // The right press itself calls `SetPointer`; the movement in the drag afterwards leaves a tint in place.
        p.choice.tint = PointerTint::BuildInvalid;
        p.moved();
        p.step(
            &PointerFrame {
                right_drag: true,
                ..WORLD
            },
            || unreachable!(),
        );
        assert_eq!(p.choice.tint, PointerTint::BuildInvalid);
    }

    /// Menu and sent right click call `SetPointer(7)`: white.
    #[test]
    fn menu_and_sent_right_click_reset_the_tint() {
        let mut p = PointerState::default();
        p.choice.tint = PointerTint::BuildValid;
        p.right_click_sent();
        assert_eq!(p.choice.tint, PointerTint::White);
        p.choice.tint = PointerTint::TargetInvalid;
        p.menu();
        assert_eq!(p.choice, PointerChoice::NORMAL);
    }

    #[test]
    fn tint_rgba_like_the_original() {
        assert_eq!(PointerTint::White.rgba(), [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(PointerTint::BuildValid.rgba(), [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(PointerTint::BuildInvalid.rgba(), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(PointerTint::TargetValid.rgba(), [0.5, 1.0, 0.5, 0.8]);
        assert_eq!(PointerTint::TargetInvalid.rgba(), [1.0, 0.5, 0.5, 0.8]);
    }

    #[test]
    fn choices_outside_the_stock_are_out_of_range() {
        assert!(choice(0, 0).in_range() && choice(11, 2).in_range());
        assert!(!choice(12, 0).in_range());
        assert!(!choice(-1, 0).in_range());
        assert!(!choice(7, 3).in_range());
    }
}
