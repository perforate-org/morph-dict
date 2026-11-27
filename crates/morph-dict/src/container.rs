//! The MPD container: the four dictionary images in one addressable byte string, so one region
//! of storage backs a whole dictionary and opening it is a structural check.
//!
//! ```text
//! offset  size  field
//! 0       3     magic b"MPD"
//! 3       1     layout_version: u8 (current: 1)
//! 4       4     entry_count: u32 LE
//! 8       ...   entry table, entry_count times:
//!                 1    name_len: u8 (1..=255)
//!                 n    name: UTF-8 bytes
//!                 8    offset: u64 LE (from the container's start)
//!                 8    len: u64 LE
//!                 32   sha256 of the image
//! ...     ...   the images, concatenated
//! ```
//!
//! The magic names the format family; the version byte names the layout revision, and a reader
//! refuses any other (the fix is to rebuild the container: dictionaries are re-uploadable, so no
//! migration exists). Opening checks the magic, the table and every entry's range; it does not
//! rehash the images, which for ipadic would cost a 53 MB hash per open. The digests are
//! build-time integrity metadata that [`attest`] re-derives offline.

use crate::error::{ContainerError, Error, ImageKind};
use crate::image::{ByteImage, read_array, read_vec};
use alloc::string::String;
use alloc::vec::Vec;
use sha2::{Digest, Sha256};

/// Container magic: `MPD` (morph dictionary).
pub const MAGIC: &[u8; 3] = b"MPD";

/// The container layout revision this crate reads and writes.
pub const LAYOUT_VERSION: u8 = 1;

/// Fixed header: magic, version, entry count.
const HEADER_LEN: u64 = 8;

/// Per-entry bytes after the name: offset, length, digest.
const ENTRY_TAIL_LEN: u64 = 48;

/// A runaway-table bound far above any real entry count.
const MAX_ENTRIES: u32 = 4096;

/// One entry of a container's table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerEntry {
    name: String,
    offset: u64,
    len: u64,
    sha256: [u8; 32],
}

impl ContainerEntry {
    /// The entry's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Where the image starts, from the container's start.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// The image's length in bytes.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// Whether the image is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The image's SHA-256, recorded when the container was built.
    #[must_use]
    pub const fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// One past the image's last byte.
    const fn end(&self) -> u64 {
        self.offset.saturating_add(self.len)
    }
}

/// Parses and structurally validates a container, returning its entry table.
///
/// # Errors
///
/// [`Error::Container`] on a bad magic or version, a truncated or runaway table, an entry outside
/// the container, a duplicate name or overlapping entries.
pub fn validate(image: &dyn ByteImage) -> Result<Vec<ContainerEntry>, Error> {
    let len = image.len();
    if len < HEADER_LEN {
        return Err(ContainerError::TooSmall { len }.into());
    }
    let count = parse_header(read_array(image, 0)?)?;
    let mut pos = HEADER_LEN;
    let mut entries = Vec::new();
    for _ in 0..count {
        let (entry, next) = read_entry(image, pos)?;
        entries.push(entry);
        pos = next;
    }
    check_names_unique(&entries)?;
    check_disjoint(&entries)?;
    Ok(entries)
}

/// The entry count a header declares.
const fn parse_header(header: [u8; 8]) -> Result<u32, ContainerError> {
    let [m0, m1, m2, version, c0, c1, c2, c3] = header;
    if m0 != MAGIC[0] || m1 != MAGIC[1] || m2 != MAGIC[2] {
        return Err(ContainerError::BadMagic);
    }
    if version != LAYOUT_VERSION {
        return Err(ContainerError::UnsupportedVersion { found: version });
    }
    let count = u32::from_le_bytes([c0, c1, c2, c3]);
    if count > MAX_ENTRIES {
        return Err(ContainerError::TooManyEntries { count });
    }
    Ok(count)
}

