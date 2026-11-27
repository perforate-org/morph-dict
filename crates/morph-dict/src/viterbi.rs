//! Viterbi: the minimum-cost path through a lattice.
//!
//! A path's cost is the sum of its nodes' word costs and the connection costs between adjacent
//! nodes. The forward pass keeps, per node, the cheapest way to reach it (the first of equals);
//! the backward pass walks those choices back from EOS.

use crate::dict::Dictionary;
use crate::error::Error;
use crate::lattice::{Feature, Lattice, Node};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// The cheapest way found to reach one node.
#[derive(Clone, Copy)]
struct Reached<'l, 't> {
    node: &'l Node<'t>,
    cost: i64,
    /// The predecessor: its slot and its index there.
    prev: Option<(usize, usize)>,
}

/// The minimum-cost path through `lattice`, BOS and EOS excluded, or `None` when no path
/// reaches EOS.
///
/// # Errors
///
/// [`Error::OutOfBounds`] when a feature string cannot be read.
pub(crate) fn best_path(
    lattice: &Lattice<'_>,
    dict: &Dictionary,
) -> Result<Option<Vec<Morpheme>>, Error> {
    let reached = forward(lattice, dict);
    backward(&reached, dict)
}

fn forward<'l, 't>(lattice: &'l Lattice<'t>, dict: &Dictionary) -> Vec<Vec<Reached<'l, 't>>> {
    let slots = lattice.slots();
    let mut reached: Vec<Vec<Reached<'l, 't>>> = vec![Vec::new(); slots.len()];
    if let (Some(bos), Some(first)) = (slots.first().and_then(|s| s.first()), reached.first_mut()) {
        first.push(Reached {
            node: bos,
            cost: 0,
            prev: None,
        });
    }
    // Slot 0 holds BOS alone, which `reached` already starts from.
    for (pos, slot) in slots.iter().enumerate().skip(1) {
        for node in slot {
            // A node starting the line follows BOS; any other follows a node ending at its
            // start, which sits in slot `start + 1`.
            let prev_slot = if node.start() == 0 {
                0
            } else {
                node.start().saturating_add(1)
            };
            if let Some(best) = cheapest_predecessor(&reached, prev_slot, node, dict)
                && let Some(here) = reached.get_mut(pos)
            {
                here.push(best);
            }
        }
    }
    reached
}

/// The cheapest way to reach `node` from the nodes reached in `prev_slot`.
fn cheapest_predecessor<'l, 't>(
    reached: &[Vec<Reached<'l, 't>>],
    prev_slot: usize,
    node: &'l Node<'t>,
    dict: &Dictionary,
) -> Option<Reached<'l, 't>> {
    let mut best: Option<Reached<'l, 't>> = None;
    for (index, prev) in reached.get(prev_slot)?.iter().enumerate() {
        let connection = i64::from(dict.connection_cost(prev.node.right_id(), node.left_id()));
        let cost = prev
            .cost
            .saturating_add(connection)
            .saturating_add(i64::from(node.wcost()));
        // Costs are i16 sums over a bounded line, far from saturating. Ties keep the first
        // predecessor in slot order.
        if best.is_none_or(|b| cost < b.cost) {
            best = Some(Reached {
                node,
                cost,
                prev: Some((prev_slot, index)),
            });
        }
    }
    best
}

fn backward(
    reached: &[Vec<Reached<'_, '_>>],
    dict: &Dictionary,
) -> Result<Option<Vec<Morpheme>>, Error> {
    let Some(last) = reached.last() else {
        return Ok(None);
    };
    // Prefer the EOS node itself; with none, the cheapest word in the last slot stands in for
    // it (upstream MeCrab took the cheapest entry unconditionally, which dropped the last word
    // of a sentence whenever a word beat EOS).
    let end = last
        .iter()
        .find(|r| r.node.surface().is_empty())
        .or_else(|| last.iter().min_by_key(|r| r.cost));
    let Some(end) = end else {
        return Ok(None);
    };
    let mut path = Vec::new();
    let mut cursor = end.prev;
    while let Some(current) = cursor.and_then(|(slot, index)| reached.get(slot)?.get(index)) {
        let node = current.node;
        if !node.surface().is_empty() {
            path.push(Morpheme::new(
                String::from(node.surface()),
                feature(node, dict)?,
            ));
        }
        cursor = current.prev;
    }
    path.reverse();
    Ok(Some(path))
}

/// A node's feature string: read from the dictionary only here, for the chosen path.
fn feature(node: &Node<'_>, dict: &Dictionary) -> Result<String, Error> {
    match node.feature() {
        // Only dictionary entries carry `Feature::Dictionary`, so the word id names a token.
        Feature::Dictionary => dict.feature(node.word_id()),
        Feature::Empty => Ok(String::new()),
        Feature::Inline(text) => Ok(text.clone()),
    }
}

/// One morpheme of the chosen path: its surface and its feature string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Morpheme {
    surface: String,
    feature: String,
}

impl Morpheme {
    pub(crate) const fn new(surface: String, feature: String) -> Self {
        Self { surface, feature }
    }

    /// The morpheme's text.
    #[must_use]
    pub fn surface(&self) -> &str {
        &self.surface
    }

    /// The morpheme's text, owned.
    pub(crate) fn into_surface(self) -> String {
        self.surface
    }

    /// Its comma-separated feature string; empty for unknown words.
    #[must_use]
    pub fn feature(&self) -> &str {
        &self.feature
    }
}
