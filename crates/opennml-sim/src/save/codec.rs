//! Encoding of the savegame bodies: MessagePack with field names (`rmp_serde::to_vec_named`), packed with zstd.
//! The field names let a later version read an older body; the checksum, the network and the recording keep
//! `postcard` on the same types (`super::checksum_of`).
//!
//! Reading limits what a damaged or forged file can cost: the unpacked size ([`MAX_UNPACKED`]) and the nesting of
//! arrays and maps ([`MAX_DEPTH`]); bytes left over behind the value are an error.

use std::io::Read;

use anyhow::{Context, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Largest unpacked body in bytes (256 MiB; a 1024 x 1024 map with all grids takes a small part of it).
pub const MAX_UNPACKED: usize = 256 * 1024 * 1024;
/// Deepest nesting of arrays and maps while reading: far above the types of the state, small enough that an
/// arbitrarily deep value (for example under an unknown field, which is skipped recursively) does not blow the
/// stack.
pub const MAX_DEPTH: usize = 512;
/// zstd level of the packing.
const LEVEL: i32 = 3;

/// MessagePack of `value` with field names, packed with zstd.
pub fn encode_named<T: Serialize>(value: &T) -> Vec<u8> {
    let plain = rmp_serde::to_vec_named(value).expect("serializable");
    zstd::bulk::compress(&plain, LEVEL).expect("packing in memory")
}

/// Reads a body of [`encode_named`]: unpacked at most [`MAX_UNPACKED`] bytes, nested at most [`MAX_DEPTH`], without
/// bytes left over.
pub fn decode_named<T: DeserializeOwned>(packed: &[u8]) -> anyhow::Result<T> {
    read_named(&unpack(packed, MAX_UNPACKED)?)
}

/// Reads a body of [`encode_named`] as an untyped value, with the limits of [`decode_named`].
pub fn decode_value(packed: &[u8]) -> anyhow::Result<rmpv::Value> {
    let plain = unpack(packed, MAX_UNPACKED)?;
    let mut rest = plain.as_slice();
    let value = rmpv::decode::read_value_with_max_depth(&mut rest, MAX_DEPTH)
        .context("MessagePack damaged")?;
    check_rest(rest)?;
    Ok(value)
}

/// Converts a value of [`decode_value`] into `T`. The value goes back to MessagePack and through the reader of
/// [`decode_named`]: `rmpv::ext::from_value` expects enums as arrays, `rmp_serde` writes them as a map with the
/// variant name or as the bare name.
pub fn from_value<T: DeserializeOwned>(value: rmpv::Value) -> anyhow::Result<T> {
    let mut plain = Vec::new();
    rmpv::encode::write_value(&mut plain, &value).expect("writing to memory");
    read_named(&plain)
}

/// `T` from unpacked MessagePack, nested at most [`MAX_DEPTH`], without bytes left over.
fn read_named<T: DeserializeOwned>(plain: &[u8]) -> anyhow::Result<T> {
    let mut rest = plain;
    let value = {
        let mut de = rmp_serde::Deserializer::new(&mut rest);
        de.set_max_depth(MAX_DEPTH);
        T::deserialize(&mut de).context("MessagePack does not match")?
    };
    check_rest(rest)?;
    Ok(value)
}

/// Unpacks at most `limit` bytes. A frame that announces more is rejected before reading; one without or with a
/// wrong announcement stops at the limit (the bulk API of zstd would reserve the whole limit up front).
fn unpack(packed: &[u8], limit: usize) -> anyhow::Result<Vec<u8>> {
    let announced = zstd::zstd_safe::get_frame_content_size(packed)
        .ok()
        .flatten();
    if announced.is_some_and(|n| n > limit as u64) {
        bail!("body unpacks to more than {limit} bytes");
    }
    let mut plain = Vec::new();
    zstd::stream::read::Decoder::with_buffer(packed)
        .and_then(|d| d.take(limit as u64 + 1).read_to_end(&mut plain))
        .context("body not packed with zstd")?;
    if plain.len() > limit {
        bail!("body unpacks to more than {limit} bytes");
    }
    Ok(plain)
}

fn check_rest(rest: &[u8]) -> anyhow::Result<()> {
    if !rest.is_empty() {
        bail!("{} extra bytes behind the value", rest.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fog::grid::ByteGrid;
    use crate::save::SaveGame;
    use crate::save::testkit;

    /// A savegame of a running match comes back from the new encoding with the same `postcard` bytes (checksum),
    /// typed and via the untyped value.
    #[test]
    fn round_trip_keeps_postcard_bytes() {
        let (rules, mut state) = super::super::tests::scene();
        for _ in 0..11 {
            crate::tick::step(&mut state, &rules, &[]);
        }
        let orig = SaveGame {
            start: crate::setup::start_package(&rules, &testkit::setup()),
            state,
            commands: Vec::new(),
        };
        let packed = encode_named(&orig);
        let back: SaveGame = decode_named(&packed).unwrap();
        let bytes = postcard::to_stdvec(&orig).unwrap();
        assert_eq!(postcard::to_stdvec(&back).unwrap(), bytes);
        let value: SaveGame = from_value(decode_value(&packed).unwrap()).unwrap();
        assert_eq!(postcard::to_stdvec(&value).unwrap(), bytes);
    }

    #[test]
    fn rejects_trailing_bytes() {
        let grid = ByteGrid::new(4, 4);
        let mut plain = rmp_serde::to_vec_named(&grid).unwrap();
        let pack = |plain: &[u8]| zstd::bulk::compress(plain, LEVEL).unwrap();
        assert_eq!(decode_named::<ByteGrid>(&pack(&plain)).unwrap(), grid);
        plain.push(0);
        assert!(decode_named::<ByteGrid>(&pack(&plain)).is_err());
        assert!(decode_value(&pack(&plain)).is_err());
    }

    /// Byte grids are one MessagePack `bin`, not a sequence of numbers (cells from 128 on would take two bytes each).
    #[test]
    fn grids_are_bytes() {
        let mut grid = ByteGrid::new(4, 4);
        grid.fill(0xC8);
        let value = decode_value(&encode_named(&grid)).unwrap();
        let cells = value
            .as_map()
            .unwrap()
            .iter()
            .find(|(k, _)| k.as_str() == Some("cells"))
            .map(|(_, v)| v.clone());
        assert_eq!(cells, Some(rmpv::Value::Binary(vec![0xC8; 16])));
    }

    /// A frame without an announced size stops at the limit.
    #[test]
    fn unpacking_stops_at_the_limit() {
        let mut packed = Vec::new();
        zstd::stream::copy_encode(&[7u8; 1000][..], &mut packed, LEVEL).unwrap();
        assert_eq!(
            zstd::zstd_safe::get_frame_content_size(&packed).unwrap(),
            None
        );
        assert_eq!(unpack(&packed, 1000).unwrap(), vec![7; 1000]);
        assert!(unpack(&packed, 999).is_err());
    }
}
