# Working on Hortus

## Build and check

Use rustup stable. On Apple Silicon, add the toolchain to `PATH` if needed:

```sh
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./tools/build-macos.sh
./tools/test-macos.sh
python3 -B tools/check-public-tree.py
```

The macOS app requires Apple Silicon, macOS 27.0, Xcode 27.1 and Swift 6.4. Build with warnings treated as errors. The app is built at `build/Hortus.app`. Regression tools accept `--output`; their defaults are under `target/reports/`.

## Code and dependencies

- Keep changes focused and follow the surrounding code style. Existing comments are in Chinese and will be translated to English together later; until then, match the language of the file being edited.
- Pin external dependencies to exact versions (`=x.y.z` for Cargo).
- Dependencies in `crates/` must use permissive licenses such as MIT, Apache-2.0 or BSD; MPL-2.0 is also allowed. Dependencies in `apps/` may use GPL-compatible licenses.
- `crates/` and `tools/` use MIT OR Apache-2.0; `apps/` uses GPL-3.0-only. Preserve the CC0 notices on sample notes. Contributions use the license of the directory they change; no CLA is required.

## Product principles

- Note folders contain user content only. Write there only when saving the user's notes, never for application state, caches, indexes or leftover temporary files. If an entire external volume is the vault, the system replacement directory `.TemporaryItems/…/NSIRD_*` may exist briefly during a save and must be cleaned afterward.
- Store configuration in `~/.config/hortus/`; store indexes, sessions, caches and recovery drafts in the system cache directory (`~/Library/Caches/Hortus/` on macOS).
- Native text controls own key handling, input methods, the cursor and undo. The Rust core receives edits and returns semantic structure.
- Rust lays out Mermaid diagrams; each platform only draws them. Layout results must match across platforms and the CLI.
- Keep the interface restrained: few color blocks, borders or lines. Establish hierarchy with color, weight and indentation.
- The whole application must avoid WebView. Do not link WebKit or JavaScriptCore, import HTML through `NSAttributedString`, or bundle HTML or JavaScript assets.
- No runtime networking or telemetry.

## Local samples

Private local notes may be available in the ignored `fixtures/mermaid/tree/` directory. Tests that require them must print why they are skipped when the samples are absent. Never copy their text into committed files or snapshots; assert numeric and structural properties only. Do not commit generated private sample artifacts, build products, credentials or machine-specific settings.