/// An entry's offset, length and digest, from the 48 bytes after its name.
fn parse_tail(tail: [u8; 48]) -> (u64, u64, [u8; 32]) {
    let mut offset = [0u8; 8];
    let mut len = [0u8; 8];
    let mut digest = [0u8; 32];
    for (i, byte) in tail.into_iter().enumerate() {
        let slot = match i {
            0..8 => offset.get_mut(i),
            8..16 => len.get_mut(i.saturating_sub(8)),
            _ => digest.get_mut(i.saturating_sub(16)),
        };
        if let Some(slot) = slot {
            *slot = byte;
        }
    }
    (u64::from_le_bytes(offset), u64::from_le_bytes(len), digest)
}

/// Reads the table entry at `pos`, returning it and the position after it.
fn read_entry(image: &dyn ByteImage, pos: u64) -> Result<(ContainerEntry, u64), Error> {
    let len = image.len();
    let [name_len] = read_array::<1>(image, pos).map_err(|_| ContainerError::Truncated)?;
    let name_start = pos.saturating_add(1);
    let tail_start = name_start.saturating_add(u64::from(name_len));
    let next = tail_start.saturating_add(ENTRY_TAIL_LEN);
    if next > len {
        return Err(ContainerError::Truncated.into());
    }
    let name = read_vec(image, name_start, usize::from(name_len))?;
    let name = String::from_utf8(name).map_err(|_| ContainerError::NameNotUtf8)?;
    let (offset, entry_len, sha256) = parse_tail(read_array(image, tail_start)?);
    if offset.checked_add(entry_len).is_none_or(|end| end > len) {
        return Err(ContainerError::EntryOutOfRange { name }.into());
    }
    let entry = ContainerEntry {
        name,
        offset,
        len: entry_len,
        sha256,
    };
    Ok((entry, next))
}

/// Names are the lookup key; a duplicate would silently shadow the later image.
fn check_names_unique(entries: &[ContainerEntry]) -> Result<(), ContainerError> {
    let mut names: Vec<&str> = entries.iter().map(ContainerEntry::name).collect();
    names.sort_unstable();
    let duplicate = names.windows(2).find_map(|pair| match pair {
        [a, b] if a == b => Some(*a),
        _ => None,
    });
    duplicate.map_or(Ok(()), |name| {
        Err(ContainerError::DuplicateName { name: name.into() })
    })
}

fn check_disjoint(entries: &[ContainerEntry]) -> Result<(), ContainerError> {
    let mut sorted: Vec<&ContainerEntry> = entries.iter().collect();
    sorted.sort_by_key(|entry| entry.offset);
    for pair in sorted.windows(2) {
        if let [a, b] = pair
            && a.end() > b.offset
        {
            return Err(ContainerError::Overlap {
                first: a.name.clone(),
                second: b.name.clone(),
            });
        }
    }
    Ok(())
}

/// The entry named `kind`'s file name.
///
/// # Errors
///
/// [`ContainerError::Missing`] when there is none.
pub(crate) fn entry(
    entries: &[ContainerEntry],
    kind: ImageKind,
) -> Result<&ContainerEntry, ContainerError> {
    let name = kind.file_name();
    entries
        .iter()
        .find(|entry| entry.name == name)
        .ok_or(ContainerError::Missing { name })
}

