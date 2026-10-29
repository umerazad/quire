# Native rewrite task queue

Read [the architecture and contracts](native-rewrite.md) and [the parity ledger](native-parity.md) first. Implementation has started in `quire`; complete task acceptance still requires the proof recorded below. `rt` and `zed` are read-only sources. Paths below are proposed write sets unless explicitly described as source paths.

Execute in dependency order. Every task needs changed paths, exact commands, observed output, and unresolved risk in its proof notes. If a task proves too large for one focused implementation session, split it into named child tasks with their own verification before starting it. Never mark a whole task done because only its happy path passes.

Shared acceptance rules: build independently of sibling repos; preserve source notices; keep failures actionable; no private notebook contents in fixtures; no silent reductions of feature or durability contracts. The task status field is authoritative. Gate descriptions do not grant release or publication authority.

## Foundation and feasibility

### NR01: Freeze source behavior and benchmark fixtures

What to do: Reconcile the finished `rt` plan/code/tests and pin its commit alongside Zed's. Copy synthetic fixture generators and focused reference behavior into a development-only oracle. Resolve differences in codec, coalescing, imports, Git, and release status.
Constraints: Do not edit a running agent's checkout. The oracle must run without `../rt`. Preserve unknowns instead of inventing test evidence.
Write set: `tests/reference/`, `tests/fixtures/`, `tools/fixtures/`, source manifest and parity ledger.
Dependencies: none.
Verify: Generate 1x/5x fixtures reproducibly; capture baseline first-editable timings and RSS with a documented environment; map every existing storage-inventory row and additional UI row to a test or explicit gap. Record source and dependency hashes.
Proof notes: Source baseline advanced to clean `rt` commit `e91f3e23603c76a1b1d824ec3dd64c60096e5d01` (the added commit only supplies a Windows copy target). Copied format/graph/replica modules and eight codec tests run independently through Node/Vitest: `bun run test:reference`, 8 passed. `tests/reference/UPSTREAM.json` records hashes. Synthetic seed 1 produced 6,536 pages/13,023,130 bytes and 32,680 pages/65,503,501 bytes. Remaining: reference harness/semantic coverage, reproducibility manifest, and fresh first-editable/RSS baseline. Native launch is unavailable in this sandbox; previous source-plan timings are not new measurements.
Status: in_progress

### NR02: Build a standalone GPUI application

What to do: Copy the pinned GPUI/platform dependency closure, required resources, licenses, Cargo configuration, and test support. Create the initial workspace and one window with an editable input.
Constraints: All local Cargo paths, build includes, and runtime assets stay inside this repo. No Zed submodule, sibling symlink, or Cargo dependency fetching the Zed repository. Normal features must not activate Zed network services.
Write set: root Cargo/toolchain files, `vendor/zed/`, `crates/quire/`, dependency/resource manifest.
Dependencies: NR01.
Verify: Inspect resolved Cargo metadata and build resources, then build and run from an isolated copy containing only this repo; draw actual glyphs, handle keyboard/focus/resize, and run a GPUI test. Build native Windows/Linux smoke targets with the right platform features as runners become available.
Proof notes: Copied 26 GPUI/support crates including dev/test dependencies, shader sources, and licensed fonts. Rust 1.98.1 and Cargo/Bun locks are present as source artifacts. `cargo build -p quire --locked` passed; `cargo test -p quire --features test-support` passed 3 input tests (UTF-16 selection, composition replacement, long-line selection geometry); `cargo fmt -p quire -- --check` and scoped Clippy with warnings denied passed. A separate copy containing only Cargo/toolchain files, crates, and vendor sources passed `cargo check --locked -p quire` under `/private/tmp/quire-isolated`, confirming it needs neither sibling repository. Commands used `CARGO_HOME=/private/tmp/quire-cargo` because the default shared cache is read-only. Launching the binary failed when sandbox-denied macOS services returned a null pasteboard; `bun run dev` now detects that sandbox and exits with an actionable message. Remaining: native desktop draw/input acceptance, other OS builds, and host/editor integration.
Status: in_progress

### NR03: Host the copied Zed editor and Vim

