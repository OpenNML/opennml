/*
 * Bink demuxer
 * Copyright (c) 2008-2010 Peter Ross (pross@xvid.org)
 * Copyright (c) 2009 Daniel Verkamp (daniel@drv.nu)
 *
 * This file is part of FFmpeg.
 *
 * FFmpeg is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation; either
 * version 2.1 of the License, or (at your option) any later version.
 *
 * FFmpeg is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with FFmpeg; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA
 */

//! Bink container: header, frame index and splitting of a frame into audio and video packets.
//!
//! Ported from FFmpeg n9.0.2, `libavformat/bink.c` (`read_header`, `read_packet`). Not ported:
//! SMUSH preamble, Bink 2 (`KB2`), revision `k` with its additional header field, `read_seek` and the
//! timestamp computation of the demuxer. Header layout of revision `i`: re/video.
//!
//! Modification notice (LGPL-2.1 section 2 b): Ported to Rust and modified for `opennml-bink`, 2026-10-25.
//!
//! This part of the repository is licensed under LGPL-2.1-or-later (see `COPYING.LGPLv2.1`).

use std::io::{ErrorKind, Read, Seek, SeekFrom};

use crate::error::{BinkError, Result};

/// `BINK_AUD_16BITS`: 16-bit output preferred.
const BINK_AUD_16BITS: u16 = 0x4000;
/// `BINK_AUD_STEREO`.
const BINK_AUD_STEREO: u16 = 0x2000;
/// `BINK_AUD_USEDCT`: DCT sound (otherwise RDFT).
const BINK_AUD_USEDCT: u16 = 0x1000;
/// `BINK_FLAG_ALPHA` (`libavcodec/bink.c`).
const BINK_FLAG_ALPHA: u32 = 0x0010_0000;
/// `BINK_FLAG_GRAY` (`libavcodec/bink.c`).
const BINK_FLAG_GRAY: u32 = 0x0002_0000;
/// `BINK_MAX_AUDIO_TRACKS`.
const MAX_AUDIO_TRACKS: u32 = 256;
/// Upper limit of the frame count (`vst->duration > 1000000` in `read_header`).
const MAX_FRAMES: u32 = 1_000_000;
/// Upper limit for width and height (FFmpeg allows 7680 x 4800; the game needs 640 x 480).
const MAX_DIMENSION: u32 = 4096;

/// An audio track from the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioTrack {
    /// Sample rate in Hz.
    pub sample_rate: u16,
    /// Flags (`0x2000` stereo, `0x4000` 16 bits, `0x1000` DCT).
    pub flags: u16,
    /// ID of the track.
    pub id: u32,
}

impl AudioTrack {
    /// Two channels (`BINK_AUD_STEREO`).
    pub fn stereo(&self) -> bool {
        self.flags & BINK_AUD_STEREO != 0
    }

    /// 16-bit output (`BINK_AUD_16BITS`).
    pub fn bits16(&self) -> bool {
        self.flags & BINK_AUD_16BITS != 0
    }

    /// DCT codec (`BINK_AUD_USEDCT`); without the flag it would be RDFT.
    pub fn dct(&self) -> bool {
        self.flags & BINK_AUD_USEDCT != 0
    }
}

/// An entry of the frame index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameEntry {
    /// File offset of the frame.
    pub offset: u64,
    /// Length of the frame (audio packets and video data) in bytes.
    pub size: u32,
    /// Key frame (frame 0 always, otherwise bit 0 of the index entry).
    pub keyframe: bool,
}

/// Header of a Bink file of revision `i`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinkHeader {
    /// Revision letter (always `b'i'`).
    pub revision: u8,
    /// File size in bytes (the field in the header contains it minus 8).
    pub file_size: u32,
    /// Number of frames.
    pub frames: u32,
    /// Largest frame in bytes (according to the header).
    pub largest_frame: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Frame rate, numerator.
    pub fps_num: u32,
    /// Frame rate, denominator.
    pub fps_den: u32,
    /// Video flags (alpha, grayscale; always 0 here, otherwise `Unsupported`).
    pub video_flags: u32,
    /// Audio tracks in file order.
    pub audio: Vec<AudioTrack>,
    /// One entry per frame.
    pub index: Vec<FrameEntry>,
}

