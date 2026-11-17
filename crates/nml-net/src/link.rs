//! Connections between two machines.
//!
//! A connection carries `NetMessage`s in both directions. Two implementations: [`tcp::TcpLink`] over TCP with a read
//! and a write thread, and [`mem::MemLink`] in the same process with played time (for tests, without threads and
//! without a clock).
//!
//! Common rules:
//! - `poll` returns **all** waiting events at once (the original processes one message per frame, E-271).
//! - Large content goes as a `Chunk` sequence; the connection reassembles it itself and returns `LinkEvent::Large`.
//! - `Ping` (heartbeat) is swallowed and only counts for `last_received_ms`.
//! - `Closed` arrives exactly once, after that `poll` returns nothing more. A received `Bye` is an ordinary message;
//!   the turn core evaluates the dropout.
//! - Whoever calls `close` itself (or drops the connection) gets no `Closed`; the peer does.

pub mod mem;
pub mod tcp;

use crate::msg::NetMessage;
use crate::wire::{Assembler, WireError, chunk_at, chunk_count};
use std::collections::VecDeque;
use std::net::IpAddr;

/// Reason in `Closed` when the peer has ended the connection cleanly (TCP: end of the data stream).
pub const PEER_CLOSED: &str = "The peer has ended the connection";

/// What a connection reports when polled.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkEvent {
    /// A received message (never `Ping`, never `Chunk`).
    Message(NetMessage),
    /// A reassembled large content.
    Large { id: u32, bytes: Vec<u8> },
    /// The connection is closed (reason as text); arrives exactly once.
    Closed(String),
}

/// A connection to a peer.
pub trait Link: Send {
    /// Queues a message for sending; does not block. Has no effect after the connection has ended.
    fn send(&mut self, msg: NetMessage);
    /// Queues a large content; it goes out piece by piece, small messages take precedence between the pieces.
    fn send_large(&mut self, id: u32, bytes: Vec<u8>);
    /// Collects all waiting events; does not block.
    fn poll(&mut self) -> Vec<LinkEvent>;
    /// Time of the last reception (also `Ping`) in ms since the connection was created; 0 as long as nothing arrived.
    fn last_received_ms(&self) -> u64;
    /// Age of the connection in ms on the same clock as `last_received_ms` (TCP: real time, `MemLink`: played time).
    /// Silence since the last reception = `age_ms() - last_received_ms()`.
    fn age_ms(&self) -> u64;
    /// Closes the connection (what is queued still goes out); the peer gets `Closed`.
    fn close(&mut self, reason: &str);
    /// IP address of the peer, if known (the host uses it to probe the client's listening port).
    fn peer_ip(&self) -> Option<IpAddr> {
        None
    }
    /// Mutes sending (`true`): nothing goes out any more, not even heartbeats (check aid `net_debug mute`); a `close`
    /// still reaches the peer. The default has no effect.
    fn mute(&mut self, _on: bool) {}
}

/// A large content in the queue, processed piece by piece.
#[derive(Debug)]
struct LargeJob {
    id: u32,
    bytes: Vec<u8>,
    next: u32,
    count: u32,
}

/// Send scheduling of a connection: a pure, deterministic queue.
///
/// Small messages go out in queueing order; large contents as `Chunk` sequences, one after the other (the peer's
/// `Assembler` knows only one sequence at a time). Before every piece all waiting small messages go out, so that a
/// resync does not hold up heartbeats and turn contributions.
#[derive(Debug, Default)]
pub struct Outbox {
    small: VecDeque<NetMessage>,
    large: VecDeque<LargeJob>,
}

impl Outbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queues a small message.
    pub fn push_small(&mut self, msg: NetMessage) {
        self.small.push_back(msg);
    }

    /// Queues a large content.
    pub fn push_large(&mut self, id: u32, bytes: Vec<u8>) {
        let count = chunk_count(bytes.len());
        self.large.push_back(LargeJob {
            id,
            bytes,
            next: 0,
            count,
        });
    }

    /// Next frame to write: first a waiting small message, otherwise the next piece.
    pub fn next_frame(&mut self) -> Option<NetMessage> {
        if let Some(msg) = self.small.pop_front() {
            return Some(msg);
        }
        let job = self.large.front_mut()?;
        let chunk = chunk_at(job.id, &job.bytes, job.next, job.count);
        job.next += 1;
        if job.next == job.count {
            self.large.pop_front();
        }
        Some(chunk)
    }

    /// Nothing more to send.
    pub fn is_empty(&self) -> bool {
        self.small.is_empty() && self.large.is_empty()
    }
}