What to do: Add text/rope/language/multi-buffer/editor/Vim and required supporting code. Open a synthetic Markdown buffer in a minimal Quire window. Record necessary host services before pruning dependencies.
Constraints: No startup project scan, login, telemetry, collaboration, or language-server launch. Keep required libraries if removing them would force replacement of tested editor behavior.
Write set: selected `vendor/zed/` crates, `crates/quire/src/editor/`, editor initialization tests.
Dependencies: NR02.
Verify: Type, select, paste, undo/redo, search, and run representative Vim operators in two views of one buffer. Inspect processes, network activity, startup work, and dependency/resource closure.
Proof notes: Implemented the shared-buffer foundation as NR03a below. Full editor/Vim hosting remains unfinished; the input harness is not a Markdown editor.
Status: in_progress

#### NR03a: Share native views through Zed text buffers

What to do: Copy `text`, `rope`, `clock`, and their required helpers; make the native input views share one document, version checks, anchored selections, and undo/redo.
Constraints: Keep the upstream snapshot fixed; reject stale or invalid UTF-8 edits before mutation. The harness does not replace the planned full editor integration.
Verify: Native GPUI tests cover two views, undo/redo from either view, selection tracking across earlier Unicode insertions, composition ranges, and rejected edits. Build and scoped Clippy including test targets pass.
Proof notes: `crates/quire/src/document.rs` wraps Zed's text buffer and immutable snapshots. Both input views share its GPUI entity and receive notifications. Added five support crates, for 31 vendored crates total. Final source-independence review found 32 upstream license links pointing into the sibling checkout. They were replaced with byte-identical regular files; `bun run verify:vendor` now rejects any vendored symlink and the copy tool dereferences new copies. The check failed on the original links and passes on the repaired tree. All eight Node reference tests also pass. Tests were first observed failing for absent document/view behavior, then passed after implementation. Nine native/model tests pass, including the original three input tests. Zed's finalization API keeps separate edit commands as separate undo steps. Commands: `cargo test -p quire --locked --features test-support` (9 passed), `cargo clippy -p quire --locked --all-targets --features test-support --no-deps -- -D warnings` (passed), `cargo build -p quire --locked` (passed), formatting and prose lint (passed). A sequential review checked snapshot/selection mapping, rejected-edit behavior, and shared history. Native desktop visual acceptance remains unavailable in the sandbox; this evidence comes from GPUI's headless test contexts.
Status: done

#### NR03b: Host the full editor and Vim

What to do: Add the remaining `language`, `multi_buffer`, `editor`, and `vim` dependency closure and replace the input harness with an embedded Markdown editor.
Constraints: Keep account, project scan, language servers, collaboration, and telemetry inactive. Preserve NR03a's shared-document and version-check contracts.
Verify: Complete the parent task's native editing/Vim scenarios, including multiple views and installed-package resource independence.
Proof notes: pending.
Status: pending

### NR04: Prove Write and editable-embed mappings

What to do: Implement the smallest projection showing hidden bullets/IDs, a task checkbox, a link, and an editable nested subtree in two places. Test Zed display-map/excerpt integration before broad UI work.
Constraints: One source buffer and history; transformations cannot edit hidden identities or escape an embed. Record a fallback design only against a demonstrated failing case.
Write set: notebook presentation and excerpt adapters, `tests/native/projection/`.
Dependencies: NR03.
Verify: Round-trip display/source positions; edit either occurrence and undo from the other; switch Write/Source; test folds, wrap, select-all, paste, keyboard boundary movement, and Vim operations.
Proof notes: pending; a rendered screenshot alone cannot close this task.
Status: pending

### NR05: Prove native typography and input

What to do: Bring Quire's Arabic/Urdu script/font rules to native shaping and validate input on each target OS. Add missing bidi, caret, or accessibility support where the spike exposes it.
Constraints: Preserve source order and grapheme boundaries. Pragmata is user-installed; packaged fallbacks need their notices. No substitution of an English-only acceptance test.
Write set: text/layout adapters, font assets, `tests/native/input/`, targeted vendor fixes.
Dependencies: NR04.
Verify: Arabic marks, Urdu Nastaliq, mixed directions, emoji, ligatures, wrap alignment, mouse selection, deletion, IME preedit/commit/cancel, clipboard, keyboard focus, and accessible editor/control names. Record macOS/Windows/Linux results separately.
Proof notes: pending; source README itself identifies native composition/accessibility coverage gaps.
Status: pending

### NR06: Select PDF and code-formatting implementations

