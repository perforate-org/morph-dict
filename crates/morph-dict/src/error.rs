//! Errors: what can be wrong with a container, a dictionary image or an input line.
//!
//! Every failure is a value. A dictionary error names the image it was found in
//! ([`ImageKind`]); a read past an image's end is [`OutOfBounds`], which a validated dictionary
//! never produces.

use crate::image::OutOfBounds;
use alloc::string::String;
use core::fmt;

/// One of the four MeCab-format images a dictionary is made of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ImageKind {
    /// `sys.dic`: the system dictionary (trie, word parameters, feature strings).
    SysDic,
    /// `unk.dic`: unknown-word entries keyed by character class, in the `sys.dic` format.
    UnkDic,
    /// `matrix.bin`: connection costs between right and left context ids.
    Matrix,
    /// `char.bin`: character classes and their unknown-word rules.
    CharBin,
}

impl ImageKind {
    /// Every image, in the order a container is built.
    pub const ALL: [Self; 4] = [Self::SysDic, Self::UnkDic, Self::Matrix, Self::CharBin];

    /// The image's file name, which is also its container entry name.
    #[must_use]
    pub const fn file_name(self) -> &'static str {
        match self {
            Self::SysDic => "sys.dic",
            Self::UnkDic => "unk.dic",
            Self::Matrix => "matrix.bin",
            Self::CharBin => "char.bin",
        }
    }
}

impl fmt::Display for ImageKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.file_name())
    }
}

/// A structural fault in a container (see [`crate::container`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ContainerError {
    /// Shorter than the fixed header.
    TooSmall {
        /// The container's length in bytes.
        len: u64,
    },
    /// The first bytes are not the container magic.
    BadMagic,
    /// A layout revision this reader does not speak: rebuild the container.
    UnsupportedVersion {
        /// The version byte found.
        found: u8,
    },
    /// More entries than any real dictionary has (a runaway table).
    TooManyEntries {
        /// The entry count found.
        count: u32,
    },
    /// The entry table runs past the end of the container.
    Truncated,
    /// An entry name is not UTF-8.
    NameNotUtf8,
    /// An entry name is empty or longer than 255 bytes (building only).
    NameLength {
        /// The name's length in bytes.
        len: usize,
    },
    /// An entry's byte range lies outside the container.
    EntryOutOfRange {
        /// The entry's name.
        name: String,
    },
    /// Two entries share a name.
    DuplicateName {
        /// The shared name.
        name: String,
    },
    /// Two entries' byte ranges overlap.
    Overlap {
        /// The entry that starts first.
        first: String,
        /// The entry it runs into.
        second: String,
    },
    /// An entry the analyzer needs is absent.
    Missing {
        /// The missing entry's name.
        name: &'static str,
    },
    /// The container would not fit the 64-bit offsets of its own table (building only).
    TooLarge,
}

impl fmt::Display for ContainerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall { len } => write!(f, "container too small: {len} bytes"),
            Self::BadMagic => f.write_str("container magic is not MPD"),
            Self::UnsupportedVersion { found } => write!(
                f,
                "container layout version {found} unsupported; rebuild the container"
            ),
            Self::TooManyEntries { count } => {
                write!(
                    f,
                    "container entry count {count} exceeds the structural bound"
                )
            }
            Self::Truncated => f.write_str("container entry table truncated"),
            Self::NameNotUtf8 => f.write_str("container entry name is not UTF-8"),
            Self::NameLength { len } => {
                write!(
                    f,
                    "container entry name of {len} bytes; names are 1..=255 bytes"
                )
            }
            Self::EntryOutOfRange { name } => {
                write!(f, "container entry {name:?} lies outside the container")
            }
            Self::DuplicateName { name } => {
                write!(f, "container has duplicate entry name {name:?}")
            }
            Self::Overlap { first, second } => {
                write!(f, "container entries {first:?} and {second:?} overlap")
            }
            Self::Missing { name } => write!(f, "container entry {name:?} missing"),
            Self::TooLarge => f.write_str("container exceeds 64-bit offsets"),
        }
    }
}

