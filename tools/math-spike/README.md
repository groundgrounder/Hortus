# Native math rendering experiment

Standalone evaluation tools for RaTeX 0.1.15. Neither Cargo project participates
in the Hortus workspace. Product code and application dependencies are unchanged.
The four core crates and pulldown-cmark are pinned to exact versions; the separate
`reference/` project uses RaTeX's PNG renderer solely as a visual control.

Requires rustup stable and, for native PNGs, macOS with the Swift toolchain.
Run builds and measurements outside a filesystem sandbox. Do not run other builds
or workloads during `bench`. The Rust and Swift tools use no network at runtime;
Cargo downloads the locked dependencies when initially building.

```sh
cd tools/math-spike
cargo build --release --locked
cargo build --release --locked --manifest-path reference/Cargo.toml
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
cargo fmt --manifest-path reference/Cargo.toml --all --check
cargo clippy --manifest-path reference/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path reference/Cargo.toml --locked
```

Supply a UTF-8 sample file with one formula per line and `# group name` headings.
An empty line is an intentional empty formula. Fixtures and measured reports are
supplied separately; this directory contains reusable source only. Replace the
example paths with your input files. All output paths can be overridden.

```sh
python3 run.py collect --samples /path/to/samples.txt --markdown /path/to/boundaries.md --output target/reports
python3 run.py bench --samples /path/to/samples.txt --output target/reports
python3 run.py render --samples /path/to/samples.txt --output target/reports
target/release/math-spike once text '\frac{a}{b}'
target/release/math-spike markdown /path/to/boundaries.md target/reports/markdown-events.json
```

`collect` must precede `bench` and `render`, using the same output directory.
`bench` measures 50 rounds of each sample in Text and Display styles, then a
550-formula note without caching. It also starts three fresh processes for every
sample/style pair. Timed regions include parse, layout, validation and conversion
to DisplayList, and exclude process spawning, JSON, file IO and drawing. Fresh
processes measure the first library call, not disk-cold startup. Median is the
upper middle sorted value; P95 uses nearest rank. Errors have separate summaries.

`render` compiles Swift with warnings as errors, finds the KaTeX TTF directory
through Cargo metadata, records font hashes/licenses, and generates 20 formulas
in two themes, reference PNGs, four contact sheets, two extra drawing cases,
CJK fallback comparisons and a baseline comparison. Rendering uses 15 pt/em,
3 device pixels/pt, top-left coordinates and baseline=`height`. Source JSON and
actual font/glyph audits accompany native PNGs. Unicode-to-TTF cmap translation
uses `ratex_font::katex_ttf_glyph_char`; using raw mathematical codepoints can
silently select the wrong KaTeX glyph. CJK metrics come from Core Text and enter
the existing `layout_with_fallback_metrics` API. This is a drawing experiment,
not an implementation of TextKit attachments, selection or accessibility.

`size-ratex` exercises only the four core crates. Compare its stripped release
binary size with `size-empty`; this is an approximate linked-binary increment,
not an application package estimate. `math-spike` also includes JSON/Markdown.
Do not include the reference renderer's dependencies in the core size estimate.

Optional comparisons need Node 22+ (built-in WebSocket) and separately installed
KaTeX or Obsidian. These are tooling, never bundled application assets. KaTeX's
mhchem extension is loaded; comparison records success/error status rather than
visual or semantic equivalence. No npm dependencies are added to this project.

```sh
node compare-katex.mjs --results target/reports/results.jsonl --katex /path/to/node_modules/katex --output target/reports/katex-compatibility.json
node compare-obsidian.mjs --fixture /path/to/boundaries.md --output target/reports/obsidian --app /Applications/Obsidian.app --port 19347
```

The Obsidian collector creates an isolated vault/profile in the system temporary
directory, opens reading mode over localhost CDP, captures DOM and a screenshot,
and terminates only its own process. It does not use the current vault or profile.
The snapshot includes the full preview HTML; use only public, self-authored
fixtures. Obsidian may run its own update checks in the disposable profile. Obsidian must support a second instance with `--user-data-dir`.

Core crates are MIT; this tooling is MIT OR Apache-2.0. KaTeX font files use
SIL OFL 1.1. `cargo-tree.txt`/`dependencies.json` and their reference equivalents
record all resolved packages and declared licenses, including target branches.
Fonts and build outputs are not committed. This directory is a temporary
experiment and does not establish the application's future math syntax or API.
