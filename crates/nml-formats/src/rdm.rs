//! Animations `.rdm` (identifier `0x3E9`/`0x3EA`), layout in `docs/formats/rdm.md`.
//! Read like `RdmFile_Read` (`0x005A11F0`).

use crate::FormatError;
use crate::binary::{Reader, index};

/// Upper limit for tracks and time markers per file (shipped data: < 150).
pub const MAX_TRACKS: usize = 1000;
/// Upper limit for keys and morph frames per track.
pub const MAX_KEYS: usize = 100_000;
/// Upper limit for vertices per morph frame.
const MAX_MORPH_VERTICES: usize = 1_000_000;

/// Standard (header `0x334`) or legacy format (header `0x238`, no time markers; only `ruder.rdm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdmLayout {
    Standard,
    Legacy,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionKey {
    /// Time in ticks (160 per frame, 4,800 per second).
    pub time: u32,
    pub value: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotationKey {
    pub time: u32,
    /// Quaternion `(w, x, y, z)`; individual keys are not normalized (normalize before use).
    pub wxyz: [f32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleKey {
    pub time: u32,
    pub scale: [f32; 3],
    /// Axis quaternion `(w, x, y, z)`; not tracked by the game (mostly identity).
    pub axis_wxyz: [f32; 4],
}

/// Skeleton track for skin meshes; mapping to the bone via [`bone_track_key`].
#[derive(Debug, Clone, PartialEq)]
pub struct BoneTrack {
    pub name: String,
    /// Not used by the game (the rest position comes from the `.rdo`).
    pub rest_position: [f32; 3],
    pub children: Vec<Option<u32>>,
    /// Absolute positions in object space.
    pub positions: Vec<PositionKey>,
    /// Absolute orientations relative to the bind pose.
    pub rotations: Vec<RotationKey>,
}

/// Rigid-body track of a mesh (exact name comparison with `RdoMesh::name`).
#[derive(Debug, Clone, PartialEq)]
pub struct NodeTrack {
    pub name: String,
    /// Object offset `X` of the Max node (unscaled).
    pub pivot: [f32; 3],
    pub position: [f32; 3],
    pub rotation_wxyz: [f32; 4],
    /// Rest scale `S` (included in the vertices).
    pub scale: [f32; 3],
    pub positions: Vec<PositionKey>,
    pub rotations: Vec<RotationKey>,
    pub scales: Vec<ScaleKey>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MorphFrame {
    pub time: u32,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}

/// Vertex animation of a mesh: complete positions and normals per frame.
#[derive(Debug, Clone, PartialEq)]
pub struct MorphTrack {
    pub name: String,
    pub pivot: [f32; 3],
    pub position: [f32; 3],
    pub rotation_wxyz: [f32; 4],
    pub scale: [f32; 3],
    pub frames: Vec<MorphFrame>,
}

/// Named frame (e.g. `EndLabel`, `Schuß`).
#[derive(Debug, Clone, PartialEq)]
pub struct TimeLabel {
    pub id: u32,
    pub name: String,
    pub frame: i32,
    pub time_ms: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RdmFile {
    pub layout: RdmLayout,
    /// Length in ticks (time of the last key or this + 1).
    pub length_ticks: u32,
    pub ticks_per_frame: u32,
    pub fps: u32,
    /// `SPEED` of the companion `.cfg`: duration factor (0.7 = faster, 4.0 = slower).
    pub time_factor: f32,
    pub start_frame: u32,
    pub looping: bool,
    /// Interpolate from the last to the first key at the end of the loop.
    pub interp_first_last: bool,
    pub bone_tracks: Vec<BoneTrack>,
    pub node_tracks: Vec<NodeTrack>,
    pub morph_tracks: Vec<MorphTrack>,
    pub labels: Vec<TimeLabel>,
}

impl RdmFile {
    /// Ticks per second (`ticks_per_frame * fps`, always 4,800 in the shipped data; 0 -> 4,800).
    pub fn ticks_per_second(&self) -> f32 {
        // Deviation from the original: The product of two raw file values is formed in `u64`
        // so that broken files do not panic through overflow.
        match u64::from(self.ticks_per_frame) * u64::from(self.fps) {
            0 => 4800.0,
            t => t as f32,
        }
    }

    /// Game time of a point in time in milliseconds: ticks in seconds times `time_factor`
    /// (same rule as the time markers `time_ms = floor(floor(frame*1000/fps) * time_factor)`).
    pub fn ticks_to_ms(&self, ticks: u32) -> f32 {
        ticks as f32 * 1000.0 / self.ticks_per_second() * self.time_factor
    }

    /// Playback duration in milliseconds.
    pub fn duration_ms(&self) -> f32 {
        self.ticks_to_ms(self.length_ticks)
    }
}

/// Key of a bone track like `BoneName_StripFirstWord` (`0x005820F0`): lowercase,
/// the part after the first space, empty without a space (`bip01 l thigh` -> `l thigh`).
pub fn bone_track_key(name: &str) -> String {
    let lower = name.to_lowercase();
    match lower.find(' ') {
        Some(i) => lower[i + 1..].to_string(),
        None => String::new(),
    }
}

/// Reads an `.rdm`; first in the standard format, then in the legacy format. Duplicate time marker IDs and markers
/// above the capacity are discarded as in the game.
pub fn parse_rdm(bytes: &[u8]) -> Result<RdmFile, FormatError> {
    match parse_layout(bytes, RdmLayout::Standard) {
        Ok(file) => Ok(file),
        Err(standard) => parse_layout(bytes, RdmLayout::Legacy).map_err(|legacy| {
            FormatError::Invalid(format!(
                "neither standard format ({standard}) nor legacy format ({legacy})"
            ))
        }),
    }
}

fn position_keys(r: &mut Reader, n: usize, what: &str) -> Result<Vec<PositionKey>, FormatError> {
    Ok(r.ints::<4>(n, what)?
        .into_iter()
        .map(|k| PositionKey {
            time: k[0] as u32,
            value: [k[1], k[2], k[3]].map(|v| f32::from_bits(v as u32)),
        })
        .collect())
}

fn rotation_keys(r: &mut Reader, n: usize, what: &str) -> Result<Vec<RotationKey>, FormatError> {
    Ok(r.ints::<5>(n, what)?
        .into_iter()
        .map(|k| RotationKey {
            time: k[0] as u32,
            wxyz: [k[1], k[2], k[3], k[4]].map(|v| f32::from_bits(v as u32)),
        })
        .collect())
}

fn scale_keys(r: &mut Reader, n: usize, what: &str) -> Result<Vec<ScaleKey>, FormatError> {
    Ok(r.ints::<8>(n, what)?
        .into_iter()
        .map(|k| ScaleKey {
            time: k[0] as u32,
            scale: [k[1], k[2], k[3]].map(|v| f32::from_bits(v as u32)),
            axis_wxyz: [k[4], k[5], k[6], k[7]].map(|v| f32::from_bits(v as u32)),
        })
        .collect())
}

/// Common header of node and morph track: name, pivot, position, rotation, scale.
type NodeHeader = (String, [f32; 3], [f32; 3], [f32; 4], [f32; 3]);

fn node_header(r: &mut Reader) -> Result<NodeHeader, FormatError> {
    Ok((
        r.fixed_str(256, "track name")?,
        r.vec3("pivot")?,
        r.vec3("position")?,
        r.vec4("rotation_wxyz")?,
        r.vec3("scale")?,
    ))
}

fn parse_layout(bytes: &[u8], layout: RdmLayout) -> Result<RdmFile, FormatError> {
    let mut r = Reader::new(bytes);
    // Identifier (unchecked), name and path (overwritten by the loader).
    r.skip(4 + 256 + 256, "identifier, name and path")?;
    let length_ticks = r.u32("length_ticks")?;
    let ticks_per_frame = r.u32("ticks_per_frame")?;
    let fps = r.u32("frames_per_second")?;
    let time_factor = r.f32("time_factor")?;
    let start_frame = r.u32("startframe")?;
    let looping = r.u32("loop")? != 0;
    let interp_first_last = r.u32("interp_first_last")? != 0;
    let (bone_count, node_count, morph_count, label_capacity, label_count) = match layout {
        RdmLayout::Standard => {
            let a = r.count(MAX_TRACKS, "bone_track_count")?;
            let b = r.count(MAX_TRACKS, "node_track_count")?;
            let c = r.count(MAX_TRACKS, "morph_track_count")?;
            let capacity = r.count(MAX_TRACKS, "time_marker_capacity")?;
            let d = r.count(MAX_TRACKS, "time_marker_count")?;
            r.skip(256, "reserved")?;
            (a, b, c, capacity, d)
        }
        RdmLayout::Legacy => {
            let a = r.count(MAX_TRACKS, "bone_track_count")?;
            r.skip(4, "legacy format pointer")?;
            let b = r.count(MAX_TRACKS, "node_track_count")?;
            r.skip(4, "legacy format pointer")?;
            let c = r.count(MAX_TRACKS, "morph_track_count")?;
            r.skip(4, "legacy format pointer")?;
            (a, b, c, 0, 0)
        }
    };

    let mut bone_headers = Vec::with_capacity(bone_count);
    for _ in 0..bone_count {
        let name = r.fixed_str(256, "bone track name")?;
        let rest_position = r.vec3("rest_position")?;
        let child_count = r.count(MAX_TRACKS, "child_count")?;
        r.skip(4, "pointer")?;
        let position_count = r.count(MAX_KEYS, "position_key_count")?;
        let rotation_count = r.count(MAX_KEYS, "rotation_key_count")?;
        r.skip(8, "pointer")?;
        let extra_count = r.count(MAX_KEYS, "extra_count")?;
        r.skip(4, "pointer")?;
        bone_headers.push((
            name,
            rest_position,
            child_count,
            position_count,
            rotation_count,
            extra_count,
        ));
    }
    let mut bone_tracks = Vec::with_capacity(bone_count);
    for (name, rest_position, child_count, position_count, rotation_count, extra_count) in
        bone_headers
    {
        let children = r
            .i32s(child_count, "children")?
            .into_iter()
            .map(|c| match c {
                -1 => Ok(None),
                c => index(
                    c,
                    bone_count,
                    &format!("bone track \"{name}\": child index"),
                )
                .map(Some),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let positions = position_keys(&mut r, position_count, "position_keys")?;
        let rotations = rotation_keys(&mut r, rotation_count, "rotation_keys")?;
        // 312-byte extra records, meaning unknown, never present in the shipped data.
        r.skip(extra_count.saturating_mul(312), "extra")?;
        bone_tracks.push(BoneTrack {
            name,
            rest_position,
            children,
            positions,
            rotations,
        });
    }

    let mut node_headers = Vec::with_capacity(node_count);
    for _ in 0..node_count {
        let header = node_header(&mut r)?;
        let position_count = r.count(MAX_KEYS, "position_key_count")?;
        let rotation_count = r.count(MAX_KEYS, "rotation_key_count")?;
        let scale_count = r.count(MAX_KEYS, "scale_key_count")?;
        // track_present (the game goes by the counts), pointers, reserved.
        r.skip(12 + 12 + 24, "flags, pointers and reserved")?;
        node_headers.push((header, position_count, rotation_count, scale_count));
    }
    let mut node_tracks = Vec::with_capacity(node_count);
    for ((name, pivot, position, rotation_wxyz, scale), np, nr, ns) in node_headers {
        node_tracks.push(NodeTrack {
            name,
            pivot,
            position,
            rotation_wxyz,
            scale,
            positions: position_keys(&mut r, np, "position_keys")?,
            rotations: rotation_keys(&mut r, nr, "rotation_keys")?,
            scales: scale_keys(&mut r, ns, "scale_keys")?,
        });
    }

    let mut morph_headers = Vec::with_capacity(morph_count);
    for _ in 0..morph_count {
        let header = node_header(&mut r)?;
        let frame_count = r.count(MAX_KEYS, "frame_count")?;
        r.skip(4, "pointer")?;
        morph_headers.push((header, frame_count));
    }
    let mut morph_tracks = Vec::with_capacity(morph_count);
    for ((name, pivot, position, rotation_wxyz, scale), frame_count) in morph_headers {
        let mut frame_heads = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            let time = r.u32("frame time")?;
            let vertex_count = r.count(MAX_MORPH_VERTICES, "vertex_count")?;
            r.skip(8, "pointer")?;
            frame_heads.push((time, vertex_count));
        }
        let mut frames = Vec::with_capacity(frame_count);
        for (time, vertex_count) in frame_heads {
            frames.push(MorphFrame {
                time,
                positions: r.floats::<3>(vertex_count, "morph positions")?,
                normals: r.floats::<3>(vertex_count, "morph normals")?,
            });
        }
        morph_tracks.push(MorphTrack {
            name,
            pivot,
            position,
            rotation_wxyz,
            scale,
            frames,
        });
    }

    let mut labels: Vec<TimeLabel> = Vec::new();
    for _ in 0..label_count {
        let label = TimeLabel {
            id: r.u32("time marker ID")?,
            name: r.fixed_str(256, "time marker name")?,
            frame: r.i32("time marker frame")?,
            time_ms: r.u32("time_ms")?,
        };
        if labels.len() < label_capacity && labels.iter().all(|l| l.id != label.id) {
            labels.push(label);
        }
    }
    r.finish()?;
    Ok(RdmFile {
        layout,
        length_ticks,
        ticks_per_frame,
        fps,
        time_factor,
        start_frame,
        looping,
        interp_first_last,
        bone_tracks,
        node_tracks,
        morph_tracks,
        labels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{sample_rdm, write_rdm};

    #[test]
    fn standard_rdm_roundtrip() {
        let file = sample_rdm(RdmLayout::Standard);
        assert_eq!(parse_rdm(&write_rdm(&file)).unwrap(), file);
    }

    #[test]
    fn legacy_rdm_roundtrip() {
        let file = sample_rdm(RdmLayout::Legacy);
        assert!(file.labels.is_empty());
        assert_eq!(parse_rdm(&write_rdm(&file)).unwrap(), file);
    }

    #[test]
    fn duplicate_labels_and_capacity_like_the_game() {
        let mut file = sample_rdm(RdmLayout::Standard);
        let label = |id: u32, name: &str| TimeLabel {
            id,
            name: name.into(),
            frame: 3,
            time_ms: 100,
        };
        file.labels = vec![
            label(1, "EndLabel"),
            label(1, "Duplicate"),
            label(2, "Schuß"),
        ];
        let parsed = parse_rdm(&write_rdm(&file)).unwrap();
        assert_eq!(parsed.labels, [label(1, "EndLabel"), label(2, "Schuß")]);
        // Capacity (header 0x22C) limits the list (`RdmFile_AddTimeLabel` 0x005A1100).
        let mut bytes = write_rdm(&file);
        bytes[0x22C..0x230].copy_from_slice(&1i32.to_le_bytes());
        assert_eq!(parse_rdm(&bytes).unwrap().labels, [label(1, "EndLabel")]);
    }

    #[test]
    fn rdm_rejects_absurd_counts() {
        let file = sample_rdm(RdmLayout::Standard);
        let mut bytes = write_rdm(&file);
        bytes[0x220..0x224].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(parse_rdm(&bytes).is_err());
        let mut bad_child = file.clone();
        bad_child.bone_tracks[0].children = vec![Some(7)];
        assert!(parse_rdm(&write_rdm(&bad_child)).is_err());
        let bytes = write_rdm(&file);
        assert!(parse_rdm(&bytes[..bytes.len() - 4]).is_err());
    }

    #[test]
    fn bone_track_key_strips_first_word() {
        assert_eq!(bone_track_key("Bip01 L Thigh"), "l thigh");
        assert_eq!(bone_track_key("bip01 pelvis"), "pelvis");
        assert_eq!(bone_track_key("bip01"), "");
    }

    #[test]
    fn time_conversion_uses_time_factor() {
        let mut file = sample_rdm(RdmLayout::Standard);
        file.time_factor = 0.7;
        assert!((file.ticks_to_ms(4800) - 700.0).abs() < 1e-3);
        file.ticks_per_frame = 0;
        assert_eq!(file.ticks_per_second(), 4800.0);
    }

    #[test]
    fn huge_tick_values_do_not_panic() {
        let mut file = sample_rdm(RdmLayout::Standard);
        file.ticks_per_frame = u32::MAX;
        file.fps = u32::MAX;
        assert!(file.ticks_per_second().is_finite());
        assert!(file.duration_ms().is_finite());
    }
}
