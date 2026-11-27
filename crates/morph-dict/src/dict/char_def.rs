//! `char.bin`: character classes and their unknown-word rules.
//!
//! `u32 csize`, `csize` 32-byte class names, then one packed 4-byte [`CharInfo`] per UCS-2
//! code point below `0xFFFF`. The table is a hot random-access region, so it is copied to the
//! heap at open; code points past the table get the default class.

use crate::error::{DictionaryError, Error, ImageKind};
use crate::image::{ByteImage, read_u32, read_vec};
use alloc::vec::Vec;
use core::fmt;

/// Code points with an entry in the table.
const TABLE_SIZE: u32 = 0xFFFF;

/// Bytes per class name.
const NAME_SIZE: u64 = 32;

/// The character classes of the ipadic `char.def`, by their id there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum CharCategory {
    #[default]
    Default,
    Space,
    Kanji,
    Symbol,
    Numeric,
    Alpha,
    Hiragana,
    Katakana,
    Kanjinumeric,
    Greek,
    Cyrillic,
}

impl CharCategory {
    const fn from_id(id: u32) -> Self {
        match id {
            1 => Self::Space,
            2 => Self::Kanji,
            3 => Self::Symbol,
            4 => Self::Numeric,
            5 => Self::Alpha,
            6 => Self::Hiragana,
            7 => Self::Katakana,
            8 => Self::Kanjinumeric,
            9 => Self::Greek,
            10 => Self::Cyrillic,
            _ => Self::Default,
        }
    }

    /// The class's name, which is also its key in `unk.dic`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Default => "DEFAULT",
            Self::Space => "SPACE",
            Self::Kanji => "KANJI",
            Self::Symbol => "SYMBOL",
            Self::Numeric => "NUMERIC",
            Self::Alpha => "ALPHA",
            Self::Hiragana => "HIRAGANA",
            Self::Katakana => "KATAKANA",
            Self::Kanjinumeric => "KANJINUMERIC",
            Self::Greek => "GREEK",
            Self::Cyrillic => "CYRILLIC",
        }
    }
}

/// One code point's class and unknown-word rules, packed as
/// `type 18 | default_type 8 | length 4 | group 1 | invoke 1` (low bits first).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CharInfo(u32);

impl CharInfo {
    /// The class unknown words of this character take.
    pub(crate) const fn category(self) -> CharCategory {
        CharCategory::from_id((self.0 >> 18) & 0xFF)
    }

    /// How many characters of the class an unknown word may span, one at a time.
    pub(crate) const fn length(self) -> u32 {
        (self.0 >> 26) & 0xF
    }

    /// Whether a run of the class is also one unknown-word candidate.
    pub(crate) const fn groups(self) -> bool {
        (self.0 >> 30) & 1 != 0
    }

    /// Whether unknown-word candidates are added even when the dictionary has entries.
    pub(crate) const fn invokes(self) -> bool {
        (self.0 >> 31) & 1 != 0
    }
}

pub(crate) struct CharDef {
    table: Vec<u32>,
}

impl fmt::Debug for CharDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CharDef").finish_non_exhaustive()
    }
}

impl CharDef {
    /// Copies the table out of `image`.
    ///
    /// # Errors
    ///
    /// [`Error::Dictionary`] when the image's size is not the one its header implies.
    pub(crate) fn read(image: &dyn ByteImage) -> Result<Self, Error> {
        let fault = |error| Error::dictionary(ImageKind::CharBin, error);
        let len = image.len();
        if len < 4 {
            return Err(fault(DictionaryError::TooSmall { len }));
        }
        let classes = u64::from(read_u32(image, 0)?);
        let names_end = classes.saturating_mul(NAME_SIZE).saturating_add(4);
        let table_bytes = u64::from(TABLE_SIZE).saturating_mul(4);
        let expected = names_end.saturating_add(table_bytes);
        if len != expected {
            return Err(fault(DictionaryError::SizeMismatch {
                expected,
                found: len,
            }));
        }
        let table_len =
            usize::try_from(table_bytes).map_err(|_| fault(DictionaryError::Overflow))?;
        let bytes = read_vec(image, names_end, table_len)?;
        let table = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|word| u32::from_le_bytes(*word))
            .collect();
        Ok(Self { table })
    }

    /// The class and rules of `c`.
    pub(crate) fn info(&self, c: char) -> CharInfo {
        let code = u32::from(c);
        if code >= TABLE_SIZE {
            return CharInfo::default();
        }
        usize::try_from(code)
            .ok()
            .and_then(|code| self.table.get(code))
            .map_or_else(CharInfo::default, |&packed| CharInfo(packed))
    }

    /// The class of `c`.
    pub(crate) fn category(&self, c: char) -> CharCategory {
        self.info(c).category()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::HeapImage;

    #[test]
    fn packed_fields_decode() {
        let info = CharInfo((1 << 31) | (1 << 30) | (4 << 26) | (5 << 18) | 0b101);
        assert_eq!(info.category(), CharCategory::Alpha);
        assert_eq!(info.length(), 4);
        assert!(info.groups());
        assert!(info.invokes());
        assert_eq!(CharInfo(11 << 18).category(), CharCategory::Default);
        let plain = CharInfo((1 << 31) | (5 << 18) | 1);
        assert!(!plain.groups());
    }

    #[test]
    fn class_ids_follow_ipadic() {
        let names: Vec<&str> = (0..=11)
            .map(|id| CharCategory::from_id(id).name())
            .collect();
        assert_eq!(
            names,
            [
                "DEFAULT",
                "SPACE",
                "KANJI",
                "SYMBOL",
                "NUMERIC",
                "ALPHA",
                "HIRAGANA",
                "KATAKANA",
                "KANJINUMERIC",
                "GREEK",
                "CYRILLIC",
                "DEFAULT",
            ]
        );
    }

    #[test]
    fn short_images_are_too_small() {
        let read = |len: usize| CharDef::read(&HeapImage::from_vec(alloc::vec![0; len])).err();
        let fault = |error| Some(Error::dictionary(ImageKind::CharBin, error));
        assert_eq!(read(3), fault(DictionaryError::TooSmall { len: 3 }));
        assert_eq!(
            read(4),
            fault(DictionaryError::SizeMismatch {
                expected: u64::from(TABLE_SIZE) * 4 + 4,
                found: 4
            })
        );
    }
}

/// Proofs over every input (`cargo kani`).
#[cfg(kani)]
mod proofs {
    use super::*;

    /// Every packed word decodes, and the fields stay in their bit widths.
    #[kani::proof]
    fn char_info_decodes_every_word() {
        let info = CharInfo(kani::any());
        assert!(info.length() < 16);
        let _category = info.category();
        let _flags = (info.groups(), info.invokes());
    }
}
