//! Plays animations like the original: key search and interpolation
//! (`AnimTrack_FindPositionKey` `0x00578D00`, `AnimTrack_EvaluatePosition` `0x005838D0`,
//! `FUN_00578C30`), bones (`Model_ComputeBoneMatrices` `0x00587060`), node tracks
//! (`Model_EvaluateNodeTracks` `0x00589680`), morph tracks (`Model_EvaluateMorphTrack`
//! `0x00587600`), crossfade and 200 ms lock (`ModelInstance_PlayAnimationCrossfade`
//! `0x0058F490`, `g_dwAnimCrossfadeMs`) and sub-objects (`ModelInstance_CreateSubObjectInstances`
//! `0x00588DA0`). The result is in the model's object space (original coordinates).

use std::sync::{Arc, OnceLock};

use glam::{Affine3A, Mat3, Quat, Vec3};
use nml_formats::object_cfg::SubObject;
use nml_formats::rdm::{PositionKey, RdmFile, RotationKey, ScaleKey, bone_track_key};
use nml_formats::rdo::{MeshKind, RdoFile, RdoMesh};

use crate::assets::ModelAsset;
use crate::math::{game_quat, game_quat_matrix, mesh_node_matrix, node_transform_quat};

/// Duration of the crossfade between two animations (`g_dwAnimCrossfadeMs` `0x0066847C`).
pub const CROSSFADE_MS: f32 = 200.0;

/// Time in ticks after `elapsed_ms` milliseconds of run time: milliseconds divided by the time factor
/// (`RdmFile::ticks_to_ms` inverted); with looping modulo the length, otherwise held at the end.
/// A non-finite run time (e.g. infinite) yields 0 instead of NaN.
pub fn ticks_at(anim: &RdmFile, elapsed_ms: f32) -> f32 {
    let factor = if anim.time_factor > 0.0 {
        anim.time_factor
    } else {
        1.0
    };
    let ticks = elapsed_ms.max(0.0) / 1000.0 * anim.ticks_per_second() / factor;
    let length = anim.length_ticks as f32;
    if length <= 0.0 || !ticks.is_finite() {
        0.0
    } else if anim.looping {
        ticks % length
    } else {
        ticks.min(length)
    }
}

/// Segment `(k, k + 1, factor)` for the time `t`: before the first key the first, after the
/// last key the last; with `wrap = Some(length)` the segment after the last key interpolates up to
/// the length toward the first key (`looping` and `interp_first_last`). `time` returns the time of
/// a key; the search runs directly in `keys` (no copy of the times per call). NaN counts as
/// "before the first key".
fn segment<K>(
    keys: &[K],
    time: impl Fn(&K) -> u32,
    t: f32,
    wrap: Option<f32>,
) -> Option<(usize, usize, f32)> {
    let last = keys.len().checked_sub(1)?;
    let at = |k: usize| time(&keys[k]) as f32;
    // NaN ends here (otherwise `partition_point` 0 -> underflow).
    if t.is_nan() || t <= at(0) {
        return Some((0, 0, 0.0));
    }
    if t >= at(last) {
        return match wrap {
            Some(length) if last > 0 && length > at(last) => {
                let f = (t - at(last)) / (length - at(last));
                Some((last, 0, f.clamp(0.0, 1.0)))
            }
            _ => Some((last, last, 0.0)),
        };
    }
    let k = keys.partition_point(|key| time(key) as f32 <= t) - 1;
    let (t0, t1) = (at(k), at(k + 1));
    let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 1.0 };
    Some((k, k + 1, f))
}

fn wrap_of(anim: &RdmFile) -> Option<f32> {
    (anim.looping && anim.interp_first_last).then_some(anim.length_ticks as f32)
}

/// Position at time `t` (linear).
pub fn sample_position(keys: &[PositionKey], t: f32, wrap: Option<f32>) -> Option<Vec3> {
    let (a, b, f) = segment(keys, |k| k.time, t, wrap)?;
    Some(Vec3::from(keys[a].value).lerp(Vec3::from(keys[b].value), f))
}

/// Rotation at time `t` (spherical, shortest path); keys are normalized.
pub fn sample_rotation(keys: &[RotationKey], t: f32, wrap: Option<f32>) -> Option<Quat> {
    let (a, b, f) = segment(keys, |k| k.time, t, wrap)?;
    Some(game_quat(keys[a].wxyz).slerp(game_quat(keys[b].wxyz), f))
}

/// Scale at time `t` (linear; the game does not evaluate the axis quaternion).
pub fn sample_scale(keys: &[ScaleKey], t: f32, wrap: Option<f32>) -> Option<Vec3> {
    let (a, b, f) = segment(keys, |k| k.time, t, wrap)?;
    Some(Vec3::from(keys[a].scale).lerp(Vec3::from(keys[b].scale), f))
}

/// Bone -> index of the bone track in `anim.bone_tracks`: for each bone of the `.rdo` the first track with
/// the same [`bone_track_key`]; `None` if the bone has no track.
pub fn bone_track_map(rdo: &RdoFile, anim: &RdmFile) -> Vec<Option<usize>> {
    let tracks: Vec<String> = anim
        .bone_tracks
        .iter()
        .map(|t| bone_track_key(&t.name))
        .collect();
    rdo.bones
        .iter()
        .map(|bone| {
            let key = bone_track_key(&bone.name);
            tracks.iter().position(|t| *t == key)
        })
        .collect()
}

/// Cache of the [`bone_track_map`] per animation of a [`ModelAsset`], computed the first time
/// the animation is evaluated (the mapping by name would otherwise cost bones x tracks strings
/// per frame). After that a model counts as immutable; it is shared via `Arc`. As a safeguard
/// each entry remembers the address of the animation and the bone count is checked; if either
/// does not match, [`Pose::evaluate`] recomputes the mapping.
#[derive(Debug, Default)]
pub struct BoneTrackCache {
    /// Per entry of `ModelAsset::animations`: address of the animation and the mapping.
    slots: OnceLock<Box<[OnceLock<CachedMap>]>>,
}

/// Address of the animation and the mapping per bone.
type CachedMap = (usize, Box<[Option<usize>]>);

impl BoneTrackCache {
    /// Mapping for `anim` if `anim` is one of the animations of `model` (same address);
    /// otherwise `None`.
    fn get<'a>(&'a self, model: &ModelAsset, anim: &RdmFile) -> Option<&'a [Option<usize>]> {
        let index = model
            .animations
            .iter()
            .position(|a| a.as_deref().is_some_and(|a| std::ptr::eq(a, anim)))?;
        let slots = self
            .slots
            .get_or_init(|| model.animations.iter().map(|_| OnceLock::new()).collect());
        let address = std::ptr::from_ref(anim).addr();
        let (stored, map) = slots
            .get(index)?
            .get_or_init(|| (address, bone_track_map(&model.rdo, anim).into()));
        (*stored == address && map.len() == model.rdo.bones.len()).then_some(&**map)
    }
}

/// Pose of a bone: `v' = R * (v - rest) + p` (object space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BonePose {
    pub rotation: Quat,
    pub position: Vec3,
    /// Which tracks the animation has for this bone; determines the crossfade.
    pub tracks: BoneTracks,
}

/// Tracks of a bone in an animation (`RdoBone+0x11C + a*4` != 0 or position and
/// rotation track != -1 in `Model_ComputeBoneMatrices` `0x00587060`). Bind pose: all `false`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoneTracks {
    /// The bone has a bone track in the animation.
    pub bone: bool,
    /// The track has position keys.
    pub position: bool,
    /// The track has rotation keys.
    pub rotation: bool,
}

/// Pose of a mesh.
#[derive(Debug, Clone, PartialEq)]
pub enum MeshPose {
    /// Node matrix of the `.rdo` (rigid) or object space (skin, helper node).
    Static,
    /// Node track: `v' = ((v - offset) * scale) * R + position` (componentwise scale; `Matrix_FromNodeTransform`).
    Node {
        offset: Vec3,
        scale: Vec3,
        rotation: Quat,
        position: Vec3,
    },
    /// Morph track: positions and normals in object space (world matrix = instance matrix,
    /// `Model_EvaluateMorphTrack` `0x00587600`).
    Morph {
        positions: Vec<[f32; 3]>,
        normals: Vec<[f32; 3]>,
    },
}

/// Pose of a model at a point in time.
#[derive(Debug, Clone, PartialEq)]
pub struct Pose {
    /// Per bone of the `.rdo`.
    pub bones: Vec<BonePose>,
    /// Per mesh of the `.rdo`.
    pub meshes: Vec<MeshPose>,
}

/// Componentwise linear blend of two vector lists (the shorter list determines the length).
fn lerp_vec3s(x: &[[f32; 3]], y: &[[f32; 3]], f: f32) -> Vec<[f32; 3]> {
    x.iter()
        .zip(y)
        .map(|(p, q)| Vec3::from(*p).lerp(Vec3::from(*q), f).to_array())
        .collect()
}

/// Blends bone quaternions: after sign alignment componentwise
/// (`Quaternion_BlendLinear` `0x00576BD0`).
// Deviation: the original does not normalize the result; here it is normalized because `glam`
// only builds rotation matrices correctly from unit quaternions (re/renderer).
fn blend_quat(a: Quat, b: Quat, t: f32) -> Quat {
    let b = if a.dot(b) < 0.0 { -b } else { b };
    (a * (1.0 - t) + b * t).normalize()
}

/// Normalized quaternion like `Quaternion_Normalize` (`0x005753B0`); length 0 or non-finite
/// yields the identity.
fn normalized(q: Quat) -> Quat {
    let length = q.length();
    if length.is_finite() && length > 1e-6 {
        q / length
    } else {
        Quat::IDENTITY
    }
}

/// Blends a bone like `Model_ComputeBoneMatrices` (`0x00587060`): only if the bone has a track in
/// both animations; otherwise the new pose is used (only old: bind pose). Position
/// or rotation are only blended if the old track has them (`0x005872FB`, `0x00587405`);
/// if the new one lacks it, the rest position or the identity is used there.
fn blend_bone(a: &BonePose, b: &BonePose, t: f32) -> BonePose {
    if !(a.tracks.bone && b.tracks.bone) {
        return *b;
    }
    BonePose {
        rotation: if a.tracks.rotation {
            blend_quat(a.rotation, b.rotation, t)
        } else {
            b.rotation
        },
        position: if a.tracks.position {
            a.position.lerp(b.position, t)
        } else {
            b.position
        },
        tracks: b.tracks,
    }
}

