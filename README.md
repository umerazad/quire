# Quire native

The Rust/GPUI rewrite is underway. The default executable now hosts Zed's full editor and Vim in two panes sharing one buffer, with a mode indicator. It opens an in-memory example note using the copied Markdown and inline grammars. Insert mode, multiline typing, line deletion, shared undo/redo, and normal editing/clipboard operations have headless tests. Notebook persistence, Write presentation, and the remaining application workflows are still in [the task queue](plans/native-rewrite-tasks.md).

Each pane has local Find and Replace. Use Cmd-F / Cmd-Option-F on macOS, or Ctrl-F / Ctrl-H elsewhere. Enter and Shift-Enter navigate matches; Escape returns to the editor. In the replacement field, Enter replaces one match and Cmd-Enter / Ctrl-Enter replaces all. Replacements share the document's undo history. Vim supports `/`, `?`, `n`/`N`, word-under-cursor search, counts, cancellation, and search motions with operators.

Use `:` for editor-local commands, including line jumps, visual/count ranges, substitution, settings, delete, and sort. For example, `:%s/old/new/g` replaces throughout the note. Errors stay in the prompt for correction; Escape cancels. Up/Down recall command history by prefix and restore unfinished input. History stays in memory for the application session. Named registers work across panes and survive toggling Vim. Save and AI commands still need their notebook-service integrations.

Use Rust 1.98.1, Bun, and Node 26 for development. macOS builds require Xcode with the Metal tools. GPUI and its platform/support crates are copied under `vendor/zed`; building does not require either sibling checkout.

```sh
bun install --frozen-lockfile
bun run verify:vendor
bun run test:reference
cargo test -p quire --locked --features test-support
bun run test:vim-search
bun run test:vim-command
bun run test:vim-mark
cargo test -p quire_store --locked
bun run test:storage-crash
cargo build -p quire --locked
bun run dev
```

`bun run dev` requires a regular desktop terminal. The macOS Codex sandbox denies desktop services, including the pasteboard; the launcher detects that environment and explains how to proceed. Headless GPUI tests still run there. In this sandbox, build commands use `CARGO_HOME=/private/tmp/quire-cargo` because the shared Cargo cache is read-only.

The native development app uses `QuireNative` data/config paths, separate from Zed and the Electron app. The earlier input harness is available through `cargo run -p quire -- --input-harness`. The current debug executable produces a macOS linker warning about large unwind metadata; release performance and native desktop acceptance remain unverified.

Run `cargo run -p quire --locked -- --excerpt-harness` in a regular desktop terminal to inspect the editable-excerpt spike. The right pane shows the first nested block from the left pane's document, with shared undo/redo. This is a source excerpt; Write rendering and hidden-identity protection remain unfinished.

Reference protocol tests deliberately run through Node/Vitest. Bun's zstd decoder reports a truncated frame differently, so running those tests with `bun test` would change the compatibility oracle.

The `test:vim-search`, `test:vim-command`, and `test:vim-mark` scripts run copied regressions, including comparisons with Neovim. They require `nvim` on PATH and enable Quire's embedded-asset policy so tests load the copied settings and queries.

`quire_store` contains the format 1 codec. Its tests read Node-generated files, ask Node to read native files, and compare seeded number and Unicode serialization. The sync JSON representation preserves unknown fields and unpaired UTF-16 surrogates. Its parsing, cloning, serialization, and destruction handle deep records without recursive traversal. Regenerate protocol fixtures with `node tools/protocol-vectors.ts`.

The storage library reuses Zed's `sqlez` with a fallible file-open path, WAL/FULL, FTS5, and macOS fullfsync. SQLite is pinned to bundled 3.54.1. Notebook identity/schema checks protect existing databases. Saves commit revisions, heads, outbox entries, and matching recovery changes together; publication selection freezes exact bytes before folder I/O. A bounded worker keeps SQLite off its caller's thread. The attachment cache verifies content hashes and synchronizes files before registering them. Tests cover full-disk failure, rollback, stale editor versions, and process-kill recovery. Windows attachment synchronization, folder publication, and connecting this service to the editor remain unfinished. Process-kill checks do not establish power-loss durability.

Generate synthetic notebook fixtures with `bun tools/synthetic-notebook.ts <empty-folder> [pages] [seed]`. Defaults are 6,536 pages and seed 1. Generated notebooks and build output stay outside version control.

Before committing copied sources, run `bun tools/verify-vendor.ts --tracked` to catch ignored build inputs. During vendoring, `bun tools/verify-upstream.ts ../zed` compares unmodified files with the pinned checkout; the eleven documented fork changes need separate diff review. This comparison is a development audit, not a build dependency. Upstream prompt fixtures intentionally contain conflict examples and whitespace, so whitespace checks apply to authored files and the documented vendor modifications while the remaining copied files retain their exact bytes.

The application uses GPL-3.0-or-later; retain the licenses and font notices in `vendor/zed`. Copied components and source revisions are recorded in `vendor/zed/UPSTREAM.json` and `tests/reference/UPSTREAM.json`. The source snapshot includes the full editor's supporting crates and assets. Quire loads editor/Vim key contexts strictly and omits bindings for unrelated Zed product panels. The earlier input harness derives from `crates/gpui/examples/input.rs` at that revision.