/// Builds a container from named images, in the given order, recording each image's SHA-256.
///
/// # Errors
///
/// [`ContainerError::NameLength`] for a name outside 1..=255 bytes, [`ContainerError::TooLarge`]
/// when the offsets overflow.
pub fn build<N: AsRef<str>, B: AsRef<[u8]>>(images: &[(N, B)]) -> Result<Vec<u8>, ContainerError> {
    let count = u32::try_from(images.len()).map_err(|_| ContainerError::TooLarge)?;
    let mut table_len = HEADER_LEN;
    for (name, _) in images {
        let name_len = checked_name_len(name.as_ref())?;
        table_len = table_len
            .checked_add(1)
            .and_then(|n| n.checked_add(u64::from(name_len)))
            .and_then(|n| n.checked_add(ENTRY_TAIL_LEN))
            .ok_or(ContainerError::TooLarge)?;
    }
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(LAYOUT_VERSION);
    out.extend_from_slice(&count.to_le_bytes());
    let mut cursor = table_len;
    for (name, image) in images {
        let (name, image) = (name.as_ref(), image.as_ref());
        let image_len = u64::try_from(image.len()).map_err(|_| ContainerError::TooLarge)?;
        out.push(checked_name_len(name)?);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&cursor.to_le_bytes());
        out.extend_from_slice(&image_len.to_le_bytes());
        out.extend_from_slice(&Sha256::digest(image));
        cursor = cursor
            .checked_add(image_len)
            .ok_or(ContainerError::TooLarge)?;
    }
    for (_, image) in images {
        out.extend_from_slice(image.as_ref());
    }
    Ok(out)
}

fn checked_name_len(name: &str) -> Result<u8, ContainerError> {
    match u8::try_from(name.len()) {
        Ok(len) if len > 0 => Ok(len),
        _ => Err(ContainerError::NameLength { len: name.len() }),
    }
}

