//! `matrix.bin`: connection costs between context ids.
//!
//! `u16 lsize, u16 rsize`, then `lsize * rsize` costs as `i16 LE`. The cost from a node with
//! right id `r` to a node with left id `l` is entry `r + lsize * l` (MeCab's
//! `matrix_[rcAttr + lsize_ * lcAttr]`). The whole matrix is a hot random-access region, so it
//! is copied to the heap at open.

use crate::error::{DictionaryError, Error, ImageKind};
use crate::image::{ByteImage, read_array, read_vec};
use alloc::vec::Vec;
use core::fmt;

/// The connection cost of an id outside the matrix: no path takes it.
const UNCONNECTED: i16 = i16::MAX;

pub(crate) struct ConnectionMatrix {
    costs: Vec<i16>,
    lsize: usize,
    rsize: usize,
}

impl fmt::Debug for ConnectionMatrix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionMatrix")
            .field("lsize", &self.lsize)
            .field("rsize", &self.rsize)
            .finish_non_exhaustive()
    }
}

impl ConnectionMatrix {
    /// Copies the matrix out of `image`.
    ///
    /// # Errors
    ///
    /// [`Error::Dictionary`] when the image's size is not the one its header implies.
    pub(crate) fn read(image: &dyn ByteImage) -> Result<Self, Error> {
        let fault = |error| Error::dictionary(ImageKind::Matrix, error);
        let len = image.len();
        if len < 4 {
            return Err(fault(DictionaryError::TooSmall { len }));
        }
        let [l0, l1, r0, r1] = read_array::<4>(image, 0)?;
        let lsize = u16::from_le_bytes([l0, l1]);
        let rsize = u16::from_le_bytes([r0, r1]);
        let cells = u64::from(lsize).saturating_mul(u64::from(rsize));
        let expected = cells.saturating_mul(2).saturating_add(4);
        if len != expected {
            return Err(fault(DictionaryError::SizeMismatch {
                expected,
                found: len,
            }));
        }
        let bytes_len = usize::try_from(cells.saturating_mul(2))
            .map_err(|_| fault(DictionaryError::Overflow))?;
        let bytes = read_vec(image, 4, bytes_len)?;
        let costs = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| i16::from_le_bytes(*pair))
            .collect();
        Ok(Self {
            costs,
            lsize: usize::from(lsize),
            rsize: usize::from(rsize),
        })
    }

    /// The cost of connecting a node with right id `right` to one with left id `left`.
    pub(crate) fn cost(&self, right: u16, left: u16) -> i16 {
        let (rc, lc) = (usize::from(right), usize::from(left));
        // `rc` ranges over the stride dimension (lsize), `lc` over rsize. The swapped bound was
        // invisible for square ipadic (1316x1316); ko-dic (3822x2693) exposed it.
        if rc >= self.lsize || lc >= self.rsize {
            return UNCONNECTED;
        }
        self.lsize
            .checked_mul(lc)
            .and_then(|row| row.checked_add(rc))
            .and_then(|index| self.costs.get(index))
            .copied()
            .unwrap_or(UNCONNECTED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::HeapImage;
    use alloc::vec;

    fn matrix(lsize: u16, rsize: u16, costs: &[i16]) -> HeapImage {
        let mut bytes = vec![];
        bytes.extend_from_slice(&lsize.to_le_bytes());
        bytes.extend_from_slice(&rsize.to_le_bytes());
        for cost in costs {
            bytes.extend_from_slice(&cost.to_le_bytes());
        }
        HeapImage::from_vec(bytes)
    }

    #[test]
    fn rectangular_matrix_is_indexed_right_plus_lsize_times_left() {
        // lsize 3, rsize 2: entry r + 3 * l.
        let m = ConnectionMatrix::read(&matrix(3, 2, &[0, 1, 2, 10, 11, 12])).unwrap();
        assert_eq!(m.cost(2, 0), 2);
        assert_eq!(m.cost(0, 1), 10);
        assert_eq!(m.cost(2, 1), 12);
        assert_eq!(m.cost(3, 0), UNCONNECTED);
        assert_eq!(m.cost(0, 2), UNCONNECTED);
    }

    #[test]
    fn short_images_are_too_small() {
        let read =
            |bytes: &[u8]| ConnectionMatrix::read(&HeapImage::from_vec(bytes.to_vec())).err();
        assert_eq!(
            read(&[0; 3]),
            Some(Error::dictionary(
                ImageKind::Matrix,
                DictionaryError::TooSmall { len: 3 }
            ))
        );
        // Four bytes are a header: a 0 × 0 matrix.
        assert_eq!(read(&[0; 4]), None);
    }

    #[test]
    fn size_must_match_header() {
        assert_eq!(
            ConnectionMatrix::read(&matrix(2, 2, &[0, 0, 0])).unwrap_err(),
            Error::dictionary(
                ImageKind::Matrix,
                DictionaryError::SizeMismatch {
                    expected: 12,
                    found: 10
                }
            )
        );
    }
}
