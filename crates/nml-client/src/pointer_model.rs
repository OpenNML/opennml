//! Model and clock of the 3D mouse pointer (`HardwareCursor 0`; re/pointer-3d MZ.2-MZ.4):
//! `mouse.rdo` with its twelve animations (animation = pointer ID), the choice from `SetPointer` (`0x004CD4C0`) and the
//! phase of the loop. GPU-free: [`PointerModel::frame`] returns a [`PointerScene`] for `nml_render::PointerRenderer`.
//!
//! - **Loading** like `Cursor3D_Load` (`0x004CDC40`): carrier with its sub-instances through a dedicated `ModelCache`,
//!   then `SetPointer(7, 1)`. Unlike the original, the `ModelCache` loads the animations together with the model
//!   (deviating from O-461; only the point in time differs).
//! - **New ID** (software branch of `SetPointer`): `ModelInstance_PlayAnimation(id, 128.0, 0, 0)` (`0x0058F300`) hard,
//!   without a crossfade. The last timestamp `+0xA4 = 0` lets the loop begin only in the **first drawn frame**
//!   (`ModelInstance_AdvanceAnimationTime` `0x0058D430`); after that it keeps running in real time, even when the
//!   pointer is not drawn (O-459). The same ID never restarts - "new" means an ID other than the one set last, not a
//!   different choice: variant and tint start nothing.
//! - **Hardware branch** sets only the ID (`0x004CD524`, without animation and without tint): after switching to
//!   software, the model keeps playing the animation of the last software change until the ID changes (O-462).
//! - **Tint** (`Cursor3D+0x68...+0x74`) is taken over only by the software branch.
//! - **Clock** is the program clock of the host (real time, deterministic with `pause`/`step` of the control channel),
//!   smooth instead of in `GetTickCount` steps (deviating from O-460). The phase `e = (now - first frame) mod D` with
//!   `D` = duration of the sub-animation (1000 ms, ID 10 1200 ms, ID 11 1500 ms) is equivalent to the original's
//!   advancing per frame. `nml_world::anim::ticks_at` computes the ticks in file ticks (`t_scaled / time_factor`,
//!   `FUN_00586F70`).
//! - **Queries** ([`PointerModel::preview`], [`PointerModel::data`]) do not fix the start of a loop.
//!
//! IDs outside 0 ... 11 draw pointer 7 as in O-298 (deviation from the original).

use std::sync::Arc;

use nml_formats::gifcon::POINTER_NORMAL;
use nml_rda::Vfs;
use nml_render::pointer::{PointerPiece, PointerScene, pointer_origin};
use nml_world::anim::Playback;
use nml_world::assets::{ModelAsset, ModelCache};

use crate::game::pointer::{POINTER_COUNT, PointerChoice, PointerTint};

/// The pointer model (`Cursor3D_Load` `0x004CDC40`).
pub const MOUSE_MODEL_PATH: &str = "data/r3d/misc/mousecursor/mouse.rdo";

/// State of the pointer model for queries (`ui_state.pointer_model`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerModelData {
    /// ID set last (`Cursor3D+0x38`).
    pub id: i32,
    /// Running animation of the model (the ID of the last software change, pointer 7 outside 0 ... 11).
    pub animation: i32,
    /// Tint `(r, g, b, a)`.
    pub tint: [f32; 4],
    /// Phase of the loop in ms (`elapsed mod D` of the sub-instance); `None` before the first frame after a change.
    pub phase_ms: Option<f32>,
}

/// Model, playback and choice of the 3D pointer.
pub struct PointerModel {
    /// Carrier `mouse.rdo`.
    asset: Arc<ModelAsset>,
    /// Playback of the carrier with its sub-instances (`Cursor3D+0x7C`).
    playback: Playback,
    /// ID set last (`Cursor3D+0x38`).
    id: i32,
    /// Animation of the last software change.
    animation: i32,
    /// Tint (`Cursor3D+0x68...+0x74`).
    tint: PointerTint,
    /// A software change is waiting for the first drawn frame (`+0xA4 = 0`).
    restart: bool,
    /// Branch of the last [`Self::set`] (for [`Self::restart_seven`]).
    software: bool,
}

