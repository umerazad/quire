# Quire native

The Rust/GPUI rewrite is underway. The default executable hosts Zed's editor and Vim in two panes sharing an in-memory example note. Each pane can switch between Source and Write presentation using the copied Markdown and inline grammars. Editing, search, shared undo, projections, and semantic excerpts have headless tests. Persistent editing, daily-page navigation, and the remaining application workflows are still in [the task queue](plans/native-rewrite-tasks.md).

Open a stored page preview with its native database and original identities:

```sh
cargo run -p quire --locked -- \
  --notebook-db /path/to/local.sqlite \
  --notebook-id notebook-id --writer-id writer-id --page-id page-id
```

The preview loads one selected revision and optional recovery through the storage worker. Concurrent opens within a notebook session share one buffer and analysis model. Recovery takes precedence in the preview, while the saved revision and recovery record remain in storage. Conflicts and deleted versions appear in its status. Unrepresentable UTF-16 strings produce an actionable error without creating a lossy buffer. A failed page read can be retried after its cause is corrected; startup failures require reopening the notebook.

Stored-page previews are read-only until recovery, save, and close protection are connected. Opening a database still performs the normal native schema validation and upgrades. This path does not open or convert a legacy Markdown folder. It reports loaded revision-body counts and time to buffer readiness; those figures do not establish first-editable or release performance acceptance.

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

Run `cargo run -p quire --locked -- --excerpt-harness` in a regular desktop terminal to inspect semantic excerpts. The right pane follows block `b-welcome` and its descendants from the left pane's document. Adding, deleting, or reparenting children updates the same view, with shared undo/redo. Missing or duplicated IDs disable editing and show how to restore the block in Source. It starts in Source; its mode control can enable the current Write projection. Complete Write rendering and notebook embed workflows remain unfinished.

`cargo run -p quire --locked -- --write-harness` opens the example note with list markers and block IDs hidden in the right pane. Both panes edit the same source. Projection spans come from the current Markdown parse and update after edits, including newly inserted blocks. Partial delete/paste, clearing and retyping, delete/undo, stale parses, and subtree mapping have regression tests. Vim line changes preserve block and task prefixes plus IDs, while line deletion removes the whole source line even at EOF; undo works from the Source pane. Empty list/task items at EOF have differential parser coverage. The remaining structural editing cases are still pending.

Each pane's Source/Write button switches its presentation. Cmd-Shift-M on macOS, or Ctrl-Shift-M elsewhere, switches the focused editor. Source exposes the original syntax for direct editing. Switching keeps the same buffer and shared undo history, retains user folds, and rebuilds Write spans from the current parse.

Write renders square task markers and Roam TODO/DONE markers as native checkboxes. Click or focus a checkbox and press Enter/Space to toggle its marker; Escape returns to the editor. This developer view uses the legacy editor-only fallback. Completion dates, task state tags, and notebook task workflows still require the planned task service. A callback from an obsolete widget cannot toggle text that has become code.

Plain-label inline HTTP/HTTPS links use underlined source text with native wrapping and selection. Pointer or Enter/Space activation requests the URL without editing the note. Focusing the editor in the containing block reveals the link syntax for editing; Source exposes all syntax. Rich labels, images, reference links, and other destinations currently retain their source presentation. Internal notebook navigation remains unfinished. Headless tests intercept URL requests; native browser and accessibility acceptance remain open.

Narrow-wrap tests cover visible Unicode positions, hidden marker/ID boundaries, long link labels, and typing before a hidden ID. Partial-label copy preserves the selected characters. Select All/copy includes the complete source, and paste or typing replaces it, including hidden syntax. Bounded excerpt replacements preserve surrounding source and share page undo. Native desktop pointer selection still needs acceptance checks.

Reference protocol tests deliberately run through Node/Vitest. Bun's zstd decoder reports a truncated frame differently, so running those tests with `bun test` would change the compatibility oracle.

