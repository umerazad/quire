# Quire native

The Rust/GPUI rewrite is underway. The default executable now hosts Zed's full editor and Vim in two panes sharing one buffer, with a mode indicator. It opens an in-memory example note. Insert mode, multiline typing, line deletion, shared undo/redo, and normal editing/clipboard operations have headless tests. Notebook persistence, Write presentation, and the remaining application workflows are still in [the task queue](plans/native-rewrite-tasks.md).

Use Rust 1.98.1, Bun, and Node 26 for development. macOS builds require Xcode with the Metal tools. GPUI and its platform/support crates are copied under `vendor/zed`; building does not require either sibling checkout.

```sh
bun install --frozen-lockfile
bun run verify:vendor
bun run test:reference
cargo test -p quire --locked --features test-support
cargo build -p quire --locked
bun run dev
```

`bun run dev` requires a regular desktop terminal. The macOS Codex sandbox denies desktop services, including the pasteboard; the launcher detects that environment and explains how to proceed. Headless GPUI tests still run there. In this sandbox, build commands use `CARGO_HOME=/private/tmp/quire-cargo` because the shared Cargo cache is read-only.

The native development app uses `QuireNative` data/config paths, separate from Zed and the Electron app. The earlier input harness is available through `cargo run -p quire -- --input-harness`. The current debug executable produces a macOS linker warning about large unwind metadata; release performance and native desktop acceptance remain unverified.

Reference protocol tests deliberately run through Node/Vitest. Bun's zstd decoder reports a truncated frame differently, so running those tests with `bun test` would change the compatibility oracle.

Generate synthetic notebook fixtures with `bun tools/synthetic-notebook.ts <empty-folder> [pages] [seed]`. Defaults are 6,536 pages and seed 1. Generated notebooks and build output stay outside version control.

Before committing copied sources, run `bun tools/verify-vendor.ts --tracked` to catch ignored build inputs. During vendoring, `bun tools/verify-upstream.ts ../zed` compares unmodified files with the pinned checkout; the two documented fork changes need separate diff review. This comparison is a development audit, not a build dependency. Upstream prompt fixtures intentionally contain conflict examples and whitespace, so whitespace checks apply to authored files and the documented vendor modifications while the remaining copied files retain their exact bytes.

The application uses GPL-3.0-or-later; retain the licenses and font notices in `vendor/zed`. Copied components and source revisions are recorded in `vendor/zed/UPSTREAM.json` and `tests/reference/UPSTREAM.json`. The source snapshot includes the full editor's supporting crates and assets. Quire loads editor/Vim key contexts strictly and omits bindings for unrelated Zed product panels. The earlier input harness derives from `crates/gpui/examples/input.rs` at that revision.