/// Animation for the ID: pointer 7 outside 0 ... 11 (O-298).
fn animation_of(id: i32) -> i32 {
    if (0..POINTER_COUNT).contains(&id) {
        id
    } else {
        POINTER_NORMAL
    }
}

impl PointerModel {
    /// Loads `mouse.rdo` with its sub-models and animations (dedicated `ModelCache`) and, like `SetPointer(7, 1)` from
    /// `Cursor3D_Load` (`0x004CDC40`), selects pointer 7 white; its loop begins at the first frame.
    pub fn load(vfs: &Vfs, now_ms: f32) -> anyhow::Result<Self> {
        let mut cache = ModelCache::new();
        let asset = Arc::new(cache.load(vfs, MOUSE_MODEL_PATH)?);
        let playback = Playback::new(
            &asset,
            |file| cache.get(vfs, file),
            Some(POINTER_NORMAL as usize),
            now_ms,
        );
        Ok(Self {
            asset,
            playback,
            id: POINTER_NORMAL,
            animation: POINTER_NORMAL,
            tint: PointerTint::White,
            restart: true,
            software: true,
        })
    }

    /// `SetPointer` (`0x004CD4C0`): software sets the tint of the choice and, for a **different ID**, restarts the
    /// animation at the next frame; hardware sets only the ID (O-462). The variant has no effect on the model.
    pub fn set(&mut self, choice: PointerChoice, software: bool) {
        self.software = software;
        if software {
            // `0x004CD4D0...0x004CD4DE`: every software call sets the tint (the choice already carries it).
            self.tint = choice.tint;
            if choice.id != self.id {
                // `0x004CD4E1`: `PlayAnimation(id, 128.0, 0, 0)` only for a new ID.
                self.animation = animation_of(choice.id);
                self.restart = true;
            }
        }
        self.id = choice.id;
    }

    /// End of a video (`0x004C9C23...0x004C9C4B` among others): `SetPointer(0, 1)`, `SetPointer(7, 1)` in the branch of
    /// the last [`Self::set`] - with software the loop of 7 restarts at the next frame, even if 7 was already running.
    pub fn restart_seven(&mut self) {
        let software = self.software;
        self.set(PointerChoice::NORMAL.with_id(0), software);
        self.set(PointerChoice::NORMAL, software);
    }

    /// One drawn frame (`Cursor3D_Render` `0x004CCC50`): an open restart is fixed now
    /// (`Playback::play_hard(now)`, `+0xA4 = now` in the first advance), then the scene for the mouse `mouse` on the
    /// screen `size`.
    pub fn frame(&mut self, now_ms: f32, mouse: (i32, i32), size: (u32, u32)) -> PointerScene {
        if self.restart {
            self.playback
                .play_hard(Some(self.animation as usize), now_ms);
            self.restart = false;
        }
        self.scene(&self.playback, now_ms, mouse, size)
    }

    /// Like [`Self::frame`], without fixing an open restart (queries): it is shown as phase 0.
    pub fn preview(&self, now_ms: f32, mouse: (i32, i32), size: (u32, u32)) -> PointerScene {
        if self.restart {
            let mut playback = self.playback.clone();
            playback.play_hard(Some(self.animation as usize), now_ms);
            self.scene(&playback, now_ms, mouse, size)
        } else {
            self.scene(&self.playback, now_ms, mouse, size)
        }
    }

    /// State for queries.
    pub fn data(&self, now_ms: f32) -> PointerModelData {
        PointerModelData {
            id: self.id,
            animation: self.animation,
            tint: self.tint.rgba(),
            phase_ms: (!self.restart).then(|| self.phase(now_ms)),
        }
    }

    /// `e mod D` of the first drawn sub-instance with an animation (`D` = its duration); without one, the run time of
    /// the carrier.
    fn phase(&self, now_ms: f32) -> f32 {
        let sub = self.playback.active_subs(true).find_map(|sub| {
            let anim = sub.asset.animations.get(sub.player.current?)?.as_ref()?;
            Some((sub.player.elapsed_ms(now_ms), anim.duration_ms()))
        });
        match sub {
            Some((elapsed, duration)) if duration > 0.0 => elapsed.rem_euclid(duration),
            Some((elapsed, _)) => elapsed,
            None => self.playback.player().elapsed_ms(now_ms),
        }
    }

