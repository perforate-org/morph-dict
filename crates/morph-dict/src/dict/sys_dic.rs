//! The MeCab system-dictionary format, shared by `sys.dic` and `unk.dic`.
//!
//! ```text
//! offset  size        field
//! 0       4           magic: image size XOR 0xef718f77
//! 4       4           version (102)
//! 8       4           dictionary type (0 system, 1 user, 2 unknown)
//! 12      12          lexicon size, left size, right size
//! 24      4           da_size: trie bytes
//! 28      4           token_size: word-parameter bytes (16 per token)
//! 32      4           feature_size: feature-string bytes
//! 36      4           (unused)
//! 40      32          charset, NUL-padded
//! 72      da_size     double-array trie (see `trie`)
//! ...     token_size  tokens: left id u16, right id u16, pos id u16, cost i16,
//!                     feature offset u32, compound u32
//! ...     feature_size NUL-terminated feature strings
//! ```
//!
//! A trie value packs `token_start << 8 | token_count`: the tokens of one surface are adjacent.
//!
//! The image is opened with the residency split: the index region (header, trie, tokens) is
//! copied to the heap, where lookups read it in place; the feature region stays behind the
//! original image and is read only for the morphemes of the chosen path.

use super::trie::DoubleArrayTrie;
use crate::error::{DictionaryError, Error, ImageKind};
use crate::image::{ByteImage, HeapImage, OffsetImage, OutOfBounds, read_array, read_vec};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

/// `magic ^ MAGIC_ID` is the image's size.
const MAGIC_ID: u32 = 0xef71_8f77;

/// The dictionary format version this reader speaks (MeCab's `DIC_VERSION`).
const DIC_VERSION: u32 = 102;

/// Header bytes: ten `u32` fields and the 32-byte charset.
const HEADER_SIZE: u64 = 72;

/// Bytes per token.
const TOKEN_SIZE: u64 = 16;

/// Bytes per chunk when scanning a feature string for its NUL terminator.
const FEATURE_CHUNK: usize = 256;

/// The dictionary type of `unk.dic`.
pub(crate) const UNKNOWN_DICTIONARY: u32 = 2;

/// One token's word parameters (the feature string is fetched separately).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Token {
    left_id: u16,
    right_id: u16,
    wcost: i16,
    feature_offset: u32,
}

/// A prefix match with its token: what a lattice node is built from.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DictEntry {
    length: usize,
    word_id: u32,
    token: Token,
}

impl DictEntry {
    /// Surface length in bytes.
    pub(crate) const fn length(&self) -> usize {
        self.length
    }

    /// The same entry with another surface length (unknown words span a run).
    pub(crate) const fn with_length(self, length: usize) -> Self {
        Self { length, ..self }
    }

    /// Token index in its dictionary.
    pub(crate) const fn word_id(&self) -> u32 {
        self.word_id
    }

    /// Left context id.
    pub(crate) const fn left_id(&self) -> u16 {
        self.token.left_id
    }

    /// Right context id.
    pub(crate) const fn right_id(&self) -> u16 {
        self.token.right_id
    }

    /// Word cost.
    pub(crate) const fn wcost(&self) -> i16 {
        self.token.wcost
    }
}

/// The section sizes a header declares.
#[derive(Clone, Copy, Debug)]
struct Header {
    dict_type: u32,
    trie_len: u64,
    token_len: u64,
    feature_len: u64,
}

impl Header {
    /// Reads and checks the header of `image`.
    fn read(image: &dyn ByteImage, kind: ImageKind) -> Result<Self, Error> {
        let len = image.len();
        if len < HEADER_SIZE {
            return Err(Error::dictionary(kind, DictionaryError::TooSmall { len }));
        }
        Self::parse(read_array(image, 0)?, len).map_err(|error| Error::dictionary(kind, error))
    }