The development oracle also compares native block boundaries and hidden spans with 49 synthetic vectors from Quire's copied legacy Markdown parser. Regenerate them with `bun tools/markdown-vectors.ts`. The pinned Lezer packages are development dependencies; the native app uses the copied Markdown grammar and requires no JavaScript parser at runtime. These fixtures cover specific cases rather than establishing complete Markdown or Write parity.

`quire_core` provides shared page-name validation, logical relative-link paths, and frontmatter boundaries. Its metadata tests compare 640 fixtures from the frozen TypeScript module; regenerate them with `bun tools/metadata-vectors.ts`. Native boundaries use UTF-8 byte offsets and are compared explicitly with the oracle's UTF-16 offsets. The editor scans frontmatter through borrowed rope characters, stopping at the delimiter without allocating a page or line string. New-page names follow the legacy trimming, NFC, length, and reserved-name rules; existing paths and text remain untouched. Logical link resolution decodes URI escapes and rejects traversal above the vault, while filesystem adapters still need to enforce notebook boundaries. Page creation and notebook navigation have yet to use these helpers.

The core's `page_title` and `frontmatter_tags` match 725 additional fixtures from the frozen module. Regenerate them with `bun tools/frontmatter-vectors.ts`; Bun and Node produce identical fixture bytes. Titles retain JSON number/array/object behavior, raw quoting, fallback filenames, and lone-surrogate escapes. `page_title` returns `JsString`, exposing UTF-16 units and an optional native UTF-8 view instead of replacing unrepresentable characters. Tags preserve the legacy inline/list forms, order, and duplicates. These helpers parse data without executing notebook content; notebook title/tag UI integration remains pending.

`references(text, code_ranges)` extracts native page, block, embed, and PDF reference records with byte ranges. Callers must supply code exclusions from their current Markdown parse; the helper adds frontmatter exclusion. It preserves raw targets/labels, PDF precedence, first-match behavior, and the legacy check of each match's start. Exclusion ranges are sorted and merged once for lookup. Tests compare 559 raw/editor fixture pairs with explicit UTF-16 offset translation; regenerate them with `bun tools/reference-vectors.ts`. Node and Bun generate identical fixture bytes. Native grammar-derived exclusions and notebook navigation remain separate work.

The `test:vim-search`, `test:vim-command`, and `test:vim-mark` scripts run copied regressions, including comparisons with Neovim. They require `nvim` on PATH and enable Quire's embedded-asset policy so tests load the copied settings and queries.

`quire_store` contains the format 1 codec. Its tests read Node-generated files, ask Node to read native files, and compare seeded number and Unicode serialization. The shared `quire_core::json` representation preserves unknown fields and unpaired UTF-16 surrogates; storage retains its existing `quire_store::sync::json` import path through a re-export. Parsing, cloning, serialization, and destruction handle deep records without recursive traversal. `JsString::as_bytes` returns WTF-8, while `as_str` returns a native UTF-8 string only when the value is representable. Regenerate protocol fixtures with `node tools/protocol-vectors.ts`.

The storage library reuses Zed's `sqlez` with a fallible file-open path, WAL/FULL, FTS5, and macOS fullfsync. SQLite is pinned to bundled 3.54.1. Notebook identity/schema checks protect existing databases. Saves commit revisions, heads, outbox entries, and matching recovery changes together. A bounded worker keeps SQLite and publication I/O off its caller's thread. The attachment cache verifies content hashes and synchronizes files before registering them. Tests cover full-disk failure, rollback, stale editor versions, and process-kill recovery.

`NotebookStore::publish_to` publishes one frozen batch to an existing format 1 notebook folder. It delivers verified attachments first, preserves immutable files, and adopts a matching pending batch after interruption. Conflicting writer sequences, unexpected own batches, changed notebook identities, and incomplete files leave local revisions pending. Publication acknowledges the local folder; peer delivery and power-loss durability need separate acceptance checks.

`NotebookStore::import_batch` applies an incoming graph and cursor atomically, retaining concurrent heads for explicit merging. Missing parents and sequence gaps wait; malformed graphs and forks preserve local data. Imported wire records retain unknown fields and lone surrogates. Own frozen publications remain with the publisher for durable acknowledgment. Both operations run through the bounded worker.

