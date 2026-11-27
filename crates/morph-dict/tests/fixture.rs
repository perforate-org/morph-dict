//! Synthetic MeCab-format dictionaries, built programmatically so the tests need no 50 MB
//! artifacts: a Darts double-array builder, the `sys.dic`/`unk.dic` writer, `matrix.bin` and
//! `char.bin`.
//!
//! The trie encoding follows darts.h (taku910/mecab): edge code = byte + 1; a node's children
//! occupy `base + code` with `check = base`; a key's end is a code-0 child at `base` whose base
//! is `-(value + 1)`; unit 0 is `{ base: root_begin, check: 0 }`.

#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test fixture builders: small, known sizes; a broken fixture should fail the test"
)]

/// One dictionary entry: surface, context ids, cost and feature string.
struct Entry {
    key: &'static str,
    left: u16,
    right: u16,
    wcost: i16,
    feature: &'static str,
}

#[derive(Default)]
struct TrieNode {
    /// `(code, child)`, sorted by code; code 0 is a key's end.
    children: Vec<(usize, Self)>,
    value: u32,
}

fn trie(keys: &[(Vec<u8>, u32)]) -> TrieNode {
    let mut root = TrieNode::default();
    for (key, value) in keys {
        let mut node = &mut root;
        for &byte in key {
            let code = usize::from(byte) + 1;
            let at = match node.children.binary_search_by_key(&code, |(c, _)| *c) {
                Ok(at) => at,
                Err(at) => {
                    node.children.insert(at, (code, TrieNode::default()));
                    at
                }
            };
            node = &mut node.children[at].1;
        }
        assert!(node.children.iter().all(|(c, _)| *c != 0), "duplicate key");
        node.children.insert(
            0,
            (
                0,
                TrieNode {
                    value: *value,
                    ..TrieNode::default()
                },
            ),
        );
    }
    root
}

#[derive(Default)]
struct DoubleArray {
    units: Vec<(i32, u32)>,
    used: Vec<bool>,
}

impl DoubleArray {
    fn ensure(&mut self, index: usize) {
        if self.units.len() <= index {
            self.units.resize(index + 1, (0, 0));
            self.used.resize(index + 1, false);
        }
    }

    fn place(&mut self, node: &TrieNode) -> usize {
        let last = node.children.last().map_or(0, |(c, _)| *c);
        let mut begin = 1;
        loop {
            self.ensure(begin + last);
            let free = !self.used[begin]
                && node
                    .children
                    .iter()
                    .all(|(code, _)| self.units[begin + code].1 == 0 && begin + code != 0);
            if free {
                break;
            }
            begin += 1;
        }
        self.used[begin] = true;
        // Every check first, then the subtrees (a subtree must not claim a sibling's slot).
        for (code, _) in &node.children {
            self.units[begin + code].1 = u32::try_from(begin).unwrap();
        }
        for (code, child) in &node.children {
            let base = if *code == 0 {
                -i32::try_from(child.value).unwrap() - 1
            } else {
                i32::try_from(self.place(child)).unwrap()
            };
            self.units[begin + code].0 = base;
        }
        begin
    }
}

/// The double-array trie of `keys` (unique), in the `sys.dic` unit layout.
fn double_array(keys: &[(Vec<u8>, u32)]) -> Vec<u8> {
    let root = trie(keys);
    let mut array = DoubleArray::default();
    array.ensure(0);
    array.used[0] = true;
    let begin = array.place(&root);
    array.units[0].0 = i32::try_from(begin).unwrap();
    array
        .units
        .iter()
        .flat_map(|(base, check)| [base.to_le_bytes(), check.to_le_bytes()].concat())
        .collect()
}

fn u32_of(n: usize) -> u32 {
    u32::try_from(n).unwrap()
}

/// A `sys.dic`-format image of `entries` (sorted by key bytes; one token per key).
fn mecab_dic(dict_type: u32, entries: &[Entry]) -> Vec<u8> {
    let mut tokens = Vec::new();
    let mut features = Vec::new();
    let mut keys = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        keys.push((entry.key.as_bytes().to_vec(), (u32_of(index) << 8) | 1));
        tokens.extend_from_slice(&entry.left.to_le_bytes());
        tokens.extend_from_slice(&entry.right.to_le_bytes());
        tokens.extend_from_slice(&1u16.to_le_bytes());
        tokens.extend_from_slice(&entry.wcost.to_le_bytes());
        tokens.extend_from_slice(&u32_of(features.len()).to_le_bytes());
        tokens.extend_from_slice(&0u32.to_le_bytes());
        features.extend_from_slice(entry.feature.as_bytes());
        features.push(0);
    }
    let trie = double_array(&keys);
    let total = 72 + trie.len() + tokens.len() + features.len();
    let mut out = Vec::with_capacity(total);
    for field in [
        u32_of(total) ^ 0xef71_8f77,
        102,
        dict_type,
        u32_of(entries.len()),
        3,
        3,
        u32_of(trie.len()),
        u32_of(tokens.len()),
        u32_of(features.len()),
        0,
    ] {
        out.extend_from_slice(&field.to_le_bytes());
    }
    let mut charset = [0u8; 32];
    charset[..4].copy_from_slice(b"utf8");
    out.extend_from_slice(&charset);
    out.extend_from_slice(&trie);
    out.extend_from_slice(&tokens);
    out.extend_from_slice(&features);
    out
}