/// Blends the pose of a mesh: only with the same track kind in both animations, otherwise the
/// new pose (jump) - `Model_EvaluateNodeTracks` (`0x00589680`) and
/// `Model_EvaluateMorphTrack` (`0x00587600`) use the old animation only for their own
/// track kind.
fn blend_mesh(a: &MeshPose, b: &MeshPose, t: f32) -> MeshPose {
    match (a, b) {
        (
            MeshPose::Node {
                offset: oa,
                scale: sa,
                rotation: ra,
                position: pa,
            },
            MeshPose::Node {
                offset: ob,
                scale: sb,
                rotation: rb,
                position: pb,
            },
        ) => MeshPose::Node {
            offset: oa.lerp(*ob, t),
            scale: sa.lerp(*sb, t),
            // `Quaternion_Slerp` (`0x00576A60`) of normalized quaternions with sign alignment.
            // Deviation (numeric): glam only blends linearly from |cos| > 0.9995, the original from
            // 1 - |cos| < 1e-6.
            rotation: normalized(*ra).slerp(normalized(*rb), t),
            position: pa.lerp(*pb, t),
        },
        (
            MeshPose::Morph {
                positions: pa,
                normals: na,
            },
            MeshPose::Morph {
                positions: pb,
                normals: nb,
            },
        ) if pa.len() == pb.len() => MeshPose::Morph {
            // Per vertex linear, normals without normalization (loop `0x00587960`...`0x00587AB8`).
            positions: lerp_vec3s(pa, pb, t),
            normals: lerp_vec3s(na, nb, t),
        },
        // Static <-> node track, morph <-> static/node track: jump to the new pose.
        _ => b.clone(),
    }
}

/// Pose of a mesh from its morph track in `anim` (frames blended linearly); `None` without a
/// morph track or with frames of differing vertex count (the mesh then stays static).
fn morph_pose(mesh: &RdoMesh, anim: &RdmFile, ticks: f32) -> Option<MeshPose> {
    let track = anim.morph_tracks.iter().find(|t| t.name == mesh.name)?;
    let (a, b, f) = segment(&track.frames, |f| f.time, ticks, wrap_of(anim))?;
    let (fa, fb) = (&track.frames[a], &track.frames[b]);
    if fa.positions.len() != mesh.positions.len() || fb.positions.len() != fa.positions.len() {
        return None;
    }
    Some(MeshPose::Morph {
        positions: lerp_vec3s(&fa.positions, &fb.positions, f),
        normals: lerp_vec3s(&fa.normals, &fb.normals, f),
    })
}

impl Pose {
    /// Bind pose: bones at rest (without tracks), meshes static.
    pub fn rest(model: &ModelAsset) -> Self {
        Self {
            bones: model
                .rdo
                .bones
                .iter()
                .map(|b| BonePose {
                    rotation: Quat::IDENTITY,
                    position: Vec3::from(b.rest_position),
                    tracks: BoneTracks::default(),
                })
                .collect(),
            meshes: vec![MeshPose::Static; model.rdo.meshes.len()],
        }
    }

    /// Pose in animation `anim` at time `ticks`. Bones without a track keep the rest position.
    /// Per mesh the precedence of `ModelInstance_UpdateMeshMatrices` (`0x0058B2D0`): skin mesh
    /// of a model with bones -> bones (morph tracks do not apply there); helper node ->
    /// static; otherwise node track, then morph track, otherwise static (node matrix).
    pub fn evaluate(model: &ModelAsset, anim: &RdmFile, ticks: f32) -> Self {
        let wrap = wrap_of(anim);
        let fresh;
        let map = match model.bone_track_cache.get(model, anim) {
            Some(map) => map,
            None => {
                fresh = bone_track_map(&model.rdo, anim);
                &fresh[..]
            }
        };
        let mut pose = Self::rest(model);
        for (track, state) in map.iter().zip(&mut pose.bones) {
            if let Some(track) = track.and_then(|i| anim.bone_tracks.get(i)) {
                state.tracks.bone = true;
                if let Some(p) = sample_position(&track.positions, ticks, wrap) {
                    state.position = p;
                    state.tracks.position = true;
                }
                if let Some(q) = sample_rotation(&track.rotations, ticks, wrap) {
                    state.rotation = q;
                    state.tracks.rotation = true;
                }
            }
        }
        let has_bones = !model.rdo.bones.is_empty();
        for (mesh, state) in model.rdo.meshes.iter().zip(&mut pose.meshes) {
            match mesh.kind {
                MeshKind::Skin if has_bones => continue,
                MeshKind::Helper => continue,
                _ => {}
            }
            if let Some(track) = anim.node_tracks.iter().find(|t| t.name == mesh.name) {
                let rest_scale = Vec3::from(track.scale);
                let scale = sample_scale(&track.scales, ticks, wrap).map_or(Vec3::ONE, |s| {
                    Vec3::select(rest_scale.cmpeq(Vec3::ZERO), Vec3::ONE, s / rest_scale)
                });
                *state = MeshPose::Node {
                    offset: Vec3::from(track.pivot) * rest_scale,
                    scale,
                    rotation: sample_rotation(&track.rotations, ticks, wrap)
                        .unwrap_or_else(|| game_quat(mesh.rotation_wxyz)),
                    position: sample_position(&track.positions, ticks, wrap)
                        .unwrap_or(Vec3::from(mesh.position)),
                };
            } else if let Some(morph) = morph_pose(mesh, anim, ticks) {
                *state = morph;
            }
        }
        pose
    }

    /// Crossfade `t` in 0..1 from `self` (old animation) to `to` (new) like the original:
    /// an element is blended only if both poses have the same track kind for it
    /// ([`blend_bone`], [`blend_mesh`]); otherwise the new pose is used.
    pub fn blend(&self, to: &Pose, t: f32) -> Pose {
        let bones = self
            .bones
            .iter()
            .zip(&to.bones)
            .map(|(a, b)| blend_bone(a, b, t))
            .collect();
        let meshes = self
            .meshes
            .iter()
            .zip(&to.meshes)
            .map(|(a, b)| blend_mesh(a, b, t))
            .collect();
        Pose { bones, meshes }
    }

    /// Bone matrix `T(p) * R * T(-rest)` (column vectors) for the vertex shader.
    pub fn bone_matrix(&self, model: &ModelAsset, bone: usize) -> Affine3A {
        let state = self.bones[bone];
        let rest = Vec3::from(model.rdo.bones[bone].rest_position);
        let rotation = Mat3::from_quat(state.rotation);
        Affine3A::from_mat3_translation(rotation, state.position - rotation * rest)
    }

    /// Transformation mesh -> object space: node track, otherwise node matrix (rigid) or identity
    /// (skin, morph and helper meshes; morph positions are already in object space).
    pub fn mesh_matrix(&self, model: &ModelAsset, mesh: usize) -> Affine3A {
        match &self.meshes[mesh] {
            MeshPose::Node {
                offset,
                scale,
                rotation,
                position,
            } => node_transform_quat(*offset, *position, *rotation, *scale),
            MeshPose::Morph { .. } => Affine3A::IDENTITY,
            MeshPose::Static => match model.rdo.meshes[mesh].kind {
                MeshKind::Rigid => mesh_node_matrix(&model.rdo.meshes[mesh]),
                _ => Affine3A::IDENTITY,
            },
        }
    }
}

/// `delta` lies in the crossfade window `[0, 200)` ms; like the unsigned comparison of the original,
/// a negative time difference (and NaN) does not count.
fn within_crossfade(delta_ms: f32) -> bool {
    (0.0..CROSSFADE_MS).contains(&delta_ms)
}

/// Playback of a model instance like `ModelInstance_PlayAnimation` (`0x0058F300`, hard) and
/// `ModelInstance_PlayAnimationCrossfade` (`0x0058F490`, crossfading): current animation,
/// start time and the source of the running crossfade.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationPlayer {
    /// Index in `config.animations` (`Instance+0x7C`); `None` = no animation (bind pose).
    pub current: Option<usize>,
    /// Start time of the current animation (`Instance+0x88`).
    pub started_ms: f32,
    /// Source of the crossfade: previous animation and its start time (`Instance+0xB4`/`+0xBC`);
    /// it keeps running during the crossfade.
    source: Option<(usize, f32)>,
    /// Start of the last crossfade (`Instance+0xA8`, 0 = none); blocks further crossfading
    /// switches for 200 ms.
    crossfade_started_ms: Option<f32>,
    /// Playback rate: speed/128 from `SetAnimation` (`AnimInstance_SetTimeScale` divides the
    /// time factor of the animation by the speed/128); 1.0 = normal. The 200 ms of the crossfade
    /// run in real time.
    pub rate: f32,
}

impl Default for AnimationPlayer {
    fn default() -> Self {
        Self::new(None, 0.0)
    }
}

impl AnimationPlayer {
    pub fn new(current: Option<usize>, now_ms: f32) -> Self {
        Self {
            current,
            started_ms: now_ms,
            source: None,
            crossfade_started_ms: None,
            rate: 1.0,
        }
    }

    /// New playback rate; the running animation keeps its pose (start times are
    /// converted).
    pub fn set_rate(&mut self, rate: f32, now_ms: f32) {
        if rate == self.rate || rate <= 0.0 || !rate.is_finite() {
            return;
        }
        let scale = self.rate / rate;
        self.started_ms = now_ms - (now_ms - self.started_ms) * scale;
        if let Some((index, started)) = self.source {
            self.source = Some((index, now_ms - (now_ms - started) * scale));
        }
        self.rate = rate;
    }

