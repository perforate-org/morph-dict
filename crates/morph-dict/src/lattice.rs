//! The lattice: every candidate segmentation of one line, as a DAG of word nodes.
//!
//! Slot 0 holds BOS; a node spanning bytes `[start, end)` sits in slot `end + 1`, and EOS sits
//! in the last slot. Candidates are the dictionary entries whose surface is a prefix of the rest
//! of the line, plus MeCab's unknown-word candidates (see [`Lattice::build`]).

use crate::dict::{CharCategory, DictEntry, Dictionary};
use crate::error::Error;
use crate::profile::DictionaryProfile;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// The word id of nodes with no dictionary token (BOS, EOS, the fallback unknown node).
const NO_WORD: u32 = u32::MAX;

/// The cost of the fallback unknown node, used when `unk.dic` has no entry for a class.
const FALLBACK_UNKNOWN_COST: i16 = 10_000;

/// Where a node's feature string comes from.
#[derive(Clone, Debug)]
pub(crate) enum Feature {
    /// Read from `sys.dic` by word id, and only for the chosen path.
    Dictionary,
    /// None: BOS, EOS and unknown-word entries.
    Empty,
    /// Carried inline: the fallback unknown node.
    Inline(String),
}

/// One candidate word.
#[derive(Clone, Debug)]
pub(crate) struct Node<'t> {
    surface: &'t str,
    start: usize,
    word_id: u32,
    left_id: u16,
    right_id: u16,
    wcost: i16,
    feature: Feature,
}

impl<'t> Node<'t> {
    const fn boundary(position: usize) -> Self {
        Self {
            surface: "",
            start: position,
            word_id: NO_WORD,
            left_id: 0,
            right_id: 0,
            wcost: 0,
            feature: Feature::Empty,
        }
    }

    /// The node for `entry` at byte `start` of `text`, when its span lies on character
    /// boundaries inside `text`.
    fn from_entry(
        text: &'t str,
        start: usize,
        entry: &DictEntry,
        feature: Feature,
    ) -> Option<Self> {
        let end = start.checked_add(entry.length())?;
        Some(Self {
            surface: text.get(start..end)?,
            start,
            word_id: entry.word_id(),
            left_id: entry.left_id(),
            right_id: entry.right_id(),
            wcost: entry.wcost(),
            feature,
        })
    }

    /// The node's text.
    pub(crate) const fn surface(&self) -> &'t str {
        self.surface
    }

    /// Its first byte.
    pub(crate) const fn start(&self) -> usize {
        self.start
    }

    /// One past its last byte.
    const fn end(&self) -> usize {
        self.start.saturating_add(self.surface.len())
    }

    /// Its slot: the end plus one.
    const fn slot(&self) -> usize {
        self.end().saturating_add(1)
    }

    /// Its dictionary token, or [`u32::MAX`].
    pub(crate) const fn word_id(&self) -> u32 {
        self.word_id
    }

    /// Left context id.
    pub(crate) const fn left_id(&self) -> u16 {
        self.left_id
    }

    /// Right context id.
    pub(crate) const fn right_id(&self) -> u16 {
        self.right_id
    }

    /// Word cost.
    pub(crate) const fn wcost(&self) -> i16 {
        self.wcost
    }

    /// Where its feature string comes from.
    pub(crate) const fn feature(&self) -> &Feature {
        &self.feature
    }
}

/// The lattice of one line.
#[derive(Debug)]
pub(crate) struct Lattice<'t> {
    slots: Vec<Vec<Node<'t>>>,
}

/// Scratch buffers reused across positions: zero allocations in the steady state.
#[derive(Default)]
struct Scratch {
    lookup: Vec<DictEntry>,
    unknown: Vec<DictEntry>,
}

/// One build: the line, the dictionary and the lattice under construction.
struct Builder<'t, 'd> {
    text: &'t str,
    dict: &'d Dictionary,
    profile: &'d DictionaryProfile,
    slots: Vec<Vec<Node<'t>>>,
}

impl<'t> Lattice<'t> {
    /// Builds the lattice of `text` (one line).
    ///
    /// At each character, the dictionary entries whose surface is a prefix of the rest of the
    /// line (bounded by [`DictionaryProfile::max_lookup_bytes`]) become nodes. Unknown-word
    /// candidates are added, as MeCab's tokenizer does, when there are no entries or when the
    /// character's class invokes them anyway.
    ///
    /// # Errors
    ///
    /// [`Error::LineTooLong`] past [`DictionaryProfile::max_line_bytes`]; [`Error::OutOfBounds`]
    /// when the dictionary cannot be read.
    pub(crate) fn build(
        text: &'t str,
        dict: &Dictionary,
        profile: &DictionaryProfile,
    ) -> Result<Self, Error> {
        if text.len() > profile.max_line_bytes() {
            return Err(Error::LineTooLong {
                len: text.len(),
                max: profile.max_line_bytes(),
            });
        }
        let mut builder = Builder {
            text,
            dict,
            profile,
            slots: vec![Vec::new(); text.len().saturating_add(2)],
        };
        // BOS sits in slot 0, before every node; EOS in the last slot, after them.
        if let Some(first) = builder.slots.first_mut() {
            first.push(Node::boundary(0));
        }
        let mut scratch = Scratch::default();
        for (pos, c) in text.char_indices() {
            builder.add_position(pos, c, &mut scratch)?;
        }
        let eos = Node::boundary(text.len());
        if let Some(last) = builder.slots.last_mut() {
            last.push(eos);
        }
        Ok(Self {
            slots: builder.slots,
        })
    }

