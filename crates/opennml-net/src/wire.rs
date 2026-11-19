//! Wire: frames, encoding and chunking.
//!
//! Frame: `MAGIC` (4 bytes), network version (`u16` little endian), length of the body (`u32` little endian), body
//! (postcard). Large payloads (resync) go as a `Chunk` sequence, so that small messages fit in between.
//! The original uses DirectPlay (E-254); the reimplementation uses TCP with its own frame.

use crate::msg::NetMessage;
use std::fmt;
use std::io::Read;

/// Magic at the start of every frame.
pub const MAGIC: [u8; 4] = *b"NMLN";
/// Version of the wire; any other value is rejected. History: 1 first version; 2 lobby, discovery and latency
/// messages, `Hello.build`, `StartInfo` with `spacing`/`lead`; 3 `Hello::seat`, `StartInfo::saved`,
/// `LobbyState::seats`, `MapId::Saved`; 4 `Hello::listen_port`, `Go::successors`, `Rejoin`, `Migrated`,
/// `NetControl::Handover`; 5 game data check: `LobbyState::session_revision`, `LobbyState::rules_checksum`,
/// `LobbyRow::data`, `LobbyWish::Data`, `NetMessage::LoadFailed`.
pub const NET_VERSION: u16 = 5;
/// Largest allowed body in bytes.
pub const MAX_FRAME: usize = 1 << 20;
/// Payload per piece.
pub const CHUNK_SIZE: usize = 256 * 1024;
/// Largest total size of a reassembled payload.
pub const MAX_ASSEMBLED: usize = 64 << 20;

/// Error of the wire.
#[derive(Debug)]
pub enum WireError {
    Io(std::io::Error),
    BadMagic,
    Version { got: u16 },
    TooLong(usize),
    Decode(String),
    Chunk(String),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "connection error: {e}"),
            Self::BadMagic => write!(f, "the peer does not speak the protocol of No Man's Land"),
            Self::Version { got } => write!(
                f,
                "wrong network version {got} (expected {NET_VERSION}); both machines need the same program version"
            ),
            Self::TooLong(n) => write!(f, "message too long ({n} bytes, at most {MAX_FRAME})"),
            Self::Decode(m) => write!(f, "message not readable: {m}"),
            Self::Chunk(m) => write!(f, "chunking error: {m}"),
        }
    }
}