    /// Crossfading switch (`ModelInstance_PlayAnimationCrossfade` `0x0058F490`). Rejected
    /// (`false`, nothing changes) as long as the last crossfade is younger than 200 ms.
    /// If no animation was running before or the same one is requested again, the new one starts
    /// immediately (the same one restarts at time 0) without a crossfade; otherwise it crossfades for 200 ms from
    /// the previous, still-running animation. `None` stops the animation
    /// (`ModelInstance_StopAnimation` `0x0058A070`: immediately, without the lock; the time of the
    /// last crossfade stays as in the original).
    // Deviation: the original also rejects an animation that is not loaded; here it is
    // accepted and shown as the bind pose (the model browser should be able to page past it).
    pub fn play(&mut self, index: Option<usize>, now_ms: f32) -> bool {
        let Some(index) = index else {
            self.current = None;
            self.started_ms = now_ms;
            self.source = None;
            return true;
        };
        if self
            .crossfade_started_ms
            .is_some_and(|start| within_crossfade(now_ms - start))
        {
            return false;
        }
        match self.current {
            Some(old) if old != index => {
                self.source = Some((old, self.started_ms));
                self.crossfade_started_ms = Some(now_ms);
            }
            _ => self.source = None,
        }
        self.current = Some(index);
        self.started_ms = now_ms;
        true
    }

    /// Hard switch without crossfade (`ModelInstance_PlayAnimation` `0x0058F300`): aborts a
    /// running crossfade and lifts the lock (`Instance+0xA8` = 0).
    pub fn play_hard(&mut self, index: Option<usize>, now_ms: f32) {
        let rate = self.rate;
        *self = Self::new(index, now_ms);
        self.rate = rate;
    }

    /// A crossfade is still running or blocks further crossfading switches.
    pub fn crossfade_locked(&self, now_ms: f32) -> bool {
        self.crossfade_started_ms
            .is_some_and(|start| within_crossfade(now_ms - start))
    }

    /// A crossfade is currently blending (source present and younger than 200 ms); read-only,
    /// for state queries.
    pub fn crossfade_active(&self, now_ms: f32) -> bool {
        self.source.is_some() && self.crossfade_locked(now_ms)
    }

    /// Remaining time of the 200 ms lock in milliseconds (0 = free); read-only, for
    /// state queries.
    pub fn lock_remaining_ms(&self, now_ms: f32) -> f32 {
        match self.crossfade_started_ms {
            Some(start) if within_crossfade(now_ms - start) => CROSSFADE_MS - (now_ms - start),
            _ => 0.0,
        }
    }

    /// Source of a currently blending crossfade (`Instance+0xB4`), otherwise `None`.
    pub fn crossfade_source(&self, now_ms: f32) -> Option<usize> {
        self.source
            .filter(|_| self.crossfade_active(now_ms))
            .map(|(index, _)| index)
    }

    /// Run time of the current animation in milliseconds.
    pub fn elapsed_ms(&self, now_ms: f32) -> f32 {
        (now_ms - self.started_ms) * self.rate
    }

    fn pose_of(model: &ModelAsset, index: Option<usize>, elapsed_ms: f32) -> Pose {
        match index
            .and_then(|i| model.animations.get(i))
            .and_then(Option::as_ref)
        {
            Some(anim) => Pose::evaluate(model, anim, ticks_at(anim, elapsed_ms)),
            None => Pose::rest(model),
        }
    }

    /// Source of the morph crossfade: `Model_EvaluateMorphTrack` (`0x00587600`) blends as soon as
    /// the old animation has a morph track for the mesh - even if its node track won there and
    /// `old` therefore holds a node pose. For these meshes (new morph, old node) the
    /// old morph track is evaluated afterward; only during the crossfade and only for these meshes.
    fn morph_sources(
        model: &ModelAsset,
        index: usize,
        elapsed_ms: f32,
        old: &mut Pose,
        new: &Pose,
    ) {
        let Some(anim) = model.animations.get(index).and_then(Option::as_ref) else {
            return;
        };
        let ticks = ticks_at(anim, elapsed_ms);
        for ((mesh, state), target) in model
            .rdo
            .meshes
            .iter()
            .zip(&mut old.meshes)
            .zip(&new.meshes)
        {
            if matches!(target, MeshPose::Morph { .. })
                && matches!(state, MeshPose::Node { .. })
                && let Some(morph) = morph_pose(mesh, anim, ticks)
            {
                *state = morph;
            }
        }
    }

    /// Pose at time `now_ms`, in the first 200 ms after a crossfading switch
    /// blended with `t = delta / 200` from the still-running old animation.
    pub fn pose(&self, model: &ModelAsset, now_ms: f32) -> Pose {
        let pose = Self::pose_of(model, self.current, self.elapsed_ms(now_ms));
        match (self.source, self.crossfade_started_ms) {
            (Some((index, started)), Some(switched)) if within_crossfade(now_ms - switched) => {
                let elapsed = (now_ms - started) * self.rate;
                let mut old = Self::pose_of(model, Some(index), elapsed);
                Self::morph_sources(model, index, elapsed, &mut old, &pose);
                old.blend(&pose, (now_ms - switched) / CROSSFADE_MS)
            }
            _ => pose,
        }
    }
}

/// Bones and weights per vertex of a skin mesh like the vertex shader `blendingN.vsh`:
/// at most 4 influences (`RdoFile::vertex_influences`), the first `N - 1` weights from the
/// file, the last `1 - sum` (non-negative); without influence bone 0 with weight 1.
pub fn skin_weights(rdo: &RdoFile, mesh: usize) -> Vec<([u16; 4], [f32; 4])> {
    rdo.vertex_influences(mesh)
        .into_iter()
        .map(|list| {
            let mut joints = [0u16; 4];
            let mut weights = [0f32; 4];
            if list.is_empty() {
                weights[0] = 1.0;
                return (joints, weights);
            }
            for (k, &(bone, weight)) in list.iter().enumerate() {
                joints[k] = bone as u16;
                weights[k] = weight;
            }
            let n = list.len();
            weights[n - 1] = (1.0 - weights[..n - 1].iter().sum::<f32>()).max(0.0);
            (joints, weights)
        })
        .collect()
}

/// Animation for the map view: the first loaded one whose file contains "stehen" (idle),
/// otherwise the first loaded one. The game chooses the animation via the game logic.
pub fn idle_animation(model: &ModelAsset) -> Option<usize> {
    let loaded = |i: &usize| model.animations[*i].is_some();
    let indices: Vec<usize> = (0..model.config.animations.len()).filter(loaded).collect();
    indices
        .iter()
        .copied()
        .find(|&i| {
            model.config.animations[i]
                .file
                .as_deref()
                .is_some_and(|f| f.contains("stehen"))
        })
        .or(indices.first().copied())
}

/// Sub-objects drawn with animation `animation` (overlay of the model browser):
/// by file like [`Playback::active_subs`].
pub fn active_sub_objects(model: &ModelAsset, animation: Option<usize>) -> Vec<&SubObject> {
    let mut subs: Vec<&SubObject> = Vec::new();
    for sub in active_links(model, animation) {
        if !subs.iter().any(|s| s.file == sub.file) {
            subs.push(sub);
        }
    }
    subs
}

/// Links that `ModelInstance_DrawSubObjects` (`0x0058B8D0`) iterates: with a running
/// animation only its list, without animation only the object level with `INDEX` != -1 (the
/// tool inventory with `INDEX` -1 thus never appears via the object level).
fn active_links(model: &ModelAsset, animation: Option<usize>) -> impl Iterator<Item = &SubObject> {
    let (links, object_level) = match animation {
        Some(i) => (
            model
                .config
                .animations
                .get(i)
                .map_or(&[][..], |a| &a.sub_objects[..]),
            false,
        ),
        None => (&model.config.sub_objects[..], true),
    };
    links
        .iter()
        .filter(move |sub| !object_level || sub.index != -1)
}

/// Maximum depth of sub-objects (sub-objects of sub-objects ...). In the shipped data it reaches
/// 2 (weapon of a unit with its own sub-object); the limit only protects against cyclic data,
/// on which the original would recurse endlessly.
pub const MAX_SUB_DEPTH: usize = 8;

/// Sub-instance of a carrier (`ModelInstance_CreateSubObjectInstances` `0x00588DA0`): exactly one
/// per sub-model, shared by all links (object level and animations), with its own
/// playback and - recursive as in the original - its own sub-instances. World matrix =
/// world matrix of the carrier (`ModelInstance_UpdateSubObjectMatrices` `0x0058B530`).
#[derive(Debug, Clone)]
pub struct SubInstance {
    pub asset: Arc<ModelAsset>,
    pub player: AnimationPlayer,
    tree: SubTree,
}

/// Link to a sub-instance: index in `SubTree::subs` and `INDEX` (animation of the
/// sub-model, -1 = none). The original does not read `START` and `LOOP` (only copies them); whether
/// the sub-animation loops is determined by its `.rdm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SubLink {
    sub: usize,
    index: i32,
}

/// Sub-instances of an instance with their links.
#[derive(Debug, Clone, Default)]
struct SubTree {
    subs: Vec<SubInstance>,
    /// Links per entry of `config.animations`.
    animation_links: Vec<Vec<SubLink>>,
    /// Links of the object level.
    object_links: Vec<SubLink>,
}

impl SubTree {
    /// Creates the sub-instances of `model` (recursive below [`MAX_SUB_DEPTH`]);
    /// links that cannot be loaded are dropped as in the original.
    fn build(
        model: &ModelAsset,
        resolve: &mut dyn FnMut(&str) -> Option<Arc<ModelAsset>>,
        depth: usize,
    ) -> Self {
        if depth >= MAX_SUB_DEPTH {
            return Self::default();
        }
        let mut subs: Vec<SubInstance> = Vec::new();
        let mut link = |sub: &SubObject| -> Option<SubLink> {
            let asset = resolve(&sub.file)?;
            // `ModelInstance_FindSubObjectInstanceByModel` (`0x00583280`).
            let index = match subs.iter().position(|s| Arc::ptr_eq(&s.asset, &asset)) {
                Some(i) => i,
                None => {
                    let tree = Self::build(&asset, &mut *resolve, depth + 1);
                    subs.push(SubInstance {
                        asset,
                        player: AnimationPlayer::default(),
                        tree,
                    });
                    subs.len() - 1
                }
            };
            Some(SubLink {
                sub: index,
                index: sub.index,
            })
        };
        let animation_links = model
            .config
            .animations
            .iter()
            .map(|a| a.sub_objects.iter().filter_map(&mut link).collect())
            .collect();
        let object_links = model
            .config
            .sub_objects
            .iter()
            .filter_map(&mut link)
            .collect();
        Self {
            subs,
            animation_links,
            object_links,
        }
    }