/// Recomputes the SHA-256 of `entry`'s image inside `container` (offline attestation; never on
/// the analysis path).
///
/// # Errors
///
/// [`Error::OutOfBounds`] when the entry lies outside `container`.
pub fn attest(container: &dyn ByteImage, entry: &ContainerEntry) -> Result<[u8; 32], Error> {
    const CHUNK: usize = 16 * 1024;
    let mut sha = Sha256::new();
    let mut chunk = [0u8; CHUNK];
    for pos in (0..entry.len).step_by(CHUNK) {
        let rest = entry.len.saturating_sub(pos);
        let take = usize::try_from(rest).map_or(CHUNK, |rest| rest.min(CHUNK));
        let buf = chunk
            .get_mut(..take)
            .ok_or_else(|| crate::image::OutOfBounds::new(pos, 0, container.len()))?;
        container.read_exact_at(entry.offset.saturating_add(pos), buf)?;
        sha.update(&*buf);
    }
    Ok(sha.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::HeapImage;
    use alloc::vec;

    fn images() -> Vec<(&'static str, Vec<u8>)> {
        vec![("a", vec![1, 2, 3]), ("bb", vec![]), ("c", vec![9; 70_000])]
    }

    #[test]
    fn round_trip_and_attest() {
        let bytes = build(&images()).unwrap();
        let image = HeapImage::from_vec(bytes);
        let entries = validate(&image).unwrap();
        assert_eq!(entries.len(), 3);
        for ((name, bytes), entry) in images().iter().zip(&entries) {
            assert_eq!(entry.name(), *name);
            assert_eq!(entry.len(), u64::try_from(bytes.len()).unwrap());
            let digest: [u8; 32] = Sha256::digest(bytes).into();
            assert_eq!(entry.sha256(), &digest);
            assert_eq!(attest(&image, entry).unwrap(), digest);
            assert_eq!(entry.is_empty(), bytes.is_empty());
        }
    }

    #[test]
    fn a_table_may_end_the_container() {
        // An empty last entry leaves no data after the table.
        let image = HeapImage::from_vec(build(&[("x", vec![])]).unwrap());
        let entries = validate(&image).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries.first().is_some_and(ContainerEntry::is_empty));
    }

    #[test]
    fn header_checks_each_magic_byte_and_the_entry_bound() {
        let header = |magic: [u8; 3], count: u32| {
            let [c0, c1, c2, c3] = count.to_le_bytes();
            let [m0, m1, m2] = magic;
            [m0, m1, m2, LAYOUT_VERSION, c0, c1, c2, c3]
        };
        assert_eq!(parse_header(header(*MAGIC, 0)), Ok(0));
        assert_eq!(parse_header(header(*MAGIC, MAX_ENTRIES)), Ok(MAX_ENTRIES));
        assert_eq!(
            parse_header(header(*MAGIC, MAX_ENTRIES + 1)),
            Err(ContainerError::TooManyEntries {
                count: MAX_ENTRIES + 1
            })
        );
        for byte in 0..3 {
            let mut magic = *MAGIC;
            magic[byte] ^= 0xFF;
            assert_eq!(
                parse_header(header(magic, 0)),
                Err(ContainerError::BadMagic)
            );
        }
    }

    #[test]
    fn malformed_containers_fail_closed() {
        let good = build(&images()).unwrap();
        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        assert_eq!(
            validate(&HeapImage::from_vec(bad_magic)),
            Err(ContainerError::BadMagic.into())
        );
        let mut bad_version = good.clone();
        bad_version[3] = 2;
        assert_eq!(
            validate(&HeapImage::from_vec(bad_version)),
            Err(ContainerError::UnsupportedVersion { found: 2 }.into())
        );
        assert_eq!(
            validate(&HeapImage::from_vec(good[..20].to_vec())),
            Err(ContainerError::Truncated.into())
        );
        assert_eq!(
            validate(&HeapImage::from_vec(Vec::new())),
            Err(ContainerError::TooSmall { len: 0 }.into())
        );
    }

    #[test]
    fn duplicate_names_fail_closed() {
        let bytes = build(&[("dup", vec![1]), ("dup", vec![2])]).unwrap();
        assert_eq!(
            validate(&HeapImage::from_vec(bytes)),
            Err(ContainerError::DuplicateName { name: "dup".into() }.into())
        );
    }

    #[test]
    fn overlapping_entries_fail_closed() {
        let mut bytes = build(&[("a", vec![1, 2]), ("b", vec![3, 4])]).unwrap();
        // Point "b" (table entry 2: after header 8 + entry 1 of 1+1+48) at "a"'s range.
        let b_offset_at = 8 + 50 + 2;
        let a_offset: [u8; 8] = bytes[10..18].try_into().unwrap();
        bytes[b_offset_at..b_offset_at + 8].copy_from_slice(&a_offset);
        assert_eq!(
            validate(&HeapImage::from_vec(bytes)),
            Err(ContainerError::Overlap {
                first: "a".into(),
                second: "b".into()
            }
            .into())
        );
    }

    #[test]
    fn names_must_be_1_to_255_bytes() {
        assert_eq!(
            build(&[("", vec![1])]),
            Err(ContainerError::NameLength { len: 0 })
        );
        let long = "x".repeat(256);
        assert_eq!(
            build(&[(long.as_str(), vec![1])]),
            Err(ContainerError::NameLength { len: 256 })
        );
    }
}

/// Proofs over every input (`cargo kani`).
#[cfg(kani)]
mod proofs {
    use super::*;

    /// Every 8-byte header is accepted or refused, and an accepted one has the magic, the version
    /// and a bounded count.
    #[kani::proof]
    fn parse_header_is_total() {
        let header: [u8; 8] = kani::any();
        if let Ok(count) = parse_header(header) {
            assert!(header[..3] == MAGIC[..] && header[3] == LAYOUT_VERSION);
            assert!(count <= MAX_ENTRIES);
        }
    }

    /// The tail splits into its three little-endian fields, losing no byte.
    #[kani::proof]
    #[kani::unwind(49)]
    fn parse_tail_round_trips() {
        let offset: u64 = kani::any();
        let len: u64 = kani::any();
        let digest: [u8; 32] = kani::any();
        let mut tail = [0u8; 48];
        tail[..8].copy_from_slice(&offset.to_le_bytes());
        tail[8..16].copy_from_slice(&len.to_le_bytes());
        tail[16..].copy_from_slice(&digest);
        assert_eq!(parse_tail(tail), (offset, len, digest));
    }
}
