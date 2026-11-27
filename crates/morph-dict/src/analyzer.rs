//! The analyzer: a dictionary opened from a container, and a profile that decides which units
//! an index sees.

use crate::container::{self, ContainerEntry};
use crate::dict::Dictionary;
use crate::error::Error;
use crate::image::ByteImage;
use crate::lattice::Lattice;
use crate::profile::DictionaryProfile;
use crate::viterbi::{self, Morpheme};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

/// A dictionary and its unit-emission profile. Cloning shares the dictionary.
#[derive(Clone)]
pub struct Analyzer {
    dictionary: Arc<Dictionary>,
    profile: DictionaryProfile,
}

impl fmt::Debug for Analyzer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Analyzer")
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}

impl Analyzer {
    /// Checks a container's structure (magic, table, entry ranges) without opening it. Cheap:
    /// header and table reads only.
    ///
    /// # Errors
    ///
    /// [`Error::Container`] when the container is malformed.
    pub fn validate_container(container: &dyn ByteImage) -> Result<Vec<ContainerEntry>, Error> {
        container::validate(container)
    }

    /// Opens the dictionary in `container` with the residency split: the connection matrix, the
    /// character table, `unk.dic`'s index and `sys.dic`'s header, trie and word parameters are
    /// copied to the heap; `sys.dic`'s feature strings stay behind `container`, which the analyzer
    /// keeps a reference to.
    ///
    /// # Errors
    ///
    /// [`Error::Container`] for a malformed container or a missing image, [`Error::Dictionary`]
    /// for a malformed image.
    pub fn open(container: &Arc<dyn ByteImage>, profile: DictionaryProfile) -> Result<Self, Error> {
        let entries = container::validate(container.as_ref())?;
        let dictionary = Arc::new(Dictionary::open(container, &entries)?);
        Ok(Self {
            dictionary,
            profile,
        })
    }

    /// The analyzer's profile.
    #[must_use]
    pub const fn profile(&self) -> &DictionaryProfile {
        &self.profile
    }

    /// Segments `text` line by line: the morphemes of each line's minimum-cost path. Blank
    /// lines, and lines no path crosses, yield nothing.
    ///
    /// # Errors
    ///
    /// [`Error::LineTooLong`] for a line past [`DictionaryProfile::max_line_bytes`];
    /// [`Error::OutOfBounds`] when the dictionary cannot be read.
    pub fn morphemes(&self, text: &str) -> Result<Vec<Morpheme>, Error> {
        let mut out = Vec::new();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let lattice = Lattice::build(line, &self.dictionary, &self.profile)?;
            if let Some(path) = viterbi::best_path(&lattice, &self.dictionary)? {
                out.extend(path);
            }
        }
        Ok(out)
    }

    /// The index units of `text`: each morpheme whose part of speech the profile keeps, as its
    /// lemma, or as its surface when the lemma is `*` or empty. Whitespace-only surfaces are
    /// skipped, which makes analysis idempotent over its own space-joined output.
    ///
    /// # Errors
    ///
    /// As [`Self::morphemes`].
    pub fn analyze(&self, text: &str) -> Result<Vec<String>, Error> {
        let units = self
            .morphemes(text)?
            .into_iter()
            .filter_map(|morpheme| self.unit(morpheme))
            .collect();
        Ok(units)
    }

    fn unit(&self, morpheme: Morpheme) -> Option<String> {
        if morpheme.surface().trim().is_empty() {
            return None;
        }
        let mut columns = morpheme.feature().split(',');
        let category = columns.next().unwrap_or("*");
        if self.profile.drops(category) {
            return None;
        }
        let lemma = self
            .profile
            .lemma_column()
            .and_then(|column| column.checked_sub(1))
            .and_then(|skip| columns.nth(skip))
            .filter(|lemma| *lemma != "*" && !lemma.is_empty())
            .map(String::from);
        Some(lemma.unwrap_or_else(|| morpheme.into_surface()))
    }
}