What to do: Run small packaged spikes for inline PDF rendering and the existing seven formatting languages. Choose native libraries or explicitly bundled helpers based on output, platform support, cost, and licensing.
Constraints: Do not assume GPUI replaces Chromium's viewer or Prettier. No undeclared Node/Bun installation requirement in the shipped app.
Write set: disposable feasibility fixtures, component decision notes, resource/dependency manifest.
Dependencies: NR03.
Verify: Render/scroll a local multi-page PDF on native targets with bounded memory; compare formatter output on existing fixtures; build chosen dependencies in an isolated application package.
Proof notes: pending; library selection is intentionally gated by this experiment.
Status: pending

Gate A: NR02 through NR06 must establish a viable editor and native feature path. If a spike fails, retain the failure evidence and repair or revise the plan before filling out the application shell.

## Storage and the first complete workflow

### NR07: Port the format 1 codec

What to do: Port canonical serialization, revision/page identity, zstd envelopes, strict filenames, format validation, and unknown-field handling into a pure Rust module.
Constraints: Hash the same uncompressed bytes as the frozen TypeScript implementation. Do not change the format to accommodate Rust serialization defaults.
Write set: `crates/quire_store/src/sync/format.rs`, protocol fixtures.
Dependencies: NR01.
Verify: Cross-language decode/encode and pinned IDs for Unicode, CRLF, numbers, escaping, parents, settings, unknown fields, corruption, decompression bounds, and newer versions. Compression byte equality is not required.
Proof notes: pending.
Status: pending

### NR08: Implement acknowledged local saves and recovery

What to do: Build the local SQLite schema and worker, save transactions, recovery drafts, outbox, schema migration, and explicit durability states using the chosen Zed SQLite primitives.
Constraints: WAL/FULL on writing connections; no memory fallback. Keep local-only content authoritative. Apply the frozen coalescing and uncertain-publication rules. Check any transitive SQLite security/correctness fixes needed at the pinned version.
Write set: `crates/quire_store/src/database.rs`, save/recovery modules and process-failure harness.
Dependencies: NR07.
Verify: Assert effective pragmas/FTS support; kill/reopen after acknowledgment; test full disk, read-only paths, transaction rollback, corrupted/newer databases, and recovery preservation. The latest acknowledged content survives.
Proof notes: pending.
Status: pending

### NR09: Port publication and replica convergence

What to do: Port graph heads, import cursors, publisher, watcher/rescan, attachment delivery, conflict detection, and crash adoption. Reuse the seeded schedule model against Rust and mixed Rust/TypeScript replicas.
Constraints: Per-device ownership and distinct writer identities; immutable durable files; no clock-based last-write-wins. Never coalesce a revision that may already have been published.
Write set: `crates/quire_store/src/sync/`, durable-file adapters, replay fixtures.
Dependencies: NR08.
Verify: At least the existing 10,000 reproducible schedules plus real-filesystem kill cases; delayed/duplicate/partial delivery, missing parents, stale office window, collisions, late attachments, uncertain publish, and copied device identity. Inject edits while a batch write is in flight; coalescing cannot discard a revision already selected for publication. Check saved content and final heads in both runtimes.
Proof notes: pending; simulation and same-folder tests do not verify cloud providers.
Status: pending

### NR10: Port Markdown analysis and identity

What to do: Implement Quire block boundaries, source positions, IDs, references, title/frontmatter handling, and incremental analysis. Evaluate parser libraries against the frozen oracle, then implement Quire's extensions.
Constraints: No incidental text normalization, new IDs, or imported-script execution. Translate UTF-16 fixture offsets explicitly.
Write set: `crates/quire_core/src/markdown/`, parser/reference fixtures.
Dependencies: NR01.
Verify: Differential block/reference output across existing Markdown fixtures and randomized edits; preserve no-setext behavior, code/frontmatter exclusions, Unicode offsets, lists, CRLF boundaries, and imported identifiers.
Proof notes: pending.
Status: pending

### NR11: Complete one native daily-page workflow

What to do: Wire the editor to the native service: create/open a daily page, edit in either presentation, record recovery, save, close, restart, and show a peer's update or conflict.
Constraints: Shared buffers keyed by notebook/page identity; expected-version checks on asynchronous operations. Opening a notebook loads metadata and the needed page, not every body.
Write set: notebook session/buffer registry, service command types, native vertical-slice tests.
Dependencies: NR04, NR05, NR08, NR09, NR10.
Verify: An end-to-end native test covers editing through an embed, undo, crash recovery, local save, publication, remote update, and a refused stale write. Measure first-editable latency and loaded body count.
Proof notes: pending.
Status: pending

