# Quire native

The Rust/GPUI rewrite is underway. The current executable is a native text-input harness adapted from Zed's GPUI input example. Notebook editing, storage, and the full editor/Vim port are still being implemented through [the task queue](plans/native-rewrite-tasks.md).

Use Rust 1.98.1, Bun, and Node 26 for development. macOS builds require Xcode with the Metal tools. GPUI and its platform/support crates are copied under `vendor/zed`; building does not require either sibling checkout.

```sh
bun install --frozen-lockfile
bun run test:reference
cargo test -p quire --locked --features test-support
cargo build -p quire --locked
bun run dev
```

`bun run dev` requires a regular desktop terminal. The macOS Codex sandbox denies desktop services, including the pasteboard; the launcher detects that environment and explains how to proceed. Headless GPUI tests still run there. In this sandbox, build commands use `CARGO_HOME=/private/tmp/quire-cargo` because the shared Cargo cache is read-only.

Reference protocol tests deliberately run through Node/Vitest. Bun's zstd decoder reports a truncated frame differently, so running those tests with `bun test` would change the compatibility oracle.

Generate synthetic notebook fixtures with `bun tools/synthetic-notebook.ts <empty-folder> [pages] [seed]`. Defaults are 6,536 pages and seed 1. Generated notebooks and build output stay outside version control.

The application uses GPL-3.0-or-later; retain the licenses and font notices in `vendor/zed`. Copied components and source revisions are recorded in `vendor/zed/UPSTREAM.json` and `tests/reference/UPSTREAM.json`. Application input code derives from `crates/gpui/examples/input.rs` at that Zed revision. Its only current functional adaptation is the Quire window title.
