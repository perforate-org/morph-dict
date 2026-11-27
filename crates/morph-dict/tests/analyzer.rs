//! The analyzer over the synthetic dictionary: emission, determinism, the failure modes, and
//! properties over arbitrary input.

mod fixture;

use morph_dict::{
    Analyzer, ByteImage, ContainerError, DictionaryError, DictionaryProfile, Error, HeapImage,
    ImageKind, container,
};
use proptest::prelude::*;
extern crate alloc;

use alloc::sync::Arc;

fn open(images: &[(&str, Vec<u8>)], profile: DictionaryProfile) -> Result<Analyzer, Error> {
    let image: Arc<dyn ByteImage> = Arc::new(HeapImage::from_vec(container::build(images)?));
    Analyzer::open(&image, profile)
}

#[expect(clippy::expect_used, reason = "every test needs the fixture to open")]
fn analyzer() -> Analyzer {
    open(&fixture::images(), DictionaryProfile::japanese_ipadic()).expect("the fixture opens")
}

#[test]
fn lemma_emission_and_drop_set() {
    let a = analyzer();
    // 走った keeps its lemma (the entry's own base form); は is a particle and drops.
    assert_eq!(a.analyze("走った").unwrap(), ["走った"]);
    assert_eq!(a.analyze("走ったは").unwrap(), ["走った"]);
    // An unknown ASCII word: the unknown feature has no lemma, so the surface is emitted.
    assert_eq!(a.analyze("abc").unwrap(), ["abc"]);
    // Every line is analyzed; blank lines yield nothing.
    assert_eq!(a.analyze("走る\n\n  \nabc").unwrap(), ["走る", "abc"]);
}

#[test]
fn lemma_column_placeholders_fall_back_to_the_surface() {
    let a = analyzer();
    // A lemma unlike the surface is emitted; `*` and an empty column fall back to the surface.
    assert_eq!(a.analyze("ネコ").unwrap(), ["猫"]);
    assert_eq!(a.analyze("猫").unwrap(), ["猫"]);
    assert_eq!(a.analyze("犬").unwrap(), ["犬"]);
}

#[test]
fn debug_names_the_profile() {
    let shown = format!("{:?}", analyzer());
    assert!(shown.starts_with("Analyzer {"), "{shown}");
    assert!(shown.contains("profile: DictionaryProfile"), "{shown}");
}

#[test]
fn morphemes_carry_features() {
    let morphemes = analyzer().morphemes("走ったは").unwrap();
    let pairs: Vec<(&str, &str)> = morphemes
        .iter()
        .map(|m| (m.surface(), m.feature()))
        .collect();
    assert_eq!(
        pairs,
        [
            (
                "走った",
                "動詞,自立,*,*,特殊・タ,基本形,走った,ハシッタ,ハシッタ"
            ),
            ("は", "助詞,係助詞,*,*,*,*,は,ハ,ワ"),
        ]
    );
}

#[test]
fn unknown_class_without_entries_falls_back() {
    // ひらがな has no unk.dic entry: the fallback node carries its class inline.
    let morphemes = analyzer().morphemes("ぬ").unwrap();
    let pairs: Vec<(&str, &str)> = morphemes
        .iter()
        .map(|m| (m.surface(), m.feature()))
        .collect();
    assert_eq!(pairs, [("ぬ", "未知語,Default")]);
}

#[expect(clippy::unwrap_used, reason = "the fixture analyzes every test input")]
fn surfaces(text: &str) -> Vec<String> {
    analyzer()
        .morphemes(text)
        .unwrap()
        .iter()
        .map(|m| String::from(m.surface()))
        .collect()
}

#[test]
fn unknown_words_follow_their_class_rules() {
    // SYMBOL spans up to two characters and does not group: `!?` is one word, and `!?#` splits
    // in two. Of its two equal-cost splits, EOS takes the predecessor added first: `?#` (from
    // position 1) before `#` (from position 2).
    assert_eq!(surfaces("!?"), ["!?"]);
    assert_eq!(surfaces("!?#"), ["!", "?#"]);
    // ALPHA groups: a run longer than its span limit is still one word.
    assert_eq!(surfaces("abcdefg"), ["abcdefg"]);
    // An unknown word starting at 1 is kept though はa (from 0) ends where it does.
    assert_eq!(surfaces("はa"), ["は", "a"]);
}

#[test]
fn the_default_line_limit_is_one_mebibyte() {
    assert_eq!(analyzer().profile().max_line_bytes(), 1 << 20);
}

#[test]
fn long_runs_cost_the_group_cap_per_position() {
    // 64 KiB of one grouped class: linear work, words of at most `unk_max_group_bytes`.
    let a = analyzer();
    let words = a.analyze(&"a".repeat(1 << 16)).unwrap();
    assert_eq!(words.len() * a.profile().unk_max_group_bytes(), 1 << 16);
}

#[test]
fn unterminated_last_feature_reads_to_the_end() {
    // 走る's feature is the last string; drop its NUL.
    let mut images = fixture::images();
    if let Some(last) = images
        .iter_mut()
        .find(|(name, _)| *name == "sys.dic")
        .and_then(|(_, sys)| sys.last_mut())
    {
        *last = b'!';
    }
    let a = open(&images, DictionaryProfile::japanese_ipadic()).unwrap();
    let morphemes = a.morphemes("走る").unwrap();
    let features: Vec<&str> = morphemes
        .iter()
        .map(morph_dict::Morpheme::feature)
        .collect();
    assert_eq!(
        features,
        ["動詞,自立,*,*,五段・ラ行,基本形,走る,ハシル,ハシル!"]
    );
}