### NR12: Serve indexed queries with a live overlay

What to do: Add incremental derived tables and bounded search/page/backlink/tag/task/template/daily queries. Merge unsaved analysis without counting old and new versions twice; expose generation-aware paging.
Constraints: Disk reads, parsing, hashing, and database waits stay off the UI thread. Pin dirty buffers; bound caches and reject stale results after notebook/query changes.
Write set: `quire_store` index/query modules, live-overlay coordinator, performance fixtures.
Dependencies: NR10, NR11.
Verify: Differential results against the reference for committed and unsaved edits; deterministic invalidation/race tests; SQL plan inspection and 1x/5x work-count checks. Search/backlinks do not enumerate all page bodies.
Proof notes: pending; this completes the unresolved intent of source storage task 6.
Status: pending

### NR13: Open existing Markdown folder notebooks

What to do: Add the folder adapter with ordinary files, watched external edits, revision checks, recoverable saves, history, and shared query indexing.
Constraints: Opening does not convert a folder. Preserve hidden metadata, symlink boundaries, case handling, and local/foreign drafts. Coordinate ownership with legacy processes rather than assuming app-data isolation suffices.
Write set: `quire_store` folder adapter and watcher, folder integration fixtures.
Dependencies: NR11, NR12.
Verify: Existing folder-contract fixtures plus native open/edit/save/restart; external edits/deletes, read-only files, path aliases, renamed pages, and stale saves preserve both versions when required.
Proof notes: pending.
Status: pending

Gate B: Both backends must run the daily-page workflow. Format compatibility, recovery, unsaved-query visibility, and bounded startup must pass before replacing more screens.

## Notebook features

### NR14: Port complete outline editing

What to do: Implement Enter, indent/outdent, block movement/join, multi-line paste, numbered lists, folding, zoom, and corresponding Vim overrides.
Constraints: Preserve descendants, IDs, caret placement, selection, and shared undo; retain ordinary paragraph behavior.
Write set: notebook editor commands and structural-edit tests.
Dependencies: NR11, NR10.
Verify: Port `block-commands`, `outline-caret`, `list-numbers`, `block-wrapping`, and relevant editor scenarios to native input tests in Write/Source/embeds, with Vim on/off.
Proof notes: pending.
Status: pending

### NR15: Complete references and editable embeds

What to do: Implement wiki/Roam links, block completion, inline previews, full subtrees, page embeds, backlinks navigation, and nested source editing.
Constraints: Resolve through current notebook queries; preserve IDs; bound cycles/depth/views. One buffer/history across every occurrence.
Write set: reference resolver, excerpt views and completion integration.
Dependencies: NR12, NR14.
Verify: Existing reference/embed fixtures and native tests cover parent/child edits, rename, missing targets, cycles, two panes, multiple occurrences, and source deletion/conflict without stale overwrites.
Proof notes: pending.
Status: pending

### NR16: Port navigation and daily layout

What to do: Add physical files, logical pages/namespaces, daily browser, completion/search palette, split tabs, history, favorites, recents, and daily-layout settings.
Constraints: Namespace navigation does not reorganize files. Workspace state is local and notebook-scoped; date/title matching follows current rules.
Write set: workspace/navigation views, daily/layout core, preference persistence.
Dependencies: NR12, NR15.
Verify: Native keyboard/mouse/tab/history tests plus daily-layout fixtures, duplicate dates, profile switches, restored reading position, and empty folders. Lists remain virtualized.
Proof notes: pending.
Status: pending

### NR17: Port tag pages and linked references

What to do: Add normalized tag completion/index, grouped context, filters, reference ordering, task filters, and source navigation.
Constraints: Viewing tags never assigns IDs; retained scroll/filter state uses bounded results.
Write set: tag and linked-reference views, query contract tests.
Dependencies: NR12, NR15, NR16.
Verify: Port tag/tag-page/linked-reference scenarios; check unsaved edits, nested/Unicode tags, ancestor context, ordering, and rapid filter changes without stale results.
Proof notes: pending.
Status: pending

### NR18: Port task semantics and dates

