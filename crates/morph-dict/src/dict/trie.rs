//! The Darts double-array trie inside `sys.dic` and `unk.dic`.
//!
//! Each unit is 8 bytes, `base: i32 LE` then `check: u32 LE`. From a node with base `b`, the
//! byte `c` leads to unit `b + c + 1` when that unit's `check` is `b`; the node is a key's end
//! when unit `b` has `check == b` and a negative base, whose bitwise complement is the stored
//! value.

use crate::image::{ByteImage, OutOfBounds, read_array};
use alloc::sync::Arc;
use core::fmt;

/// Bytes per unit.
const UNIT_SIZE: u64 = 8;

#[derive(Clone, Copy)]
struct Unit {
    base: i32,
    check: u32,
}

/// A trie of `units` units at `offset` inside `image`.
pub(crate) struct DoubleArrayTrie {
    image: Arc<dyn ByteImage>,
    offset: u64,
    units: u64,
}

impl fmt::Debug for DoubleArrayTrie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DoubleArrayTrie")
            .field("offset", &self.offset)
            .field("units", &self.units)
            .finish_non_exhaustive()
    }
}

impl DoubleArrayTrie {
    /// The trie in `[offset, offset + len)` of `image`.
    ///
    /// # Errors
    ///
    /// [`OutOfBounds`] when the range runs past `image`.
    pub(crate) fn new(
        image: Arc<dyn ByteImage>,
        offset: u64,
        len: u64,
    ) -> Result<Self, OutOfBounds> {
        let fault = OutOfBounds::new(
            offset,
            usize::try_from(len).unwrap_or(usize::MAX),
            image.len(),
        );
        match offset.checked_add(len) {
            Some(end) if end <= image.len() => Ok(Self {
                image,
                offset,
                units: len.checked_div(UNIT_SIZE).unwrap_or(0),
            }),
            _ => Err(fault),
        }
    }

    fn unit(&self, index: u64) -> Result<Option<Unit>, OutOfBounds> {
        if index >= self.units {
            return Ok(None);
        }
        let at = index
            .checked_mul(UNIT_SIZE)
            .and_then(|delta| self.offset.checked_add(delta))
            .ok_or_else(|| OutOfBounds::new(index, 8, self.image.len()))?;
        let [b0, b1, b2, b3, c0, c1, c2, c3] = read_array::<8>(self.image.as_ref(), at)?;
        Ok(Some(Unit {
            base: i32::from_le_bytes([b0, b1, b2, b3]),
            check: u32::from_le_bytes([c0, c1, c2, c3]),
        }))
    }

    /// The value stored at the node with base `base`, when the node ends a key.
    fn value_at(&self, base: i32) -> Result<Option<u32>, OutOfBounds> {
        let Ok(at) = u32::try_from(base) else {
            return Ok(None);
        };
        Ok(self
            .unit(u64::from(at))?
            .filter(|unit| unit.check == at)
            // A key's end stores `-(value + 1)`: only a negative base decodes to a value.
            .and_then(|unit| u32::try_from(!unit.base).ok()))
    }

    /// The base of the node `byte` leads to from the node with base `base`.
    fn child(&self, base: i32, byte: u8) -> Result<Option<i32>, OutOfBounds> {
        let Ok(at) = u32::try_from(base) else {
            return Ok(None);
        };
        let index = u64::from(at)
            .saturating_add(u64::from(byte))
            .saturating_add(1);
        Ok(self
            .unit(index)?
            .filter(|unit| unit.check == at)
            .map(|unit| unit.base))
    }

    /// Calls `sink(value, length)` with every stored key that is a prefix of `key`, shortest
    /// first: the key's value and its length in bytes.
    ///
    /// # Errors
    ///
    /// [`OutOfBounds`] when a unit read fails (never for a validated trie).
    pub(crate) fn for_each_prefix(
        &self,
        key: &[u8],
        mut sink: impl FnMut(u32, usize),
    ) -> Result<(), OutOfBounds> {
        let Some(root) = self.unit(0)? else {
            return Ok(());
        };
        let mut base = root.base;
        for (length, &byte) in key.iter().enumerate() {
            if let Some(value) = self.value_at(base)? {
                sink(value, length);
            }
            match self.child(base, byte)? {
                Some(next) => base = next,
                None => return Ok(()),
            }
        }
        if let Some(value) = self.value_at(base)? {
            sink(value, key.len());
        }
        Ok(())
    }
}