#[test]
fn long_line_fails_closed() {
    let a = analyzer();
    let profile = a.profile().clone().with_max_line_bytes(9);
    let short = open(&fixture::images(), profile).unwrap();
    assert_eq!(
        short.analyze("走った走った"),
        Err(Error::LineTooLong { len: 18, max: 9 })
    );
    assert_eq!(short.analyze("走った\n走る").unwrap(), ["走った", "走る"]);
}

#[test]
fn missing_image_fails_closed() {
    let images: Vec<_> = fixture::images()
        .into_iter()
        .filter(|(name, _)| *name != "char.bin")
        .collect();
    assert_eq!(
        open(&images, DictionaryProfile::japanese_ipadic()).unwrap_err(),
        Error::Container(ContainerError::Missing { name: "char.bin" })
    );
}

#[test]
fn corrupt_images_fail_closed() {
    let corrupt = |name: &str, edit: fn(&mut Vec<u8>)| {
        let mut images = fixture::images();
        for (n, bytes) in &mut images {
            if *n == name {
                edit(bytes);
            }
        }
        open(&images, DictionaryProfile::japanese_ipadic()).unwrap_err()
    };
    assert_eq!(
        corrupt("sys.dic", |b| b[0] ^= 1),
        Error::Dictionary {
            image: ImageKind::SysDic,
            error: DictionaryError::MagicMismatch
        }
    );
    assert_eq!(
        corrupt("sys.dic", |b| b[4] = 7),
        Error::Dictionary {
            image: ImageKind::SysDic,
            error: DictionaryError::VersionMismatch { found: 7 }
        }
    );
    assert_eq!(
        corrupt("unk.dic", |b| b[8] = 0),
        Error::Dictionary {
            image: ImageKind::UnkDic,
            error: DictionaryError::NotUnknownDictionary { dict_type: 0 }
        }
    );
    assert!(matches!(
        corrupt("matrix.bin", |b| b.truncate(b.len() - 1)),
        Error::Dictionary {
            image: ImageKind::Matrix,
            error: DictionaryError::SizeMismatch { .. }
        }
    ));
    assert!(matches!(
        corrupt("char.bin", |b| b.truncate(3)),
        Error::Dictionary {
            image: ImageKind::CharBin,
            error: DictionaryError::TooSmall { len: 3 }
        }
    ));
    // Section sizes that run past the image.
    assert_eq!(
        corrupt("sys.dic", |b| b[32] = 0xff),
        Error::Dictionary {
            image: ImageKind::SysDic,
            error: DictionaryError::Truncated
        }
    );
}

/// Text over the fixture's alphabet plus characters it has never seen.
fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            Just('走'),
            Just('っ'),
            Just('た'),
            Just('る'),
            Just('は'),
            proptest::char::range('a', 'z'),
            any::<char>(),
            Just('\n'),
            Just(' '),
        ],
        0..64,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    #[test]
    fn analysis_is_total_and_deterministic(input in text()) {
        let a = analyzer();
        let first = a.analyze(&input).unwrap();
        prop_assert_eq!(&first, &a.analyze(&input).unwrap());
        // Every unit is a morpheme's surface or lemma, and morphemes tile each line.
        let morphemes = a.morphemes(&input).unwrap();
        let tiled: String = morphemes.iter().map(morph_dict::Morpheme::surface).collect();
        let lines: String = input.lines().filter(|l| !l.trim().is_empty()).collect();
        prop_assert_eq!(tiled, lines);
    }

    #[test]
    fn analysis_is_idempotent_over_its_output(input in text()) {
        let a = analyzer();
        let units = a.analyze(&input).unwrap();
        prop_assert_eq!(a.analyze(&units.join(" ")).unwrap(), units);
    }

    #[test]
    fn container_round_trips(entries in proptest::collection::btree_map("[a-z]{1,8}", proptest::collection::vec(any::<u8>(), 0..64), 0..8)) {
        let images: Vec<(&str, &[u8])> = entries.iter().map(|(k, v)| (k.as_str(), v.as_slice())).collect();
        let image = HeapImage::from_vec(container::build(&images).unwrap());
        let table = container::validate(&image).unwrap();
        prop_assert_eq!(table.len(), images.len());
        for (entry, (name, bytes)) in table.iter().zip(&images) {
            prop_assert_eq!(entry.name(), *name);
            let mut read = vec![0u8; bytes.len()];
            image.read_exact_at(entry.offset(), &mut read).unwrap();
            prop_assert_eq!(&read, bytes);
            prop_assert_eq!(container::attest(&image, entry).unwrap(), *entry.sha256());
        }
    }

    #[test]
    fn arbitrary_containers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let image: Arc<dyn ByteImage> = Arc::new(HeapImage::from_vec(bytes));
        let _outcome = Analyzer::open(&image, DictionaryProfile::japanese_ipadic());
    }

    #[test]
    fn corrupted_dictionaries_never_panic(
        flips in proptest::collection::vec((any::<proptest::sample::Index>(), any::<u8>()), 1..16),
        input in text(),
    ) {
        let mut bytes = container::build(&fixture::images()).unwrap();
        for (at, value) in flips {
            let at = at.index(bytes.len());
            bytes[at] = value;
        }
        let image: Arc<dyn ByteImage> = Arc::new(HeapImage::from_vec(bytes));
        if let Ok(a) = Analyzer::open(&image, DictionaryProfile::japanese_ipadic()) {
            let _outcome = a.analyze(&input);
        }
    }
}