/// A structural fault in one dictionary image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DictionaryError {
    /// Shorter than the image's fixed header.
    TooSmall {
        /// The image's length in bytes.
        len: u64,
    },
    /// The header's magic does not encode the image's size.
    MagicMismatch,
    /// A dictionary format version this reader does not speak.
    VersionMismatch {
        /// The version found.
        found: u32,
    },
    /// The header's section sizes run past the end of the image.
    Truncated,
    /// The image's length is not the one its header implies.
    SizeMismatch {
        /// The length the header implies.
        expected: u64,
        /// The image's length.
        found: u64,
    },
    /// `unk.dic` does not carry the unknown-dictionary type.
    NotUnknownDictionary {
        /// The dictionary type found.
        dict_type: u32,
    },
    /// A section size does not fit this platform's address space.
    Overflow,
}

impl fmt::Display for DictionaryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall { len } => write!(f, "too small: {len} bytes"),
            Self::MagicMismatch => f.write_str("magic does not encode the image size"),
            Self::VersionMismatch { found } => write!(f, "unsupported version {found}"),
            Self::Truncated => f.write_str("sections run past the end of the image"),
            Self::SizeMismatch { expected, found } => {
                write!(f, "size mismatch: expected {expected} bytes, found {found}")
            }
            Self::NotUnknownDictionary { dict_type } => {
                write!(
                    f,
                    "dictionary type {dict_type} is not the unknown dictionary (2)"
                )
            }
            Self::Overflow => f.write_str("section sizes overflow the address space"),
        }
    }
}

/// Every failure of this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// A read past the end of an image.
    OutOfBounds(OutOfBounds),
    /// The container is malformed.
    Container(ContainerError),
    /// A dictionary image is malformed.
    Dictionary {
        /// The image the fault is in.
        image: ImageKind,
        /// The fault.
        error: DictionaryError,
    },
    /// An input line is longer than the profile allows
    /// ([`crate::DictionaryProfile::max_line_bytes`]).
    LineTooLong {
        /// The line's length in bytes.
        len: usize,
        /// The profile's bound.
        max: usize,
    },
}

impl Error {
    pub(crate) const fn dictionary(image: ImageKind, error: DictionaryError) -> Self {
        Self::Dictionary { image, error }
    }
}

impl From<OutOfBounds> for Error {
    fn from(error: OutOfBounds) -> Self {
        Self::OutOfBounds(error)
    }
}

impl From<ContainerError> for Error {
    fn from(error: ContainerError) -> Self {
        Self::Container(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds(error) => error.fmt(f),
            Self::Container(error) => error.fmt(f),
            Self::Dictionary { image, error } => write!(f, "{image}: {error}"),
            Self::LineTooLong { len, max } => {
                write!(f, "analysis line of {len} bytes exceeds the bound of {max}")
            }
        }
    }
}

impl core::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn errors_name_the_image_and_the_fault() {
        assert_eq!(ImageKind::Matrix.to_string(), "matrix.bin");
        assert_eq!(
            Error::dictionary(
                ImageKind::SysDic,
                DictionaryError::VersionMismatch { found: 7 }
            )
            .to_string(),
            "sys.dic: unsupported version 7"
        );
        assert_eq!(
            Error::from(ContainerError::Missing { name: "char.bin" }).to_string(),
            "container entry \"char.bin\" missing"
        );
        assert_eq!(
            Error::LineTooLong { len: 9, max: 8 }.to_string(),
            "analysis line of 9 bytes exceeds the bound of 8"
        );
        assert_eq!(
            Error::from(OutOfBounds::new(1, 2, 0)).to_string(),
            "read of 2 bytes at 1 runs past an image of 0 bytes"
        );
    }
}