/// Reads four bytes little-endian; a header that is too short is a format error, not an I/O error.
fn read_u32(r: &mut impl Read) -> Result<u32> {
    let mut b = [0u8; 4];
    read_exact(r, &mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u16(r: &mut impl Read) -> Result<u16> {
    let mut b = [0u8; 2];
    read_exact(r, &mut b)?;
    Ok(u16::from_le_bytes(b))
}

/// `read_exact`, where a premature end of file is reported as a truncated file.
fn read_exact(r: &mut impl Read, buf: &mut [u8]) -> Result<()> {
    r.read_exact(buf).map_err(|e| match e.kind() {
        ErrorKind::UnexpectedEof => BinkError::Format("file is truncated".into()),
        _ => BinkError::Io(e),
    })
}

impl BinkHeader {
    /// Reads the header and frame index (`read_header`); `r` is then behind the index, i.e. at the first frame.
    pub fn read(r: &mut impl Read) -> Result<Self> {
        let mut sig = [0u8; 4];
        read_exact(r, &mut sig)?;
        if &sig[..3] != b"BIK" {
            return Err(BinkError::Format(
                "signature \"BIK\" missing, not a Bink file".into(),
            ));
        }
        let revision = sig[3];
        if revision != b'i' {
            return Err(BinkError::Unsupported(format!(
                "Bink revision \"{}\" (only \"i\" is supported)",
                char::from(revision).escape_default()
            )));
        }

        let file_size = read_u32(r)?
            .checked_add(8)
            .ok_or_else(|| BinkError::Format("file size in the header too large".into()))?;
        let frames = read_u32(r)?;
        if frames == 0 || frames > MAX_FRAMES {
            return Err(BinkError::Format(format!(
                "frame count {frames} outside 1 to {MAX_FRAMES}"
            )));
        }
        let largest_frame = read_u32(r)?;
        if largest_frame > file_size {
            return Err(BinkError::Format(
                "largest frame is larger than the file".into(),
            ));
        }
        read_u32(r)?; // unknown field

        let width = read_u32(r)?;
        let height = read_u32(r)?;
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(BinkError::Format(format!(
                "frame size {width} x {height} invalid"
            )));
        }
        let fps_num = read_u32(r)?;
        let fps_den = read_u32(r)?;
        if fps_num == 0 || fps_den == 0 {
            return Err(BinkError::Format(format!(
                "frame rate {fps_num}/{fps_den} invalid"
            )));
        }

        let video_flags = read_u32(r)?;
        if video_flags & (BINK_FLAG_ALPHA | BINK_FLAG_GRAY) != 0 {
            return Err(BinkError::Unsupported(
                "alpha plane and grayscale are not supported".into(),
            ));
        }

        let tracks = read_u32(r)?;
        if tracks > MAX_AUDIO_TRACKS {
            return Err(BinkError::Format(format!(
                "more than {MAX_AUDIO_TRACKS} audio tracks ({tracks})"
            )));
        }
        let mut audio = Vec::with_capacity(tracks as usize);
        if tracks > 0 {
            for _ in 0..tracks {
                read_u32(r)?; // largest decoded size
            }
            for _ in 0..tracks {
                let sample_rate = read_u16(r)?;
                let flags = read_u16(r)?;
                if flags & BINK_AUD_USEDCT == 0 {
                    return Err(BinkError::Unsupported(
                        "audio track without DCT flag (RDFT) is not supported".into(),
                    ));
                }
                audio.push(AudioTrack {
                    sample_rate,
                    flags,
                    id: 0,
                });
            }
            for track in &mut audio {
                track.id = read_u32(r)?;
            }
        }

        // Frame index: `frames + 1` offsets, the last is the end of the file. Bit 0 of an entry
        // marks a key frame; frame 0 always is one (`next_keyframe = 1`).
        let mut raw = Vec::with_capacity(frames as usize + 1);
        for _ in 0..=frames {
            raw.push(read_u32(r)?);
        }
        let mut index = Vec::with_capacity(frames as usize);
        for i in 0..frames as usize {
            let pos = raw[i] & !1;
            let next = raw[i + 1] & !1;
            if next <= pos {
                return Err(BinkError::Format("frame index does not increase".into()));
            }
            index.push(FrameEntry {
                offset: u64::from(pos),
                size: next - pos,
                keyframe: i == 0 || raw[i] & 1 != 0,
            });
        }

        Ok(BinkHeader {
            revision,
            file_size,
            frames,
            largest_frame,
            width,
            height,
            fps_num,
            fps_den,
            video_flags,
            audio,
            index,
        })
    }

    /// Playing time in milliseconds (rounded down): `frames · 1000 · fps_den / fps_num`.
    pub fn duration_ms(&self) -> u64 {
        u64::from(self.frames) * 1000 * u64::from(self.fps_den) / u64::from(self.fps_num)
    }
}

/// The packets of a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePackets {
    /// Per audio track the audio packet, `None` for a size under 4 bytes.
    pub audio: Vec<Option<Vec<u8>>>,
    /// The video data (the rest of the frame).
    pub video: Vec<u8>,
}

