//! The four dictionary images, opened from a container with the residency split.

mod char_def;
mod matrix;
mod sys_dic;
mod trie;

pub(crate) use char_def::{CharCategory, CharInfo};
pub(crate) use sys_dic::DictEntry;

use crate::container::{self, ContainerEntry};
use crate::error::{DictionaryError, Error, ImageKind};
use crate::image::{ByteImage, OffsetImage};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use char_def::CharDef;
use matrix::ConnectionMatrix;
use sys_dic::{SysDic, UNKNOWN_DICTIONARY};

/// A dictionary ready for analysis.
#[derive(Debug)]
pub(crate) struct Dictionary {
    system: SysDic,
    unknown: SysDic,
    matrix: ConnectionMatrix,
    chars: CharDef,
}

impl Dictionary {
    /// Opens the dictionary in `container`, whose table is `entries`.
    ///
    /// # Errors
    ///
    /// [`Error::Container`] when an image is missing, [`Error::Dictionary`] when one is
    /// malformed.
    pub(crate) fn open(
        container: &Arc<dyn ByteImage>,
        entries: &[ContainerEntry],
    ) -> Result<Self, Error> {
        let window = |kind| -> Result<Arc<dyn ByteImage>, Error> {
            let entry = container::entry(entries, kind)?;
            Ok(Arc::new(OffsetImage::new(
                Arc::clone(container),
                entry.offset(),
                entry.len(),
            )?))
        };
        let system = SysDic::open(window(ImageKind::SysDic)?, ImageKind::SysDic)?;
        let unknown = SysDic::open(window(ImageKind::UnkDic)?, ImageKind::UnkDic)?;
        if unknown.dict_type() != UNKNOWN_DICTIONARY {
            return Err(Error::dictionary(
                ImageKind::UnkDic,
                DictionaryError::NotUnknownDictionary {
                    dict_type: unknown.dict_type(),
                },
            ));
        }
        let matrix = ConnectionMatrix::read(window(ImageKind::Matrix)?.as_ref())?;
        let chars = CharDef::read(window(ImageKind::CharBin)?.as_ref())?;
        Ok(Self {
            system,
            unknown,
            matrix,
            chars,
        })
    }

    /// System entries whose surface is a prefix of `key`, into `out`.
    pub(crate) fn lookup_into(&self, key: &str, out: &mut Vec<DictEntry>) -> Result<(), Error> {
        self.system.prefix_entries_into(key, out)
    }

    /// Unknown-word entries of `category`, each spanning `length` bytes, into `out`.
    ///
    /// The lookup is a prefix search over the class name, as upstream MeCrab's: `KANJINUMERIC`
    /// also yields the `KANJI` entries. Kept for parity with the analyzer it was measured as.
    pub(crate) fn unknown_into(
        &self,
        category: CharCategory,
        length: usize,
        out: &mut Vec<DictEntry>,
    ) -> Result<(), Error> {
        self.unknown.prefix_entries_into(category.name(), out)?;
        for entry in out.iter_mut() {
            *entry = entry.with_length(length);
        }
        Ok(())
    }

    /// The cost of connecting right id `right` to left id `left`.
    pub(crate) fn connection_cost(&self, right: u16, left: u16) -> i16 {
        self.matrix.cost(right, left)
    }

    /// The class and rules of `c`.
    pub(crate) fn char_info(&self, c: char) -> CharInfo {
        self.chars.info(c)
    }

    /// The class of `c`.
    pub(crate) fn char_category(&self, c: char) -> CharCategory {
        self.chars.category(c)
    }

    /// The feature string of system token `word_id`.
    pub(crate) fn feature(&self, word_id: u32) -> Result<String, Error> {
        self.system.feature(word_id)
    }
}