What to do: Implement checkbox/Roam task parsing, state transitions, capture, created/due/completed stamps, inherited dates/project links, and batch edits.
Constraints: Task operations edit Markdown through shared buffers and preserve IDs. Date behavior is deterministic under an injected clock/timezone.
Write set: `quire_core` tasks/projects/date modules and fixtures.
Dependencies: NR10, NR12, NR14.
Verify: Differential task/date/project tests cover state reopening, code exclusions, daily capture, stamps, inherited links, multiple selections, and undo.
Proof notes: pending.
Status: pending

### NR19: Build task and project workflows

What to do: Add Due/Inbox/Next/Waiting/Someday/Projects/Done views, capture, selection, date controls, missing-next-action indicators, and keyboard task commands.
Constraints: Source edits, card edits, embeds, and AI task writes share NR18 commands. Keep task capture drafts local until submitted.
Write set: task/project views and editor action bindings.
Dependencies: NR16, NR18.
Verify: Native capture/triage/project flows, bulk updates, focus retention, save failures, task shortcuts in Vim and non-Vim views, and 5x result paging.
Proof notes: pending.
Status: pending

### NR20: Port templates and supported macro controls

What to do: Add `jj` completion, template expansion, fresh referenced IDs, supported date/task/random/breadcrumb commands, and recognized Roam controls.
Constraints: Unsupported commands fail before editing; imported JavaScript stays inert. Inject clock/randomness for deterministic tests.
Write set: template/macro core, completion UI, expansion fixtures.
Dependencies: NR15, NR18.
Verify: Existing template fixtures compare text, IDs, references, and single-step undo; query-backed commands work with pages not currently loaded.
Proof notes: pending.
Status: pending

### NR21: Add attachments and inline media

What to do: Implement attach/open, local/remote/data image rendering, PDFs using the selected renderer, missing-file placeholders, and bounded decode/texture work.
Constraints: Preserve source syntax and notebook boundaries; a missing remote asset must not block editing. New attachments have a durable local copy.
Write set: attachment service, image/PDF views, media fixtures.
Dependencies: NR06, NR09, NR15.
Verify: Attach/restart/backup resolution, both backends, path traversal/symlink rejection, late delivery, offline failures, PDF scrolling, and bounded memory on large documents.
Proof notes: pending.
Status: pending

### NR22: Add code highlighting and formatting

What to do: Integrate copied syntax infrastructure and the NR06 formatter choice for JavaScript, TypeScript, JSON, CSS, HTML, YAML, and Markdown.
Constraints: Formatting applies only to unchanged source as one undo transaction; unsupported languages remain editable.
Write set: code-block presentation, formatter adapters/resources, fixtures.
Dependencies: NR06, NR14.
Verify: Match reference fixtures, retain indentation/IDs, reject stale responses, cancel cleanly, copy code, and format in a package with no developer runtime installed.
Proof notes: pending.
Status: pending

## Data exchange and durability

### NR23: Implement Markdown import and export

What to do: Import folders without altering the source, flatten a page for standalone export, and export an entire notebook with attachments and an explicit report of excluded state.
Constraints: Preserve paths, titles, IDs and attachment resolution; no silent overwrite on collisions. Export works without AI.
Write set: import/export modules, progress/cancel UI, round-trip fixtures.
Dependencies: NR13, NR15, NR21.
Verify: Round-trip both backends; interrupt and resume staged import; compare page/attachment counts and hashes; flattened embeds/citations match current behavior; unknown formats fail safely.
Proof notes: pending; whole-notebook export was a source-plan gap.
Status: pending

### NR24: Implement Roam JSON/EDN exchange

What to do: Port imports, source archives, metadata round trips, orphan recovery, and JSON export against the new service.
Constraints: Keep original bytes and unknown metadata. Never execute graph scripts or commit private graphs.
Write set: Roam/EDN codecs, metadata storage, synthetic fixtures.
Dependencies: NR10, NR23.
Verify: Existing import/export fixtures and detached-block cases; stable IDs, timestamps, order, references, and archive hashes; large synthetic import stays responsive. Private-copy verification records only counts/timings.
Proof notes: pending; Roam export from the new source engine was not fully verified in its plan.
Status: pending

### NR25: Port history, relocation, and recovery workflows