/// Reads the frames of a Bink file in any order.
pub struct BinkReader<R: Read + Seek> {
    /// Header and frame index.
    pub header: BinkHeader,
    reader: R,
}

impl<R: Read + Seek> BinkReader<R> {
    /// Reads the header and checks that the index does not point past the end of `r`.
    pub fn open(mut r: R) -> Result<Self> {
        let header = BinkHeader::read(&mut r)?;
        let len = r.seek(SeekFrom::End(0))?;
        let end = header
            .index
            .last()
            .map_or(0, |e| e.offset + u64::from(e.size));
        if end > len {
            return Err(BinkError::Format(format!(
                "frame index reaches to byte {end}, the file has only {len}"
            )));
        }
        Ok(BinkReader { header, reader: r })
    }

    /// Reads frame `k` and splits it into audio and video packets (`read_packet`). Each audio track has a
    /// 32-bit size up front; a packet exists only for sizes of at least 4 bytes, smaller ones are skipped.
    pub fn read_frame(&mut self, k: u32) -> Result<FramePackets> {
        let entry = *self
            .header
            .index
            .get(k as usize)
            .ok_or_else(|| BinkError::Format(format!("frame {k} does not exist")))?;
        self.reader.seek(SeekFrom::Start(entry.offset))?;
        let mut buf = vec![0u8; entry.size as usize];
        read_exact(&mut self.reader, &mut buf)?;

        let mut pos = 0usize;
        let mut audio = Vec::with_capacity(self.header.audio.len());
        for track in 0..self.header.audio.len() {
            let field = buf.get(pos..pos + 4).ok_or_else(|| {
                BinkError::Format(format!("frame {k}: audio packet size missing"))
            })?;
            let size = u32::from_le_bytes([field[0], field[1], field[2], field[3]]) as usize;
            pos += 4;
            if size > buf.len() - pos {
                return Err(BinkError::Format(format!(
                    "frame {k}: audio packet {track} ({size} bytes) is larger than the rest of the frame ({})",
                    buf.len() - pos
                )));
            }
            audio.push((size >= 4).then(|| buf[pos..pos + size].to_vec()));
            pos += size;
        }
        Ok(FramePackets {
            audio,
            video: buf[pos..].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BinkError;
    use std::io::Cursor;

    /// Builds a Bink header by hand (revision `i`).
    struct Spec {
        sig: [u8; 4],
        frames: u32,
        width: u32,
        height: u32,
        fps: (u32, u32),
        video_flags: u32,
        tracks: Vec<(u16, u16, u32)>,
        index: Vec<u32>,
    }

    impl Spec {
        fn new(frames: u32, index: Vec<u32>) -> Self {
            Spec {
                sig: *b"BIKi",
                frames,
                width: 640,
                height: 356,
                fps: (25, 1),
                video_flags: 0,
                tracks: vec![(44100, 0x7000, 0)],
                index,
            }
        }

        /// Length of header and index (the index counts).
        fn header_len(&self) -> usize {
            44 + 12 * self.tracks.len() + 4 * self.index.len()
        }

        fn bytes(&self, file_size_field: u32) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&self.sig);
            for x in [
                file_size_field,
                self.frames,
                40,
                0,
                self.width,
                self.height,
                self.fps.0,
                self.fps.1,
                self.video_flags,
                self.tracks.len() as u32,
            ] {
                v.extend_from_slice(&x.to_le_bytes());
            }
            for _ in &self.tracks {
                v.extend_from_slice(&0x4000u32.to_le_bytes());
            }
            for t in &self.tracks {
                v.extend_from_slice(&t.0.to_le_bytes());
                v.extend_from_slice(&t.1.to_le_bytes());
            }
            for t in &self.tracks {
                v.extend_from_slice(&t.2.to_le_bytes());
            }
            for e in &self.index {
                v.extend_from_slice(&e.to_le_bytes());
            }
            v
        }
    }

    /// Three frames of 20, 30 and 40 bytes right behind the header, bit 0 of entry 0 not set.
    fn three_frame_spec() -> Spec {
        let mut spec = Spec::new(3, vec![]);
        let start = spec.header_len() as u32 + 4 * 4;
        spec.index = vec![start, start + 20, start + 50, start + 90];
        spec
    }

    fn read_header(spec: &Spec) -> Result<BinkHeader> {
        let bytes = spec.bytes(spec.index.last().copied().unwrap_or(8) - 8);
        BinkHeader::read(&mut Cursor::new(bytes))
    }

    #[test]
    fn header_reads_fields_and_index() {
        let spec = three_frame_spec();
        let end = *spec.index.last().unwrap();
        let h = read_header(&spec).unwrap();
        assert_eq!(h.revision, b'i');
        // `file_size` is the file size (the field in the header contains it minus 8).
        assert_eq!(h.file_size, end);
        assert_eq!((h.frames, h.largest_frame), (3, 40));
        assert_eq!((h.width, h.height), (640, 356));
        assert_eq!((h.fps_num, h.fps_den), (25, 1));
        assert_eq!(h.video_flags, 0);
        assert_eq!(h.audio.len(), 1);
        let a = &h.audio[0];
        assert_eq!((a.sample_rate, a.flags, a.id), (44100, 0x7000, 0));
        assert!(a.stereo() && a.bits16() && a.dct());
        let start = spec.index[0] as u64;
        assert_eq!(
            h.index,
            vec![
                FrameEntry {
                    offset: start,
                    size: 20,
                    keyframe: true
                },
                FrameEntry {
                    offset: start + 20,
                    size: 30,
                    keyframe: false
                },
                FrameEntry {
                    offset: start + 50,
                    size: 40,
                    keyframe: false
                },
            ]
        );
        assert_eq!(h.duration_ms(), 120);

        // Bit 0 of an entry marks a key frame, the offset is entry & !1.
        let mut spec = three_frame_spec();
        spec.index[2] |= 1;
        let h = read_header(&spec).unwrap();
        assert!(h.index[0].keyframe && !h.index[1].keyframe && h.index[2].keyframe);
        assert_eq!(h.index[2].offset, start + 50);
        assert_eq!(h.index[1].size, 30);
    }

    #[test]
    fn header_without_audio_and_with_two_tracks() {
        let mut spec = Spec::new(1, vec![]);
        spec.tracks.clear();
        let s = spec.header_len() as u32 + 8;
        spec.index = vec![s, s + 10];
        let h = read_header(&spec).unwrap();
        assert!(h.audio.is_empty());

        let mut spec = Spec::new(1, vec![]);
        spec.tracks = vec![(22050, 0x7000, 7), (48000, 0x5000, 9)];
        let s = spec.header_len() as u32 + 8;
        spec.index = vec![s, s + 10];
        let h = read_header(&spec).unwrap();
        assert_eq!(h.audio.len(), 2);
        assert_eq!((h.audio[0].sample_rate, h.audio[0].id), (22050, 7));
        assert_eq!((h.audio[1].sample_rate, h.audio[1].id), (48000, 9));
        assert!(!h.audio[1].stereo() && h.audio[1].dct());
    }

    /// A file made of header and frames (frame lengths divisible by 4 so that bit 0 of the offsets stays free).
    fn build_file(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut spec = Spec::new(frames.len() as u32, vec![]);
        let mut pos = (spec.header_len() + 4 * (frames.len() + 1)) as u32;
        let mut index = vec![pos];
        for f in frames {
            pos += f.len() as u32;
            index.push(pos);
        }
        spec.index = index;
        let mut v = spec.bytes(pos - 8);
        for f in frames {
            v.extend_from_slice(f);
        }
        v
    }

    /// A frame with one audio packet (size field, then content) and the rest as video data.
    fn frame_with_audio(audio: &[u8], video: &[u8]) -> Vec<u8> {
        let mut f = (audio.len() as u32).to_le_bytes().to_vec();
        f.extend_from_slice(audio);
        f.extend_from_slice(video);
        f
    }

    #[test]
    fn frame_packets_split_audio_and_video() {
        let audio = [9u8, 0, 0, 0, 1, 2, 3, 4];
        let frames = vec![
            frame_with_audio(&audio, &[0xAA; 4]),
            frame_with_audio(&[], &[0xBB; 8]),
            frame_with_audio(&[1, 2, 3], &[0xCC; 1]),
        ];
        let mut rd = BinkReader::open(Cursor::new(build_file(&frames))).unwrap();
        assert_eq!(rd.header.frames, 3);
        let p = rd.read_frame(0).unwrap();
        assert_eq!(p.audio, vec![Some(audio.to_vec())]);
        assert_eq!(p.video, vec![0xAA; 4]);
        // Size 0: no audio packet.
        let p = rd.read_frame(1).unwrap();
        assert_eq!(p.audio, vec![None]);
        assert_eq!(p.video, vec![0xBB; 8]);
        // Size 3 (< 4): skipped, not part of the frame.
        let p = rd.read_frame(2).unwrap();
        assert_eq!(p.audio, vec![None]);
        assert_eq!(p.video, vec![0xCC; 1]);
        // Random access, backwards.
        assert_eq!(rd.read_frame(0).unwrap().video, vec![0xAA; 4]);
        assert!(matches!(rd.read_frame(3), Err(BinkError::Format(_))));
    }

    #[test]
    fn container_rejects_broken_files() {
        let good = three_frame_spec();
        let open = |bytes: Vec<u8>| BinkReader::open(Cursor::new(bytes));
        let pad = |mut b: Vec<u8>, n: usize| {
            b.resize(b.len() + n, 0);
            b
        };
        let full = good.bytes(good.index.last().unwrap() - 8);
        let total = *good.index.last().unwrap() as usize;

        // Valid basis: file as long as the last index entry.
        assert!(open(pad(full.clone(), total - full.len())).is_ok());

        // Signature.
        let mut s = three_frame_spec();
        s.sig = *b"XIKi";
        assert!(matches!(read_header(&s), Err(BinkError::Format(_))));
        // Revision h: not supported.
        let mut s = three_frame_spec();
        s.sig = *b"BIKh";
        assert!(matches!(read_header(&s), Err(BinkError::Unsupported(_))));
        // Alpha and grayscale flag.
        for flag in [0x0010_0000, 0x0002_0000] {
            let mut s = three_frame_spec();
            s.video_flags = flag;
            assert!(matches!(read_header(&s), Err(BinkError::Unsupported(_))));
        }
        // RDFT audio track (flag 0x1000 missing).
        let mut s = three_frame_spec();
        s.tracks = vec![(44100, 0x6000, 0)];
        assert!(matches!(read_header(&s), Err(BinkError::Unsupported(_))));
        // Numbers out of range.
        let edits: [fn(&mut Spec); 8] = [
            |s| s.frames = 0,
            |s| s.frames = 1_000_001,
            |s| s.width = 0,
            |s| s.width = 4097,
            |s| s.height = 0,
            |s| s.height = 4097,
            |s| s.fps.0 = 0,
            |s| s.fps.1 = 0,
        ];
        for edit in edits {
            let mut s = three_frame_spec();
            edit(&mut s);
            assert!(matches!(read_header(&s), Err(BinkError::Format(_))));
        }
        // Truncated header and truncated index.
        for cut in [3, 20, 44, full.len() - 4, full.len() - 1] {
            let e = BinkHeader::read(&mut Cursor::new(full[..cut].to_vec()));
            assert!(matches!(e, Err(BinkError::Format(_))), "cut at {cut}");
        }
        // Index descending or equal.
        let mut s = three_frame_spec();
        s.index[2] = s.index[1];
        assert!(matches!(read_header(&s), Err(BinkError::Format(_))));
        let mut s = three_frame_spec();
        s.index.swap(1, 2);
        assert!(matches!(read_header(&s), Err(BinkError::Format(_))));
        // Index past the end of the file (file one byte too short).
        let short = pad(full.clone(), total - full.len() - 1);
        assert!(matches!(open(short), Err(BinkError::Format(_))));

        // Audio packet larger than the frame.
        let mut big = 100u32.to_le_bytes().to_vec();
        big.extend_from_slice(&[0; 8]);
        let frames = vec![frame_with_audio(&[], &[1; 4]), big];
        let mut rd = open(build_file(&frames)).unwrap();
        assert!(rd.read_frame(0).is_ok());
        assert!(matches!(rd.read_frame(1), Err(BinkError::Format(_))));
        // Frame shorter than the size field of the audio packet.
        let mut rd = open(build_file(&[vec![1, 2]])).unwrap();
        assert!(matches!(rd.read_frame(0), Err(BinkError::Format(_))));
    }
}