    /// Carrier and the sub-objects of the running animation with their pose at the time `now_ms`
    /// (`ModelInstance_DrawSubObjects` `0x0058B8D0`); all at the origin of the mouse (`0x0058B530`).
    fn scene(
        &self,
        playback: &Playback,
        now_ms: f32,
        mouse: (i32, i32),
        size: (u32, u32),
    ) -> PointerScene {
        let mut pieces = vec![PointerPiece {
            asset: self.asset.clone(),
            pose: Some(playback.player().pose(&self.asset, now_ms)),
        }];
        pieces.extend(playback.active_subs(true).map(|sub| PointerPiece {
            asset: sub.asset.clone(),
            pose: Some(sub.player.pose(&sub.asset, now_ms)),
        }));
        PointerScene {
            pieces,
            origin: pointer_origin(mouse.0, mouse.1, size.0, size.1),
            tint: self.tint.rgba(),
            mouse,
            depth_zero: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use nml_world::anim::ticks_at;

    const SMALL: (u32, u32) = (1024, 768);
    const MOUSE: (i32, i32) = (512, 384);

    /// Pointer model of the installation, loaded at 0 ms; `None` without an installation.
    fn model() -> Option<PointerModel> {
        let Some(dir) = nml_rda::testutil::corpus_game_dir() else {
            eprintln!("Original installation not found - corpus test skipped");
            return None;
        };
        let vfs = Vfs::open_game_dir(&dir).unwrap();
        Some(PointerModel::load(&vfs, 0.0).expect("loading mouse.rdo"))
    }

    fn choice(id: i32) -> PointerChoice {
        PointerChoice::NORMAL.with_id(id)
    }

    fn phase(m: &PointerModel, now: f32) -> Option<f32> {
        m.data(now).phase_ms
    }

    fn assert_near(got: Option<f32>, want: f32, what: &str) {
        let got = got.unwrap_or_else(|| panic!("{what}: no phase"));
        assert!((got - want).abs() < 1e-2, "{what}: {got} instead of {want}");
    }

    /// Sub-animation of the drawn sub-instance: file ticks at the time `now` and time factor.
    fn sub_ticks(m: &PointerModel, now: f32) -> (f32, f32) {
        let sub = m.playback.active_subs(true).next().expect("sub-object");
        let index = sub.player.current.expect("sub-animation");
        let anim = sub.asset.animations[index].as_ref().expect("loaded");
        (ticks_at(anim, sub.player.elapsed_ms(now)), anim.time_factor)
    }

    /// re/pointer-3d MZ.11 "Phase": the loop begins in the first drawn frame (not when loading), e = 1250 ms -> 250 ms,
    /// tick 1200.
    // QUIRK O-459: kept
    // QUIRK O-460: deviation
    #[test]
    fn first_drawn_frame_starts_the_loop() {
        let Some(mut m) = model() else { return };
        assert_eq!(phase(&m, 500.0), None, "before the first frame");
        m.frame(500.0, MOUSE, SMALL);
        assert_near(phase(&m, 500.0), 0.0, "first frame");
        m.frame(1750.0, MOUSE, SMALL);
        assert_near(phase(&m, 1750.0), 250.0, "after 1250 ms");
        let (ticks, factor) = sub_ticks(&m, 1750.0);
        assert_eq!(factor, 1.0);
        assert!((ticks - 1200.0).abs() < 1e-2, "tick {ticks}");
    }

    /// `nml-game` sets the pointer in every frame; the same ID never restarts, not even with a different variant or
    /// tint.
    // QUIRK O-459: kept
    // QUIRK O-460: deviation
    #[test]
    fn same_choice_every_frame_keeps_the_loop() {
        let Some(mut m) = model() else { return };
        for i in 0..100 {
            let now = i as f32 * 16.0;
            m.set(PointerChoice::NORMAL, true);
            m.frame(now, MOUSE, SMALL);
        }
        // First frame at 0, last at 1584 ms: (1584 mod 1000) = 584.
        assert_near(phase(&m, 1584.0), 584.0, "100 frames with 7 white");
        for (variant, tint) in [
            (2, PointerTint::TargetValid),
            (1, PointerTint::BuildInvalid),
        ] {
            m.set(
                PointerChoice {
                    id: 7,
                    variant,
                    tint,
                },
                true,
            );
            m.frame(1600.0, MOUSE, SMALL);
        }
        assert_near(phase(&m, 1600.0), 600.0, "variant and tint start nothing");
    }

    /// O-459: every new ID begins hard at 0 in its first frame, also the return to 7.
    // QUIRK O-459: kept
    #[test]
    fn new_id_restarts_hard() {
        let Some(mut m) = model() else { return };
        m.frame(100.0, MOUSE, SMALL);
        m.frame(400.0, MOUSE, SMALL);
        assert_near(phase(&m, 400.0), 300.0, "7 is running");
        m.set(choice(6), true);
        assert_eq!(m.data(450.0).animation, 6);
        assert_eq!(phase(&m, 450.0), None, "6 before its first frame");
        m.frame(500.0, MOUSE, SMALL);
        assert_near(phase(&m, 500.0), 0.0, "6 in the first frame");
        m.frame(800.0, MOUSE, SMALL);
        assert_near(phase(&m, 800.0), 300.0, "6 is running");
        m.set(choice(7), true);
        m.frame(900.0, MOUSE, SMALL);
        assert_near(phase(&m, 900.0), 0.0, "back to 7: at 0 again");
        assert_eq!(m.data(900.0).animation, 7);
    }

    /// re/pointer-3d MZ.11 "2 s invisible": the phase keeps running without a drawn frame (O-459).
    // QUIRK O-459: kept
    #[test]
    fn hidden_pointer_keeps_running() {
        let Some(mut m) = model() else { return };
        m.frame(0.0, MOUSE, SMALL);
        m.frame(300.0, MOUSE, SMALL);
        assert_near(phase(&m, 300.0), 300.0, "before");
        assert_near(phase(&m, 2300.0), 300.0, "2 s without a frame");
        m.frame(2300.0, MOUSE, SMALL);
        assert_near(phase(&m, 2300.0), 300.0, "next frame");
    }

    /// re/pointer-3d MZ.11: pointer 10 (`ausnehmen`, `zeitfaktor` 1.2) loops in 1200 ms; at 600 ms the scaled tick
    /// is 2880 (= 2400 file ticks).
    // QUIRK O-459: kept
    // QUIRK O-461: deviation
    #[test]
    fn cutting_runs_1200_ms() {
        let Some(mut m) = model() else { return };
        m.set(choice(10), true);
        m.frame(1000.0, MOUSE, SMALL);
        assert_near(phase(&m, 1600.0), 600.0, "600 ms");
        let (ticks, factor) = sub_ticks(&m, 1600.0);
        assert!((factor - 1.2).abs() < 1e-6, "time factor {factor}");
        assert!((ticks * factor - 2880.0).abs() < 1e-1, "tick {ticks}");
        assert_near(phase(&m, 2000.0), 1000.0, "no wrap after 1000 ms");
        assert_near(phase(&m, 2200.0), 0.0, "wrap after 1200 ms");
    }

    /// O-462: the hardware branch sets only the ID; back with software the old animation keeps running until the ID
    /// changes.
    // QUIRK O-462: kept
    #[test]
    fn hardware_sets_the_id_without_animation() {
        let Some(mut m) = model() else { return };
        m.set(choice(7), true);
        m.frame(0.0, MOUSE, SMALL);
        m.set(choice(6), false);
        let d = m.data(100.0);
        assert_eq!((d.id, d.animation), (6, 7));
        assert_near(d.phase_ms, 100.0, "no restart in the hardware branch");
        m.set(choice(6), true);
        m.frame(200.0, MOUSE, SMALL);
        let d = m.data(200.0);
        assert_eq!((d.id, d.animation), (6, 7), "same ID: old animation");
        assert_near(d.phase_ms, 200.0, "no restart");
        m.set(choice(9), true);
        assert_eq!(m.data(250.0).phase_ms, None);
        m.frame(300.0, MOUSE, SMALL);
        let d = m.data(300.0);
        assert_eq!((d.id, d.animation), (9, 9));
        assert_near(d.phase_ms, 0.0, "new ID: restart");
    }

    /// Only the software branch takes over the tint.
    // QUIRK O-462: kept
    #[test]
    fn tint_follows_the_choice_only_with_software() {
        let Some(mut m) = model() else { return };
        assert_eq!(m.data(0.0).tint, [1.0; 4], "loaded white");
        let tinted = |tint| PointerChoice {
            tint,
            ..PointerChoice::NORMAL
        };
        m.set(tinted(PointerTint::BuildValid), true);
        assert_eq!(m.data(0.0).tint, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(m.frame(0.0, MOUSE, SMALL).tint, [0.0, 1.0, 0.0, 1.0]);
        m.set(tinted(PointerTint::TargetValid), false);
        assert_eq!(m.data(0.0).tint, [0.0, 1.0, 0.0, 1.0], "hardware leaves it");
        m.set(tinted(PointerTint::TargetInvalid), true);
        assert_eq!(m.frame(16.0, MOUSE, SMALL).tint, [1.0, 0.5, 0.5, 0.8]);
        m.set(tinted(PointerTint::White), true);
        assert_eq!(m.data(32.0).tint, [1.0; 4]);
    }

    /// A query before the first frame shows phase 0 of the new ID but does not start the loop; the following frame
    /// begins at 0.
    // QUIRK O-459: kept
    #[test]
    fn preview_does_not_start_the_loop() {
        let Some(mut m) = model() else { return };
        let scene = m.preview(500.0, MOUSE, SMALL);
        assert_eq!(scene.pieces.len(), 2);
        assert_eq!(phase(&m, 500.0), None, "query fixes nothing");
        m.frame(800.0, MOUSE, SMALL);
        assert_near(
            phase(&m, 800.0),
            0.0,
            "start in the first frame, not at the query",
        );
        m.set(choice(6), true);
        let scene = m.preview(900.0, MOUSE, SMALL);
        assert!(
            scene.pieces[1].asset.path.ends_with("mouse kämpfen.rdo"),
            "{}",
            scene.pieces[1].asset.path
        );
        assert_eq!(phase(&m, 900.0), None);
        assert_eq!(m.data(900.0).animation, 6);
    }

    /// End of a video: `SetPointer(0)`, `SetPointer(7)` - the loop of 7 restarts even if 7 was already running; in the
    /// hardware branch only the ID.
    // QUIRK O-459: kept
    #[test]
    fn video_end_restarts_seven() {
        let Some(mut m) = model() else { return };
        m.set(choice(6), true);
        m.frame(100.0, MOUSE, SMALL);
        m.restart_seven();
        let d = m.data(150.0);
        assert_eq!((d.id, d.animation, d.phase_ms), (7, 7, None));
        m.frame(200.0, MOUSE, SMALL);
        m.frame(500.0, MOUSE, SMALL);
        assert_near(phase(&m, 500.0), 300.0, "7 is running");
        m.restart_seven();
        m.frame(600.0, MOUSE, SMALL);
        assert_near(phase(&m, 600.0), 0.0, "7 restarts");
        m.set(choice(9), false);
        m.restart_seven();
        let d = m.data(700.0);
        assert_eq!((d.id, d.animation), (7, 7));
        assert_near(d.phase_ms, 100.0, "hardware: no restart");
    }

    /// re/pointer-3d MZ.11 "position": origin from `pointer_origin`, pieces = carrier and the sub-object of animation
    /// 7.
    // QUIRK O-461: deviation
    #[test]
    fn scene_origin_and_pieces() {
        let Some(mut m) = model() else { return };
        let scene = m.frame(0.0, MOUSE, SMALL);
        assert_eq!(scene.origin, pointer_origin(512, 384, 1024, 768));
        assert_eq!(scene.mouse, MOUSE);
        assert_eq!(scene.tint, [1.0; 4]);
        assert!(scene.depth_zero.is_empty());
        let paths: Vec<&str> = scene.pieces.iter().map(|p| p.asset.path.as_str()).collect();
        assert_eq!(paths.len(), 2, "{paths:?}");
        assert!(paths[0].ends_with("mousecursor/mouse.rdo"), "{paths:?}");
        assert!(paths[1].ends_with("mouse normal.rdo"), "{paths:?}");
        assert!(scene.pieces.iter().all(|p| p.pose.is_some()));
    }
}
