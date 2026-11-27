//! MeCab-format morphological analysis over byte-image dictionaries.
//!
//! A MeCab dictionary is four images: `sys.dic` (double-array trie, word parameters and
//! feature strings), `unk.dic` (unknown-word entries, same format), `matrix.bin` (connection
//! costs) and `char.bin` (character classes). This crate reads them through the [`ByteImage`]
//! offset accessor instead of a contiguous slice, so the same analyzer runs over a heap copy or
//! over an image in paged storage (the Internet Computer's stable memory, a file, a page store)
//! without materializing it.
//!
//! [`Analyzer::open`] takes one [container] holding the four images and applies the
//! residency split the engine was measured with: the hot random-access regions (connection
//! matrix, character table, unknown dictionary, and `sys.dic`'s header, trie and word
//! parameters) are copied to the heap once; `sys.dic`'s feature strings stay behind the image
//! and are read only for the morphemes of the chosen path.
//!
//! What varies by language is the feature layout and which parts of speech an index keeps;
//! [`DictionaryProfile`] carries that policy. [`normalize`] holds the text normalization that
//! index and query share.
//!
//! The engine derives from MeCrab (github.com/cool-japan/mecrab @ 85444b5, MIT OR Apache-2.0);
//! see `LICENSE-MECRAB`. Compiled mecab-ipadic dictionaries carry the BSD acknowledgment in
//! `LICENSE-IPADIC`.
//!
//! Library code never panics: every read, slice and sum is checked, and a malformed dictionary is
//! an [`Error`]. The crate is `no_std` + `alloc`.

#![no_std]

extern crate alloc;

// Layers, each using only those before it: `image` and `error`, then `container` and `dict`,
// then `lattice` and `viterbi`, then `analyzer`. `profile` and `normalize` stand alone.
mod analyzer;
pub mod container;
mod dict;
mod error;
mod image;
mod lattice;
pub mod normalize;
mod profile;
mod viterbi;

pub use analyzer::Analyzer;
pub use error::{ContainerError, DictionaryError, Error, ImageKind};
pub use image::{ByteImage, HeapImage, OutOfBounds};
pub use profile::DictionaryProfile;
pub use viterbi::Morpheme;
