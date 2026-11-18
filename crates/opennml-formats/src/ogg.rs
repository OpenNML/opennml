//! Ogg Vorbis header data and the length that FMOD reports for a stream.
//!
//! The simulation needs the speech length of a voice file (re/scripts 8) and must pull in neither
//! the decoder nor `opennml-audio` for it: only the page headers and the identification packet are read.

use crate::FormatError;

/// Header data of an Ogg Vorbis stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OggInfo {
    pub channels: u8,
    /// Sample rate of the file in Hz.
    pub rate: u32,
    /// Total number of PCM frames: granule of the last page with granule != -1 (start granule 0).
    pub pcm_total: i64,
}

/// Header of an Ogg page: length of the header including the segment table, length of the payload, granule,
/// serial number.
struct PageHead {
    header_len: usize,
    body_len: usize,
    granule: i64,
    serial: u32,
}

fn page_head(bytes: &[u8], at: usize) -> Result<PageHead, FormatError> {
    let rest = &bytes[at..];
    if rest.len() < 27 || &rest[..4] != b"OggS" {
        return Err(FormatError::Invalid(format!(
            "Ogg page at offset {at:#x} missing or corrupted"
        )));
    }
    let segments = usize::from(rest[26]);
    if rest.len() < 27 + segments {
        return Err(FormatError::Truncated {
            offset: at,
            what: "Ogg segment table".to_string(),
        });
    }
    let body_len = rest[27..27 + segments]
        .iter()
        .map(|&s| usize::from(s))
        .sum();
    Ok(PageHead {
        header_len: 27 + segments,
        body_len,
        granule: i64::from_le_bytes(rest[6..14].try_into().unwrap()),
        serial: u32::from_le_bytes(rest[14..18].try_into().unwrap()),
    })
}

/// Reads channels, rate and PCM length of an Ogg Vorbis file. The pages are traversed in order
/// (the checksum stays unchecked); exactly one logical stream, otherwise an error. A truncated last
/// page does not count.
pub fn ogg_info(bytes: &[u8]) -> Result<OggInfo, FormatError> {
    let first = page_head(bytes, 0)?;
    let first_end = first.header_len + first.body_len;
    if bytes.len() < first_end {
        return Err(FormatError::Truncated {
            offset: 0,
            what: "first Ogg page".to_string(),
        });
    }
    let body = &bytes[first.header_len..first_end];
    if body.len() < 16 || body[0] != 1 || &body[1..7] != b"vorbis" {
        return Err(FormatError::Invalid(
            "Ogg file without Vorbis identification packet".to_string(),
        ));
    }
    let channels = body[11];
    let rate = u32::from_le_bytes(body[12..16].try_into().unwrap());

    let mut pcm_total = 0i64;
    let mut at = 0usize;
    while at < bytes.len() {
        let Ok(head) = page_head(bytes, at) else {
            break;
        };
        let end = at + head.header_len + head.body_len;
        if end > bytes.len() {
            break;
        }
        if head.serial != first.serial {
            return Err(FormatError::Invalid(
                "Ogg file with multiple logical streams".to_string(),
            ));
        }
        if head.granule != -1 {
            pcm_total = head.granule;
        }
        at = end;
    }
    Ok(OggInfo {
        channels,
        rate,
        pcm_total,
    })
}

/// Length in ms that FMOD reports for the stream (`FUN_1001D86B`, `ov_time_total`): in single
/// precision `(pcm_total / rate) * 1000`, truncated. More than two channels or `pcm_total <= 0` (FMOD
/// does not open the stream or reports no length) yield 0.
pub fn fmod_stream_length_ms(info: &OggInfo) -> u32 {
    if info.channels > 2 || info.pcm_total <= 0 || info.rate == 0 {
        return 0;
    }
    let seconds: f32 = info.pcm_total as f32 / info.rate as f32;
    (seconds * 1000.0f32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An Ogg page with one segment per 255 bytes of payload (segment table from the length).
    fn page(serial: u32, granule: i64, flags: u8, body: &[u8]) -> Vec<u8> {
        let mut segments = vec![255u8; body.len() / 255];
        segments.push((body.len() % 255) as u8);
        let mut out = b"OggS".to_vec();
        out.push(0);
        out.push(flags);
        out.extend(granule.to_le_bytes());
        out.extend(serial.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.extend(0u32.to_le_bytes());
        out.push(segments.len() as u8);
        out.extend(segments);
        out.extend(body);
        out
    }

    fn ident(channels: u8, rate: u32) -> Vec<u8> {
        let mut body = vec![1];
        body.extend(b"vorbis");
        body.extend(0u32.to_le_bytes());
        body.push(channels);
        body.extend(rate.to_le_bytes());
        body.extend([0u8; 12]);
        body
    }

    #[test]
    fn reads_rate_channels_and_last_granule() {
        let mut file = page(7, 0, 2, &ident(2, 22_050));
        file.extend(page(7, 1000, 0, &[1, 2, 3]));
        file.extend(page(7, 354_123, 4, &[4; 300]));
        assert_eq!(
            ogg_info(&file).unwrap(),
            OggInfo {
                channels: 2,
                rate: 22_050,
                pcm_total: 354_123
            }
        );
    }

    #[test]
    fn pages_with_granule_minus_one_are_skipped() {
        let mut file = page(7, 0, 2, &ident(1, 44_100));
        file.extend(page(7, 5000, 0, &[1]));
        file.extend(page(7, -1, 0, &[2]));
        assert_eq!(ogg_info(&file).unwrap().pcm_total, 5000);
    }

    #[test]
    fn two_serials_are_rejected() {
        let mut file = page(7, 0, 2, &ident(1, 44_100));
        file.extend(page(8, 100, 0, &[1]));
        let err = ogg_info(&file).unwrap_err();
        assert!(
            err.to_string().contains("multiple logical streams"),
            "{err}"
        );
    }

    #[test]
    fn broken_input_is_an_error() {
        assert!(ogg_info(b"").is_err());
        assert!(ogg_info(b"RIFF").is_err());
        assert!(ogg_info(&page(7, 0, 2, b"\x03vorbis")).is_err());
    }

    // QUIRK E-171: kept
    #[test]
    fn fmod_length_vectors() {
        let info = |channels, rate, pcm_total| OggInfo {
            channels,
            rate,
            pcm_total,
        };
        assert_eq!(fmod_stream_length_ms(&info(1, 22_050, 354_123)), 16_059);
        assert_eq!(fmod_stream_length_ms(&info(1, 22_050, 355_887)), 16_139);
        assert_eq!(fmod_stream_length_ms(&info(1, 22_050, 441)), 20);
        assert_eq!(fmod_stream_length_ms(&info(2, 44_100, 44_100)), 1_000);
        assert_eq!(fmod_stream_length_ms(&info(3, 22_050, 354_123)), 0);
        assert_eq!(fmod_stream_length_ms(&info(1, 22_050, 0)), 0);
    }
}