    /// The slots, BOS first and EOS last.
    pub(crate) fn slots(&self) -> &[Vec<Node<'t>>] {
        &self.slots
    }
}

impl<'t> Builder<'t, '_> {
    fn push(&mut self, node: Node<'t>) {
        if let Some(slot) = self.slots.get_mut(node.slot()) {
            slot.push(node);
        }
    }

    /// The rest of the line from `pos`, cut at the last character boundary within
    /// `max_lookup_bytes`.
    fn lookup_key(&self, pos: usize) -> &'t str {
        let rest = self.text.get(pos..).unwrap_or_default();
        let bound = rest
            .char_indices()
            .find(|&(i, _)| i >= self.profile.max_lookup_bytes())
            .map_or(rest.len(), |(i, _)| i);
        rest.get(..bound).unwrap_or(rest)
    }

    fn add_position(&mut self, pos: usize, c: char, scratch: &mut Scratch) -> Result<(), Error> {
        self.dict
            .lookup_into(self.lookup_key(pos), &mut scratch.lookup)?;
        for entry in &scratch.lookup {
            if let Some(node) = Node::from_entry(self.text, pos, entry, Feature::Dictionary) {
                self.push(node);
            }
        }
        if scratch.lookup.is_empty() || self.dict.char_info(c).invokes() {
            self.add_unknown(pos, c, &mut scratch.unknown)?;
        }
        Ok(())
    }

    /// MeCab's unknown-word candidates at `pos`: the single character, the whole run of its
    /// class when the class groups (bounded by `unk_max_group_bytes`), and the runs of 1 to
    /// `length` characters.
    fn add_unknown(&mut self, pos: usize, c: char, unk: &mut Vec<DictEntry>) -> Result<(), Error> {
        let info = self.dict.char_info(c);
        let category = info.category();
        let first_end = pos.saturating_add(c.len_utf8());

        self.dict.unknown_into(category, c.len_utf8(), unk)?;
        if unk.is_empty() {
            self.add_fallback(pos, first_end, category);
        } else {
            self.add_unknown_entries(pos, unk);
        }

        if info.groups() {
            // A run of one character repeats the candidates above, which the span check in
            // `add_unknown_entries` drops.
            let run_end = self.run_end(first_end, category, pos);
            self.dict
                .unknown_into(category, run_end.saturating_sub(pos), unk)?;
            self.add_unknown_entries(pos, unk);
        }

        let mut end = first_end;
        for _ in 1..info.length() {
            match self.text.get(end..).and_then(|rest| rest.chars().next()) {
                Some(next) if self.dict.char_category(next) == category => {
                    end = end.saturating_add(next.len_utf8());
                    self.dict
                        .unknown_into(category, end.saturating_sub(pos), unk)?;
                    self.add_unknown_entries(pos, unk);
                }
                _ => break,
            }
        }
        Ok(())
    }

    /// The end of the run of `category` that starts at `pos` (its first character ends at
    /// `first_end`), capped at `pos + unk_max_group_bytes` on a character boundary.
    fn run_end(&self, first_end: usize, category: CharCategory, pos: usize) -> usize {
        // Walk no further than the cap, so a long run costs the cap per position, not its length.
        let cap = pos.saturating_add(self.profile.unk_max_group_bytes());
        let mut run_end = first_end;
        for next in self.text.get(first_end..).unwrap_or_default().chars() {
            let after = run_end.saturating_add(next.len_utf8());
            if after > cap || self.dict.char_category(next) != category {
                break;
            }
            run_end = after;
        }
        run_end
    }

    /// The node for a class `unk.dic` has no entry for.
    fn add_fallback(&mut self, pos: usize, end: usize, category: CharCategory) {
        if let Some(surface) = self.text.get(pos..end) {
            self.push(Node {
                surface,
                start: pos,
                word_id: NO_WORD,
                left_id: 0,
                right_id: 0,
                wcost: FALLBACK_UNKNOWN_COST,
                feature: Feature::Inline(format!("未知語,{category:?}")),
            });
        }
    }

    /// Adds each unknown entry whose span no node at `start` covers yet.
    fn add_unknown_entries(&mut self, start: usize, entries: &[DictEntry]) {
        for entry in entries {
            let Some(node) = Node::from_entry(self.text, start, entry, Feature::Empty) else {
                continue;
            };
            let taken = self.slots.get(node.slot()).is_some_and(|slot| {
                slot.iter()
                    .any(|other| other.start == start && other.end() == node.end())
            });
            if !taken {
                self.push(node);
            }
        }
    }
}