    /// Checks the header's first 40 bytes against an image of `len` bytes.
    fn parse(bytes: [u8; 40], len: u64) -> Result<Self, DictionaryError> {
        let word = |at: usize| {
            let mut out = [0u8; 4];
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = bytes.get(at.saturating_add(i)).copied().unwrap_or(0);
            }
            u32::from_le_bytes(out)
        };
        if u32::try_from(len).ok() != Some(word(0) ^ MAGIC_ID) {
            return Err(DictionaryError::MagicMismatch);
        }
        let version = word(4);
        if version != DIC_VERSION {
            return Err(DictionaryError::VersionMismatch { found: version });
        }
        let header = Self {
            dict_type: word(8),
            trie_len: u64::from(word(24)),
            token_len: u64::from(word(28)),
            feature_len: u64::from(word(32)),
        };
        if header.feature_offset().saturating_add(header.feature_len) > len {
            return Err(DictionaryError::Truncated);
        }
        Ok(header)
    }

    /// Where the feature strings start: the end of the index region.
    const fn feature_offset(&self) -> u64 {
        HEADER_SIZE
            .saturating_add(self.trie_len)
            .saturating_add(self.token_len)
    }

    const fn token_offset(&self) -> u64 {
        HEADER_SIZE.saturating_add(self.trie_len)
    }
}

/// A `sys.dic`-format dictionary opened with the residency split.
pub(crate) struct SysDic {
    /// Header, trie and tokens, on the heap.
    index: Arc<dyn ByteImage>,
    /// The feature strings, addressed from zero.
    features: Arc<dyn ByteImage>,
    trie: DoubleArrayTrie,
    header: Header,
}

impl fmt::Debug for SysDic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SysDic")
            .field("header", &self.header)
            .field("trie", &self.trie)
            .finish_non_exhaustive()
    }
}

impl SysDic {
    /// Opens `image` (the whole dictionary, any backing): its index region is copied to the
    /// heap, its feature region stays behind `image`.
    ///
    /// # Errors
    ///
    /// [`Error::Dictionary`] for a malformed header, [`Error::OutOfBounds`] if `image` cannot be
    /// read.
    pub(crate) fn open(image: Arc<dyn ByteImage>, kind: ImageKind) -> Result<Self, Error> {
        let header = Header::read(image.as_ref(), kind)?;
        let overflow = Error::dictionary(kind, DictionaryError::Overflow);
        let index_len = usize::try_from(header.feature_offset()).map_err(|_| overflow.clone())?;
        let index: Arc<dyn ByteImage> =
            Arc::new(HeapImage::from_vec(read_vec(image.as_ref(), 0, index_len)?));
        let feature_len = image.len().saturating_sub(header.feature_offset());
        let features: Arc<dyn ByteImage> = Arc::new(OffsetImage::new(
            image,
            header.feature_offset(),
            feature_len,
        )?);
        let trie = DoubleArrayTrie::new(Arc::clone(&index), HEADER_SIZE, header.trie_len)?;
        Ok(Self {
            index,
            features,
            trie,
            header,
        })
    }

    /// The dictionary type field.
    pub(crate) const fn dict_type(&self) -> u32 {
        self.header.dict_type
    }

    /// Every entry whose surface is a prefix of `key`, into `out` (cleared first): the lattice's
    /// per-position lookup, allocation-free once `out` has grown.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfBounds`] when the index cannot be read (never for a validated dictionary).
    pub(crate) fn prefix_entries_into(
        &self,
        key: &str,
        out: &mut Vec<DictEntry>,
    ) -> Result<(), Error> {
        out.clear();
        let mut failure = None;
        self.trie.for_each_prefix(key.as_bytes(), |value, length| {
            if failure.is_none()
                && let Err(error) = self.push_tokens(value, length, out)
            {
                failure = Some(error);
            }
        })?;
        failure.map_or(Ok(()), |error| Err(error.into()))
    }

