//! Per-language unit emission.
//!
//! MeCab-format dictionaries share one structure across languages; what varies is the feature
//! layout (which comma-separated column holds the lemma) and which parts of speech an index
//! drops. A profile carries that policy, plus the bounds that keep analysis linear.

use alloc::string::String;
use alloc::vec::Vec;

/// Bytes per line beyond which analysis fails ([`crate::Error::LineTooLong`]).
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Bytes the trie walk and unknown-word grouping consume per position.
const MAX_LOOKUP_BYTES: usize = 64;

/// Unit-emission policy over a MeCab feature string.
///
/// For ipadic the layout is `品詞,品詞細分類1,品詞細分類2,品詞細分類3,活用型,活用形,基本形,読み,発音`;
/// other dictionaries carry other columns, and only the lemma column's index matters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictionaryProfile {
    lemma_column: Option<usize>,
    drop_categories: Vec<String>,
    drop_prefixes: Vec<String>,
    max_lookup_bytes: usize,
    unk_max_group_bytes: usize,
    max_line_bytes: usize,
}

impl DictionaryProfile {
    /// Japanese mecab-ipadic: column 0 (品詞) values 助詞, 助動詞, 記号, 接頭辞, 接尾辞 and フィラー
    /// are dropped, column 6 (基本形) is the lemma.
    #[must_use]
    pub fn japanese_ipadic() -> Self {
        Self {
            lemma_column: Some(6),
            drop_categories: owned(&["助詞", "助動詞", "記号", "接頭辞", "接尾辞", "フィラー"]),
            drop_prefixes: Vec::new(),
            max_lookup_bytes: MAX_LOOKUP_BYTES,
            unk_max_group_bytes: MAX_LOOKUP_BYTES,
            max_line_bytes: MAX_LINE_BYTES,
        }
    }

    /// Korean mecab-ko-dic: the layout is
    /// `POS,semantic,jongseong,lemma,Compound/Inflect,POS2,POS3,analysis`, so column 3 is the lemma
    /// (verbs are stored as stems). Compound tags such as `VV+EC` make exact matching unusable, so
    /// drops match the first `+`-separated subtag by prefix: particles (J), endings (E), suffixes
    /// (XSN, XSV, XSA), punctuation and symbols (SF, SE, SC, SSC, SSO, SP, SY, SW) and unanalyzed
    /// tokens (UNA, NA). Foreign words and hanja (SL, SH) carry content and are kept.
    #[must_use]
    pub fn korean_mecab_ko_dic() -> Self {
        Self {
            lemma_column: Some(3),
            drop_categories: Vec::new(),
            drop_prefixes: owned(&[
                "J", "E", "XSN", "XSV", "XSA", "SF", "SE", "SC", "SSC", "SSO", "SP", "SY", "SW",
                "UNA", "NA",
            ]),
            max_lookup_bytes: MAX_LOOKUP_BYTES,
            unk_max_group_bytes: MAX_LOOKUP_BYTES,
            max_line_bytes: MAX_LINE_BYTES,
        }
    }

    /// The same profile with another line bound.
    #[must_use]
    pub const fn with_max_line_bytes(mut self, max_line_bytes: usize) -> Self {
        self.max_line_bytes = max_line_bytes;
        self
    }

    /// The feature column emitted as the lemma; `None` emits every kept surface as is.
    #[must_use]
    pub const fn lemma_column(&self) -> Option<usize> {
        self.lemma_column
    }

    /// The common-prefix search bound: the trie walk consumes at most this many bytes of the
    /// remaining input per position (clamped to a character boundary), which keeps a long line
    /// linear. No ipadic surface is longer.
    #[must_use]
    pub const fn max_lookup_bytes(&self) -> usize {
        self.max_lookup_bytes
    }

    /// The bound on one unknown-word run of a grouping class (MeCab's max-grouping-size,
    /// tightened so the lattice stays linear).
    #[must_use]
    pub const fn unk_max_group_bytes(&self) -> usize {
        self.unk_max_group_bytes
    }

    /// The longest line analysis accepts; a longer one is [`crate::Error::LineTooLong`].
    #[must_use]
    pub const fn max_line_bytes(&self) -> usize {
        self.max_line_bytes
    }

    /// Whether units whose first feature column is `category` are dropped: an exact match in
    /// the drop set, or a prefix match of its first `+`-separated subtag.
    #[must_use]
    pub fn drops(&self, category: &str) -> bool {
        if self.drop_categories.iter().any(|c| c == category) {
            return true;
        }
        let subtag = category.split('+').next().unwrap_or(category);
        self.drop_prefixes
            .iter()
            .any(|p| subtag.starts_with(p.as_str()))
    }
}

fn owned(items: &[&str]) -> Vec<String> {
    items.iter().map(|&item| String::from(item)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipadic_drops_by_exact_category() {
        let profile = DictionaryProfile::japanese_ipadic();
        assert!(profile.drops("助詞"));
        assert!(!profile.drops("名詞"));
        assert!(!profile.drops("助詞+X"));
    }

    #[test]
    fn ko_dic_drops_by_subtag_prefix() {
        let profile = DictionaryProfile::korean_mecab_ko_dic();
        assert!(profile.drops("JKS"));
        assert!(profile.drops("EC+JX"));
        assert!(profile.drops("ETM+NNG+JC"));
        assert!(!profile.drops("NNG"));
        assert!(!profile.drops("VV+EC"));
        assert!(!profile.drops("SL"));
    }
}