What to do: Add history browsing/restore-as-new-page, stale-safe conflict resolution, foreign drafts, and journaled page/subtree moves with link updates.
Constraints: Preserve history and identities, including case-only renames and collision handling. Do not promise cross-file undo that the source lacks.
Write set: history/conflict/move commands and views, interruption fixtures.
Dependencies: NR09, NR13, NR15, NR16.
Verify: Local multi-page atomicity, kill/restart at move steps, external changes, competing heads, save-copy IDs, and restore history without overwriting the current page.
Proof notes: pending.
Status: pending

### NR26: Implement verified backup and restore

What to do: Add consistent staged snapshots, manifests/hashes, scheduling/listing, and restore into a new notebook identity. Include recovery, hidden metadata, archives, attachments, and folder journals.
Constraints: No raw copy of a live SQLite file. Preserve unpublished work or fail explicitly; never claim a partial snapshot completed.
Write set: backup/restore service, settings UI, snapshot fixtures.
Dependencies: NR23, NR24, NR25.
Verify: Restore both backends into fresh roots; compare content, heads/history, drafts, archives, attachments, and query results; inject damage/full-disk/interruption and retain older completed backups.
Proof notes: pending.
Status: pending

### NR27: Verify provider transport and Personal Git

What to do: Port the finished Git algorithm and provider handling, retaining per-profile restrictions and credentials through Git. Measure a synthetic year of repository growth.
Constraints: No force-push, unexpected remote changes, or claim that folder publication confirms delivery. Preserve logs after failed pushes and do not assume immutable filenames alone prevent every Git conflict.
Write set: provider/Git adapters, transport acceptance scripts, status view.
Dependencies: NR09, NR26.
Verify: Two-device OneDrive/Dropbox/Git handover, placeholders, conflict copies, divergent commits, failed upload/push, copied identity, and excluded transcripts/live databases. Record actual provider results separately from simulations.
Proof notes: pending; unavailable devices/providers leave this gate unverified.
Status: pending

## AI and application completion

### NR28: Port CLI transforms and proposals

What to do: Implement backend detection/test, process streaming/cancellation, range transforms, proposal diff/review, prompt history, and native Vim/shortcut entry points.
Constraints: Proposals stay outside notebook storage until accepted. Preserve target versions, IDs, single-step undo, backend settings, and documented CLI isolation limits.
Write set: AI process host, proposal/prompt views, protocol fixtures.
Dependencies: NR14, NR15.
Verify: Fixture CLI processes cover streaming, malformed output, failure/cancel, backend switching, accepted/rejected proposals, stale ranges, and notebook switch. Test installed supported CLIs separately.
Proof notes: pending.
Status: pending

### NR29: Port live notebook tools and terminal MCP

What to do: Implement compatible tool schemas, read queries, staged/apply write ledger, token-checked bridge, stdio server, and reversible terminal registration.
Constraints: Tools use the live notebook and overlay, never a second filesystem view. Keep Off/Review/Apply and notebook identity/version checks.
Write set: notebook tools/bridge/MCP mode, tool transcript fixtures.
Dependencies: NR12, NR18, NR28.
Verify: Existing tool/bridge/MCP tests translated to native processes; unsaved reads, stale writes, denied writes, concurrent tool calls, server closure, and calls from a terminal produce expected results.
Proof notes: pending.
Status: pending

### NR30: Port questions, synthesis, and Tidy

What to do: Build the answer pane, citations, tool activity, keep-in-daily action, reference synthesis, terminal changes, and enforced Tidy proposal limits.
Constraints: Tidy cannot expand its allowed page/range or alter existing prose; it remains review-required even in Apply mode.
Write set: AI notebook workflows and answer/change views.
Dependencies: NR17, NR19, NR29.
Verify: Notebook question fixtures include unloaded/unsaved pages, citations and stale targets; Tidy rejects prohibited changes; synthesis reruns retain instruction/context and do not run on accidental Enter.
Proof notes: pending.
Status: pending

### NR31: Port AI-assisted PDF and Word export

What to do: Flatten source with NR23, run the selected CLI in an isolated temporary export directory, validate the result, and offer save/cancel with progress.
Constraints: Preserve current backend sandbox behavior and transcript privacy. Failures leave the notebook untouched and clean temporary files.
Write set: document-export runner/UI, artifact validation fixtures.
Dependencies: NR23, NR28.
Verify: Fake-runner failure/cancel/malformed-artifact tests plus actual PDF/DOCX export from a synthetic page; re-open outputs to confirm valid formats and retained content.
Proof notes: pending.
Status: pending

### NR32: Port preferences, help, and the showcase