    /// Links of the animation `current` (empty without animation).
    fn links_of(&self, current: Option<usize>) -> &[SubLink] {
        current
            .and_then(|i| self.animation_links.get(i))
            .map_or(&[][..], |l| &l[..])
    }

    /// Stops the sub-objects of the list of `current`
    /// (`ModelInstance_StopSubObjectAnimations` `0x00588430`).
    fn stop(&mut self, current: Option<usize>, now_ms: f32) {
        let Self {
            subs,
            animation_links,
            ..
        } = self;
        let links = current
            .and_then(|i| animation_links.get(i))
            .map_or(&[][..], |l| &l[..]);
        for link in links {
            let sub = &mut subs[link.sub];
            play_instance(&mut sub.player, &mut sub.tree, None, now_ms, false);
        }
    }

    /// Starts the animation `INDEX` in each sub-instance of the list of `current`
    /// (`Model_MapAnimationIndexToGuid` `0x00583530`: without `GUID` in the data = `INDEX`; -1
    /// stops it). The start time is that of the carrier (`sub+0x88 = Carrier+0x88`).
    fn propagate(&mut self, current: Option<usize>, now_ms: f32, crossfade: bool) {
        let Self {
            subs,
            animation_links,
            ..
        } = self;
        let Some(links) = current.and_then(|i| animation_links.get(i)) else {
            return;
        };
        for link in links {
            let sub = &mut subs[link.sub];
            let index = usize::try_from(link.index).ok();
            play_instance(&mut sub.player, &mut sub.tree, index, now_ms, crossfade);
        }
    }

    /// Helper nodes in the sub-instances (`ModelInstance_GetHelperNodePositionInSubObjects`
    /// `0x0058BAF0`): with a running animation `current` first its list, then always the
    /// object level with `INDEX` != -1; per sub-instance first its model, then recursively its
    /// sub-instances. The position stays in the model space of the sub-instance. `now_ms` = `None`:
    /// bind pose without animation lists (F5 off).
    // Deviation: playback does not know the upgrade filter `ModelInstance_IsSubObjectLinkEnabled`
    // (`0x0058ABD0`); without an entry every link is active in the original.
    fn helper_node(&self, current: Option<usize>, now_ms: Option<f32>, name: &str) -> Option<Vec3> {
        let animation = self.links_of(current.filter(|_| now_ms.is_some()));
        let object = self.object_links.iter().filter(|link| link.index != -1);
        animation.iter().chain(object).find_map(|link| {
            let sub = &self.subs[link.sub];
            instance_helper_node(&sub.player, &sub.asset, &sub.tree, now_ms, name)
        })
    }
}

impl SubTree {
    /// Playback rate of all sub-instances (recursive).
    fn set_rate(&mut self, rate: f32, now_ms: f32) {
        for sub in &mut self.subs {
            sub.player.set_rate(rate, now_ms);
            sub.tree.set_rate(rate, now_ms);
        }
    }
}

/// Switch of an instance including its sub-instances: `None` first stops the sub-objects of the
/// running list, an accepted switch is passed on to the new list.
fn play_instance(
    player: &mut AnimationPlayer,
    tree: &mut SubTree,
    index: Option<usize>,
    now_ms: f32,
    crossfade: bool,
) -> bool {
    if index.is_none() {
        tree.stop(player.current, now_ms);
    }
    if crossfade {
        if !player.play(index, now_ms) {
            return false;
        }
    } else {
        player.play_hard(index, now_ms);
    }
    tree.propagate(player.current, now_ms, crossfade);
    true
}

/// Helper node of an instance (`ModelInstance_GetHelperNodePosition` `0x0058DAB0`): first the
/// own model, then the sub-instances ([`SubTree::helper_node`]).
fn instance_helper_node(
    player: &AnimationPlayer,
    model: &ModelAsset,
    tree: &SubTree,
    now_ms: Option<f32>,
    name: &str,
) -> Option<Vec3> {
    let own = match now_ms {
        Some(now) => player.helper_node(model, now, name),
        None => AnimationPlayer::default().helper_node(model, 0.0, name),
    };
    own.or_else(|| tree.helper_node(player.current, now_ms, name))
}

/// Playback of a model with its sub-objects: animation switches of the carrier are passed on like
/// `ModelInstance_PropagatePlayToSubObjects` (`0x0058B9E0`) or
/// `ModelInstance_PropagateCrossfadeToSubObjects` (`0x0058F7C0`) to the sub-instances of the new
/// animation list (same mode, same start time), recursively down to the sub-objects of
/// sub-objects.
// Deviation: only the first level of sub-objects is drawn and included in hulls; the
// deeper ones are created and advanced so that the helper node search finds them.
#[derive(Debug, Clone)]
pub struct Playback {
    player: AnimationPlayer,
    tree: SubTree,
}

impl Playback {
    /// Creates the sub-instances of `model` (recursive down to [`MAX_SUB_DEPTH`]) and starts
    /// `animation` hard. `resolve` loads a sub-model by path (same model -> the same
    /// `Arc`, like `ModelCache`); links that cannot be loaded are dropped as in the original.
    pub fn new(
        model: &ModelAsset,
        mut resolve: impl FnMut(&str) -> Option<Arc<ModelAsset>>,
        animation: Option<usize>,
        now_ms: f32,
    ) -> Self {
        let mut playback = Self {
            player: AnimationPlayer::new(None, now_ms),
            tree: SubTree::build(model, &mut resolve, 0),
        };
        playback.play_hard(animation, now_ms);
        playback
    }

    /// Playback of the carrier.
    pub fn player(&self) -> &AnimationPlayer {
        &self.player
    }

    /// All sub-instances of the first level (including those not currently drawn).
    pub fn subs(&self) -> &[SubInstance] {
        &self.tree.subs
    }

    /// Crossfading switch of the carrier ([`AnimationPlayer::play`]); if accepted, each
    /// sub-instance of the new list likewise starts its `INDEX` animation crossfading. `None` stops
    /// and first stops the sub-objects of the running list
    /// (`ModelInstance_StopSubObjectAnimations` `0x00588430`).
    pub fn play(&mut self, index: Option<usize>, now_ms: f32) -> bool {
        play_instance(&mut self.player, &mut self.tree, index, now_ms, true)
    }

    /// Hard switch of the carrier and of the sub-instances of the new list
    /// ([`AnimationPlayer::play_hard`]).
    pub fn play_hard(&mut self, index: Option<usize>, now_ms: f32) {
        play_instance(&mut self.player, &mut self.tree, index, now_ms, false);
    }

    /// Playback rate of the carrier and all sub-instances ([`AnimationPlayer::set_rate`]).
    pub fn set_rate(&mut self, rate: f32, now_ms: f32) {
        self.set_rates(rate, rate, now_ms);
    }

    /// Separate rates for the carrier (`carrier`) and the sub-instances (`subs`). The original sets the speed
    /// when starting an animation for all (`ModelInstance_PlayAnimation` `0x0058F300` with
    /// `ModelInstance_PropagatePlayToSubObjects`), a later `AnimInstance_SetTimeScale` (`0x0058DA30`, via
    /// `ObjectRenderer` vf+0x70) only for the carrier's instance - this is how the wheel animation of the
    /// train works (`GameObject_RefreshAnimIfCurrent` `0x00467080`, re/train-multipart 7).
    pub fn set_rates(&mut self, carrier: f32, subs: f32, now_ms: f32) {
        self.player.set_rate(carrier, now_ms);
        self.tree.set_rate(subs, now_ms);
    }

    /// Sub-instances that `ModelInstance_ComputeBoundingBox` (`0x0058AC40`) includes: with
    /// `animating` and a running animation its list, otherwise the object level - unlike when
    /// drawing **without** the `INDEX` filter (also the tool inventory with `INDEX` -1) and with
    /// repetitions.
    pub fn bounding_subs(&self, animating: bool) -> impl Iterator<Item = &SubInstance> {
        let links = match self.player.current.filter(|_| animating) {
            Some(i) => self.tree.links_of(Some(i)),
            None => &self.tree.object_links[..],
        };
        links.iter().map(|link| &self.tree.subs[link.sub])
    }

    /// Sub-instances that are drawn (`ModelInstance_DrawSubObjects` `0x0058B8D0`): with a
    /// running animation (and `animating`) the list of the current animation, otherwise the
    /// object level without `INDEX` -1. Each sub-instance at most once (the original draws
    /// duplicates in a list several times at the same point).
    pub fn active_subs(&self, animating: bool) -> impl Iterator<Item = &SubInstance> {
        let (links, object_level) = match self.player.current.filter(|_| animating) {
            Some(i) => (self.tree.links_of(Some(i)), false),
            None => (&self.tree.object_links[..], true),
        };
        let shown = move |link: &SubLink| !object_level || link.index != -1;
        links
            .iter()
            .enumerate()
            .filter(move |(k, link)| {
                shown(link) && !links[..*k].iter().any(|l| l.sub == link.sub && shown(l))
            })
            .map(|(_, link)| &self.tree.subs[link.sub])
    }
}

/// Node track of a helper node at a time: pivot `X`, position `p`, rotation `q`, scale `s`
/// (defaults `p = X`, `q` = identity, `s` = 1 like `ModelInstance_GetHelperNodePosition`).
fn helper_track(anim: &RdmFile, name: &str, elapsed_ms: f32) -> Option<(Vec3, Vec3, Quat, Vec3)> {
    let track = anim.node_tracks.iter().find(|t| t.name == name)?;
    let ticks = ticks_at(anim, elapsed_ms);
    let wrap = wrap_of(anim);
    let pivot = Vec3::from(track.pivot);
    Some((
        pivot,
        sample_position(&track.positions, ticks, wrap).unwrap_or(pivot),
        sample_rotation(&track.rotations, ticks, wrap).unwrap_or(Quat::IDENTITY),
        sample_scale(&track.scales, ticks, wrap).unwrap_or(Vec3::ONE),
    ))
}