`NotebookStore::rescan` discovers delivered device chains, retries parent dependencies, and streams requested attachments into the verified local cache. Count and payload budgets return continuations across devices and attachment names; one oversized item can proceed alone in each phase. Partial delivery and writer forks leave source files unchanged. Rescans can adopt an observed frozen publication while retaining later local saves. Batches commit independently, so a later storage error can leave an imported prefix; callers must refresh mounted pages after errors as well as successful reports. Directory discovery currently enumerates filename metadata. The full mixed-runtime convergence schedules, checkpoints/status, watcher lifecycle, Windows synchronization, and editor integration remain unfinished.

Schema 2 retains attachment requests and known batch hashes across restart. Requests clear with a verified attachment's catalog commit; historical hashes detect older writer forks. Schema 1 upgrades atomically while preserving drafts, local revisions, pending publication, and the attachment catalog. The one-time upgrade reads legacy bodies individually through pages of 128 IDs to reconstruct dependencies. Migrated hash history starts at recorded cursors; unavailable earlier hashes remain unknown. Routine metadata queries use bounded pages without reading page bodies.

Schema 3 prevents coalescing a local pending revision after it appears in an imported peer batch. Peer observation commits with the imported cursor without consuming the local outbox or changing revision provenance. Upgrades from schemas 1 and 2 conservatively protect all existing pending revisions because those databases did not record peer observation. This prevents future deletion; it cannot reconstruct a revision already removed by an older build.

The library's offline `rollback_schema_upgrade` consumes its store handle and restores schema 1 only when the added metadata is reconstructable from existing cursors, no attachment requests remain, and peer-observed saves are durably published or already frozen for publication. It refuses a rollback that would lose those protections. Stop storage workers before using this maintenance operation; normal opening only upgrades.

`CARGO_HOME=/private/tmp/quire-cargo bun run test:sync-schedules` runs 10,000 seeds with native replicas and again with a native office among frozen TypeScript replicas. Use `--from 123 --count 1 --runtime mixed` to replay a seed. `--runtime alternating` runs one schedule per seed, choosing native peers on even seeds and mixed peers on odd seeds. Each schedule uses 60 actions by default; `--steps` changes that count. The runner builds a persistent native probe, uses real SQLite databases and independently delivered folder views, and checks saved contents, ancestry, unpublished saves, and converged heads. Seven additional cases cover delayed attachments across runtimes, a frozen publication followed by another save and reopen, tombstone conflicts, and copied writer identities. Progress includes completed counts; failures retain the fixture and JSON request trace and print a replay command. This Unix FIFO harness runs on macOS/Linux and keeps WAL/FULL storage policy. Provider placeholders become invalid empty files in the native folder mirror, so their import diagnostics can differ from the TypeScript reader's unavailable content. It does not establish Windows, real provider, power-loss, or release performance acceptance.

Generate synthetic notebook fixtures with `bun tools/synthetic-notebook.ts <empty-folder> [pages] [seed]`. Defaults are 6,536 pages and seed 1. Generated notebooks and build output stay outside version control.

Before committing copied sources, run `bun tools/verify-vendor.ts --tracked` to catch ignored build inputs. During vendoring, `bun tools/verify-upstream.ts ../zed` compares unmodified files with the pinned checkout; the documented source forks need separate diff review. This comparison is a development audit, not a build dependency. Upstream prompt fixtures intentionally contain conflict examples and whitespace, so whitespace checks apply to authored files and the documented vendor modifications while the remaining copied files retain their exact bytes.

The application uses GPL-3.0-or-later; retain the licenses and font notices in `vendor/zed`. Copied components and source revisions are recorded in `vendor/zed/UPSTREAM.json` and `tests/reference/UPSTREAM.json`. The source snapshot includes the full editor's supporting crates and assets. Quire loads editor/Vim key contexts strictly and omits bindings for unrelated Zed product panels. The earlier input harness derives from `crates/gpui/examples/input.rs` at that revision.
