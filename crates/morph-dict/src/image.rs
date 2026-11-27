//! The byte-image seam: the one place dictionary bytes are read.
//!
//! Accessors never assume a contiguous slice: every read is `read_exact_at(offset, buf)`, so an
//! image may live in paged storage. An image that *is* one heap slice says so through
//! [`ByteImage::as_contiguous`], and the hot accessors then read it in place instead of copying.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

/// A read-only, byte-addressed dictionary image.
pub trait ByteImage: Send + Sync {
    /// Total size in bytes.
    fn len(&self) -> u64;

    /// Whether the image holds no bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copies `buf.len()` bytes starting at `offset` into `buf`.
    ///
    /// # Errors
    ///
    /// [`OutOfBounds`] when the range runs past [`Self::len`]; `buf` is then unspecified.
    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), OutOfBounds>;

    /// The whole image as one slice, when it is one (a heap copy). Paged images return `None`
    /// and keep paying [`Self::read_exact_at`].
    fn as_contiguous(&self) -> Option<&[u8]> {
        None
    }
}

/// A read of `len` bytes at `offset` that runs past an image of `image_len` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfBounds {
    offset: u64,
    len: usize,
    image_len: u64,
}

impl OutOfBounds {
    /// The failed read: `len` bytes at `offset` in an image of `image_len` bytes.
    #[must_use]
    pub const fn new(offset: u64, len: usize, image_len: u64) -> Self {
        Self {
            offset,
            len,
            image_len,
        }
    }

    /// Where the read started.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// How many bytes it asked for.
    #[must_use]
    pub const fn read_len(&self) -> usize {
        self.len
    }

    /// The image's length.
    #[must_use]
    pub const fn image_len(&self) -> u64 {
        self.image_len
    }
}

impl fmt::Display for OutOfBounds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "read of {} bytes at {} runs past an image of {} bytes",
            self.len, self.offset, self.image_len
        )
    }
}

impl core::error::Error for OutOfBounds {}

/// The in-bounds range `[offset, offset + len)` of an image of `image_len` bytes, as `usize`
/// indices.
///
/// # Errors
///
/// [`OutOfBounds`] when the range runs past `image_len` or past the address space.
pub(crate) fn checked_range(
    offset: u64,
    len: usize,
    image_len: u64,
) -> Result<Range<usize>, OutOfBounds> {
    let fault = OutOfBounds::new(offset, len, image_len);
    let end = u64::try_from(len)
        .ok()
        .and_then(|len| offset.checked_add(len))
        .filter(|&end| end <= image_len)
        .ok_or(fault)?;
    let start = usize::try_from(offset).map_err(|_| fault)?;
    let end = usize::try_from(end).map_err(|_| fault)?;
    Ok(start..end)
}

/// Reads `N` bytes at `offset`, through the contiguous view when there is one.
pub(crate) fn read_array<const N: usize>(
    image: &dyn ByteImage,
    offset: u64,
) -> Result<[u8; N], OutOfBounds> {
    let mut out = [0u8; N];
    match image.as_contiguous() {
        Some(slice) => {
            let range = checked_range(offset, N, image.len())?;
            let bytes = slice
                .get(range)
                .ok_or_else(|| OutOfBounds::new(offset, N, image.len()))?;
            out.copy_from_slice(bytes);
        }
        None => image.read_exact_at(offset, &mut out)?,
    }
    Ok(out)
}

/// Reads a little-endian `u32` at `offset`.
pub(crate) fn read_u32(image: &dyn ByteImage, offset: u64) -> Result<u32, OutOfBounds> {
    read_array(image, offset).map(u32::from_le_bytes)
}