/// Turns a received message into an event: `Ping` -> nothing, `Chunk` -> via the `Assembler`, otherwise `Message`. An
/// error in the chunking ends the connection. Without an `Assembler` (a connection accepted by the host: clients never
/// send large contents, and before the `Hello` nobody should occupy up to [`crate::wire::MAX_ASSEMBLED`] bytes of
/// memory) even the first piece is an error.
fn deliver(
    assembler: Option<&mut Assembler>,
    msg: NetMessage,
) -> Result<Option<LinkEvent>, WireError> {
    match msg {
        NetMessage::Ping => Ok(None),
        chunk @ NetMessage::Chunk { .. } => match assembler {
            Some(assembler) => Ok(assembler
                .push(&chunk)?
                .map(|(id, bytes)| LinkEvent::Large { id, bytes })),
            None => Err(WireError::Chunk(NO_LARGE_REASON.into())),
        },
        other => Ok(Some(LinkEvent::Message(other))),
    }
}

/// Reason when a connection without chunking receives a piece.
const NO_LARGE_REASON: &str = "The host does not accept large content";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::CHUNK_SIZE;

    fn small(slot: u8) -> NetMessage {
        NetMessage::Welcome { slot }
    }

    fn index_of(msg: &NetMessage) -> (u32, u32) {
        match msg {
            NetMessage::Chunk { id, index, .. } => (*id, *index),
            other => panic!("expected piece, got {other:?}"),
        }
    }

    #[test]
    fn outbox_sends_small_before_next_chunk() {
        let mut o = Outbox::new();
        assert!(o.is_empty());
        assert_eq!(o.next_frame(), None);
        o.push_large(1, vec![1; 2 * CHUNK_SIZE + 5]);
        o.push_large(2, vec![2; 10]);
        o.push_small(small(0));
        assert!(!o.is_empty());
        // The small message was already waiting: it goes before the first piece.
        assert_eq!(o.next_frame(), Some(small(0)));
        assert_eq!(index_of(&o.next_frame().unwrap()), (1, 0));
        // Queued between two pieces: both small ones before the next piece, in queueing order.
        o.push_small(small(1));
        o.push_small(small(2));
        assert_eq!(o.next_frame(), Some(small(1)));
        assert_eq!(o.next_frame(), Some(small(2)));
        assert_eq!(index_of(&o.next_frame().unwrap()), (1, 1));
        o.push_small(small(3));
        assert_eq!(o.next_frame(), Some(small(3)));
        let last = o.next_frame().unwrap();
        assert_eq!(index_of(&last), (1, 2));
        let NetMessage::Chunk { count, bytes, .. } = &last else {
            unreachable!()
        };
        assert_eq!((*count, bytes.len()), (3, 5));
        // The second sequence only after the first, whole.
        let second = o.next_frame().unwrap();
        assert_eq!(
            second,
            NetMessage::Chunk {
                id: 2,
                index: 0,
                count: 1,
                bytes: vec![2; 10]
            }
        );
        assert_eq!(o.next_frame(), None);
        assert!(o.is_empty());
    }

    #[test]
    fn outbox_chunks_equal_split() {
        let bytes: Vec<u8> = (0..(3 * CHUNK_SIZE + 17))
            .map(|i| (i % 251) as u8)
            .collect();
        let mut o = Outbox::new();
        o.push_large(9, bytes.clone());
        let mut got = Vec::new();
        while let Some(f) = o.next_frame() {
            got.push(f);
        }
        assert_eq!(got, crate::wire::split(9, &bytes));
        // Empty content: one empty piece.
        o.push_large(4, Vec::new());
        assert_eq!(o.next_frame(), Some(crate::wire::split(4, &[])[0].clone()));
        assert_eq!(o.next_frame(), None);
    }

    #[test]
    fn deliver_swallows_pings_and_assembles_chunks() {
        let mut a = Assembler::new();
        assert_eq!(deliver(Some(&mut a), NetMessage::Ping).unwrap(), None);
        assert_eq!(
            deliver(Some(&mut a), NetMessage::Resume).unwrap(),
            Some(LinkEvent::Message(NetMessage::Resume))
        );
        let bye = NetMessage::Bye {
            reason: "bye".into(),
        };
        assert_eq!(
            deliver(Some(&mut a), bye.clone()).unwrap(),
            Some(LinkEvent::Message(bye))
        );
        let bytes = vec![5u8; CHUNK_SIZE + 1];
        let chunks = crate::wire::split(3, &bytes);
        assert_eq!(deliver(Some(&mut a), chunks[0].clone()).unwrap(), None);
        assert_eq!(
            deliver(Some(&mut a), chunks[1].clone()).unwrap(),
            Some(LinkEvent::Large { id: 3, bytes })
        );
        // Piece out of order: error, no panic.
        assert!(deliver(Some(&mut a), chunks[1].clone()).is_err());
    }

    /// Without chunking (a connection accepted by the host): even the first piece is an error with a reason text, small
    /// messages pass through.
    #[test]
    fn deliver_without_assembler_rejects_chunks() {
        assert_eq!(
            deliver(None, NetMessage::Resume).unwrap(),
            Some(LinkEvent::Message(NetMessage::Resume))
        );
        let chunks = crate::wire::split(1, &[1, 2, 3]);
        let err = deliver(None, chunks[0].clone()).unwrap_err();
        assert!(err.to_string().contains("large content"), "{err}");
    }
}