/// `matrix.bin` of `size × size` costs, all `cost`.
fn matrix(size: u16, cost: i16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    for _ in 0..usize::from(size) * usize::from(size) {
        out.extend_from_slice(&cost.to_le_bytes());
    }
    out
}

/// `char.bin` flag bits: unknown words are tried even with dictionary entries.
const INVOKE: u32 = 1 << 31;
/// `char.bin` flag bits: a run of the class is one candidate.
const GROUP: u32 = 1 << 30;

/// `char.bin`: `names`, then for each `(c, class)` in `members` the class id with its rule from
/// `rules` (`(id, flags, length)`); every other code point is class 0 with no flags.
fn char_bin(names: &[&str], rules: &[(u32, u32, u32)], members: &[(char, u32)]) -> Vec<u8> {
    let packed = |id: u32| {
        rules
            .iter()
            .find(|(rule_id, _, _)| *rule_id == id)
            .map_or(id << 18, |&(_, flags, length)| {
                flags | (length << 26) | (id << 18)
            })
    };
    let mut out = Vec::new();
    out.extend_from_slice(&u32_of(names.len()).to_le_bytes());
    for name in names {
        let mut buf = [0u8; 32];
        buf[..name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(&buf);
    }
    let mut table = vec![0u32; 0xFFFF];
    for &(c, id) in members {
        table[usize::try_from(u32::from(c)).unwrap()] = packed(id);
    }
    for word in table {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

/// A small Japanese dictionary: 走った, 走る, the particle は, nouns whose lemma column is
/// another word (ネコ → 猫), `*` (猫) or empty (犬), a costly はa, an `ALPHA` unknown class
/// (a–z, invoked and grouped) and a `SYMBOL` one (`!?#`, invoked, up to two characters, not
/// grouped).
pub(crate) fn images() -> Vec<(&'static str, Vec<u8>)> {
    let mut system = vec![
        Entry {
            key: "は",
            left: 0,
            right: 0,
            wcost: 100,
            feature: "助詞,係助詞,*,*,*,*,は,ハ,ワ",
        },
        Entry {
            key: "走った",
            left: 1,
            right: 0,
            wcost: 300,
            feature: "動詞,自立,*,*,特殊・タ,基本形,走った,ハシッタ,ハシッタ",
        },
        Entry {
            key: "走る",
            left: 1,
            right: 0,
            wcost: 300,
            feature: "動詞,自立,*,*,五段・ラ行,基本形,走る,ハシル,ハシル",
        },
        Entry {
            key: "ネコ",
            left: 1,
            right: 0,
            wcost: 300,
            feature: "名詞,一般,*,*,*,*,猫,ネコ,ネコ",
        },
        Entry {
            key: "猫",
            left: 1,
            right: 0,
            wcost: 300,
            feature: "名詞,一般,*,*,*,*,*,ネコ,ネコ",
        },
        Entry {
            key: "犬",
            left: 1,
            right: 0,
            wcost: 300,
            feature: "名詞,一般,*,*,*,*,,イヌ,イヌ",
        },
        Entry {
            key: "はa",
            left: 1,
            right: 0,
            wcost: 9000,
            feature: "名詞,一般,*,*,*,*,はa,ハエー,ハエー",
        },
    ];
    system.sort_by(|a, b| a.key.as_bytes().cmp(b.key.as_bytes()));
    let unknown = [
        Entry {
            key: "ALPHA",
            left: 1,
            right: 0,
            wcost: 5000,
            feature: "名詞,固有名詞,一般,*,*,*,*,*,*",
        },
        Entry {
            key: "SYMBOL",
            left: 1,
            right: 0,
            wcost: 5000,
            feature: "名詞,サ変接続,*,*,*,*,*,*,*",
        },
    ];
    let names = [
        "DEFAULT", "SPACE", "KANJI", "SYMBOL", "NUMERIC", "ALPHA", "HIRAGANA",
    ];
    let alpha = (5, INVOKE | GROUP, 4);
    let symbol = (3, INVOKE, 2);
    let members: Vec<(char, u32)> = ('a'..='z')
        .map(|c| (c, 5))
        .chain("!?#".chars().map(|c| (c, 3)))
        .collect();
    vec![
        ("sys.dic", mecab_dic(0, &system)),
        ("unk.dic", mecab_dic(2, &unknown)),
        ("matrix.bin", matrix(3, 0)),
        ("char.bin", char_bin(&names, &[alpha, symbol], &members)),
    ]
}