What to do: Add font/spacing/Vim/tag preferences, system/light/dark themes, offline help/shortcuts, and the isolated editable README sample with reset.
Constraints: Apply appearance before first draw; sample content never enters real queries/backups/exports. Respect reduced motion and accessible focus.
Write set: preferences/theme/help/showcase modules and bundled assets.
Dependencies: NR16, NR19, NR20, NR21, NR22, NR30.
Verify: Native preference persistence, theme switching, script fallbacks, platform shortcut labels, help navigation, sample reset/isolation, and screenshots at multiple sizes/scales.
Proof notes: pending.
Status: pending

### NR33: Complete profiles and application lifecycle

What to do: Add Current/Work/Personal settings, overlap checks, independent backup/operation status, local workspace restoration, single-instance handling, menus/window controls, and close coordination.
Constraints: Flush before switching, preserve edits after failures, and distinguish local durability from publication/push/backup. Stalled services cannot cause silent data loss or an unexplained endless close.
Write set: application lifecycle/profile modules, native window/menu tests.
Dependencies: NR16, NR26, NR27, NR32.
Verify: Switch profiles with pending edits and AI runs; close/restart with slow, failed, and unavailable services; second-instance behavior; native shortcuts and window actions on supported OSes.
Proof notes: pending.
Status: pending

## Acceptance and cutover

### NR34: Enforce performance budgets

What to do: Run the documented workload on release builds, identify costs with traces, and fix measured bottlenecks in bounded slices. Record process/GPU memory and loaded-body/view counts.
Constraints: No feature disabling, durability reductions, hidden helpers, or comparison of first paint with first editable. Run timing gates on controlled hardware.
Write set: `tools/bench/`, performance reports and narrowly scoped measured fixes.
Dependencies: NR12, NR15, NR19, NR21, NR27, NR33.
Verify: Every main-plan budget, 1x/5x scale ratio, large-page typing, and concurrent sync/index/backup workload. Preserve raw measurements and rerun correctness tests affected by each optimization.
Proof notes: pending; performance instrumentation starts in NR11, not only here.
Status: pending

### NR35: Package and verify native applications

What to do: Build macOS/Windows packages and Linux artifacts with icons, fonts, grammars, PDF/formatter dependencies, CLI/MCP mode, and correct application identities.
Constraints: No Zed/rt checkout or developer runtime at build/run acceptance. Keep development identity/data distinct until cutover; signing status must be explicit.
Write set: packaging/CI configuration, manifests/notices, native acceptance harness.
Dependencies: NR05, NR06, NR31, NR33, NR34.
Verify: Clean isolated builds and install/open/update smoke checks; native input/IME/clipboard/accessibility, dialogs, scale factors, close saves, media, formatters, and terminal tools on each OS. Report unsigned/notarization status accurately.
Proof notes: pending.
Status: pending

### NR36: Demonstrate full-state migration and rollback

What to do: Implement the documented transfer of old local-only saves/drafts, settings, and history to the native data root. Rehearse copied format 1 and folder-notebook adoption and rollback after new native edits.
Constraints: Never reuse an old writer ID, delete source state, or imply Markdown export preserves all history. Require explicit conversion before changing storage mode.
Write set: migration/recovery utilities, migration report and synthetic acceptance fixtures.
Dependencies: NR24, NR25, NR26, NR27, NR35.
Verify: Before/after manifests cover text, IDs, history, drafts, archives, attachments, and profile isolation. Interrupt migration, resume safely, then demonstrate return to the legacy app with post-cutover edits retained through a supported path.
Proof notes: pending.
Status: pending

### NR37: Review complete parity and approve cutover readiness

What to do: Review every parity row and task proof against the built package and final source contract. Resolve gaps, update README/CONTEXT/help, and record a release-readiness decision.
Constraints: A missing provider/OS/native-input result is unverified, not passed. Readiness is separate from publishing or deleting old data.
Write set: parity ledger, proof reports, product documentation, concrete gap fixes.
Dependencies: NR01 through NR36.
Verify: Complete protocol, semantic, durability, native UI, performance, package, and migration gates; fresh review of code/diff/proof; prose lint on final documentation. No required parity row has only a screenshot or source-test citation as native proof.
Proof notes: pending.
Status: pending

Gate C: Daily-driver cutover begins only after NR37 passes. Keep the old app and a verified independent backup available. Any open gate keeps the build experimental.