    /// Pushes the tokens a trie value names.
    fn push_tokens(
        &self,
        value: u32,
        length: usize,
        out: &mut Vec<DictEntry>,
    ) -> Result<(), OutOfBounds> {
        let start = value >> 8;
        let count = value & 0xff;
        for word_id in (0..count).map_while(|i| start.checked_add(i)) {
            if let Some(token) = self.token(word_id)? {
                out.push(DictEntry {
                    length,
                    word_id,
                    token,
                });
            }
        }
        Ok(())
    }

    /// The token `word_id`, or `None` past the token array.
    fn token(&self, word_id: u32) -> Result<Option<Token>, OutOfBounds> {
        let index = u64::from(word_id);
        if index >= self.header.token_len.checked_div(TOKEN_SIZE).unwrap_or(0) {
            return Ok(None);
        }
        let at = self
            .header
            .token_offset()
            .saturating_add(index.saturating_mul(TOKEN_SIZE));
        let [l0, l1, r0, r1, _p0, _p1, w0, w1, f0, f1, f2, f3, ..] =
            read_array::<16>(self.index.as_ref(), at)?;
        Ok(Some(Token {
            left_id: u16::from_le_bytes([l0, l1]),
            right_id: u16::from_le_bytes([r0, r1]),
            wcost: i16::from_le_bytes([w0, w1]),
            feature_offset: u32::from_le_bytes([f0, f1, f2, f3]),
        }))
    }

    /// The feature string of token `word_id`; empty for an unknown token, an offset past the
    /// feature region, or bytes that are not UTF-8.
    ///
    /// # Errors
    ///
    /// [`Error::OutOfBounds`] when the feature region cannot be read.
    pub(crate) fn feature(&self, word_id: u32) -> Result<String, Error> {
        let Some(token) = self.token(word_id)? else {
            return Ok(String::new());
        };
        let end = self.header.feature_len;
        let mut out = Vec::with_capacity(64);
        let mut chunk = [0u8; FEATURE_CHUNK];
        // Up to the NUL, or the region's end for an unterminated last string.
        for pos in (u64::from(token.feature_offset)..end).step_by(FEATURE_CHUNK) {
            let want = usize::try_from(end.saturating_sub(pos))
                .map_or(FEATURE_CHUNK, |rest| rest.min(FEATURE_CHUNK));
            let Some(buf) = chunk.get_mut(..want) else {
                break;
            };
            self.features.read_exact_at(pos, buf)?;
            if let Some(nul) = buf.iter().position(|&b| b == 0) {
                out.extend_from_slice(buf.get(..nul).unwrap_or_default());
                break;
            }
            out.extend_from_slice(buf);
        }
        Ok(String::from_utf8(out).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_images_are_too_small() {
        let read =
            |len: usize| Header::read(&HeapImage::from_vec(alloc::vec![0; len]), ImageKind::UnkDic);
        let fault = |error| Err(Error::dictionary(ImageKind::UnkDic, error));
        assert_eq!(
            read(71).map(|_| ()),
            fault(DictionaryError::TooSmall { len: 71 })
        );
        assert_eq!(
            read(3).map(|_| ()),
            fault(DictionaryError::TooSmall { len: 3 })
        );
        // A full header is parsed: zeros carry the wrong magic.
        assert_eq!(read(72).map(|_| ()), fault(DictionaryError::MagicMismatch));
    }
}

/// Proofs over every input (`cargo kani`).
#[cfg(kani)]
mod proofs {
    use super::*;

    /// Every header is accepted or refused, and an accepted one's sections lie inside the image.
    #[kani::proof]
    #[kani::unwind(5)]
    fn header_parse_is_total() {
        let bytes: [u8; 40] = kani::any();
        let len: u64 = kani::any();
        if let Ok(header) = Header::parse(bytes, len) {
            assert!(header.token_offset() <= header.feature_offset());
            assert!(
                header
                    .feature_offset()
                    .checked_add(header.feature_len)
                    .is_some_and(|end| end <= len)
            );
            assert!(len <= u64::from(u32::MAX));
        }
    }
}
