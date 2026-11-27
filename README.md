# morph-dict

MeCab-format morphological analysis over dictionaries read through an offset accessor. A
dictionary is four images (`sys.dic`, `unk.dic`, `matrix.bin`, `char.bin`) packed into one
container; the analyzer reads them through `ByteImage` instead of a contiguous slice, so the same
code runs over a heap copy, a file or paged storage such as Internet Computer stable memory. It
keeps two promises:

1. **Bounded residency.** The hot random-access regions (connection matrix, character table,
   unknown dictionary, and the system dictionary's trie and word parameters) are copied to the
   heap once at open. Feature strings stay behind the image and are read only for the morphemes
   of the chosen path.
2. **No panics.** Every read, slice and sum is checked. A malformed container or dictionary is an
   `Error`, and so is a line longer than the profile allows; analysis stays linear in the line.

What varies by language is the feature layout and the parts of speech an index drops;
`DictionaryProfile` carries that (`japanese_ipadic`, `korean_mecab_ko_dic`). `normalize` holds the
text folding that index and query share. The crate is `no_std` + `alloc`.

## Use

```rust
use std::sync::Arc;

use morph_dict::{Analyzer, ByteImage, DictionaryProfile, HeapImage, container, normalize};

// Pack the compiled dictionary once (at build time, or when it is uploaded).
let images: Vec<(&str, Vec<u8>)> = ["sys.dic", "unk.dic", "matrix.bin", "char.bin"]
    .into_iter()
    .map(|name| (name, std::fs::read(format!("{dir}/{name}")).unwrap()))
    .collect();
let packed = container::build(&images)?;

// Open it from wherever it lives; any `ByteImage` will do.
let image: Arc<dyn ByteImage> = Arc::new(HeapImage::from_vec(packed));
let analyzer = Analyzer::open(&image, DictionaryProfile::japanese_ipadic())?;

assert_eq!(
    analyzer.analyze(&normalize::prepass("東京都に住んでいる"))?,
    ["東京", "都", "住む", "いる"],
);
for morpheme in analyzer.morphemes("走った")? {
    println!("{}\t{}", morpheme.surface(), morpheme.feature());
}
```

`container::validate` checks a container's table without opening it, and `container::attest`
recomputes an entry's SHA-256 against the one recorded at build time.

## Where things are written down

The code is the one source of truth. Each module's documentation (`cargo doc
--document-private-items`) draws its byte layout and gives the reasons for its choices;
`crates/morph-dict/tests/public-api.txt` pins the public API, and
`crates/morph-dict/tests/ipadic-golden.txt` pins the analysis of the real mecab-ipadic, morpheme
by morpheme.

## Development

`just gate` runs fmt, clippy, tests, docs, deslop-please and qlty; `just kani` the proofs and
`just mutants` mutation testing. `just ipadic` fetches the compiled mecab-ipadic and prints the
`MORPH_DICT_IPADIC` export the real-dictionary tests read; without it they pass vacuously.

## Origin and license

The engine derives from [MeCrab](https://github.com/cool-japan/mecrab) (MIT OR Apache-2.0); see
`crates/morph-dict/LICENSE-MECRAB`. Compiled mecab-ipadic dictionaries carry the acknowledgment
in `crates/morph-dict/LICENSE-IPADIC`.

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
