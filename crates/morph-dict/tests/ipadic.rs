//! Smoke test over the real mecab-ipadic, when it is present.
//!
//! Set `MORPH_DICT_IPADIC` to a directory holding the compiled `sys.dic`, `unk.dic`,
//! `matrix.bin` and `char.bin` (for example the `dicdir` of `PyPI`'s `ipadic` 1.0.0); without it
//! the test passes vacuously. CI fetches the dictionary and sets the variable.

use morph_dict::{Analyzer, ByteImage, DictionaryProfile, HeapImage, container, normalize};
extern crate alloc;

use alloc::sync::Arc;

/// The analyzer over the dictionary `MORPH_DICT_IPADIC` names, or `None` when it is unset.
#[expect(
    clippy::unwrap_used,
    reason = "a named dictionary that fails to open fails the test"
)]
fn ipadic() -> Option<Analyzer> {
    let Ok(dir) = std::env::var("MORPH_DICT_IPADIC") else {
        eprintln!("MORPH_DICT_IPADIC unset: skipped");
        return None;
    };
    let images: Vec<(&str, Vec<u8>)> = ["sys.dic", "unk.dic", "matrix.bin", "char.bin"]
        .into_iter()
        .map(|name| (name, std::fs::read(format!("{dir}/{name}")).unwrap()))
        .collect();
    let image: Arc<dyn ByteImage> =
        Arc::new(HeapImage::from_vec(container::build(&images).unwrap()));
    Some(Analyzer::open(&image, DictionaryProfile::japanese_ipadic()).unwrap())
}

#[test]
fn ipadic_segments_and_lemmatizes() {
    let Some(analyzer) = ipadic() else {
        return;
    };
    let analyze = |text: &str| analyzer.analyze(&normalize::prepass(text)).unwrap();
    assert_eq!(
        analyze("東京都に住んでいる"),
        ["東京", "都", "住む", "いる"]
    );
    assert_eq!(
        analyze("関ヶ原の戦いは1600年"),
        ["関ヶ原", "戦い", "1600", "年"]
    );
    assert_eq!(
        analyze("Rustで書かれたデータベース"),
        ["rust", "書く", "れる", "データベース"]
    );
    assert_eq!(analyze("走った"), ["走る"]);
}

/// Every class of `char.def` and the bounds on unknown runs, morpheme by morpheme. The golden
/// file was produced by the MeCrab-derived engine this crate replaced (the two agree on 211,810
/// inputs); a deliberate change regenerates it with `UPDATE_EXPECT=1`.
#[test]
fn ipadic_morphemes_match_the_golden_file() {
    let Some(analyzer) = ipadic() else {
        return;
    };
    let mut out = String::new();
    for line in include_str!("ipadic-lines.txt").lines() {
        out.push_str("> ");
        out.push_str(line);
        out.push('\n');
        for morpheme in analyzer.morphemes(line).unwrap() {
            out.push_str(morpheme.surface());
            out.push('\t');
            out.push_str(morpheme.feature());
            out.push('\n');
        }
        out.push_str("= ");
        out.push_str(
            &analyzer
                .analyze(&normalize::prepass(line))
                .unwrap()
                .join(" "),
        );
        out.push('\n');
    }
    expect_test::expect_file!["ipadic-golden.txt"].assert_eq(&out);
}