impl AnimationPlayer {
    /// Position of a helper node (mesh type 3005, name exactly after lowercasing) in object space
    /// like `ModelInstance_GetHelperNodePosition` (`0x0058DAB0`) without sub-objects: with a node
    /// track in the running animation `((v - X) * s) * R(q) + p` (during a crossfade `X`, `p`,
    /// `s` linearly and `q` spherically blended with the old track), otherwise static
    /// `v * R(node) + position`; `v` = center of the node hull.
    pub fn helper_node(&self, model: &ModelAsset, now_ms: f32, name: &str) -> Option<Vec3> {
        let name = name.to_lowercase();
        let mesh = model
            .rdo
            .meshes
            .iter()
            .find(|m| m.kind == MeshKind::Helper && m.name == name)?;
        let v = Vec3::from(mesh.bounds.center);
        let track_of = |index: Option<usize>, started: f32| {
            let anim = model.animations.get(index?)?.as_ref()?;
            helper_track(anim, &mesh.name, (now_ms - started) * self.rate)
        };
        let Some((x, p, q, s)) = track_of(self.current, self.started_ms) else {
            return Some(game_quat_matrix(mesh.rotation_wxyz) * v + Vec3::from(mesh.position));
        };
        let (x, p, q, s) = match (self.source, self.crossfade_started_ms) {
            (Some((old, started)), Some(switched)) if within_crossfade(now_ms - switched) => {
                match track_of(Some(old), started) {
                    Some((x0, p0, q0, s0)) => {
                        let t = (now_ms - switched) / CROSSFADE_MS;
                        (
                            x0.lerp(x, t),
                            p0.lerp(p, t),
                            q0.normalize().slerp(q.normalize(), t),
                            s0.lerp(s, t),
                        )
                    }
                    None => (x, p, q, s),
                }
            }
            _ => (x, p, q, s),
        };
        Some(Mat3::from_quat(q) * ((v - x) * s) + p)
    }
}

impl Playback {
    /// Helper node like `ModelInstance_GetHelperNodePosition` (`0x0058DAB0`): first the model
    /// (object space of the carrier), otherwise recursively the sub-instances
    /// (`ModelInstance_GetHelperNodePositionInSubObjects` `0x0058BAF0`): with a running animation
    /// its list, then the object level with `INDEX` != -1; a node of a sub-instance stays
    /// in its model space.
    pub fn helper_node(&self, model: &ModelAsset, now_ms: f32, name: &str) -> Option<Vec3> {
        instance_helper_node(&self.player, model, &self.tree, Some(now_ms), name)
    }