/// Copies `len` bytes at `offset` into a new vector.
pub(crate) fn read_vec(
    image: &dyn ByteImage,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, OutOfBounds> {
    let mut out = alloc::vec![0u8; len];
    image.read_exact_at(offset, &mut out)?;
    Ok(out)
}

/// One heap copy, read in place.
pub struct HeapImage(Box<[u8]>);

impl HeapImage {
    /// Takes ownership of `bytes`.
    #[must_use]
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        Self(bytes.into_boxed_slice())
    }

    /// The image's bytes.
    #[must_use]
    pub const fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for HeapImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeapImage")
            .field("len", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl ByteImage for HeapImage {
    fn len(&self) -> u64 {
        u64::try_from(self.0.len()).unwrap_or(u64::MAX)
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), OutOfBounds> {
        let fault = OutOfBounds::new(offset, buf.len(), self.len());
        let range = checked_range(offset, buf.len(), self.len())?;
        buf.copy_from_slice(self.0.get(range).ok_or(fault)?);
        Ok(())
    }

    fn as_contiguous(&self) -> Option<&[u8]> {
        Some(&self.0)
    }
}

/// A window `[base, base + len)` of another image, addressed from zero: one container entry as
/// an image of its own, without a copy.
pub(crate) struct OffsetImage {
    inner: Arc<dyn ByteImage>,
    base: u64,
    len: u64,
}

impl OffsetImage {
    /// The window `[base, base + len)` of `inner`.
    ///
    /// # Errors
    ///
    /// [`OutOfBounds`] when the window runs past `inner`.
    pub(crate) fn new(inner: Arc<dyn ByteImage>, base: u64, len: u64) -> Result<Self, OutOfBounds> {
        let fault = OutOfBounds::new(
            base,
            usize::try_from(len).unwrap_or(usize::MAX),
            inner.len(),
        );
        match base.checked_add(len) {
            Some(end) if end <= inner.len() => Ok(Self { inner, base, len }),
            _ => Err(fault),
        }
    }
}

impl ByteImage for OffsetImage {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_exact_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), OutOfBounds> {
        let _in_window = checked_range(offset, buf.len(), self.len)?;
        let absolute = self
            .base
            .checked_add(offset)
            .ok_or_else(|| OutOfBounds::new(offset, buf.len(), self.len))?;
        self.inner.read_exact_at(absolute, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn out_of_bounds_reports_the_read() {
        let image = HeapImage::from_vec(vec![1, 2, 3]);
        assert!(!image.is_empty());
        assert!(HeapImage::from_vec(vec![]).is_empty());
        assert_eq!(image.as_contiguous(), Some(&[1, 2, 3][..]));
        let fault = image.read_exact_at(2, &mut [0; 4]).unwrap_err();
        assert_eq!(
            (fault.offset(), fault.read_len(), fault.image_len()),
            (2, 4, 3)
        );
        assert_eq!(
            fault.to_string(),
            "read of 4 bytes at 2 runs past an image of 3 bytes"
        );
    }

    #[test]
    fn heap_image_reads_in_bounds_and_refuses_past_the_end() {
        let image = HeapImage::from_vec(vec![1, 2, 3, 4]);
        let mut buf = [0u8; 2];
        image.read_exact_at(2, &mut buf).unwrap();
        assert_eq!(buf, [3, 4]);
        assert_eq!(
            image.read_exact_at(3, &mut buf),
            Err(OutOfBounds::new(3, 2, 4))
        );
        assert!(image.read_exact_at(u64::MAX, &mut buf).is_err());
    }

    #[test]
    fn offset_image_is_a_window() {
        let inner: Arc<dyn ByteImage> = Arc::new(HeapImage::from_vec(vec![0, 1, 2, 3, 4, 5]));
        let window = OffsetImage::new(Arc::clone(&inner), 2, 3).unwrap();
        assert_eq!(read_array::<3>(&window, 0).unwrap(), [2, 3, 4]);
        assert!(read_array::<2>(&window, 2).is_err());
        assert!(OffsetImage::new(inner, 4, 3).is_err());
    }
}

/// Proofs over every input (`cargo kani`).
#[cfg(kani)]
mod proofs {
    use super::*;

    /// `checked_range` accepts exactly the in-bounds reads, and the range it returns has the
    /// read's length.
    #[kani::proof]
    fn checked_range_is_exact() {
        let offset: u64 = kani::any();
        let len: usize = kani::any();
        let image_len: u64 = kani::any();
        let in_bounds = u64::try_from(len)
            .ok()
            .and_then(|len| offset.checked_add(len))
            .is_some_and(|end| end <= image_len);
        match checked_range(offset, len, image_len) {
            Ok(range) => {
                assert!(in_bounds);
                assert_eq!(range.end.checked_sub(range.start), Some(len));
                assert_eq!(u64::try_from(range.start).ok(), Some(offset));
            }
            Err(fault) => {
                assert!(!in_bounds || usize::try_from(image_len).is_err());
                assert_eq!(fault, OutOfBounds::new(offset, len, image_len));
            }
        }
    }

    /// A heap image never reads past its end, and reads in bounds always succeed.
    #[kani::proof]
    #[kani::unwind(9)]
    fn heap_image_reads_are_total() {
        let bytes: [u8; 8] = kani::any();
        let image = HeapImage::from_vec(bytes.to_vec());
        let offset: u64 = kani::any();
        let mut buf = [0u8; 3];
        let result = image.read_exact_at(offset, &mut buf);
        assert_eq!(result.is_ok(), offset <= 5);
    }
}
