//! List of fixed length in the savegame.
//!
//! [`FixedVec<T, N>`] always holds exactly `N` entries. In the savegame it looks like a `Vec<T>` (length prefix and
//! entries, same bytes); loading reads a `Vec<T>` and rejects every other length. The type thus replaces the
//! length checks when loading, and the accesses `v[i]` with `i < N` cannot be out of range.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, DerefMut};

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
        let v = Vec::<T>::deserialize(deserializer)?;
        Self::try_from(v).map_err(serde::de::Error::custom)
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
}