impl std::error::Error for WireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WireError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Encodes a message as a whole frame.
pub fn encode(msg: &NetMessage) -> Result<Vec<u8>, WireError> {
    let body = postcard::to_allocvec(msg).map_err(|e| WireError::Decode(e.to_string()))?;
    if body.len() > MAX_FRAME {
        return Err(WireError::TooLong(body.len()));
    }
    let mut out = Vec::with_capacity(10 + body.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&NET_VERSION.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Reads exactly one frame. The length field is checked before the body.
pub fn read_frame(r: &mut impl Read) -> Result<NetMessage, WireError> {
    let mut head = [0u8; 10];
    r.read_exact(&mut head)?;
    if head[..4] != MAGIC {
        return Err(WireError::BadMagic);
    }
    let got = u16::from_le_bytes([head[4], head[5]]);
    if got != NET_VERSION {
        return Err(WireError::Version { got });
    }
    let len = u32::from_le_bytes([head[6], head[7], head[8], head[9]]) as usize;
    if len > MAX_FRAME {
        return Err(WireError::TooLong(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    postcard::from_bytes(&body).map_err(|e| WireError::Decode(e.to_string()))
}

/// Splits `bytes` into `Chunk` messages of at most `CHUNK_SIZE` bytes each (empty payload: one empty piece).
///
/// Contract: the pieces of a sequence go without gaps and in the order `0 … count-1` over the same connection; per
/// connection at most one large sequence runs at a time (small messages may go in between).
pub fn split(id: u32, bytes: &[u8]) -> Vec<NetMessage> {
    let count = chunk_count(bytes.len());
    (0..count)
        .map(|index| chunk_at(id, bytes, index, count))
        .collect()
}

/// Number of pieces for `len` bytes (at least 1).
pub(crate) fn chunk_count(len: usize) -> u32 {
    len.div_ceil(CHUNK_SIZE).max(1) as u32
}

/// Piece `index` of `count` from `bytes` (as in [`split`]); the send scheduling (`link::Outbox`) produces one piece
/// after the other like this, without copying the whole sequence beforehand.
pub(crate) fn chunk_at(id: u32, bytes: &[u8], index: u32, count: u32) -> NetMessage {
    let from = (index as usize * CHUNK_SIZE).min(bytes.len());
    let to = (from + CHUNK_SIZE).min(bytes.len());
    NetMessage::Chunk {
        id,
        index,
        count,
        bytes: bytes[from..to].to_vec(),
    }
}

/// Reassembles `Chunk` sequences. Pieces must come in the order `0, 1, …`.
///
/// Contract: one `Assembler` per connection, one large sequence at a time (no map by `id`). A piece with a foreign
/// `id`, a wrong count or a wrong index in the middle of a sequence is a `WireError::Chunk` error; the running
/// sequence is lost in that case, and the assembler is ready again afterwards for a new sequence from index 0.
#[derive(Debug, Default)]
pub struct Assembler {
    id: u32,
    count: u32,
    next: u32,
    buf: Vec<u8>,
}

impl Assembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts a piece; returns `(id, bytes)` as soon as the last one is there. On an error the state is discarded.
    pub fn push(&mut self, chunk: &NetMessage) -> Result<Option<(u32, Vec<u8>)>, WireError> {
        let result = self.push_inner(chunk);
        if result.is_err() {
            *self = Self::default();
        }
        result
    }

    fn push_inner(&mut self, chunk: &NetMessage) -> Result<Option<(u32, Vec<u8>)>, WireError> {
        let NetMessage::Chunk {
            id,
            index,
            count,
            bytes,
        } = chunk
        else {
            return Err(WireError::Chunk("not a chunk message".into()));
        };
        if *count == 0 {
            return Err(WireError::Chunk("piece count 0".into()));
        }
        if bytes.len() > CHUNK_SIZE {
            return Err(WireError::Chunk(format!(
                "piece of {} bytes larger than {CHUNK_SIZE}",
                bytes.len()
            )));
        }
        if self.next == 0 {
            self.id = *id;
            self.count = *count;
            self.buf.clear();
        } else if *id != self.id || *count != self.count {
            return Err(WireError::Chunk(format!(
                "piece does not belong to the running sequence (ID {id}, count {count}; expected {}, {})",
                self.id, self.count
            )));
        }
        if *index != self.next {
            return Err(WireError::Chunk(format!(
                "piece {index} out of order (expected {})",
                self.next
            )));
        }
        if self.buf.len() + bytes.len() > MAX_ASSEMBLED {
            return Err(WireError::Chunk(format!(
                "payload larger than {MAX_ASSEMBLED} bytes"
            )));
        }
        self.buf.extend_from_slice(bytes);
        self.next += 1;
        if self.next == self.count {
            let out = std::mem::take(&mut self.buf);
            let id = self.id;
            *self = Self::default();
            Ok(Some((id, out)))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::msg::*;
    use opennml_sim::command::{Command, CommandKind};
    use opennml_sim::setup::{MatchSetup, StartPackage};

    fn package() -> StartPackage {
        StartPackage {
            format: 1,
            level: "multiplayer/test.lvl".into(),
            rules_checksum: 0x1234_5678_9abc_def0,
            setup: MatchSetup::default(),
            random_map: None,
        }
    }

    /// Lobby state of a loaded match: map `Saved`, slots 2 and 5, the host on 5.
    fn loaded_lobby() -> crate::lobby::LobbyState {
        use crate::lobby::{LobbyBook, LobbyRow, MapId, SessionInfo};
        let mut session = SessionInfo::plain("Session", "", 2, crate::turn::TurnConfig::default());
        session.map = MapId::Saved {
            name: "Netgame".into(),
            checksum: 0xfeed_beef,
        };
        let row = |team: i32| LobbyRow {
            kind: 0,
            name: String::new(),
            nation: 1,
            team,
            handicap: 0,
            color: team,
            ready: false,
            peer: None,
            data: crate::lobby::DataState::Equal,
        };
        LobbyBook::loaded(session, vec![row(0), row(1)], vec![2, 5], 5, "Host")
            .unwrap()
            .state()
            .clone()
    }

    fn cmd(player: u8, slot: u8) -> Command {
        Command {
            player,
            kind: CommandKind::SetPlayerResult { slot, result: 1 },
        }
    }

    fn seats() -> [Seat; 8] {
        std::array::from_fn(|i| Seat {
            peer: (i < 3).then_some(i as u8),
            name: format!("Player {i}"),
        })
    }

    #[test]
    fn every_message_round_trips() {
        let msgs = vec![
            NetMessage::Hello(Hello {
                listen_port: Some(40_001),
                net_version: NET_VERSION,
                protocol: 42,
                format: 29,
                edition: "de".into(),
                name: "Alice".into(),
                build: "test".into(),
                seat: Some(SeatClaim {
                    slot: 3,
                    checksum: 0x0123_4567_89ab_cdef,
                }),
            }),
            NetMessage::Welcome { slot: 3 },
            NetMessage::Reject {
                reason: "Wrong version".into(),
            },
            NetMessage::Start(Box::new(StartInfo {
                package: package(),
                seats: seats(),
                your_slot: 1,
                spacing: 8,
                lead: 4,
                game_speed: 5,
                session: "session".into(),
                saved: Some(SavedStart {
                    name: "Netgame".into(),
                    checksum: 0xfeed_beef,
                    steps: 4_321,
                }),
            })),
            NetMessage::Loaded { error: None },
            NetMessage::Loaded {
                error: Some("broken".into()),
            },
            NetMessage::Go { successors: vec![] },
            NetMessage::Turn(TurnContribution {
                turn: 7,
                hash: u64::MAX,
                commands: vec![cmd(1, 1)],
                wishes: vec![Wish::Pause, Wish::Speed(3), Wish::Save("save".into())],
            }),
            NetMessage::Bundle(TurnBundle {
                turn: 7,
                commands: vec![cmd(0, 0), cmd(2, 2)],
                control: vec![
                    NetControl::Halt { by: 1 },
                    NetControl::Speed(4),
                    NetControl::Save { name: "a".into() },
                    NetControl::Resync { slots: vec![1, 2] },
                    NetControl::Stop { name: "b".into() },
                    NetControl::End { name: "c".into() },
                    NetControl::Handover { peers: vec![1, 3] },
                ],
            }),
            NetMessage::Resume,
            NetMessage::Chunk {
                id: 9,
                index: 1,
                count: 2,
                bytes: vec![1, 2, 3],
            },
            NetMessage::Synced,
            NetMessage::Bye {
                reason: "bye".into(),
            },
            NetMessage::Ping,
            NetMessage::Lobby(Box::new(
                crate::lobby::LobbyBook::open(
                    crate::lobby::SessionInfo::plain(
                        "Session",
                        "multiplayer/a.lvl",
                        3,
                        crate::turn::TurnConfig::default(),
                    ),
                    "Host",
                )
                .unwrap()
                .state()
                .clone(),
            )),
            NetMessage::Lobby(Box::new(loaded_lobby())),
            NetMessage::LobbyWish(crate::lobby::LobbyWish::Row {
                nation: 2,
                team: 1,
                handicap: 0,
                color: 3,
            }),
            NetMessage::LobbyWish(crate::lobby::LobbyWish::Ready(true)),
            NetMessage::LobbyWish(crate::lobby::LobbyWish::Chat("hello".into())),
            NetMessage::LobbyWish(crate::lobby::LobbyWish::Data {
                session_revision: 4,
                report: crate::lobby::DataReport::Rules {
                    checksum: None,
                    inputs: vec![crate::lobby::DataInput {
                        path: "units/a.ini".into(),
                        hash: Some(9),
                    }],
                },
            }),
            NetMessage::LobbyWish(crate::lobby::LobbyWish::Data {
                session_revision: 5,
                report: crate::lobby::DataReport::Missing,
            }),
            NetMessage::LoadFailed {
                name: "Bert".into(),
            },
            NetMessage::Chat {
                row: 2,
                text: "hello".into(),
            },
            NetMessage::Probe { id: 7 },
            NetMessage::ProbeAck { id: 7 },
        ];
        let mut all = Vec::new();
        for m in &msgs {
            all.extend(encode(m).unwrap());
        }
        let mut r = std::io::Cursor::new(all);
        for m in &msgs {
            assert_eq!(&read_frame(&mut r).unwrap(), m);
        }
        assert!(matches!(read_frame(&mut r), Err(WireError::Io(_))));
    }

    fn header(version: u16, len: u32) -> Vec<u8> {
        let mut h = MAGIC.to_vec();
        h.extend_from_slice(&version.to_le_bytes());
        h.extend_from_slice(&len.to_le_bytes());
        h
    }

    // QUIRK E-270: deviation
    #[test]
    fn oversized_frame_is_rejected() {
        // No bytes follow the header: an attempt to read the body would be an Io error.
        let mut r = std::io::Cursor::new(header(NET_VERSION, MAX_FRAME as u32 + 1));
        match read_frame(&mut r) {
            Err(WireError::TooLong(n)) => assert_eq!(n, MAX_FRAME + 1),
            other => panic!("expected TooLong, got {other:?}"),
        }
    }

    #[test]
    fn foreign_version_is_rejected() {
        let mut r = std::io::Cursor::new(header(NET_VERSION + 1, 0));
        match read_frame(&mut r) {
            Err(WireError::Version { got }) => assert_eq!(got, NET_VERSION + 1),
            other => panic!("expected Version, got {other:?}"),
        }
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut h = header(NET_VERSION, 0);
        h[0] = b'X';
        assert!(matches!(
            read_frame(&mut std::io::Cursor::new(h)),
            Err(WireError::BadMagic)
        ));
    }

    #[test]
    fn truncated_frame_is_an_io_error() {
        let full = encode(&NetMessage::Bye {
            reason: "truncated".into(),
        })
        .unwrap();
        for cut in [0, 5, full.len() - 1] {
            let mut r = std::io::Cursor::new(full[..cut].to_vec());
            assert!(
                matches!(read_frame(&mut r), Err(WireError::Io(_))),
                "cut {cut}"
            );
        }
    }

    fn pseudo_random(n: usize) -> Vec<u8> {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    #[test]
    fn split_and_assemble_restore_the_bytes() {
        let bytes = pseudo_random(3 << 20);
        let chunks = split(5, &bytes);
        assert_eq!(chunks.len(), 12);
        for c in &chunks {
            assert!(encode(c).unwrap().len() <= MAX_FRAME + 10);
        }
        let mut a = Assembler::new();
        let mut done = None;
        for (i, c) in chunks.iter().enumerate() {
            let r = a.push(c).unwrap();
            if i + 1 < chunks.len() {
                assert!(r.is_none());
            } else {
                done = r;
            }
        }
        let (id, out) = done.unwrap();
        assert_eq!(id, 5);
        assert_eq!(out, bytes);

        // Empty payload: one empty piece.
        let empty = split(1, &[]);
        assert_eq!(empty.len(), 1);
        assert_eq!(Assembler::new().push(&empty[0]).unwrap(), Some((1, vec![])));

        // Out of order.
        let mut a = Assembler::new();
        assert!(matches!(a.push(&chunks[1]), Err(WireError::Chunk(_))));
        // Duplicate.
        let mut a = Assembler::new();
        a.push(&chunks[0]).unwrap();
        assert!(matches!(a.push(&chunks[0]), Err(WireError::Chunk(_))));
        // Wrong count.
        let mut a = Assembler::new();
        a.push(&chunks[0]).unwrap();
        let NetMessage::Chunk { id, bytes, .. } = chunks[1].clone() else {
            unreachable!()
        };
        let bad = NetMessage::Chunk {
            id,
            index: 1,
            count: 13,
            bytes,
        };
        assert!(matches!(a.push(&bad), Err(WireError::Chunk(_))));
        // Not a chunk message.
        assert!(matches!(
            Assembler::new().push(&NetMessage::Ping),
            Err(WireError::Chunk(_))
        ));
        // Sum above MAX_ASSEMBLED.
        let count = (MAX_ASSEMBLED / CHUNK_SIZE + 2) as u32;
        let mut a = Assembler::new();
        let mut failed = false;
        for index in 0..count {
            let c = NetMessage::Chunk {
                id: 2,
                index,
                count,
                bytes: vec![0; CHUNK_SIZE],
            };
            if let Err(e) = a.push(&c) {
                assert!(matches!(e, WireError::Chunk(_)));
                failed = true;
                break;
            }
        }
        assert!(failed);
    }

    fn chunk(id: u32, index: u32, count: u32, n: usize) -> NetMessage {
        NetMessage::Chunk {
            id,
            index,
            count,
            bytes: vec![7; n],
        }
    }

    #[test]
    fn foreign_id_mid_sequence_is_an_error_and_assembler_recovers() {
        let mut a = Assembler::new();
        assert_eq!(a.push(&chunk(1, 0, 3, 4)).unwrap(), None);
        assert!(matches!(
            a.push(&chunk(2, 1, 3, 4)),
            Err(WireError::Chunk(_))
        ));
        // Ready again: new sequence from index 0.
        assert_eq!(a.push(&chunk(3, 0, 2, 1)).unwrap(), None);
        assert_eq!(a.push(&chunk(3, 1, 2, 2)).unwrap(), Some((3, vec![7; 3])));
    }

    #[test]
    fn oversized_chunk_is_rejected() {
        let mut a = Assembler::new();
        assert!(matches!(
            a.push(&chunk(1, 0, 1, CHUNK_SIZE + 1)),
            Err(WireError::Chunk(_))
        ));
        assert!(a.push(&chunk(1, 0, 1, CHUNK_SIZE)).unwrap().is_some());
    }

    #[test]
    fn errors_display_in_german() {
        assert!(WireError::BadMagic.to_string().contains("protocol"));
        assert!(
            WireError::Version { got: 2 }
                .to_string()
                .contains("network version")
        );
        let e: &dyn std::error::Error = &WireError::TooLong(1);
        assert!(e.to_string().contains("too long"));
    }

    #[test]
    fn go_rejoin_migrated_round_trip() {
        use std::net::SocketAddr;
        let addr: SocketAddr = "10.0.0.7:40001".parse().unwrap();
        let list = vec![
            Successor {
                peer: 1,
                slot: 2,
                addr: Some(addr),
            },
            Successor {
                peer: 2,
                slot: 3,
                addr: None,
            },
            Successor {
                peer: 3,
                slot: 5,
                addr: Some("[::1]:2302".parse().unwrap()),
            },
        ];
        let bundle = |turn| TurnBundle {
            turn,
            commands: vec![cmd(0, 0)],
            control: vec![NetControl::Handover { peers: vec![1, 2] }],
        };
        let msgs = vec![
            NetMessage::Go {
                successors: list.clone(),
            },
            NetMessage::Rejoin(Box::new(Rejoin {
                peer: 2,
                slot: 3,
                next_close: 12,
                next_bundle: 11,
                bundles: vec![bundle(9), bundle(10)],
                open: vec![TurnContribution {
                    turn: 12,
                    hash: 77,
                    commands: vec![cmd(3, 3)],
                    wishes: vec![Wish::Pause],
                }],
                desyncs: 2,
            })),
            NetMessage::Migrated(Box::new(Migrated {
                host: 1,
                bundles: vec![bundle(11)],
                paused_by: Some(4),
                successors: list,
                desyncs: 3,
            })),
        ];
        for m in &msgs {
            let bytes = encode(m).unwrap();
            assert_eq!(&read_frame(&mut std::io::Cursor::new(bytes)).unwrap(), m);
        }
    }

    #[test]
    fn frame_of_version_4_is_rejected() {
        assert_eq!(NET_VERSION, 5);
        let mut r = std::io::Cursor::new(header(4, 0));
        match read_frame(&mut r) {
            Err(WireError::Version { got }) => assert_eq!(got, 4),
            other => panic!("expected Version, got {other:?}"),
        }
    }
}
