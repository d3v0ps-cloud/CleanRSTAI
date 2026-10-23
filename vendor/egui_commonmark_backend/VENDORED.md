# Vendored egui_commonmark_backend 0.26.0

A copy of [egui_commonmark_backend](https://crates.io/crates/egui_commonmark_backend) 0.26.0
(MIT OR Apache-2.0, see the license files), used through `[patch.crates-io]` in the root
`Cargo.toml`. Local changes are marked with `CleanRSTAI:` comments:

- `src/misc.rs`, `Style::to_richtext`: bold text and heading levels 1–4 also set the `wght`
  variation axis to 600, because egui's `RichText::strong()` only changes the colour.
- `src/elements.rs`, `code_block`: the copy button is placed inside the block's top-right
  corner, the text gets a right margin so it never runs under the button, and the button uses
  the app's Phosphor copy/check glyphs.

When upgrading `egui_commonmark`, copy the matching backend version from the registry and
re-apply these changes (or drop the patch if upstream supports them).
