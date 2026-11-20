//! List of fixed length in the savegame.
//!
//! [`FixedVec<T, N>`] always holds exactly `N` entries. In the savegame it looks like a `Vec<T>` (length prefix and
//! entries, same bytes); loading reads a `Vec<T>` and rejects every other length. The type thus replaces the
//! length checks when loading, and the accesses `v[i]` with `i < N` cannot be out of range.
//!
//! [`at_most`] reads a `Vec<T>` of bounded length (also behind `FixedVec`): a longer list is rejected while reading.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// List with exactly `N` entries; serialized like `Vec<T>`.
#[derive(Clone, PartialEq, Eq)]
pub struct FixedVec<T, const N: usize>(Vec<T>);

impl<T, const N: usize> FixedVec<T, N> {
    /// List from `f(0)`, ..., `f(N - 1)`.
    pub fn from_fn(f: impl FnMut(usize) -> T) -> Self {
        Self((0..N).map(f).collect())
    }

    /// The entries as a `Vec`.
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
}

impl<T, const N: usize> TryFrom<Vec<T>> for FixedVec<T, N> {
    type Error = String;

    fn try_from(v: Vec<T>) -> Result<Self, String> {
        if v.len() == N {
            Ok(Self(v))
        } else {
            Err(format!("length {} instead of {N}", v.len()))
        }
    }
}

impl<T: Default, const N: usize> Default for FixedVec<T, N> {
    fn default() -> Self {
        Self::from_fn(|_| T::default())
    }
}

impl<T, const N: usize> Deref for FixedVec<T, N> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T, const N: usize> DerefMut for FixedVec<T, N> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.0
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a FixedVec<T, N> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a mut FixedVec<T, N> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}

impl<T, const N: usize> IntoIterator for FixedVec<T, N> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Like `Vec<T>` (same hash value).
impl<T: Hash, const N: usize> Hash for FixedVec<T, N> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// Like `Vec<T>`.
impl<T: fmt::Debug, const N: usize> fmt::Debug for FixedVec<T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<T: Serialize, const N: usize> Serialize for FixedVec<T, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for FixedVec<T, N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = at_most::<N, D, T>(deserializer)?;
        Self::try_from(v).map_err(serde::de::Error::custom)
    }
}

/// Reads a `Vec<T>` with at most `MAX` entries, for `#[serde(deserialize_with = "at_most::<MAX, _, _>")]`: same bytes
/// as `Vec<T>`, but a longer list is rejected by its announced length or at entry `MAX + 1`, before the entries take
/// memory. For lists whose entries are large in memory and a byte or two in the savegame (`None`, for example), whose
/// length the reading of `Vec<T>` would otherwise follow up to the unpacked size.
pub fn at_most<'de, const MAX: usize, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    deserializer.deserialize_seq(AtMost::<MAX, T>(PhantomData))
}

struct AtMost<const MAX: usize, T>(PhantomData<T>);

impl<'de, const MAX: usize, T: Deserialize<'de>> Visitor<'de> for AtMost<MAX, T> {
    type Value = Vec<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a sequence of at most {MAX} entries")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
        let announced = seq.size_hint().unwrap_or(0);
        if announced > MAX {
            return Err(de::Error::invalid_length(announced, &self));
        }
        let mut v = Vec::with_capacity(announced);
        while let Some(entry) = seq.next_element()? {
            if v.len() == MAX {
                return Err(de::Error::invalid_length(MAX + 1, &self));
            }
            v.push(entry);
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_vec_serializes_like_vec() {
        let v = vec![1u16, 70_000u32 as u16, 3];
        let f = FixedVec::<u16, 3>::try_from(v.clone()).unwrap();
        assert_eq!(
            postcard::to_stdvec(&f).unwrap(),
            postcard::to_stdvec(&v).unwrap()
        );
        let short = postcard::to_stdvec(&vec![1u16, 2]).unwrap();
        assert!(postcard::from_bytes::<FixedVec<u16, 3>>(&short).is_err());
        let long = postcard::to_stdvec(&vec![1u16, 2, 3, 4]).unwrap();
        assert!(postcard::from_bytes::<FixedVec<u16, 3>>(&long).is_err());
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Bounded(#[serde(deserialize_with = "at_most::<3, _, _>")] Vec<Option<u16>>);

    /// `at_most` reads the bytes of `Vec<T>` up to `MAX` entries; a longer list is rejected, by its announced length
    /// and, if the announcement is not checked first, at the entry behind `MAX`.
    #[test]
    fn at_most_bounds_the_length() {
        for v in [vec![], vec![None, Some(7), None]] {
            let bytes = postcard::to_stdvec(&v).unwrap();
            assert_eq!(postcard::to_stdvec(&Bounded(v.clone())).unwrap(), bytes);
            assert_eq!(postcard::from_bytes::<Bounded>(&bytes).unwrap(), Bounded(v));
        }
        let long = postcard::to_stdvec(&vec![None::<u16>; 4]).unwrap();
        assert!(postcard::from_bytes::<Bounded>(&long).is_err());
        // A sequence without a length announcement (the filter hides it).
        let unannounced = de::value::SeqDeserializer::<_, de::value::Error>::new(
            std::iter::repeat_n(0u8, 4).filter(|_| true),
        );
        assert!(
            unannounced
                .deserialize_seq(AtMost::<3, u8>(PhantomData))
                .is_err()
        );
    }
}
