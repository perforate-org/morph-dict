# Gates for `morph-dict`.

crate := "morph-dict"

# Every gate; CI and pre-commit run this.
gate: fmt clippy test doc deslop smells

fmt:
    cargo fmt --all --check

clippy:
    cargo clippy -p {{crate}} --all-targets --all-features -- -D warnings

# The modules' documentation is the specification: every link in it must resolve.
doc:
    RUSTDOCFLAGS="-D warnings" cargo doc -p {{crate}} --all-features --document-private-items --no-deps

# Architecture: modules depend one way, stay deep, and keep their fields to themselves
# (deslop-please: `cargo install deslop-please`).
deslop:
    deslop-please {{crate}} --all-features

# Includes the public API snapshot (`tests/public_api.rs`) and, when `MORPH_DICT_IPADIC` names a
# compiled ipadic directory, the real-dictionary smoke test (`just ipadic` fetches one).
test:
    cargo test -p {{crate}} --all-features

# Proofs over every input of the parsers (`#[cfg(kani)] mod proofs`). Needs Kani
# (`cargo install --locked kani-verifier && cargo kani setup`).
kani:
    cargo kani -p {{crate}}

# Every surviving mutant is a test the suite is missing. Set `MORPH_DICT_IPADIC` first: the
# real-dictionary golden file is what catches the lattice's and Viterbi's mutants.
# A timed-out mutant hung the suite, so the tests noticed it: only a missed one (exit 2) fails.
mutants:
    #!/usr/bin/env bash
    cargo mutants -p {{crate}} --test-tool cargo || [ $? -eq 3 ]

# Duplication and complexity (thresholds in `.qlty/qlty.toml`). `qlty smells` exits 0 even with
# findings, so the gate counts the SARIF results and prints them before failing.
smells:
    #!/usr/bin/env bash
    set -euo pipefail
    count=$(qlty smells --all --sarif --no-upgrade-check | jq '[.runs[].results[]] | length')
    if [ "$count" -ne 0 ]; then
        qlty smells --all --no-snippets --no-upgrade-check
        echo "qlty: $count smell(s)" >&2
        exit 1
    fi

# Fetches the compiled mecab-ipadic 2.7.0 (PyPI `ipadic` 1.0.0) into `crates/morph-dict/tests/dict`.
ipadic:
    #!/usr/bin/env bash
    set -euo pipefail
    dir=crates/morph-dict/tests/dict
    mkdir -p "$dir"
    curl -fsSL https://files.pythonhosted.org/packages/source/i/ipadic/ipadic-1.0.0.tar.gz \
        | tar xz -C "$dir" --strip-components=3 ipadic-1.0.0/ipadic/dicdir
    echo "export MORPH_DICT_IPADIC=$PWD/$dir"

# Use the repository's git hooks (`.githooks`): format and lints before a commit, the gate before a
# push. Once per clone.
hooks:
    git config core.hooksPath .githooks

# Regenerate `tests/public-api.txt` after a deliberate API change; review the diff.
bless-api:
    UPDATE_EXPECT=1 cargo test -p {{crate}} --test public_api
