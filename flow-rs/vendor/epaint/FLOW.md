# epaint 0.37.0, patched

A copy of [epaint 0.37.0](https://crates.io/crates/epaint/0.37.0) (MIT OR
Apache-2.0), used through `[patch.crates-io]` in the workspace `Cargo.toml`.

One change, in `src/text/text_layout.rs` (`line_break`): a whitespace glyph
never triggers a row break. CSS lets trailing spaces hang past the line box, so
Chromium keeps a word on its line when only the space after it overflows; stock
epaint moved the word down, and settings text wrapped one word earlier than the
Electron app. Search for `Flow:` to find it.

To move to a newer egui, copy the new epaint over this folder and reapply the
change (or drop the patch if upstream lets trailing whitespace hang).