    /// Like [`Playback::helper_node`] in the bind pose (F5 off): model, then the object level
    /// (`INDEX` != -1) of the sub-instances, likewise in the bind pose.
    pub fn bind_helper_node(&self, model: &ModelAsset, name: &str) -> Option<Vec3> {
        instance_helper_node(&self.player, model, &self.tree, None, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::assets::ModelAsset;
    use nml_formats::object_cfg::{AnimationEntry, ObjectConfig};
    use nml_formats::rdm::RdmLayout;
    use nml_formats::rdo::RdoLayout;
    use nml_formats::testutil::{sample_rdm, sample_rdo};
    use std::sync::Arc;

    /// Sample model with the sample animation as animation 0 ("laufen") and a missing
    /// animation 1 ("stehen").
    pub(super) fn model() -> ModelAsset {
        let entry = |file: &str| AnimationEntry {
            file: Some(file.into()),
            guid: None,
            max_bounding_box: None,
            sub_objects: vec![],
        };
        ModelAsset {
            path: "data/r3d/test/kiste.rdo".into(),
            rdo: sample_rdo(RdoLayout::Standard),
            config: ObjectConfig {
                model_file: None,
                sub_objects: vec![],
                animations: vec![entry("laufen.rdm"), entry("stehen.rdm")],
            },
            animations: vec![Some(Arc::new(sample_rdm(RdmLayout::Standard))), None],
            material_textures: vec![None, None],
            bone_track_cache: Default::default(),
        }
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    /// Sample model with `second` as animation 1 ("stehen").
    fn model_with(second: RdmFile) -> ModelAsset {
        let mut model = model();
        model.animations[1] = Some(Arc::new(second));
        model
    }

    /// Sample animation without node and morph tracks (kiste static) with the bone at
    /// (10, 40, 0), without rotation.
    fn standing() -> RdmFile {
        let mut anim = sample_rdm(RdmLayout::Standard);
        anim.node_tracks.clear();
        anim.morph_tracks.clear();
        for key in &mut anim.bone_tracks[0].positions {
            key.value = [10.0, 40.0, 0.0];
        }
        for key in &mut anim.bone_tracks[0].rotations {
            key.wxyz = [1.0, 0.0, 0.0, 0.0];
        }
        anim
    }

    #[test]
    fn helper_nodes_follow_their_track_or_stay_at_their_position() {
        // `zauber01` (helper node at (0, 120, 0)) gets a node track in animation 1:
        // y 120 -> 150 over 4,800 ticks (1 s).
        let mut anim = standing();
        let mut track = sample_rdm(RdmLayout::Standard).node_tracks[0].clone();
        track.name = "zauber01".into();
        track.rotations.clear();
        track.scales.clear();
        track.positions[0].value = [0.0, 120.0, 0.0];
        track.positions[1].value = [0.0, 150.0, 0.0];
        anim.node_tracks.push(track);
        let model = model_with(anim);
        // Without a track (animation 0) and in the bind pose: position of the mesh head.
        let still = AnimationPlayer::new(Some(0), 0.0);
        assert_eq!(
            still.helper_node(&model, 500.0, "Zauber01"),
            Some(Vec3::new(0.0, 120.0, 0.0))
        );
        let rest = AnimationPlayer::new(None, 0.0);
        assert_eq!(
            rest.helper_node(&model, 500.0, "zauber01"),
            Some(Vec3::new(0.0, 120.0, 0.0))
        );
        // With a track: p(t); unknown names and names of non-helper nodes return nothing.
        let moving = AnimationPlayer::new(Some(1), 0.0);
        assert!(close(
            moving.helper_node(&model, 500.0, "zauber01").unwrap(),
            Vec3::new(0.0, 135.0, 0.0)
        ));
        assert_eq!(moving.helper_node(&model, 500.0, "zauber02"), None);
        assert_eq!(moving.helper_node(&model, 500.0, "kiste"), None);
        // Crossfade from 0 (no track) to 1: without an old track nothing is blended.
        let mut player = AnimationPlayer::new(Some(0), 0.0);
        player.play(Some(1), 0.0);
        assert!(close(
            player.helper_node(&model, 100.0, "zauber01").unwrap(),
            Vec3::new(0.0, 123.0, 0.0)
        ));
    }

    #[test]
    fn crossfade_between_node_track_and_static_mesh_snaps() {
        let model = model_with(standing());
        let origin = |player: &AnimationPlayer, now: f32| {
            let pose = player.pose(&model, now);
            pose.mesh_matrix(&model, 0).transform_point3(Vec3::ZERO)
        };
        // Node track -> static (`ModelInstance_SetStaticMeshMatrix`): the node matrix immediately.
        let mut player = AnimationPlayer::new(Some(0), 0.0);
        player.play(Some(1), 500.0);
        assert!(close(origin(&player, 500.0), Vec3::new(5.0, 0.0, 5.0)));
        assert!(close(origin(&player, 600.0), Vec3::new(5.0, 0.0, 5.0)));
        // Static -> node track: the new pose immediately (at 100 ms y = 2).
        let mut player = AnimationPlayer::new(Some(1), 0.0);
        player.play(Some(0), 0.0);
        assert!(close(origin(&player, 100.0), Vec3::new(5.0, 2.0, 5.0)));
    }

    #[test]
    fn crossfade_mixes_bones_only_with_tracks_in_both_animations() {
        let mut model = model_with(standing());
        // Second bone without a track in both animations.
        let mut extra = model.rdo.bones[0].clone();
        extra.name = "bip01 head".into();
        extra.rest_position = [0.0, 90.0, 0.0];
        model.rdo.bones.push(extra);
        // Animation 1 without a track for the pelvis: old only in 0 -> bind pose immediately.
        let mut no_pelvis = standing();
        no_pelvis.bone_tracks[0].name = "Bip01 Nowhere".into();
        model.animations[1] = Some(Arc::new(no_pelvis));
        let mut player = AnimationPlayer::new(Some(0), 0.0);
        player.play(Some(1), 500.0);
        let pose = player.pose(&model, 600.0);
        assert_eq!(pose.bones[0].position, Vec3::new(10.0, 50.0, 0.0));
        assert_eq!(pose.bones[0].rotation, Quat::IDENTITY);
        assert_eq!(pose.bones[1].position, Vec3::new(0.0, 90.0, 0.0));
        // Conversely only in the new one: its pose immediately (at 100 ms y = 51).
        let mut player = AnimationPlayer::new(Some(1), 0.0);
        player.play(Some(0), 0.0);
        let y = player.pose(&model, 100.0).bones[0].position.y;
        assert!((y - 51.0).abs() < 1e-3, "{y}");
        // Old animation without position keys: position unblended from the new one, rotation
        // blended.
        let mut no_positions = standing();
        no_positions.bone_tracks[0].positions.clear();
        model.animations[1] = Some(Arc::new(no_positions));
        let mut player = AnimationPlayer::new(Some(1), 0.0);
        player.play(Some(0), 0.0);
        let pose = player.pose(&model, 100.0);
        assert!((pose.bones[0].position.y - 51.0).abs() < 1e-3);
        let new = Pose::evaluate(&model, model.animations[0].as_ref().unwrap(), 480.0);
        let angle = pose.bones[0].rotation.angle_between(Quat::IDENTITY);
        let full = new.bones[0].rotation.angle_between(Quat::IDENTITY);
        assert!((angle - full / 2.0).abs() < 1e-3, "{angle} {full}");
    }

    #[test]
    fn no_crossfade_from_the_bind_pose_or_into_the_same_animation() {
        let model = model();
        let y = |player: &AnimationPlayer, now: f32| player.pose(&model, now).bones[0].position.y;
        // No animation before (`Instance+0x7C` = -1): the new one immediately.
        let mut player = AnimationPlayer::new(None, 0.0);
        player.play(Some(0), 0.0);
        assert!((y(&player, 100.0) - 51.0).abs() < 1e-3);
        // The same animation again: restart at 0 without blending.
        let mut player = AnimationPlayer::new(Some(0), 0.0);
        player.play(Some(0), 500.0);
        assert!((y(&player, 500.0) - 50.0).abs() < 1e-3);
    }

    #[test]
    fn node_tracks_crossfade_with_slerp() {
        let model = model();
        let node = |rotation: Quat, x: f32| MeshPose::Node {
            offset: Vec3::ZERO,
            scale: Vec3::ONE,
            rotation,
            position: Vec3::new(x, 0.0, 0.0),
        };
        let mut a = Pose::rest(&model);
        let mut b = Pose::rest(&model);
        a.meshes[0] = node(Quat::IDENTITY, 0.0);
        b.meshes[0] = node(Quat::from_rotation_y(std::f32::consts::PI), 8.0);
        let MeshPose::Node {
            rotation, position, ..
        } = a.blend(&b, 0.25).meshes[0]
        else {
            panic!("no node track");
        };
        // Slerp: a quarter of 180 degrees = 45 degrees (a normalized linear blend would give 36.9 degrees).
        let angle = rotation.angle_between(Quat::IDENTITY).to_degrees();
        assert!((angle - 45.0).abs() < 0.01, "{angle}");
        assert_eq!(position, Vec3::new(2.0, 0.0, 0.0));
    }

    #[test]
    fn morph_tracks_crossfade_per_vertex_with_unnormalized_normals() {
        let model = model();
        let morph = |y: f32, n: [f32; 3]| MeshPose::Morph {
            positions: vec![[0.0, y, 0.0]],
            normals: vec![n],
        };
        let mut a = Pose::rest(&model);
        let mut b = Pose::rest(&model);
        a.meshes[0] = morph(0.0, [1.0, 0.0, 0.0]);
        b.meshes[0] = morph(10.0, [0.0, 1.0, 0.0]);
        assert_eq!(
            a.blend(&b, 0.5).meshes[0],
            morph(5.0, [0.5, 0.5, 0.0]) // length ~ 0.71, not normalized
        );
        // Morph -> static jumps.
        assert_eq!(
            b.blend(&Pose::rest(&model), 0.5).meshes[0],
            MeshPose::Static
        );
    }

    #[test]
    fn morph_crossfade_uses_the_old_morph_track_even_behind_a_node_track() {
        // Old: kiste with a node and a morph track (drawn via the node track); new: only a
        // morph track. `Model_EvaluateMorphTrack` blends as soon as the old animation has a morph
        // track for the mesh - even if its node track won there.
        let mut morph_only = sample_rdm(RdmLayout::Standard);
        morph_only.node_tracks.clear();
        let model = model_with(morph_only);
        let mut player = AnimationPlayer::new(Some(0), 0.0);
        assert!(player.play(Some(1), 500.0));
        let y = |now: f32| match &player.pose(&model, now).meshes[0] {
            MeshPose::Morph { positions, .. } => positions[0][1],
            other => panic!("{other:?}"),
        };
        // At 600 ms: old 2,880 ticks -> y = 18, new 480 ticks -> y = 3, half blended 10.5.
        assert!((y(600.0) - 10.5).abs() < 1e-3, "{}", y(600.0));
        assert!((y(700.0) - 6.0).abs() < 1e-3, "{}", y(700.0));
        // Conversely (new node and morph track): the node track wins, the old has none -> jump.
        let mut player = AnimationPlayer::new(Some(1), 0.0);
        assert!(player.play(Some(0), 500.0));
        assert!(matches!(
            player.pose(&model, 600.0).meshes[0],
            MeshPose::Node { .. }
        ));
    }

    #[test]
    fn skin_meshes_of_a_model_with_bones_ignore_morph_tracks() {
        let mut model = model();
        model.rdo.meshes[0].kind = MeshKind::Skin;
        model.rdo.meshes[0].type_id = 3001;
        let anim = model.animations[0].clone().unwrap();
        // The model has bones: the bones win (`Model_ComputeBoneMatrices`).
        assert_eq!(
            Pose::evaluate(&model, &anim, 2400.0).meshes[0],
            MeshPose::Static
        );
        // Without bones the precedence node track -> morph track applies, as with rigid meshes.
        model.rdo.bones.clear();
        assert!(matches!(
            Pose::evaluate(&model, &anim, 2400.0).meshes[0],
            MeshPose::Node { .. }
        ));
    }

    #[test]
    fn ticks_follow_time_factor_and_loop() {
        let mut anim = sample_rdm(RdmLayout::Standard);
        assert_eq!(ticks_at(&anim, 500.0), 2400.0);
        assert_eq!(ticks_at(&anim, 1500.0), 2400.0); // loop over 4,800 ticks
        anim.time_factor = 2.0; // double duration
        assert_eq!(ticks_at(&anim, 1000.0), 2400.0);
        anim.looping = false;
        assert_eq!(ticks_at(&anim, 99_000.0), 4800.0);
        // Length 0 (35 files without keys) and time factor 0: no NaN.
        anim.length_ticks = 0;
        anim.time_factor = 0.0;
        assert_eq!(ticks_at(&anim, 500.0), 0.0);
    }

    #[test]
    fn infinite_and_nan_time_give_a_finite_pose() {
        let model = model();
        let anim = model.animations[0].as_ref().unwrap();
        // infinity % length would be NaN; NaN would run into an underflow in the key search.
        for ms in [f32::INFINITY, f32::NAN] {
            assert_eq!(ticks_at(anim, ms), 0.0, "{ms}");
        }
        let mut once = (**anim).clone();
        once.looping = false;
        assert_eq!(ticks_at(&once, f32::INFINITY), 0.0);
        for ticks in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            let pose = Pose::evaluate(&model, anim, ticks);
            assert!(pose.bones.iter().all(|b| b.position.is_finite()), "{ticks}");
            assert!(pose.bone_matrix(&model, 0).is_finite(), "{ticks}");
            assert!(pose.mesh_matrix(&model, 0).is_finite(), "{ticks}");
        }
        let keys = &anim.bone_tracks[0].positions;
        assert_eq!(
            sample_position(keys, f32::NAN, Some(6000.0)),
            Some(Vec3::new(10.0, 50.0, 0.0))
        );
        let player = AnimationPlayer::new(Some(0), 0.0);
        for now in [f32::INFINITY, f32::NAN] {
            let pose = player.pose(&model, now);
            assert!(pose.bone_matrix(&model, 0).is_finite(), "{now}");
        }
    }

    #[test]
    fn cached_bone_track_map_matches_a_fresh_one() {
        let mut model = model();
        let anim = model.animations[0].clone().unwrap();
        assert_eq!(bone_track_map(&model.rdo, &anim), [Some(0)]);
        // Own animation: mapping in the cache; foreign copy: computed fresh.
        let cached = Pose::evaluate(&model, &anim, 1200.0);
        assert_eq!(Pose::evaluate(&model, &anim, 1200.0), cached);
        assert_eq!(Pose::evaluate(&model, &(*anim).clone(), 1200.0), cached);
        assert_ne!(cached.bones[0], Pose::rest(&model).bones[0]);
        // Bone count changed: the cache no longer matches, fresh mapping.
        let mut extra = model.rdo.bones[0].clone();
        extra.name = "bip01 head".into();
        model.rdo.bones.push(extra);
        let pose = Pose::evaluate(&model, &anim, 1200.0);
        assert_eq!(pose.bones[0], cached.bones[0]);
        assert_eq!(pose.bones[1], Pose::rest(&model).bones[1]);
    }

    #[test]
    fn keys_hold_at_the_ends_and_interpolate_between() {
        let keys = sample_rdm(RdmLayout::Standard).bone_tracks[0]
            .positions
            .clone();
        assert_eq!(
            sample_position(&keys, -5.0, None),
            Some(Vec3::new(10.0, 50.0, 0.0))
        );
        assert_eq!(
            sample_position(&keys, 2400.0, None),
            Some(Vec3::new(10.0, 55.0, 0.0))
        );
        assert_eq!(
            sample_position(&keys, 9999.0, None),
            Some(Vec3::new(10.0, 60.0, 0.0))
        );
        // Loop transition last -> first over [4,800, 6,000].
        assert_eq!(
            sample_position(&keys, 5400.0, Some(6000.0)),
            Some(Vec3::new(10.0, 55.0, 0.0))
        );
        assert_eq!(sample_position(&[], 0.0, None), None);
        let rot = sample_rdm(RdmLayout::Standard).bone_tracks[0]
            .rotations
            .clone();
        let half = sample_rotation(&rot, 2400.0, None).unwrap();
        // Half of the 90 degree rotation: (1, 0, 0) -> (cos 45 degrees, 0, sin 45 degrees) like game_quat.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        assert!(close(half * Vec3::X, Vec3::new(h, 0.0, h)));
    }

    #[test]
    fn bone_matrix_moves_vertices_like_the_shader() {
        let model = model();
        let anim = model.animations[0].as_ref().unwrap();
        let pose = Pose::evaluate(&model, anim, 4800.0);
        // End: p = (10, 60, 0), 90 degrees about y; rest position (10, 50, 0).
        let m = pose.bone_matrix(&model, 0);
        assert!(close(
            m.transform_point3(Vec3::new(10.0, 50.0, 0.0)),
            Vec3::new(10.0, 60.0, 0.0)
        ));
        assert!(close(
            m.transform_point3(Vec3::new(11.0, 50.0, 0.0)),
            Vec3::new(10.0, 60.0, 1.0)
        ));
        // Bind pose: identity.
        let rest = Pose::rest(&model).bone_matrix(&model, 0);
        assert!(close(
            rest.transform_point3(Vec3::new(3.0, 4.0, 5.0)),
            Vec3::new(3.0, 4.0, 5.0)
        ));
    }

    #[test]
    fn node_and_morph_tracks_replace_the_static_mesh_transform() {
        let model = model();
        let anim = model.animations[0].as_ref().unwrap();
        let pose = Pose::evaluate(&model, anim, 2400.0);
        // kiste has a node and a morph track; the node track takes precedence (rigid mesh).
        assert!(matches!(pose.meshes[0], MeshPose::Node { .. }));
        let m = pose.mesh_matrix(&model, 0);
        assert!(close(
            m.transform_point3(Vec3::ZERO),
            Vec3::new(5.0, 10.0, 5.0)
        ));
        // Without animation: node matrix (5, 0, 5).
        let rest = Pose::rest(&model).mesh_matrix(&model, 0);
        assert!(close(
            rest.transform_point3(Vec3::ZERO),
            Vec3::new(5.0, 0.0, 5.0)
        ));
        // Without a node track the morph track applies: positions interpolated in object space.
        let mut morph_only = (**anim).clone();
        morph_only.node_tracks.clear();
        let pose = Pose::evaluate(&model, &morph_only, 2400.0);
        match &pose.meshes[0] {
            MeshPose::Morph { positions, .. } => assert_eq!(positions[0], [0.0, 15.0, 0.0]),
            other => panic!("{other:?}"),
        }
        assert_eq!(pose.mesh_matrix(&model, 0), Affine3A::IDENTITY);
    }

    #[test]
    fn morph_frames_with_the_wrong_vertex_count_leave_the_mesh_static() {
        let model = model();
        let mut anim = (**model.animations[0].as_ref().unwrap()).clone();
        anim.node_tracks.clear(); // without a node track the morph track applies
        // Only the second frame has one vertex too many: the segment in between is unusable.
        anim.morph_tracks[0].frames[1].positions.push([0.0; 3]);
        anim.morph_tracks[0].frames[1].normals.push([0.0; 3]);
        let pose = Pose::evaluate(&model, &anim, 2400.0);
        assert_eq!(pose.meshes[0], MeshPose::Static);
        // Both frames fit each other, but not the mesh (3 vertices).
        for frame in &mut anim.morph_tracks[0].frames {
            frame.positions.truncate(2);
            frame.normals.truncate(2);
        }
        for ticks in [0.0, 2400.0, 4800.0] {
            let pose = Pose::evaluate(&model, &anim, ticks);
            assert_eq!(pose.meshes[0], MeshPose::Static, "at {ticks} ticks");
            assert_eq!(
                pose.mesh_matrix(&model, 0),
                Pose::rest(&model).mesh_matrix(&model, 0)
            );
        }
    }

    #[test]
    fn crossfade_blends_for_200_ms() {
        let model = model_with(standing());
        let mut player = AnimationPlayer::new(Some(0), 0.0);
        assert!(player.play(Some(1), 500.0));
        // Old animation keeps running: at 500 ms y = 55, at 600 ms y = 56; new y = 40,
        // half blended 48.
        let start = player.pose(&model, 500.0).bones[0].position;
        let mid = player.pose(&model, 600.0).bones[0].position;
        let end = player.pose(&model, 700.0).bones[0].position;
        assert!(close(start, Vec3::new(10.0, 55.0, 0.0)), "{start:?}");
        assert!(close(mid, Vec3::new(10.0, 48.0, 0.0)), "{mid:?}");
        assert!(close(end, Vec3::new(10.0, 40.0, 0.0)), "{end:?}");
        assert_eq!(player.elapsed_ms(800.0), 300.0);
        // Stopping (`None`) jumps to the bind pose immediately.
        assert!(player.play(None, 800.0));
        assert_eq!(player.pose(&model, 800.0), Pose::rest(&model));
    }

    #[test]
    fn a_second_crossfade_within_200_ms_is_rejected() {
        let model = model_with(standing());
        let mut player = AnimationPlayer::new(Some(0), 0.0);
        assert!(player.play(Some(1), 500.0));
        let blending = player.clone();
        // 100 ms later: rejected, the running crossfade stays unchanged.
        assert!(!player.play(Some(0), 600.0));
        assert_eq!(player, blending);
        assert!(player.crossfade_locked(699.0));
        assert!(!player.crossfade_locked(700.0));
        // Accepted after expiry: the source is the previous target animation (still running).
        assert!(player.play(Some(0), 700.0));
        let y = player.pose(&model, 700.0).bones[0].position.y;
        assert!((y - 40.0).abs() < 1e-3, "{y}");
        // The hard switch aborts the crossfade and lifts the lock.
        player.play_hard(Some(1), 750.0);
        let y = player.pose(&model, 750.0).bones[0].position.y;
        assert!((y - 40.0).abs() < 1e-3, "{y}");
        assert!(player.play(Some(0), 760.0));
        // Stopping does not reset the lock (`Instance+0xA8` stays).
        assert!(player.play(None, 800.0));
        assert!(!player.play(Some(1), 900.0));
        assert!(player.play(Some(1), 960.0));
    }

    #[test]
    fn crossfade_and_lock_state_are_readable() {
        let mut player = AnimationPlayer::new(Some(0), 0.0);
        assert!(!player.crossfade_active(0.0));
        assert_eq!(player.lock_remaining_ms(0.0), 0.0);
        assert!(player.play(Some(1), 500.0));
        assert!(player.crossfade_active(550.0));
        assert_eq!(player.lock_remaining_ms(550.0), 150.0);
        assert!(!player.crossfade_active(700.0));
        assert_eq!(player.lock_remaining_ms(700.0), 0.0);
        // Stopping ends the blend, the lock keeps running (as in the original).
        assert!(player.play(Some(0), 700.0));
        assert!(player.play(None, 750.0));
        assert!(!player.crossfade_active(760.0));
        assert_eq!(player.lock_remaining_ms(760.0), 140.0);
        // The hard switch lifts both.
        player.play_hard(Some(1), 770.0);
        assert!(!player.crossfade_active(770.0));
        assert_eq!(player.lock_remaining_ms(770.0), 0.0);
    }

    #[test]
    fn crossfade_aligns_opposite_quaternion_signs() {
        let model = model();
        let at = |rotation: Quat| Pose {
            bones: vec![BonePose {
                rotation,
                position: Vec3::ZERO,
                tracks: BoneTracks {
                    bone: true,
                    position: true,
                    rotation: true,
                },
            }],
            meshes: Pose::rest(&model).meshes,
        };
        let a = at(Quat::from_rotation_y(0.2));
        // The same rotation as 0.4 rad, but with the opposite sign.
        let b = at(-Quat::from_rotation_y(0.4));
        let mid = a.blend(&b, 0.5).bones[0].rotation;
        let expected = Quat::from_rotation_y(0.3);
        assert!(close(mid * Vec3::X, expected * Vec3::X), "{mid:?}");
        assert!(mid.dot(a.bones[0].rotation) > 0.99, "{mid:?}");
        assert!((mid.length() - 1.0).abs() < 1e-5);
        // No jump in the opposite direction either.
        let back = b.blend(&a, 0.5).bones[0].rotation;
        assert!(close(back * Vec3::X, expected * Vec3::X), "{back:?}");
    }

    #[test]
    fn skin_weights_follow_the_vertex_shader() {
        let mut rdo = sample_rdo(RdoLayout::Standard);
        let mut second = rdo.bones[0].clone();
        second.name = "bip01 spine".into();
        second.influences[0].weights = vec![0.3; 4];
        rdo.bones[0].influences[0].weights = vec![0.6; 4];
        rdo.bones.push(second);
        let weights = skin_weights(&rdo, 1);
        // First weight from the file, last 1 - sum: (0.6, 0.4) instead of (0.6, 0.3).
        assert_eq!(weights[0].0, [0, 1, 0, 0]);
        assert!((weights[0].1[0] - 0.6).abs() < 1e-6 && (weights[0].1[1] - 0.4).abs() < 1e-6);
        assert!(
            skin_weights(&rdo, 0)
                .iter()
                .all(|(_, w)| *w == [1.0, 0.0, 0.0, 0.0])
        );
    }

    #[test]
    fn skin_weights_edge_cases() {
        // Vertex 3 without influence: bone 0 with weight 1.
        let mut rdo = sample_rdo(RdoLayout::Standard);
        rdo.bones[0].influences[0].vertices[3] = None;
        let weights = skin_weights(&rdo, 1);
        assert_eq!(weights[3], ([0; 4], [1.0, 0.0, 0.0, 0.0]));
        assert_eq!(weights[0], ([0; 4], [1.0, 0.0, 0.0, 0.0]));

        // File weights with sum > 1: the last weight is clamped to 0, never negative.
        let mut rdo = sample_rdo(RdoLayout::Standard);
        let template = rdo.bones[0].clone();
        rdo.bones = [0.8, 0.9, 0.5]
            .into_iter()
            .map(|weight| {
                let mut bone = template.clone();
                bone.influences[0].weights = vec![weight; 4];
                bone
            })
            .collect();
        let (joints, weights) = skin_weights(&rdo, 1)[0];
        assert_eq!(joints, [0, 1, 2, 0]);
        assert_eq!(weights, [0.8, 0.9, 0.0, 0.0]);

        // More than four influences: only the first four bones, weights sum to 1.
        let mut rdo = sample_rdo(RdoLayout::Standard);
        let template = rdo.bones[0].clone();
        rdo.bones = (0..6)
            .map(|i| {
                let mut bone = template.clone();
                bone.name = format!("bip01 b{i}");
                bone.influences[0].weights = vec![0.2; 4];
                bone
            })
            .collect();
        let (joints, weights) = skin_weights(&rdo, 1)[0];
        assert_eq!(joints, [0, 1, 2, 3]);
        assert_eq!(weights[..3], [0.2, 0.2, 0.2]);
        assert!((weights[3] - 0.4).abs() < 1e-6);
        assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn idle_animation_and_sub_objects() {
        let mut model = model();
        assert_eq!(idle_animation(&model), Some(0)); // "stehen" is missing -> first loaded one
        model.animations[1] = model.animations[0].clone();
        assert_eq!(idle_animation(&model), Some(1));
        // Object level: tool (`INDEX` -1) and hat (`INDEX` 0); animation 1: staff, twice.
        model.config.sub_objects = vec![sub("axe.rdo", -1), sub("hat.rdo", 0)];
        model.config.animations[1].sub_objects =
            vec![sub("staff.rdo", 1), sub("hat.rdo", 0), sub("staff.rdo", 1)];
        let files = |a| {
            active_sub_objects(&model, a)
                .iter()
                .map(|s| s.file.clone())
                .collect::<Vec<_>>()
        };
        // With animation only its list (duplicates once), without it only the object level without -1.
        assert_eq!(files(Some(0)), Vec::<String>::new());
        assert_eq!(files(Some(1)), ["staff.rdo", "hat.rdo"]);
        assert_eq!(files(None), ["hat.rdo"]);
    }

    fn sub(file: &str, index: i32) -> SubObject {
        SubObject {
            file: file.into(),
            index,
            start: 0,
            looping: true,
        }
    }

    /// Carrier with two animations: object level axe (`INDEX` -1) and hat (`INDEX` 0); animation 0
    /// with staff (`INDEX` 0, linked twice) and hat, animation 1 with staff (`INDEX` 1). The
    /// sub-models are copies of the sample model with two animations, path = file name.
    fn carrier() -> (ModelAsset, impl FnMut(&str) -> Option<Arc<ModelAsset>>) {
        let mut model = model_with(standing());
        model.config.sub_objects = vec![sub("axe.rdo", -1), sub("hat.rdo", 0)];
        model.config.animations[0].sub_objects =
            vec![sub("staff.rdo", 0), sub("hat.rdo", 0), sub("staff.rdo", 0)];
        model.config.animations[1].sub_objects = vec![sub("staff.rdo", 1)];
        let library: Vec<(&str, Arc<ModelAsset>)> = ["axe.rdo", "hat.rdo", "staff.rdo"]
            .into_iter()
            .map(|file| {
                let mut asset = model_with(standing());
                asset.path = file.into();
                (file, Arc::new(asset))
            })
            .collect();
        let resolve = move |file: &str| {
            library
                .iter()
                .find(|(name, _)| *name == file)
                .map(|(_, asset)| asset.clone())
        };
        (model, resolve)
    }

    /// Paths of the drawn sub-instances in drawing order.
    fn shown(playback: &Playback, animating: bool) -> Vec<String> {
        playback
            .active_subs(animating)
            .map(|s| s.asset.path.clone())
            .collect()
    }

    /// Sub-instance with path `file`.
    fn instance<'a>(playback: &'a Playback, file: &str) -> &'a SubInstance {
        playback
            .subs()
            .iter()
            .find(|s| s.asset.path == file)
            .unwrap()
    }

    #[test]
    fn one_sub_instance_per_sub_model_and_tools_stay_hidden() {
        let (model, resolve) = carrier();
        let playback = Playback::new(&model, resolve, Some(0), 0.0);
        // One sub-instance per sub-model, shared by all links; created in the
        // order of the lists (animations, then object level): staff, hat, axe.
        let all: Vec<&str> = playback.subs().iter().map(|s| &*s.asset.path).collect();
        assert_eq!(all, ["staff.rdo", "hat.rdo", "axe.rdo"]);
        // Animation 0 started: staff and hat with their `INDEX` animation 0, the axe never.
        assert_eq!(instance(&playback, "staff.rdo").player.current, Some(0));
        assert_eq!(instance(&playback, "hat.rdo").player.current, Some(0));
        assert_eq!(instance(&playback, "axe.rdo").player.current, None);
        // Drawn: the list of animation 0, the twice-linked staff once; the axe of the
        // object level not.
        assert_eq!(shown(&playback, true), ["staff.rdo", "hat.rdo"]);
        // Without animation (F5 off): only the object level without `INDEX` -1 -> the hat.
        assert_eq!(shown(&playback, false), ["hat.rdo"]);
        let stopped = Playback::new(&model, carrier().1, None, 0.0);
        assert_eq!(shown(&stopped, true), ["hat.rdo"]);
        // Sub-models that cannot be loaded are dropped.
        let stopped = Playback::new(&model, |_| None, Some(0), 0.0);
        assert!(stopped.subs().is_empty() && shown(&stopped, true).is_empty());
    }

    #[test]
    fn switching_the_carrier_crossfades_a_sub_model_in_both_lists() {
        let (model, resolve) = carrier();
        let mut playback = Playback::new(&model, resolve, Some(0), 0.0);
        let stab = |p: &Playback| instance(p, "staff.rdo").clone();
        assert_eq!(stab(&playback).player.current, Some(0));
        assert!(playback.play(Some(1), 500.0));
        let sub = stab(&playback);
        // The staff switches with a crossfade to its animation 1 (start time of the carrier).
        assert_eq!(sub.player.current, Some(1));
        assert_eq!(sub.player.started_ms, playback.player().started_ms);
        let mid = sub.player.pose(&sub.asset, 600.0).bones[0].position.y;
        assert!((mid - 48.0).abs() < 1e-3, "{mid}");
        // The hat is not in the list of animation 1: no longer drawn.
        assert_eq!(shown(&playback, true), ["staff.rdo"]);
        // Locked like the carrier; a hard switch also switches the sub-instances hard.
        assert!(!playback.play(Some(0), 600.0));
        playback.play_hard(Some(0), 650.0);
        assert_eq!(stab(&playback).player, AnimationPlayer::new(Some(0), 650.0));
        // Stopping stops the sub-objects of the running list.
        assert!(playback.play(None, 700.0));
        assert!(playback.subs().iter().all(|s| s.player.current.is_none()));
    }

    /// Model with exactly the helper nodes `nodes` (name, position) instead of `zauber01`.
    fn with_helpers(mut model: ModelAsset, nodes: &[(&str, f32)]) -> ModelAsset {
        let helper = model
            .rdo
            .meshes
            .iter()
            .position(|m| m.kind == MeshKind::Helper)
            .unwrap();
        let template = model.rdo.meshes.remove(helper);
        for (name, x) in nodes {
            let mut mesh = template.clone();
            mesh.name = (*name).into();
            mesh.position = [*x, 0.0, 0.0];
            mesh.bounds.center = [0.0; 3];
            model.rdo.meshes.push(mesh);
        }
        model
    }

    #[test]
    fn helper_nodes_are_searched_in_both_lists_and_at_any_depth() {
        // Carrier without own nodes; animation 0 with staff, animation 1 without sub-objects;
        // object level: axe (`INDEX` -1) and hat. The staff itself has a sub-object (tip).
        let mut carrier = with_helpers(model_with(standing()), &[("carrier01", 1.0)]);
        carrier.config.sub_objects = vec![sub("axe.rdo", -1), sub("hat.rdo", 0)];
        carrier.config.animations[0].sub_objects = vec![sub("staff.rdo", 0)];
        let make = |path: &str, nodes: &[(&str, f32)]| {
            let mut asset = with_helpers(model_with(standing()), nodes);
            asset.path = path.into();
            asset
        };
        let mut stab = make("staff.rdo", &[("staff01", 2.0), ("both01", 3.0)]);
        stab.config.sub_objects = vec![sub("tip.rdo", 0)];
        let library: Vec<Arc<ModelAsset>> = vec![
            Arc::new(stab),
            Arc::new(make("hat.rdo", &[("hat01", 4.0), ("both01", 5.0)])),
            Arc::new(make("axe.rdo", &[("axe01", 6.0)])),
            Arc::new(make("tip.rdo", &[("tip01", 7.0)])),
        ];
        let resolve = |file: &str| library.iter().find(|a| a.path == file).cloned();
        let mut playback = Playback::new(&carrier, resolve, Some(0), 0.0);
        let x = |p: &Playback, name: &str| p.helper_node(&carrier, 0.0, name).map(|v| v.x);
        // First the model, then the list of the running animation (staff before hat), then the
        // object level without `INDEX` -1; the tip on the second level is found as well.
        assert_eq!(x(&playback, "carrier01"), Some(1.0));
        assert_eq!(x(&playback, "both01"), Some(3.0));
        assert_eq!(x(&playback, "hat01"), Some(4.0));
        assert_eq!(x(&playback, "tip01"), Some(7.0));
        assert_eq!(x(&playback, "axe01"), None);
        // Animation 1 without sub-objects: the hat of the object level can still be found, the staff cannot.
        playback.play_hard(Some(1), 100.0);
        assert_eq!(x(&playback, "both01"), Some(5.0));
        assert_eq!(x(&playback, "staff01"), None);
        // Bind pose (F5 off): only model and object level.
        playback.play_hard(Some(0), 200.0);
        assert_eq!(
            playback.bind_helper_node(&carrier, "both01").map(|v| v.x),
            Some(5.0)
        );
        assert_eq!(playback.bind_helper_node(&carrier, "staff01"), None);
    }

    #[test]
    fn cyclic_sub_objects_stop_at_the_depth_limit() {
        let mut model = model_with(standing());
        model.path = "cycle.rdo".into();
        model.config.sub_objects = vec![sub("cycle.rdo", 0)];
        let model = Arc::new(model);
        let playback = Playback::new(&model, |_| Some(model.clone()), Some(0), 0.0);
        let mut depth = 0;
        let mut subs = playback.subs();
        while let Some(sub) = subs.first() {
            depth += 1;
            subs = &sub.tree.subs;
        }
        assert_eq!(depth, MAX_SUB_DEPTH);
        assert_eq!(playback.helper_node(&model, 0.0, "missing"), None);
    }

    #[test]
    fn the_playback_rate_scales_time_and_keeps_the_pose_when_changed() {
        let mut p = AnimationPlayer::new(Some(0), 1000.0);
        assert_eq!(p.elapsed_ms(1500.0), 500.0);
        p.set_rate(2.0, 1500.0);
        // Pose stays, then twice as fast.
        assert_eq!(p.elapsed_ms(1500.0), 500.0);
        assert_eq!(p.elapsed_ms(1600.0), 700.0);
        p.play_hard(Some(1), 2000.0);
        assert_eq!((p.rate, p.elapsed_ms(2100.0)), (2.0, 200.0));
        p.set_rate(0.0, 2100.0);
        assert_eq!(p.rate, 2.0);
        assert_eq!(AnimationPlayer::default().rate, 1.0);
    }

    // QUIRK D-370: kept
    #[test]
    fn carrier_and_sub_instances_take_separate_rates() {
        let (model, resolve) = carrier();
        let mut playback = Playback::new(&model, resolve, Some(0), 0.0);
        playback.set_rates(4.0, 1.0, 100.0);
        assert_eq!(playback.player().rate, 4.0);
        assert_eq!(instance(&playback, "staff.rdo").player.rate, 1.0);
        // The common rate applies to all again.
        playback.set_rate(2.0, 200.0);
        assert_eq!(playback.player().rate, 2.0);
        assert_eq!(instance(&playback, "staff.rdo").player.rate, 2.0);
    }
}
