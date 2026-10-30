# Native rewrite task queue

Read [the architecture and contracts](native-rewrite.md) and [the parity ledger](native-parity.md) first. Implementation has started in `quire`; complete task acceptance still requires the proof recorded below. `rt` and `zed` are read-only sources. Paths below are proposed write sets unless explicitly described as source paths.

Execute in dependency order. Every task needs changed paths, exact commands, observed output, and unresolved risk in its proof notes. If a task proves too large for one focused implementation session, split it into named child tasks with their own verification before starting it. Never mark a whole task done because only its happy path passes.

Read formatted source in a completed tool call before preparing a patch, including newly created files after running the formatter. Use the observed identifier and surrounding text as the anchor. Keep inspection and mutation in separate tool calls; a read followed by a prewritten patch in the same call is not an inspection gate. For import edits, read the current import block after the last formatter run; never reconstruct a merged import line from an earlier layout. For scripted replacements, require each exact anchor to match once before writing.

Shared acceptance rules: build independently of sibling repos; preserve source notices; keep failures actionable; no private notebook contents in fixtures; no silent reductions of feature or durability contracts. The task status field is authoritative. Gate descriptions do not grant release or publication authority.

For GPUI tests, inspect the actual assertion APIs before writing them. Editor selections use `MultiBufferOffset` and a `DisplaySnapshot` obtained inside an entity update; `FocusHandle::is_focused` takes only the window. `Editor::snapshot` also needs mutable app access: call it inside `editor.update`, never through `editor.read(cx).snapshot(window, cx)`. Keep these details in local helpers rather than repeating guessed calls. Find selects a range with Vim off and a cursor with Vim on; assert the intended mode explicitly. Route search submission using the actual focus handle: Vim can focus query input without updating the search bar's cached query-focus flag.

Before using a copied UI API for the first time, read its signature and a local call site. `Tooltip::text` returns the tooltip closure, `ButtonCommon::tab_index` needs an `isize`, and window focus mutations take app access. Button keyboard tests need a focused draw followed by both key-down and key-up events; `simulate_keystrokes` sends key-down only. Use the copied GPUI activation tests as the reference rather than changing product behavior to accommodate an incomplete test event sequence.

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

The first full-workspace test attempt failed in `extension_host/build.rs`: it reads sibling `extension_api/wit` files absent from Cargo's dependency closure. Copied all 85 files from the pinned source, recorded that directory under `UPSTREAM.json` build inputs, and taught both the generator and vendor verifier to retain/check it. The verifier first failed on the missing directory, then passed with the copied inputs. Byte comparison now verifies 3,033 unchanged files and the same three documented source forks.

The workspace rerun finished compilation and passed the first two test binaries (302 and 30 tests). The copied `agent` test binary then passed 808 tests and failed `internal_tests::test_native_terminal_tool_releases_pty_resources`: nested `sandbox-exec` returned `sandbox_apply: Operation not permitted`, exit 71, rather than the expected successful terminal command. Eleven upstream tests were already ignored; none were changed. Cargo stopped at that binary, so later workspace tests are unverified. Keep the whole-workspace gate open for a regular terminal run; do not disable the sandbox assertion or report the application-only suite as a full-workspace pass.

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
Proof notes: `editor_host.rs` now hosts two full Zed Editors over one `language::Buffer`, without constructing a project, workspace, client, or server. The default app uses this host; the earlier input harness remains behind `--input-harness`. Initialized settings/theme/editor/Vim, disabled Zed session-buffer serialization, and reused the mode indicator. Only core editor/Vim key contexts are loaded; a small public keymap filter hook was added to the copied settings crate, and selected bindings still fail loudly on invalid actions. The source closure now contains 179 crates including optional/test/platform dependencies, copied assets, and the upstream transitive patches. Re-seeded the lock from the pinned source to avoid mixing registry and patched tree-sitter types. A supported release-channel override replaces the executable-crate include; the paths helper uses `QuireNative` for data/config isolation. `cargo test -p quire --locked --features test-support` passes 14 tests, including real Vim insert/line-delete/multiline input, undo from the other editor, redo, Vim-disabled clipboard editing, and the two-pane window/mode indicator. `cargo build -p quire --locked` passed. `cargo clippy -p quire --locked --all-targets --features test-support --no-deps -- -D warnings` passed. A copy containing only this repo's Cargo/config/crate/vendor files passed `cargo check -p quire --locked` from `/private/tmp/quire-editor-isolated.Vm38aA`, without either sibling checkout. A Git-index archive also passed `cargo check -p quire --locked` from `/private/tmp/quire-staged-editor.oupctP`, proving required source files are tracked rather than merely present locally. `verify-vendor.ts --tracked` now guards that invariant, including the local scratch patch previously hidden by a global ignore rule. `verify-upstream.ts` compared 2,949 unmodified files byte-for-byte with the pinned source; the two declared fork files were reviewed separately. Authored-file/modified-vendor whitespace checks and prose lint passed; intentional upstream diff examples retain their bytes. Final application tests and Clippy passed after the window/mode-indicator and actionable-error changes; all eight reference protocol tests passed. Native desktop visual acceptance remains unavailable. The debug linker reports large unwind metadata; release performance is not yet measured. Remaining: explicit Markdown language/grammar setup, search/register/composition scenarios, complete Quire Vim overrides, and native OS acceptance. The earlier Document version gate remains in the input harness; native notebook commands must enforce expected versions when wired in NR11.
Status: in_progress

##### NR03b.1: Assign bundled Markdown grammars to the shared document

What to do: Construct Markdown and Markdown-Inline languages from the copied configs/queries and pinned native grammar. Assign them to the notebook window's single shared language buffer.
Constraints: No project, LSP adapter, runtime download, or all-language initialization. Bundle resources in debug and release builds. Syntax parsing must preserve source text and shared buffer identity.
Verify: A real notebook-window test observes heading/list syntax and inline emphasis after parsing, with both editors retaining the exact source. Existing editing/Vim tests, application build, formatting, and scoped Clippy pass.
Proof notes: The window-level test first failed because the buffer had no assigned language. `markdown.rs` now constructs Markdown and Markdown-Inline from the copied configs/queries and the already pinned `tree-sitter-md`, registers only those languages, and assigns them to the shared document. It follows active theme changes and constructs no LSP adapters or project. Quire already enables `util/debug-embed`; the copied `fs_embed!` macro embeds queries/configs in debug as well as release builds. The existing Cargo lock gains two direct application dependency edges without changing dependency versions.

`cargo test -p quire --locked --features test-support` passes 16 tests. New cases assert shared buffer identity, heading/list/inline-emphasis trees, unchanged Arabic source, reparsing after Vim line deletion, and undo from the other pane restoring both text and syntax. A copied test binary run from a temporary directory containing only that binary passes all 16 tests too. `cargo build -p quire --locked`, scoped application Clippy with all targets/test support and `-D warnings`, and formatting pass. The debug linker retains its unwind-metadata warning. This closes the earlier Markdown setup item; parent NR03b still needs search/register/composition scenarios, Quire Vim overrides, package acceptance, and native OS verification.
Status: done

##### NR03b.2: Host local Find and Replace without a workspace

What to do: Put the copied buffer search bar beside each editor and use its standalone registrar for Find, Replace, result navigation, and dismissal. Retain the current active editor and shared undo behavior.
Constraints: No Zed workspace/project construction or project-search initialization. Query input must not enter the note. Keep each pane's target explicit.
Verify: Real keyboard Find/Replace, next/previous, Escape focus restoration, live result updates after edits, and undo from the other pane. Existing tests and scoped build/Clippy gates pass.
Proof notes: Each editor now owns a copied `BufferSearchBar` targeted through `ToolbarItemView`, with action routing through the existing `DivRegistrar`. Parent observers show/hide the bar when its state changes. No Zed workspace, project, or project-search service is constructed. `search` and `workspace` become direct application dependencies for these already copied interfaces; no dependency versions change.

The first Find test reproduced a missing binding allowing query keystrokes to execute Vim edits in the note. After hosting the bar, another test caught Enter inserting a newline in the query because the generic Editor binding had a deeper context. Search-field navigation now uses `BufferSearchBar > Editor`, with replacement actions scoped to the replacement field. Tests explicitly distinguish Vim's match cursor from the normal editor's selected match range.

`cargo test -p quire --locked --features test-support` passes 21 tests. Five new keyboard cases cover query isolation, next/previous, Escape focus, Vim cursor placement, per-pane query/selection isolation, live result updates, Replace Next, Replace All, field cycling, and undo of a multi-match replacement from the other pane. Scoped application Clippy with all targets/test support and `-D warnings` passes. Vim slash search still looks up its bar through a Zed pane and remains a separate adapter task. Native desktop and other-OS acceptance remain open; these are GPUI headless results.
`cargo build -p quire --locked`, formatting, authored whitespace, vendor verification, and prose lint pass. The debug linker warning remains unchanged.
Status: done

##### NR03b.3: Route Vim search through the embedded bar

What to do: Give the copied Vim implementation an explicit editor-local search-bar hook and route its existing search methods through it. Dispatch search submission from the native host.
Constraints: Preserve the existing workspace lookup for upstream callers. Keep Vim search, counts, cancellation, operator motions, and registers in the copied implementation. Avoid a second search algorithm or Zed workspace construction.
Verify: Keyboard slash/backwards search, submission, cancellation, next/previous, under-cursor search, operator-search motions, pane targeting, and Vim toggling. Run application tests and the copied Vim search regressions; inspect and record each vendor fork.
Proof notes: The slash and under-cursor tests first failed because the copied methods returned early without `Workspace::for_window`. The fork adds an editor addon holding a weak bar reference, a bar resolver retaining the workspace fallback, and targeted submission functions. The association survives removal/recreation of the Vim addon when preferences change. The application routes submission to the editor belonging to the actually focused query field and propagates ordinary Find submission. A diagnostic caught the cached query-focus flag remaining false after Vim focused the query directly; routing now checks the focus handle.

The existing copied search, motion, count, cancellation, operator, and search-register code remains responsible for behavior. Eleven native search tests pass, including the five ordinary Find/Replace cases plus slash/repeat, word search, cancellation, backward/count handling, delete-to-search with shared undo, and second-pane targeting after disabling/re-enabling Vim. The new fork files are recorded in `UPSTREAM.json`; source comparison verifies 3,031 unchanged files and five explicit source forks.

The first copied-test filter matched zero tests. Listing the compiled binary identified `normal::search::test::`, with 27 cases. That scoped run then exposed development asset loading from the wrong root (`settings/default.json`), which poisoned the shared initializer. `test:vim-search` now selects the actual module and enables the same `util/debug-embed` policy used by Quire. The corrected command, `cargo test -p vim --locked --features util/debug-embed --lib normal::search::test::`, passes all 27 copied cases, including Neovim comparisons and the original workspace path. `cargo test -p quire --locked --features test-support` passes all 27 application tests. Command-palette/Ex integration remains separate.

Scoped application Clippy with all targets/test support and `-D warnings` passes. `cargo clippy -p vim --locked --features util/debug-embed --all-targets --no-deps -- -D warnings` passes for the modified copied crate. Application build, formatting, authored/modified-vendor whitespace, tracked-vendor verification, and prose lint pass. Sequential review checked the weak association, toggle lifecycle, focus-based routing, unchanged motion/register logic, and workspace fallback. The debug unwind warning and full-workspace sandbox failure remain open; no native OS acceptance is implied.
Status: done

##### NR03b.4: Host editor-local Ex commands

What to do: Add a colon prompt using the copied editor and Ex parser. Support line/range jumps, local options, range edits, and substitution, including visual/count prefixes. Keep unsupported or malformed commands in the prompt with an actionable error.
Constraints: Reuse the copied parser/range/motion implementation. Do not construct a Zed workspace or dispatch unrelated IDE commands. Save and AI commands connect to notebook services in NR11 and NR28; this slice must never report an in-memory example as saved.
Verify: Keyboard open/submit/cancel, input isolation, line/range commands, substitution and shared undo, visual/count ranges, errors, and pane targeting. Run parser and search regressions for modified copied code plus application/build/Clippy checks.
Proof notes: `editor_command.rs` adds a single-line copied editor for command input, targeted through a weak editor reference. Colon bindings handle normal, visual, and counted commands. Submission uses the copied Ex parser with filename completion omitted; its original workspace entry point still passes the workspace through. A recursive capability check limits the embedded entry point to editor-local actions, including actions wrapped in range/global commands. No project, workspace, file completion, or shell service is constructed. Query/epoch checks discard results after input changes or cancellation.

The first line-jump test failed because colon still relied on the Zed command palette; `:2` behaved like a normal-mode count instead. Visual-range and substitution tests then exposed workspace-only mark storage and bar lookup. Embedded editors now keep session-local marks and use the existing host-aware bar resolver for Ex find/substitution. Validation checks ranges/marks and converted regex patterns before closing the prompt, preserving editable errors. An invalid substitution range now stops before replacement. All algorithms for parsing, range resolution, motions, global commands, and replacement remain copied code.

`cargo test -p quire --locked --features test-support` passes 37 tests. Ten command cases cover line jumps, substitution with undo from the other pane, unknown-command cancellation, missing marks/invalid regex, correcting errors in the second pane, visual/count ranges, ignorecase settings, global delete/sort, advanced backreferences, and rejection of unavailable workspace commands. The in-memory example cannot acknowledge a save. Save and AI commands remain assigned to NR11/NR28.

The copied Vim gates pass with `--locked --features util/debug-embed --lib`: `command::test::` (25), `normal::mark::test::` (2), and `normal::search::test::` (27). These include Neovim comparisons and the original workspace behavior. A parallel command run passed but logged contention in copied command-palette history. The serial rerun passed all 25 without those errors; `test:vim-command` now uses one test thread. The native prompt does not construct that command palette. Application build and combined app/Vim Clippy with all targets and test support pass, with warnings denied in Clippy. Source comparison verifies 3,029 unchanged files and seven recorded source forks. Formatting, authored/modified-vendor whitespace, and tracked vendor inputs pass. The debug linker warning and whole-workspace sandbox failure remain unchanged.

Parent NR03 remains open. Before closing it, compare the legacy register/mark and prompt-history scope and verify the required toggle, view, and reopen behavior. Embedded marks currently live on the Vim entity; cross-page persistence is not established by this slice. Native composition and OS/package acceptance also remain open.

Fresh review reopened the task after finding that the new validator used a simpler regex engine than the copied search bar. A native backreference test reproduced a rejected valid pattern. Find/substitution validation now calls the same `SearchQuery::regex` builder; global commands retain their original engine. Pattern errors have an explicit category independent of library wording. The final 37 application tests, 25 copied command tests, combined Clippy, and build pass after this repair. Sequential review also checked capability recursion, stale completion cancellation, range errors, focus targeting, and the unchanged workspace parser entry point.
Status: done

##### NR03b.5: Preserve command history and verify shared registers

What to do: Add in-memory Ex history with Up/Down prefix navigation and draft restoration. Verify named registers across panes and Vim toggles through real keyboard input.
Constraints: Reuse the copied command-palette navigation approach without its database, telemetry, or workspace. Keep submitted and cancelled nonempty commands in application-session history, as the legacy adapter does. Keep navigation cursors local to each prompt. Use the copied palette's 1,000-entry retention bound; do not persist notebook command text.
Verify: Submit/cancel recall, prefix filtering, draft restoration, edits during navigation, cross-pane/window recall, register paste, and shared undo. Run application tests, formatting, scoped Clippy, and build. Record provenance and review completion/error interactions.
Proof notes: Legacy `codemirror-vim-core/vim.js` stores Ex history and registers globally; its prompt remembers submitted/cancelled input and navigates by the original prefix. `editor_command.rs` adapts Zed's `command_palette::QueryHistory` navigation and duplicate promotion, using an application global for entries and a cursor per prompt. Its history DB is unnecessary for this session-only host. No new dependencies are added.

The first recall test failed with empty input instead of the last command. A second regression caught an unsuccessful prefix lookup retaining the old prefix after typing. Both pass after history routing and cursor validation. The register test separately reproduced loss of a named register after disabling Vim: the copied global-settings observer reset all state. The native host now calls an opt-in initializer retaining text registers and clipboard provenance while resetting transient operators. Original `vim::init` retains its original reset behavior. Macro recording retention, search-history parity, and mark lifetime remain separate parent checks.

`cargo test -p quire --locked --features test-support` passes 41 tests. Four new cases cover submitted/cancelled recall, prefix filtering/draft restoration, edits after a missed lookup and while navigating, independent cursors in two windows, recall targeting the second pane, and named-register paste after a Vim toggle with undo from the other pane. `cargo test -p vim --locked --features util/debug-embed --lib test::test_toggle_through_settings -- --exact` passes the original toggle scenario. Combined app/Vim Clippy with all targets/test support and `-D warnings`, application build, and formatting pass. Source comparison verifies 3,028 unchanged files and eight recorded source forks. The debug linker warning and full-workspace sandbox failure remain open.

Sequential review checked history ownership, duplicate promotion, editable errors, weak targets, completion epochs, and preservation of the default copied initializer. Successful submission now also clears a prior prompt error. Native OS acceptance remains open; these results use headless GPUI contexts.
Status: done

##### NR03b.6: Verify composition in the shared full editor

What to do: Exercise the copied editor's platform input interface inside the real notebook window, with UTF-16 replacement ranges, composition updates/commit, Vim input gating, and undo from the other view.
Constraints: Use mixed Unicode fixtures and actual shared editors. These headless calls verify host integration; they do not establish native IME, font shaping, bidi, or accessibility acceptance.
Verify: Composition across surrogate pairs preserves surrounding text, updates both panes, clears marked ranges on commit, and undoes/redoes as one edit from the other pane. Normal-mode input is disabled and Insert mode accepts composition. Run the application suite and scoped Clippy.
Proof notes: `editor_input_tests.rs` calls the copied editor's `EntityInputHandler` inside the actual two-pane notebook window. One case replaces an emoji using UTF-16 offsets, updates Japanese preedit twice, checks marked/selected ranges, commits, and verifies shared text plus undo/redo from the other pane while preserving Arabic context. The second verifies normal-mode input gating and Insert-mode composition followed by Escape and shared Vim undo. Both tests pass against the existing copied implementation; no production adapter or vendor changes were needed.

`cargo test -p quire --locked --features test-support` passes all 43 application tests. `cargo clippy -p quire --locked --features test-support --all-targets --no-deps -- -D warnings`, formatting, and authored whitespace checks pass. Sequential review checked the actual platform trait, surrogate-pair offsets, marked ranges, shared-buffer assertions, and separate Vim modes. The earlier build remains applicable because this slice changes only test code and test module registration. Native IME, bidi/shaping, OS/package, and full-workspace acceptance remain open under their existing gates.
Status: done

### NR04: Prove Write and editable-embed mappings

What to do: Implement the smallest projection showing hidden bullets/IDs, a task checkbox, a link, and an editable nested subtree in two places. Test Zed display-map/excerpt integration before broad UI work.
Constraints: One source buffer and history; transformations cannot edit hidden identities or escape an embed. Record a fallback design only against a demonstrated failing case.
Write set: notebook presentation and excerpt adapters, `tests/native/projection/`.
Dependencies: NR03.
Verify: Round-trip display/source positions; edit either occurrence and undo from the other; switch Write/Source; test folds, wrap, select-all, paste, keyboard boundary movement, and Vim operations.
Proof notes: NR04.1 establishes source excerpts and shared history. NR04.2's children implement hidden syntax, checkboxes, plain web links, pane switching, selected Vim operations, Unicode wrap mappings, and complete-selection replacement. NR04.3 adds semantic subtree recomputation and unavailable-block recovery. The complete interaction matrix, custom-control wrapping, and full notebook embed behavior remain open. A rendered screenshot alone cannot close this task.
Status: in_progress

#### NR04.1: Establish editable excerpt boundaries and shared history

What to do: Add a standalone excerpt adapter over the existing Markdown buffer, expose a development example, and exercise edits/undo in both source and excerpt views.
Constraints: Preserve exact byte ranges using Zed anchors, with no copied document text or independent history. Validate UTF-8 range boundaries before constructing views. Do not interpret this as completion of subtree parsing, hidden identities, or the full Write projection. NR03's verified headless editor integration permits this feasibility work while its native OS gates remain open.
Verify: Source/excerpt text and range mapping, shared undo/redo, source edits before the range, boundary Backspace/Delete, select-all paste, Vim delete/join, and Ex substitution cannot mutate outside the excerpt. Generic editor context expansion must not widen it. Record failures before choosing a fallback or modifying upstream behavior.
Proof notes: `editor_excerpt.rs` validates byte boundaries and constructs an exact-range copied multi-buffer over the existing language buffer. `NotebookWindow::for_buffer` hosts it with the same search/command services. `--excerpt-harness` displays the source excerpt in the right pane. The Cargo lock only gains a direct edge to the already copied `multi_buffer`; no dependency versions change.

The first keyboard test reproduced separate history: an excerpt edit could be undone from Source, but redo in the excerpt did nothing. The copied multi-buffer now has an opt-in single-page excerpt constructor with source-buffer transaction IDs, undo/redo, grouping, and transaction removal. It retains non-singleton display mapping and exact source anchors. Cloning retains that source association. Original constructors keep their existing history behavior. Inserting a different source into this mode fails before changing its excerpt map.

Fresh review then reproduced a boundary escape through the copied `ExpandExcerpts` action: it revealed the entire surrounding page. Generic context expansion now leaves single-page embeds bounded; the host's future semantic subtree adapter owns range changes. Undo intentionally follows shared page history, including Source edits outside the excerpt. A substitution test initially assumed case-sensitive matching despite the copied default ignorecase setting; it now explicitly uses the supported `\C` override while retaining its exact source assertion.

`cargo test -p quire --locked --features test-support` passes 50 tests. Seven excerpt cases cover UTF-8 validation, byte/anchor mapping after an earlier source insertion, shared history between Source and excerpt and between two cloned excerpt windows, boundary Backspace/Delete, clipboard replacement of the selection, Vim delete/join, bounded Ex substitution, and disabled generic context expansion. `cargo test -p multi_buffer --locked --features util/debug-embed --lib` passes all 72 copied tests, including original history and randomized mapping cases. Combined app/multi-buffer Clippy with all targets/test support and `-D warnings`, application build, formatting, and authored/modified-vendor whitespace checks pass. Source comparison verifies 3,025 unchanged files and eleven recorded source forks.

Sequential review checked source ownership, all ten singleton transaction branches, cloning/grouping, coordinate validation, context expansion, the unchanged default constructors, and the narrow lock change. Hidden-identity protection and semantic subtree acceptance remain pending. Native desktop and OS/package checks, the debug unwind warning, and the existing full-workspace sandbox failure remain open.
Status: done

#### NR04.2: Map Write syntax and protect hidden identities

What to do: Inspect the legacy projection/edit rules, then add hidden bullet/ID presentation, a task checkbox, and a rendered link using the copied display map. Keep Source available over the same buffer.
Constraints: Display transformations must preserve source bytes and explicit source/display mappings. A hidden ID must not be edited accidentally by cursor movement, selection, paste, or Vim operators. Use observed failures to choose any required edit guard; do not build a second editable document.
Verify: Compare legacy synthetic fixtures, round-trip positions around hidden spans and Unicode, switch Write/Source, edit through both views, and undo from either. Test folds, wrap, boundary movement, select-all paste, Vim motions/operators, and unchanged hidden identities. Parent NR04 remains open for semantic subtree growth and complete embed behavior.
Proof notes: Legacy `Editor.tsx` replaces list markers and trailing IDs in the display; `outline-caret.ts` separately clamps caret positions and filters partial deletions through IDs. Child tasks below record ID protection, semantic span derivation, pane switching, native checkbox/plain web-link controls, selected Vim line operations, Unicode wrap mappings, and complete-selection replacement. Complete semantic acceptance, custom-control wrapping/selection, and the full editing matrix remain required by this task.
Status: in_progress

##### NR04.2.1: Support hidden spans and partial edits over the source buffer

What to do: Exercise empty replacement spans with the copied display map. Add the smallest view edit hook needed to preserve an ID during partial deletion/replacement, keep caret positions visible, and demonstrate this in a fixed developer fixture.
Constraints: Syntax coordinates come from an explicit fixture in this feasibility slice. Preserve the shared source/history and unrelated user folds. Keep source-view edits outside the projection policy. Do not close the parent task without its semantic producer and full acceptance matrix.
Verify: Empty replacements have no ellipsis or source mutation; legacy ID-deletion vectors match; clear/retype retains the ID and typing order; partial clipboard replacement retains the ID; deleting a complete newline-terminated block and undo restore source and projection. Copied tab-map and multi-buffer regressions, application tests, build, formatting, and scoped Clippy pass.
Proof notes: The baseline display test passed, but Vim deletion crashed in `TabStopCursor::seek_forward`: an empty fold chunk reached `chunk_len - 1`. The copied cursor now skips empty chunks. A regression with empty replacements, Greek characters, and tabs passes alongside all 21 copied tab-map tests.

Once that crash was fixed, Vim D reproduced removal of the hidden ID. `editor_projection.rs` now uses typed folds over the shared buffer, an editor-owned subscription for caret/fold refresh, and an optional view-local multi-buffer edit hook. The native policy clamps partial replacements at the ID while preserving complete block deletion in the tested newline case. The hook rewrites a mutable slice, preserving edit count and order for the copied autoindent metadata; it remains absent in ordinary copied views. No notebook text is duplicated as a second editable document.

A multi-character retyping test then exposed a CRDT tombstone anchor resolving before newly inserted body text. The policy now identifies the live trailing ID within its bounded anchored span using the legacy ID spelling. Single-character success was insufficient evidence; the final test keeps three characters. The copied `keepIdOnDelete` fixtures for ranges 4..12, 7..8, and 2..12 of `- alpha ^abc` also pass through the native edit path. The fixed `--write-harness` exposes this experiment for native desktop inspection.

Final verification: `cargo test -p quire --locked --features test-support` passes 59 tests. Nine projection cases cover opening input without a forced test caret, the legacy vectors, partial clipboard replacement and typing order, clear/retype with shared undo/redo while automatic grouping is disabled, block delete/undo, new text after block deletion, Source edits, and retention of unrelated user folds. `cargo test -p editor --locked --features util/debug-embed --lib display_map::tab_map::tests::` passes 21 tests; the corresponding complete `multi_buffer` library suite passes 72. Combined app/editor/multi-buffer Clippy with all targets/test support and `-D warnings`, application build, formatting, and authored/modified-vendor whitespace checks pass. The broader editor Clippy gate exposed two stale `wrong_self_convention` expectations in copied LSP test helpers; removing those unused expectations preserves the warning gate and test behavior. Source comparison verifies 3,023 unchanged files and thirteen recorded source forks. The CLI rejects combined Write/excerpt harness flags with an actionable message and exit 2 before desktop initialization.

Sequential review checked policy ownership per view, hook ordering before source mapping, edit counts for autoindent, retained user folds, source/history identity, callback lifetime, and the bounds of the supplied-span contract. Remaining parent work includes semantic span generation/recomputation, checkbox/link rendering, full mode switching, folds/wrap/Unicode position coverage, complete EOF/no-newline and cc/S/Vim operator semantics, and incremental updates/performance measurement on large pages. The current fixture refreshes its supplied spans after edits; those broader contracts remain open. Native desktop acceptance, the debug unwind warning, and the existing full-workspace sandbox failure also remain open.
Status: done

##### NR04.2.2: Derive projection spans from Markdown semantics

What to do: Freeze legacy block/syntax vectors and derive the native projection spans from the shared page's parsed Markdown. Replace the fixed coordinate provider with versioned results that update after Source and Write edits. Keep this producer reusable by the later semantic core.
Constraints: Preserve code/frontmatter exclusions, no-setext behavior, nesting, IDs, and UTF-16/UTF-8 boundary conversions. Use the copied Markdown parse where it satisfies the source contract. Keep source fixtures and any development oracle independent of sibling checkouts; ship no JavaScript parser requirement.
Verify: Compare synthetic fixtures for lists/continuations, headings, quotes, fenced/indented code, frontmatter, IDs, tabs, line endings, and Unicode. Newly added/removed blocks and source changes recompute spans without changing document text or applying stale parse results. All existing projection tests pass through the semantic producer. Checkbox/link rendering and the remaining edit matrix stay required by parent NR04.2.
Proof notes: The inspected `rt` HEAD is `4985e4080d2302b2abb416f2db7fa390080786c8`; its Markdown/RangeSet sources match the frozen revision. Those files are copied unchanged with recorded hashes, and pinned Lezer development dependencies provide an independent local oracle. Native block analysis uses the copied Markdown syntax tree and returns UTF-8 coordinates; differential tests convert to the legacy UTF-16 contract. The 39 vectors cover nested/continued lists, IDs, code/frontmatter exclusions, no-setext behavior, indentation, CRLF, emoji, combining marks, Arabic/Urdu, and JavaScript whitespace. Review found and reproduced an extra-BOM mismatch: frontmatter accepts at most one BOM, and paragraph coordinates retain the leading BOM skipped by tree-sitter.

Projection now subscribes to the shared source's parse/language events. It rejects pending or interpolated trees and caches the current text version plus syntax update count. Only ranges fully contained in an excerpt are mapped into that view. Existing editing tests now use semantic spans; added integration cases cover new Source blocks, Unicode ID protection, obsolete background parses, and an excerpt after edits outside its range. The first excerpt fixture accidentally included a lazy list continuation; the corrected fixture separates the outside paragraph and bounds the excerpt before its newline.

Final gates: 64 native application tests and 47 reference tests pass. Combined all-target Clippy for Quire/editor/multi-buffer passes with `-D warnings`; the native build passes with the existing debug unwind warning. Formatting, authored whitespace, prose lint, tracked-vendor checks, and comparison of 3,023 unchanged vendor files pass. No vendor implementation changed in this slice. Sequential review covered parse freshness, source/view coordinate conversion, subscription lifetime, lockfile scope, and parent feature limits. Full legacy decoration/widget behavior, dirty-subtree updates, performance measurements, native desktop acceptance, and the existing full-workspace sandbox failure remain open.
Status: done

##### NR04.2.3: Switch each pane between Write and Source

What to do: Expose each pane's presentation through a native button and a focused-editor keyboard action. Disabling Write removes its folds, edit policy, and parse subscription; enabling it derives current spans again.
Constraints: Keep the same editor, source buffer, history, selections, and unrelated user folds. Mode state belongs to the projection addon rather than a second host flag. Use the copied native UI controls.
Verify: Keyboard switching affects the focused pane, Source can edit formerly hidden IDs, undo works across views, and edits made while Source is active appear after switching back. Repeated switching must retain user folds and avoid duplicate subscriptions. Native visual/accessibility acceptance remains open until it can run outside the sandbox.
Proof notes: `editor_projection.rs` now exposes idempotent enable/disable operations using the addon as mode state. Disabling drops its subscriptions, removes only its typed folds, and clears the view edit hook. `editor_host.rs` adds a native copied-UI button to each pane and Cmd-Shift-M / Ctrl-Shift-M for the focused editor; successful changes return focus to that editor. Errors remain visible in the window. The direct `ui` dependency uses the existing copied crate and adds no package/version to the graph.

The keyboard red run left `alpha` projected after the toggle. The implementation now exposes the source, allows deletion of the formerly hidden ID, shares undo from the other pane, and rebuilds the projection. Three new integration tests cover that sequence, repeated switching with retained user folds and edits while disabled, and actual button focus traversal/Enter activation. The button test initially sent key-down without key-up; it now follows the copied GPUI activation test sequence, draws with `TestAppContext::update_window` to avoid borrowing the rendered root entity, and checks returned editor focus.

`cargo test -p quire --locked --features test-support` passes all 67 tests. Combined Quire/editor/multi-buffer all-target Clippy with test support and `-D warnings`, native build, formatting, authored whitespace, prose lint, and tracked-vendor checks pass. The existing debug unwind warning remains. Sequential review checked view-local ownership, typed fold removal, subscription drop/rebind, current parse reuse, focus routing, error display, and the one-edge lockfile change. Native visual/accessibility acceptance, complete Write rendering/edit semantics, and performance remain open parent requirements.
Status: done

##### NR04.2.4: Render a task marker as an editable native checkbox

What to do: Replace supported task markers in Write lists with a native checkbox using a typed fold placeholder. Reuse legacy marker spelling and the editor-only marker toggle fallback. Keep the full notebook task service, state tags, and completion timestamps assigned to NR21.
Constraints: Do not duplicate document text or mutate an obsolete rendered range. Source keeps the literal marker. Widget edits share undo and preserve the hidden ID. Retain keyboard activation and an accessible checkbox name/state.
Verify: Supported square/Roam markers, invalid boundary spellings, task-looking text inside code, Source/Write switching, actual checkbox activation, shared undo, Unicode, and a stale widget callback after Source changes.
Proof notes: Legacy `readTaskMarker` accepts square markers and Roam TODO/DONE, including its independently optional wiki brackets. `CheckboxWidget` has a marker-only fallback when no notebook task service is supplied. `task_marker.rs` ports that spelling/boundary behavior. `editor_projection.rs` reads at most 13 prefix characters per list, then uses the copied display map's custom fold element and measured width to render the native checkbox. The control reports its checkbox role/name/state, supports pointer/keyboard activation, and sends a single marker edit through shared history. Source exposes the original bytes. Task state tags, completion dates, and notebook workflows remain NR21 requirements.

The first red test displayed literal `[ ]` rather than a checkbox. The initial native control passed click/undo and code-exclusion tests. Keyboard testing then reproduced Enter inserting a newline through its ancestor editor. A `QuireControl` context now excludes host editor/search/command bindings and copied Vim bindings; unbound typing stays in the control, and Escape returns focus. The existing settings keymap fork gains `constrain_contexts`, preserving section order, actions, and key equivalents while applying the host constraint. No additional vendor file is forked. Native regressions verify that `ddix` cannot edit the note while the checkbox has focus, Enter and Space toggle a Roam marker, and Escape restores editor focus.

Sequential review found that checking only the marker bytes allowed an old callback to toggle a task newly wrapped in a code fence. A red test reproduced `[ ]` becoming `[x]` inside code. Callbacks now also require a current parsed source version/syntax count and a matching projected list start. The regression checks both pending parsing and the completed code parse. Other cases cover disabled Write mode, changed markers, square/Roam spelling, invalid marker boundaries, Unicode/IDs, actual mouse activation, shared undo, and Source switching.

Final gates: `cargo test -p quire --locked --features test-support` passes all 73 tests. `cargo test -p settings --locked --features util/debug-embed --lib keymap_file::tests::` passes all ten copied keymap tests. Combined Quire/editor/multi-buffer/settings all-target Clippy with test support and `-D warnings`, native build, formatting, authored whitespace, prose lint, tracked-vendor checks, and upstream comparison pass. The existing debug unwind warning remains. The vendor comparison still finds 3,023 unchanged files and 13 documented fork files. Native visual/accessibility acceptance and the complete task editing matrix remain open.
Status: done

##### NR04.2.5: Prove native external link rendering

What to do: Derive plain-label inline HTTP/HTTPS links from the copied inline grammar and render an activatable native link over the same source buffer. Use a single display placeholder character for geometry rather than putting arbitrary label bytes into the fold placeholder bitmap.
Constraints: Preserve raw syntax in Source. Leave unsupported rich labels, reference links, images, and other destinations as source text until their required renderers/resolvers exist. Do not activate an obsolete rendered link or treat text inside code/frontmatter as a link. Keep link rendering independent of project/workspace services.
Verify: Native pointer/keyboard activation records the expected URL without editing the note; Source switching restores raw syntax. Check Unicode, code/frontmatter, nested code in lists, images, unsupported schemes/labels, long labels, source edits, stale callbacks, and excerpt mapping. Full rich rendering, wrapping/selection acceptance, and internal notebook navigation remain parent/NR15 work.
Proof notes: Inspected the pinned inline grammar's `inline_link`, `link_text`, and `link_destination` node definitions and the legacy `MarkdownWidget`/RichText path. `markdown_links.rs` derives supported plain-label links from current inline syntax layers, excludes code/frontmatter, and validates HTTP/HTTPS destinations through the copied `http_client::Url` export. This adds one direct dependency edge to an existing crate, with no new package or version. Eligible block lookup uses source-ordered boundaries rather than scanning every block for each link. Dirty-subtree analysis and performance measurement remain parent work.

`editor_projection.rs` anchors links into their containing view, retains literal source for incomplete excerpt ranges, and renders native label controls through typed folds. Shared inline-control and parse-freshness helpers keep checkbox/link input isolation and callbacks consistent. Activation validates the current parsed source, current link membership, and exact source bytes before requesting the current URL. Source mode removes the projection and exposes the full syntax. This is the stated external-link subset; rich labels, other link forms, and complete wrapping/selection behavior remain parent requirements, with internal notebook resolvers assigned to NR15.

The first red test displayed literal Markdown. The initial custom renderer used U+FFFC, which the copied invisible-character pass replaced with its own string, preventing the native control from painting. Reusing the copied fold marker preserves the custom label renderer and measured width. A 512-character label plus emoji passes native keyboard activation without placing arbitrary label bytes into the fold bitmap. Pointer activation also passes with an Arabic/emoji label. The GPUI test platform records requested URLs without launching a browser.

Five new tests cover native pointer/keyboard activation without text mutation, Source switching, parser exclusions/unsupported forms, long labels, stale callbacks before/after a code parse, and complete/partial excerpt mapping after source edits. The excerpt test initially replaced a URL containing the partial excerpt's end anchor; its intended outside edit now changes the URL tail, with an explicit assertion that the edit stays beyond that boundary. The complete excerpt opens the new target, while the partial one stays literal and has no native link control.

Final gates: `cargo test -p quire --locked --features test-support` passes all 78 tests. Combined Quire/editor/multi-buffer/settings all-target Clippy with test support and `-D warnings`, native build, formatting, authored whitespace, and prose lint pass. No vendor source changed in this slice. The existing debug unwind warning remains. Sequential review checked source/inline offsets, code ancestry, parse freshness, callback lifetime, scheme validation, excerpt containment, and the lockfile's single-edge change. Native visual/accessibility/browser acceptance, full rich link rendering, wrapping/selection parity, and measured performance remain open.
Status: done

##### NR04.2.6: Preserve block identities across Vim line changes and EOF deletion

What to do: Complete the distinction between clearing a list block with cc/S and deleting its line with dd, including a final line without a newline. Preserve markers and IDs for changes, delete whole blocks for line deletion, and retain shared undo.
Constraints: Derive any required operation metadata from the copied editor/Vim path. Keep ordinary Source/copy-editor behavior unchanged. Do not infer different intents from identical text edit ranges.
Verify: Native keystrokes cover cc/S/C, indented/ordered/unidentified blocks, terminal newline/no-newline cases, complete deletion, retyping, and undo from the other pane. Run affected copied Vim/multi-buffer regressions after any fork change.
Proof notes: The initial keyboard matrix confirmed nonempty cc/S/C behavior but reproduced dd retaining an EOF block's hidden identity. Expanded cases also reproduced deletion of the next block's hidden marker. Opt-in view filters now receive scoped Default/Change/DeleteLines intent. Vim linewise delete operations calculate complete source-line ranges before display clipping; the copied editor's replacement path retains those source selections while DeleteLines is active. This reuses linked edits and history. Ordinary views retain their original selection resolution and have no filter intent cell. A guard restores prior intent on drop; cloned filters begin at Default. The new multi-buffer test observes nested scopes and clone isolation through real edits.

Further tests reproduced removal of empty list/task prefixes during cc/S/C. The view policy now preserves a single changed line's prefix and identity, including checkbox/Roam markers and their whitespace. Caret and ID bounds use the legacy editable-text start after that prefix. Change/substitute operations publish the corresponding scoped intent. Counted multi-line structure and the complete clipboard/operator matrix remain parent/NR14 work.

A staged empty-block test then found an earlier parser mismatch: tree-sitter classifies an empty list/task at EOF without a newline as a paragraph, unlike the frozen Quire parser. New differential fixtures reproduce it. The producer now incrementally parses a cloned root tree with a virtual body and newline for empty-item suffixes, reading borrowed rope chunks and clipping resulting ranges to the real source length. Expanded fixtures caught a newline-only repair turning a nested bullet into a setext underline. The virtual body avoids that ambiguity; an empty item directly after a paragraph still follows the legacy rule that it cannot interrupt the paragraph. `OwnedSyntaxLayer::into_tree` exposes the already owned clone for this analysis; the shared buffer and its syntax tree remain untouched. Markers and web links reuse one block analysis per rebuild, avoiding duplicate normalization. A direct tree-sitter dependency adds an edge to the existing pinned package. All 47 Markdown vectors now match, including nested/ordered empty items, siblings, paragraph continuation, and code/fence exclusions. `cargo test -p quire --locked --features test-support` passes 82 tests; `bun run test:reference` passes 55. `cargo test -p multi_buffer --locked --features util/debug-embed --lib` passes 73, including the new scope/clone regression. Copied Vim tests with `--locked --features util/debug-embed --lib` pass `normal::change::test::` (18) and `normal::substitute::test::` (4); the earlier checks in this slice passed `normal::delete::test::` (18) and `visual::test::` (32). Combined Quire/editor/multi-buffer/Vim/language all-target Clippy with test support and `-D warnings` passes. Source comparison verifies 3,016 unchanged copied files and 20 documented source forks. Final review checked canonical source ranges, bounded excerpt deletion, shared undo, intent restoration/clone isolation, virtual EOF clipping, default copied paths, and the single existing tree-sitter dependency edge. `cargo build -p quire --locked`, formatting, prose lint, tracked-vendor verification, and scoped whitespace checks pass. The debug build retains the existing unwind-metadata linker warning. Native desktop acceptance, broader structural clipboard behavior, and the existing whole-workspace sandbox failure remain open.
Status: done

##### NR04.2.7: Verify wrapped Write mappings and complete-selection replacement

What to do: Exercise Unicode source/display boundaries with hidden syntax at narrow wrap widths and replace a complete wrapped selection through the normal clipboard path. Fix demonstrated mapping or protection failures in the owning layer.
Constraints: Keep the same source buffer/history, preserve partial-edit IDs, and retain explicit ambiguity at collapsed spans. No copied display string as an editable document.
Verify: Real soft wraps exist; visible interior UTF-8 positions round-trip exactly; all hidden-boundary aliases map to the same display point and editor input returns to the visible body boundary; Source mode restores all bytes; select-all replacement and undo work in a full page and bounded excerpt.
Proof notes: The first two tests reproduced Select All/paste retaining the first hidden bullet and exposed an incorrect test assumption about copied fold bias. `FoldPoint::to_inlay_point` deliberately resolves empty folds to their right source boundary regardless of the outer tab/block bias. No vendor mapping change is needed: the corrected contract verifies exact visible-interior round trips, every prefix/ID alias at its one display point, and actual editor caret/input normalization at a wrapped ID boundary. Source mode restores the complete original bytes.

Expanded tests reproduced the same hidden-prefix remnant when typing over a complete bounded excerpt. The copied input path now detects a single complete source selection only in filtered views, retains it before display clipping, and shares this detection with clipboard paste. Typing and ordinary replacements share one resolver with the existing DeleteLines intent. Partial selections and ordinary views retain their existing behavior. Another red test reproduced retaining an old identity when replacing a complete paragraph or ID-only view; nonempty complete-view replacements now bypass partial-ID protection. Empty deletion keeps the existing block policy. No dependency or buffer/history replacement is introduced.

`cargo test -p quire --locked --features test-support` passes 87 tests. The five new cases cover real narrow soft wraps, Arabic/Urdu/emoji/combining-mark byte boundaries, hidden aliases, body-end caret input, full-page paste/shared undo, bounded excerpt paste/typing/deletion/shared undo, and complete paragraph/ID-only replacement. Clipboard cases cover both external text and the copied editor's selection metadata. Full-page and excerpt clipboard tests draw at a narrow window width with persistent EditorWidth wrapping before sending keys. `cargo test -p editor --locked --features util/debug-embed --lib editor_tests::test_paste` passes 19 copied tests; the corresponding `editor_tests::test_select_all` filter passes three. Combined Quire/editor all-target Clippy with test support and `-D warnings`, `cargo build -p quire --locked`, formatting, prose lint, whitespace, and tracked-vendor checks pass. Upstream comparison verifies 3,015 unchanged files and 21 recorded source forks. Sequential review checked complete versus partial selection policy, UTF-8/hidden-boundary aliases, bounded source mappings, shared undo, metadata paste, ordinary copied paths, and the unchanged dependency graph. The existing debug unwind-metadata warning remains. Native OS pointer selection, shaping/accessibility, full wrapping of custom link controls, and broader structural clipboard semantics remain open.
Status: done

##### NR04.2.8: Keep rendered link labels in native text layout

What to do: Verify long links at narrow widths and clipboard selection around them. Replace any demonstrated atomic-placeholder limitation with source-backed labels that use native wrapping and selection, retaining pointer/keyboard URL activation and source editing.
Constraints: One source buffer/history; no copied editable label. Preserve accessible link names, stale-callback checks, Source/Write switching, user folds, and ordinary copied editor behavior. Ground any vendor hook in the failing runtime case.
Verify: Long spaced and unbroken Unicode labels stay within the viewport and wrap across rows; pointer and Enter/Space activation open the right URL; focused editing reveals source syntax; partial-label and full-view copying preserve the intended source; edits/undo and stale/code cases pass. Run affected copied editor layout/clipboard checks, application tests, build, Clippy, and vendor audits.
Proof notes: A native regression measured the former atomic placeholder at 3,544 pixels in a 360-pixel window. Write now hides only the surrounding link syntax and leaves its label in the original buffer's native text layout. A viewport-scoped editor addon hook returns source-anchored regions; the copied element layer derives clipped fragments from the existing shaped/wrapped display lines, excluding folded text. Transparent controls follow those fragments without painting a second label. Focusing the editor in the containing block reveals the link syntax, matching the source app's active-block behavior. No dependency or second editable document is introduced.

The first stateful overlay run exposed mismatched prepaint/paint element namespaces; both phases now use the existing navigation-overlay namespace, covered by a native regression. A later pointer test reproduced the union rectangle taking focus in gaps beside its fragments. Controls now occupy the fragments, with one keyboard focus target and accessible name per link. The gap test verifies normal editor focus and unchanged source. Long spaced multilingual and unbroken labels wrap inside the viewport; first and final fragment clicks and Enter/Space activation request the current destination. Partial Arabic-label copy, source editing, shared undo, full-view copy, user folds, excerpt mapping, obsolete callbacks, and code exclusions pass. Full-view copy also uses the existing complete-source-selection helper so its hidden first marker is preserved; ordinary editor clipboard tests retain their behavior. Geometry tests resize the actual window before drawing, avoiding the automatic root render overwriting measurements made with a synthetic size.

Final gates: `cargo test -p quire --locked --features test-support` passes all 99 tests. Copied editor `element::tests::` passes 43, `editor_tests::test_copy` passes 13, `editor_tests::test_paste` passes 19, and `editor_tests::test_select_all` passes three. Reference tests pass all 55. Combined Quire/editor all-target Clippy with test support and `-D warnings`, native build, formatting, scoped whitespace, prose lint, and tracked-vendor checks pass. Upstream comparison verifies 3,013 unchanged files and 23 recorded source forks. The existing debug unwind-metadata linker warning remains.

Sequential review covered source ownership, viewport filtering, wrapped/folded geometry, input focus and fragment gaps, a single accessible keyboard target, current URL validation, source-edit/undo behavior, complete versus partial clipboard selection, theme refresh and unbind cleanup, and the unchanged dependency graph. The simplification pass reuses the existing overlay lifecycle and clipboard helper; no additional rendering subsystem is needed. Native desktop shaping, pointer and accessibility acceptance, richer link forms, internal notebook navigation, the broader Write interaction matrix, and measured release performance remain open parent/NR05/NR15/NR35 work.
Status: done

#### NR04.3: Follow a semantic block's changing subtree in an editable excerpt

What to do: Resolve a unique block ID against the shared page's current Markdown parse, open its subtree in an excerpt, and recompute the same view's range after Source or embedded edits. Show this in the excerpt developer view.
Constraints: Keep one source buffer/history, source anchors, existing Editor entities, and exact subtree bounds. A missing or ambiguous ID must disable editing rather than expose surrounding text. Do not accept stale parses or invent replacement IDs. Cycles, notebook reference resolution, and the final embed UI remain parent/NR15 work.
Dependencies: NR04.1 and NR04.2.2.
Verify: Adding/removing/reparenting children updates two views of the same block; earlier page edits retain source mapping; editing either occurrence and undo/redo share history; deletion and duplicate IDs fail closed and recover after source undo; generic expansion and boundary edits cannot escape the resolved subtree. Application tests, affected copied multi-buffer gates, build, Clippy, and vendor checks pass.
Proof notes: `PageAnalysis` shares a versioned block result across excerpt models, keyed by source text version and primary-language identity. Each model updates its existing shared-history multi-buffer through the copied `set_excerpt_ranges_for_path`; no new vendor API or dependency is needed. Missing or duplicate IDs remove its source excerpts, so even direct buffer edits cannot modify the surrounding page. Editor binding adds read-only/status state; the excerpt developer view now resolves `b-welcome` by ID. Source and Write keep the existing editor entities and source history.

Native tests reproduced a stale-selection assertion when a deleted block left caret anchors pointing into a removed excerpt. The binding parks inactive selections at a resolvable boundary, retains their source anchors, and restores valid selections after resolution. Another test reproduced losing y/z from `A x y z escape` when background parsing temporarily disabled the view. The semantic producer now reads a clean current syntax tree or incrementally parses an owned tree clone over borrowed current rope bytes; the shared syntax tree remains untouched. Write accepts the same current block analysis while unverified inline links wait for the background syntax result. Source version checks prevent accepting stale block data. Background completion retains the same shared analysis Arc when bytes and language are unchanged.

`cargo test -p quire --locked --features test-support` passes 95 tests. Eight new cases cover two models growing/shrinking after child insertion/deletion/reparenting and earlier Arabic/emoji edits, missing/duplicate IDs with direct-edit rejection and source-undo recovery, native edits in either occurrence, shared undo/redo, generic expansion and a caret left outside the shrunken subtree, deleted-block read-only input and recovery, opening Write before background parsing finishes, complete fast typing during background parsing, all 47 frozen vectors through fresh versus clean parsing with unchanged shared syntax, and reuse of the shared analysis after background completion. `cargo test -p multi_buffer --locked --features util/debug-embed --lib` passes 73 copied tests; `bun run test:reference` passes 55. Application build and scoped all-target Clippy with test support and `-D warnings` pass. Upstream comparison still verifies 3,015 unchanged files and 21 recorded source forks; this slice modifies none of them. Formatting, prose lint, scoped whitespace checks, tracked-vendor verification, and sequential review pass. The debug build retains the existing unwind-metadata linker warning.

Fresh review covers source ownership, exact subtree ranges, shared history, version/language validation, unchanged source syntax, inactive selection recovery, cached analysis lifetime, Source/Write recomputation, and retained ordinary excerpt behavior. Foreground block collection can still walk a whole page; large-outline/1 MiB release latency and mounted-view work remain NR35 gates. Native desktop acceptance, cyclic/notebook embeds, and the parent Write interaction matrix remain open.
Status: done

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
Write set: `crates/quire_store/src/sync/{format,json}.rs`, protocol fixtures.
Dependencies: NR01.
Verify: Cross-language decode/encode and pinned IDs for Unicode, CRLF, numbers, escaping, parents, settings, unknown fields, corruption, decompression bounds, and newer versions. Compression byte equality is not required.
Proof notes: Added the UI-independent `quire_store` crate and its experimental format 1 codec. It implements JavaScript number rendering, UTF-16 object-key sorting, canonical SHA-256 identities, NFC/lowercase page IDs, revision validation, strict log/checkpoint/attachment names, zstd envelopes, bounded decompression, header/parent/cursor/status checks, and newer-format errors. Supported unknown fields survive decode/re-encode. Writers validate records and reject sequences that cannot fit the nine-digit name. Dependencies are pinned in the shared lock; no Node or sibling repository is needed by the native codec at runtime.

The TDD red run failed on the initially empty module. The first implementation passed the Node golden fixtures. A later differential test caught Rust's different rendering of a large newer-format number; the regression now passes using JavaScript rendering. The latest `cargo test -p quire_store --locked` passes 17 tests. The Node subprocess checks native batches/checkpoints, 4,096 generated floating-point patterns with finite values compared, and 1,024 seeded Unicode/settings objects against the frozen source. It also checks malformed revision parents/counts/content, checkpoint cursors/heads, and checkpoint heads from other devices. Corruption, partial compression, strict names, decompression bounds, newer formats, and source-generated IDs have separate checks. Scoped Clippy with `-D warnings` and formatting pass. The preceding editor/reference gates passed 14 and 8 tests; those components did not change in the lossless repair. Full workspace and native desktop acceptance remain open.

Fresh sequential review found and repaired the JSON-model mismatch. The replacement uses a flat arena, lossless UTF-16-compatible strings, serde's grammar validation, and iterative traversal. The former rejection test is now `surrogate_and_deep_json_envelopes_round_trip_losslessly`, with successful native and Node reads of unknown-field surrogates, surrogate page/path/revision text, and deep JSON. A separate 20,000-level test covers parsing, cloning, serialization, and destruction. A red test exposed unchecked malformed bytes in the public string deserializer; validation now rejects them. Another red test caught overflowing numeric literals: the reader now matches JavaScript's infinities while canonical writers return a typed error. Node confirms that behavior and malformed UTF-8 replacement. Scalar conversion returns `None` for unpaired UTF-16 strings, so future editor integration must preserve their wire record rather than silently replace text. No source envelope is edited during these tests. The native app does not yet use the codec or persist notebooks; this closes the codec gate only.

Adding the application crate also required fixing `tools/vendor-zed.ts` to preserve application workspace/default members and local workspace dependencies. A rerun retained both application crates and the 179-crate vendor set without changing copied sources. The tool now rejects tracked source edits before copying. Review the source guard separately if vendoring policy changes.
Status: done

### NR08: Implement acknowledged local saves and recovery

What to do: Build the local SQLite schema and worker, save transactions, recovery drafts, outbox, schema migration, and explicit durability states using the chosen Zed SQLite primitives.
Constraints: WAL/FULL on writing connections; no memory fallback. Keep local-only content authoritative. Apply the frozen coalescing and uncertain-publication rules. Check any transitive SQLite security/correctness fixes needed at the pinned version.
Write set: `crates/quire_store/src/database.rs`, save/recovery modules and process-failure harness.
Dependencies: NR07.
Verify: Assert effective pragmas/FTS support; kill/reopen after acknowledgment; test full disk, read-only paths, transaction rollback, corrupted/newer databases, and recovery preservation. The latest acknowledged content survives.
Proof notes: Started the connection-policy foundation in `database.rs`, using copied `sqlez` primitives. `open_writable` requires an absolute file path, returns errors on failed/read-only opens, sets WAL/FULL, foreign keys, and an untrusted schema, and verifies effective durability/FTS5 capabilities. Added a narrow `Connection::try_open_file` fork hook that rejects SQLite's special memory/temporary names and read-only handles without falling back to memory. The older upstream infallible helper remains for unrelated library callers; authoritative Quire storage must use the fallible path.

The dependency audit found bundled SQLite 3.46.0 in `libsqlite3-sys` 0.30.1. SQLite reports that version is affected by the WAL-reset race, fixed in 3.51.3 and later. Updated the workspace to locked `libsqlite3-sys` 0.39.1 with bundled SQLite 3.54.1. Its `sqlite3.c` SHA3-256 is `e2b869d3003b33c942b7dd7063604fbc53553e1b66609570df39b9b7b558ee81`, matching the published release hash. The vendor generator retains this explicit override, and `UPSTREAM.json` records the source/API and dependency changes. Sources: [SQLite WAL-reset details](https://www.sqlite.org/wal.html), [SQLite 3.54.1 release](https://www.sqlite.org/releaselog/3_54_1.html).

The first database test run failed for missing APIs. Six connection tests now pass: unavailable/memory/relative paths, effective pragmas and FTS5 Arabic search on initial/reopened handles, corruption preservation, Unix read-only files, committed-row reopening plus savepoint rollback with bound text, and macOS fullfsync. The last case first failed with the default flag off; the initializer now enables and verifies it on each macOS connection. SQLite documents that this selects F_FULLFSYNC for sync operations, including checkpoints. [SQLite fullfsync](https://www.sqlite.org/pragma.html#pragma_fullfsync).

The connection-policy commit passed 14 editor tests, 23 storage tests, and 19 copied-wrapper tests. Scoped Clippy passed with `-D warnings`. The source comparison verified 2,948 unchanged files and separately recorded three fork files. The debug application test retained the earlier unwind-metadata linker warning.

The next slice adds the versioned notebook schema, preflight identity/schema checks before journal changes, compressed and hashed revision/recovery records, transactional multi-page saves, heads/outbox, editor-session versions, coalescing, and frozen publication selections. It uses explicit transactions rather than the copied migration helper's orphan-deletion policy. Schema 1 is immutable after this commit; future versions need explicit migrations. No old local state is automatically adopted. Reopening marks publication uncertain until the publisher verifies its folder. Exact selected bytes and IDs survive reopen and cannot be coalesced while publication is pending. Folder I/O is NR09.

Recovery uses compare-and-swap stamps and saved session clocks. Tests first reproduced delayed recovery resurrecting saved text, newer saves leaving older recovery, consumed-draft ordering, cross-page unchanged-content acknowledgment, and same-version retries after acknowledgment. The fixes preserve newer typing and accept retries from either side of the original acknowledgment while rejecting changed contents at the same version. A full-disk test uses SQLite's actual page limit; an outbox trigger failure proves coalescing and recovery changes roll back together.

`StorageWorker` owns the connection on one thread, returns futures, rejects excess queued requests/estimated payload bytes without waiting for SQLite, admits one oversized atomic request alone, and drains accepted requests on close. Tests hold a real write lock to check count/byte rejection, completion, and shutdown. Startup errors return through the ready receipt. Callers must inspect each save receipt; worker shutdown alone does not acknowledge saves.

`cargo test -p quire_store --locked` passes 47 tests. `bun run test:storage-crash` passes five child-process kill/reopen cases: committed save, recovery snapshot, later save consuming recovery, frozen publication selection, and interrupted SQLite transaction. The probe emits its receipt after the relevant store call commits. This proves process-crash recovery on the current filesystem, not power loss or cloud delivery. Scoped storage Clippy including examples passes with `-D warnings`.

The combined editor/storage run passes 14 editor tests and all 47 storage tests; the eight frozen reference tests also pass. Formatting, authored whitespace, and prose lint pass. The debug linker still reports its unwind-metadata warning. A generated Finder `.DS_Store` initially failed the tracked-vendor check; removing it restored that check.

The attachment slice adds `attach` and `cache_attachment` on the store and worker. It verifies downloaded bytes against strict content-derived names, writes a same-directory temporary file, synchronizes it, installs without overwriting an existing file, synchronizes the installed file and parent directories, then commits the catalog row. Retries verify and re-synchronize unregistered valid files. Damaged files remain untouched. Existing symlink files/directories are rejected; the local cache is app-owned and these checks do not claim protection against a concurrent hostile process with the same filesystem permissions. Save/publication checks share the same file/hash verifier.

The implementation uses the already locked `tempfile` crate, promoted from a development dependency. Its no-clobber persistence API never overwrites an existing destination and may leave an extra temporary hard link after interruption. We synchronize directory entries explicitly. Rust's Apple implementation uses F_FULLFSYNC for `sync_all`; unsupported synchronization errors propagate. Sources: [tempfile persistence contract](https://docs.rs/tempfile/3.27.0/tempfile/struct.NamedTempFile.html#method.persist_noclobber), [Rust Unix synchronization](https://doc.rust-lang.org/src/std/sys/fs/unix.rs.html), [directory synchronization requirement](https://www.man7.org/linux/man-pages/man2/fsync.2.html).

Five attachment tests pass: content naming/reopen/save, invalid names and changed bytes, symlink rejection, catalog failure with file retention/retry, and worker ordering. The full storage suite passes 52 tests and scoped Clippy passes. The crash harness adds a sixth child kill after attachment acknowledgment; a later process saves its reference using the retained file/catalog. Non-Unix directory synchronization returns an error; Windows implementation/acceptance remains open. Linux runtime and power-loss checks are also unverified.

Review leaves NR08 in progress: semantic dependency classification needs the Markdown layer; editor integration, Windows attachment synchronization, and fuller commit-I/O fault injection remain open. No notebook UI uses this service yet. The entire-workspace test gate remains open. The source `rt` sync format/graph/replica files remain unchanged through `ae02aab13f0725000a9ba09e72c9849b4d6eb9cb`; its current Go terminal-client work remains independent of the native GUI rewrite.
Status: in_progress

### NR09: Port publication and replica convergence

What to do: Port graph heads, import cursors, publisher, watcher/rescan, attachment delivery, conflict detection, and crash adoption. Reuse the seeded schedule model against Rust and mixed Rust/TypeScript replicas.
Constraints: Per-device ownership and distinct writer identities; immutable durable files; no clock-based last-write-wins. Never coalesce a revision that may already have been published.
Write set: `crates/quire_store/src/sync/`, durable-file adapters, replay fixtures.
Dependencies: NR08.
Verify: At least the existing 10,000 reproducible schedules plus real-filesystem kill cases; delayed/duplicate/partial delivery, missing parents, stale office window, collisions, late attachments, uncertain publish, and copied device identity. Inject edits while a batch write is in flight; coalescing cannot discard a revision already selected for publication. Check saved content and final heads in both runtimes.
Proof notes: NR09.1 connects frozen publication to durable folder files; NR09.2 adds atomic graph/cursor import. Folder convergence, status/checkpoints, watcher/rescan, mixed-runtime schedules, Windows synchronization, and cloud-provider acceptance remain open.
Status: in_progress

#### NR09.1: Publish frozen local batches and attachments to the notebook folder

What to do: Connect the durable local outbox to format 1 folder writes. Validate notebook identity and writer paths, reconcile the writer's observed log, publish referenced attachments before its batch, adopt a matching frozen batch after interruption, and acknowledge only after synchronization and final collision checks. Expose the operation through the existing bounded storage worker.
Constraints: Preserve immutable files and frozen source bytes; reject conflicting writer sequences and unrecorded own batches without overwriting them or inventing a new writer identity. Retain pending revisions after errors. Share file durability primitives with the attachment cache. The importer will handle own batches not matched by a frozen selection in a later child task. Windows support remains an explicit NR08/NR09 acceptance gate.
Verify: Real filesystem publish/reopen/retry, attachment delivery/hash validation, exact pending adoption, edits made after selection, corrupt/partial files, conflicting sequences, missing recorded files, changed/newer notebook headers, symlink/path rejection, worker ordering, and child-process kill/reopen after publication. Storage/reference tests, build, scoped Clippy, formatting, and vendor checks pass.
Proof notes: `NotebookStore::publish_to` and the existing bounded worker now publish one frozen batch to an existing format 1 folder. The operation validates `quire-notebook.json` and the writer's path, scans its sequence names, verifies the last acknowledged file, and adopts only an exact pending filename/byte match. It copies and verifies referenced attachments before installing the batch, scans again for collisions, then commits the cursor and removes only that batch's outbox selection. Later saves retain their revisions. Errors leave publication uncertain, disabling unsafe coalescing even when no batch was selected. Unrecorded own files without a matching frozen selection are preserved and refused until the importer can reconcile them.

The native publisher and local attachment cache share the extracted immutable-file helper. It writes to a same-directory temporary file, verifies its contents, synchronizes, installs without replacement, verifies/synchronizes adopted destinations, and synchronizes directory entries before database acknowledgment. Attachments stream through the existing file/hash verifier rather than loading whole files into memory. Directory creation is synchronized once per destination directory, including retries after interruption. A red test reproduced invalid copied bytes acquiring an immutable filename when validation happened only after installation. Validation now also runs before installation; failed verification and interrupted-write tests confirm that neither a destination nor a temporary file remains and that retries succeed. No schema, protocol, dependency, lockfile, or vendor implementation changed.

All 66 storage tests pass, including 12 new filesystem/worker publisher cases and two durability-helper cases. They cover attachment delivery, native format decode, chain continuation after reopen, exact frozen adoption with a later edit, partial batches, damaged attachments with retry, conflicting sequences, a copied writer identity from a different database, missing acknowledged logs, coalescing protection after failed scans, changed/newer/partial headers, unsafe writer paths, linked roots/directories/files, actual read-only directory failure, and worker ordering. The process-kill harness passes nine child-process cases, adding kill/reopen after folder acknowledgment, a later save, and a second publication while retaining the earlier log. These checks establish process-crash behavior on this macOS filesystem, not power-loss or cloud delivery.

`cargo test -p quire -p quire_store --locked --features quire/test-support` passes 99 application tests and 66 storage tests; the final scoped storage rerun also passes. Reference tests pass all 55. `cargo build -p quire -p quire_store --locked`, storage all-target Clippy with `-D warnings`, formatting, scoped whitespace, prose lint, and tracked-vendor verification pass. Upstream comparison remains at 3,013 unchanged files and 23 documented forks. The debug app retains the existing unwind-metadata linker warning. The inspected `rt` publisher/folder/identity files at `4985e4080d2302b2abb416f2db7fa390080786c8` match the frozen baseline.

Sequential review checked the shared file helper against the previous attachment implementation, temporary validation and cleanup, adoption synchronization, strict path/name boundaries, bounded file reads, preservation after failures, source selection and shared database state, worker admission/ordering, and unchanged dependencies. The simplification pass shares the durability operations and avoids repeated parent-directory synchronization per attachment. Same-user hostile filesystem races are outside this adapter's protection; concurrent or later cloud deliveries can still reveal a writer fork on the next scan. Own-log scans currently enumerate sequence metadata and re-read the last batch; NR35 must measure that work. Import/convergence, checkpoints, watcher/rescan, mixed-runtime schedules, Windows synchronization, and the real editor/notebook session remain open.
Status: done

#### NR09.2: Import revision graphs and advance replica cursors atomically

What to do: Port the frozen graph and batch-chain rules into the native store, preserving wire records. Apply complete incoming batches with their cursor in one transaction; distinguish duplicates, missing parents, invalid lineage, writer forks, and concurrent page heads. Expose structured results through the worker and retain pending local revisions/recovery.
Constraints: No clock-based winner, dropped heads, scalar conversion of lone surrogates, or body enumeration for metadata checks. Reuse native compression/validation and the existing schema; migrate explicitly if additional durable metadata is required. Own-batch adoption must not silently consume unrelated local saves. Missing attachments may be reported for later delivery without discarding revisions.
Verify: Differential graph/chain fixtures against the frozen TypeScript implementation; duplicate/out-of-order/partial delivery, missing/cross-page parents, invalid generations, concurrent edits and explicit merge, unchanged stale-window saves, own pending publication, copied writer identities, unknown-field/WTF-8 retention, and transaction rollback. Follow with folder scanning and the parent task's 10,000 mixed-runtime schedules.
Proof notes: `NotebookStore::import_batch` and the bounded worker now decode an incoming batch, check its chain and ordered parent metadata, and commit all revisions/heads plus the peer cursor together. Structured outcomes distinguish applied/duplicate batches, sequence gaps, missing parents, rejected graphs/newer formats, and a frozen own publication awaiting its publisher. Concurrent heads stay present until an explicit multi-parent save merges them. An unchanged stale-window save cannot resurrect an older head. Incoming revisions do not enter or consume the local outbox and do not replace local recovery. Attachment dependencies are returned for the folder adapter without requiring their bytes before importing the graph.

The existing validated revision insertion accepts a borrowed JSON value, so imported records retain unknown fields and WTF-8 strings without a scalar conversion or per-record JSON reparse. Changed-page reports use lossless `JsString` identities and include current heads. A reopen test checks an imported lone-surrogate page/path/body and unknown field exactly. Schema 1, dependencies, and copied sources remain unchanged. Own frozen batches deliberately remain with `publish_to`, which synchronizes the observed file before acknowledging it. An empty rebuilt store can import its own log; a store with unrelated pending local saves refuses unrecorded own batches rather than consume its outbox or fabricate a writer identity.

The final storage suite passes 80 tests, including 13 import integration cases and a cursor-failure rollback case. Coverage includes remote branches/merge, ordered parent dependencies, sequence gaps and duplicates, partial input, cross-page parents, invalid generations, current-sequence and previous-hash forks, unchanged stale saves, retained drafts, own pending publications, copied writer state, same-ID peer revisions preserving the local frozen outbox/provenance, newer formats, absent attachments, lossless wire records, and worker ordering. A temporary SQLite trigger aborts cursor insertion after revision writes; the whole graph rolls back and the identical batch succeeds after removing the injected fault.

`tests/graph-oracle.ts` executes the frozen TypeScript graph and chain functions. Sixty-four seeded native import schedules exercise 1,536 graph operations, including shuffled dependencies, duplicates, concurrent parents, tombstones, and invalid lineage; eight chain cases cover apply/gap/duplicate/fork behavior. All native outcomes and page heads match. This pure graph oracle uses Bun's TypeScript resolver and does not decode compressed files; protocol envelope comparison continues to use Node. The process-kill harness passes ten child cases, adding a Node-encoded peer batch whose revision, conflicting heads, and peer cursor survive kill/reopen together. These are current-filesystem process-crash checks, not power-loss or cloud-delivery acceptance.

Application regression tests pass all 99, the final scoped storage run passes all 80, and reference tests pass all 55. Native/store build, storage all-target Clippy with `-D warnings`, formatting, scoped whitespace, prose lint, and tracked-vendor checks pass. Upstream comparison remains at 3,013 unchanged files and 23 documented forks. The existing debug unwind-metadata linker warning remains.

Sequential review checked chain ordering, missing-versus-invalid graph handling before writes, transaction rollback, retained local heads/outbox/recovery, origin-preserving duplicate insertion, own-publication durability, borrowed wire values and lossless reports, worker admission, and unchanged schema/dependencies. The simplification pass reuses the existing insertion/storage path instead of adding a second graph store. Older batches follow the frozen duplicate rule without a historical hash lookup; folder scans still need to detect colliding sequence files. Filesystem discovery, dependency retries across devices, late attachment caching, checkpoints, 10,000 mixed-runtime schedules, Windows synchronization, performance measurements, and editor integration remain open parent work.
Status: done

#### NR09.3: Rescan notebook folders and converge delivered device chains

What to do: Connect verified format 1 folder reads to atomic batch import. Discover safe device/log entries, group sequence candidates, detect collisions, report partial or missing delivery, and repeat dependency waits while imports make progress. Route frozen own batches through durable publication adoption and copy/hash-verify delivered attachments into the local cache.
Constraints: Preserve all source files and local pending work. Validate notebook identity and reject linked/escaping paths; keep current protocol errors distinct from waiting delivery and newer formats. Retain bounded batch reads and off-thread I/O. Do not claim a watcher or cloud guarantee from a manual rescan.
Verify: Two native replicas and mixed TypeScript/native fixtures converge after delayed/reordered/duplicate delivery, retry missing parents and late attachments, preserve conflicts and tombstones, reject writer forks and partial files, and recover own frozen publication without consuming later saves. Add real-filesystem process-failure cases. The parent task still requires at least 10,000 reproducible schedules, checkpoints/status, watcher/rescan lifecycle, and OS acceptance.
Proof notes: NR09.3.1 provides restart-safe attachment requests and known batch history. NR09.3.2 connects folder discovery, dependency retries, fair continuations, streamed delivery, and frozen publication adoption. The full mixed-runtime schedule gate, checkpoints/status, watcher lifecycle, scale measurements, and OS acceptance remain required before this parent can close.
Status: in_progress

##### NR09.3.1: Retain attachment requests and batch hashes across rescans

What to do: Add an explicit schema 2 migration for a durable attachment request queue and known per-device batch hashes. Record dependencies/history with each imported cursor, record locally published hashes with acknowledgment, and clear a request only after its verified attachment file/catalog commit. Make metadata queries available through the bounded worker.
Constraints: Keep schema 1 unchanged and verify each schema/history before migration. Preserve revisions, drafts, outbox, frozen publication, existing catalog entries, and cursors. Upgrade atomically; failed upgrades must leave schema 1 recoverable. Reconstruct legacy attachment requests one body at a time with paged IDs during this one-time upgrade, avoiding body enumeration during routine rescans. Seed historical hashes from recorded cursors without inventing unavailable history.
Verify: Upgrade/reopen a populated schema 1 database without changing local state; migration rollback on a real SQLite size limit and damaged bodies; paged legacy attachment reconstruction, queued late attachments across reopen, catalog failure/retry, and historical fork rejection. Compare known-hash chain behavior with the frozen TypeScript callback contract. Full storage/crash/reference checks, build, Clippy, and source audits pass.
Rollback: An offline consuming-handle operation may restore schema 1 only when no attachment requests or noncurrent historical hashes would be discarded; test upgrade/down/upgrade and refused rollback preservation. Normal notebook opening never downgrades.
Proof notes: Schema 2 adds `attachment_needs` and `batch_hashes`. Imported cursors, known hashes, and declared dependencies now commit with the graph; published hashes commit with the local folder acknowledgment. Attachment catalog registration and request deletion share the verified file's transaction. Paginated request and hash queries use the existing bounded worker. Current cursor/history disagreement is rejected during preflight before changing journal policy. A red test reproduced accepting this disagreement; the guarded preflight now preserves the database byte-for-byte in the regression.

Schema 1's twelve DDL statements remain unchanged; a test pins their prior checksum `92d718278b41f834c594bbce9683f9af1a02017500cb3c9ae9a71e7e45fdb25f`. Both schema histories and the exact tables are checked before upgrading. Migration reads at most 128 revision IDs per page and one validated body at a time, using the existing conservative literal attachment reader and lossless wire strings. This one-time legacy pass seeds attachment requests and retains unknown fields/surrogates without rewriting bodies. Existing cursors seed known current hashes; unavailable older history stays unknown. Ordinary metadata queries do not read bodies.

Five migration tests cover populated upgrade/reopen with a frozen batch, later outbox save, draft, existing attachment catalog/file, 130 lossless legacy records across the ID-page boundary, and a recorded foreign cursor; SQLite's actual page limit failing mid-upgrade with schema 1 intact and successful reopen/retry; damaged body preservation with no partial schema; upgrade/down/upgrade preserving local work; and rollback refusing pending requests or a noncurrent historical hash. The offline rollback consumes its handle, verifies exact state, and restores schema 1 only when no added protection is lost. It has no worker/UI action and is never called by normal opening.

Additional cases verify restart-safe requests, old-fork rejection after later batches and reopen, bounded pagination across 2,003 requests without acknowledging or starving later filenames, queue/catalog transaction failure and retained-file retry, and worker query ordering. Import fault injection now aborts both at cursor insertion and later request insertion, verifying rollback of revisions, heads, cursors, hashes, and requests before retry. The frozen graph oracle uses its existing known-hash callback: 64 seeded schedules with 1,536 graph steps and eight chain cases pass. No reference implementation is changed.

The process-kill harness passes eleven child cases. A Node-encoded peer batch's cursor, hash history, and late attachment request survive kill/reopen; a later verified attachment/catalog acknowledgment clears the request and also survives kill/reopen. These are process-crash checks on the current macOS filesystem, not power-loss or cloud-provider acceptance.

Final gates: all 90 storage tests and 99 application tests pass; reference tests pass all 55. Native/store build, storage all-target Clippy with `-D warnings`, formatting, scoped whitespace, prose lint, and tracked-vendor checks pass. Upstream comparison remains at 3,013 unchanged files and 23 documented forks. No dependency, lockfile, wire format, or copied implementation changes. The existing debug unwind-metadata linker warning remains.

Sequential review covered schema 1 immutability, preflight before journal changes, additive upgrade/guarded down transactions, bounded legacy backfill, preserved acknowledged/local-only state, queue retention and fair pagination, atomic hash/cursor/dependency writes, cache/catalog synchronization, known versus unavailable history, and worker ownership. The simplification pass keeps these writes in the existing store transactions rather than adding a second retry service. Large legacy migration latency, Windows/runtime synchronization, folder discovery and streamed delivery, the full 10,000 mixed-runtime schedules, checkpoints/status, watcher lifecycle, and native editor integration remain open parent requirements.
Status: done

##### NR09.3.2: Import bounded folder chains and stream delivered attachments

What to do: Discover ordinary device/log folders, validate known sequence hashes, apply delivered batches through atomic import, and retry cross-device parent dependencies. Stream verified attachment files through the durable cache and resume queued delivery fairly across bounded calls. Reuse notebook identity/reader helpers in the publisher and expose rescans through the storage worker.
Constraints: Preserve all source files and exact own frozen publications. A rescan can adopt an already observed frozen file but cannot select/publish later local saves. Per-call batch/attachment counts and payload budgets must allow progress for one oversized item in each phase; continuations must not starve another device or a later attachment. Each batch commits independently, so later storage errors may leave an already committed prefix. A manual rescan is not a watcher or a cloud guarantee.
Verify: Two native replicas exchange edits/attachments, preserve conflicts, merge, and retry dependencies/partial or missing delivery. Check budgets/continuations under a busy device and unavailable earlier attachment; own frozen adoption with a later save, fork/newer-format/path rejection, empty files, streamed cache validation, and worker routing. Exercise Node-generated folder records and process-kill recovery. The parent's 10,000 schedules and watcher/checkpoint/status acceptance remain open.
Proof notes: `NotebookStore::rescan`, exposed through the existing bounded worker, discovers ordinary device/log folders, rejects colliding or known conflicting sequence hashes, and imports complete batches through the atomic graph path. It repeats parent waits while other devices make progress and reports sequence/file/attachment waits separately from protocol, path, fork, and newer-format problems. Changed-page reports retain lossless identities. Each batch commits independently; README and NR11 explicitly require refreshing mounted heads even after a later storage failure.

Count limits and separate compressed-batch/attachment-payload budgets return a continuation after the last processed device and attachment name, permitting one oversized atomic item alone in each phase. The first fairness regression returned `bbb` after stopping before its first file, which let a continuously writing `aaa` take its next turn. The cursor now records the last device actually read. The red test passes while `aaa` receives another publication between calls. A separate case confirms that an unavailable earlier attachment cannot starve a delivered later request. Budget exhaustion is represented by `Option`, so an actual empty file remains a protocol error rather than an ambiguous control value.

The publisher shares notebook identity/bounded-read helpers with the folder adapter. Rescan adoption requires both the observed exact frozen selection and an already written matching file; it cannot prepare a later local save. Tests cover missing files and stale selection callbacks, plus reopening/adopting an exact batch while retaining a later outbox revision and the original file bytes. Local publication certainty is restored only when the own chain has no unrecorded batch or own-path problem.

`cache_attachment_file` verifies and streams ordinary source files through the existing immutable installer and catalog/request transaction. Byte and file APIs share registration, hash/size checks, synchronization, and request acknowledgment. An 8 MiB case checks streamed copying, a linked source, changed download bytes, and a damaged destination without overwriting bad files. A shared usable-store guard rejects queued cache work before file I/O after an uncertain commit; rescans stop on that storage state rather than continue copying.

All 105 storage tests pass, including twelve folder cases and the new cache/adoption guards. Filesystem cases cover two native replicas exchanging attachments and divergent edits, retained heads and explicit merge, later-sorted parent delivery in one scan, missing sequences and tombstones, zero-byte/partial delivery with retry, count and both payload-budget continuations, unavailable attachment fairness, historical forks/newer formats, linked directories/files, exact own adoption, and worker ordering. The existing 64-schedule graph/chain comparison also passes.

The process-kill harness passes twelve child cases. Its new folder case places a Node-encoded second peer batch into the device log, runs native rescan, kills after acknowledgment, and verifies the advanced cursor, retained original hash history, updated peer head/body, and concurrent local head after reopen. This proves the exercised local-filesystem process-crash path, not cloud-provider delivery or power loss.

Final gates: combined Quire/storage tests pass 99 application and 105 storage cases; reference tests pass all 55. Native/store build, scoped storage all-target Clippy with `-D warnings`, formatting, scoped whitespace, prose lint, and tracked-vendor verification pass. Upstream comparison remains at 3,013 unchanged files and 23 documented forks. No dependency, lockfile, schema, wire-format, or copied implementation changes. The existing debug unwind-metadata linker warning remains.

Sequential review checked source ownership and path guards, missing-versus-invalid delivery, historical hash checks, independent batch transactions, bounded packet reads, continuation fairness, exact frozen-file adoption without later publication, local certainty/coalescing, streamed cache registration, poisoned-store handling, and worker ownership. The simplification pass shares filesystem readers, the immutable installer, and cache registration. Directory discovery still enumerates filename metadata and queries known hashes; payload budgets count content sizes rather than every verification read or decompression allocation. Whole-file work, including one oversized item, can delay other storage requests and needs NR35 release measurements. Full mixed-runtime schedules, watcher/checkpoint/status behavior, Windows synchronization, scale/desktop acceptance, and editor integration remain open.
Status: done

##### NR09.3.2a: Retain local revisions observed in peer publications

What to do: Persist evidence that a peer batch contains a local pending revision and exclude it from save coalescing. Upgrade existing stores conservatively without changing revision provenance, outbox selection, or wire format.
Constraints: Commit exposure metadata with the incoming graph/cursor. Preserve schema 1/2 definitions and exact publication acknowledgment. Refuse rollback when the older schema cannot retain the protection.
Verify: Reproduce the unselected duplicate-revision deletion, then verify retention across reconciliation/reopen and later peer descendants, import rollback/retry, populated upgrades, failed upgrades, and guarded down/up migration. Run all storage and process-crash gates.
Proof notes: The focused `a_peer_observed_revision_cannot_be_coalesced_out_of_the_local_graph` test failed on the previous implementation. Revision IDs exclude writer identity, so a duplicate peer record retained local provenance and remained eligible for coalescing after its peer cursor advanced. Schema 3 adds a foreign-key-protected observation set. Import records pending duplicate IDs in the same transaction as graph/cursor/history, and save excludes them from coalescing. Local provenance, pending outbox, frozen selections, and the wire format remain intact. Upgrades conservatively protect existing outbox IDs; schemas 1/2 have pinned unchanged checksums. Offline rollback refuses observed unselected saves, while frozen selections can survive down/up without losing their existing protection. No new dependency or vendor fork is required.

The regression now verifies unchanged provenance/outbox, close/reopen and restored publication certainty, retained ancestors, and successful later peer descendants with both concurrent heads. A cursor-failure test proves observation metadata rolls back and retry protects the parent. Schema 2 upgrade tests preserve pending saves, refuse unsafe rollback, allow a frozen down/up round trip, and roll back a failed backfill/history commit before successful reopen. Existing populated schema 1, full-disk migration, damaged-body, and rollback refusal tests remain green. `cargo test -p quire_store --locked` passes 109 tests; `bun run test:reference` passes 55; `CARGO_HOME=/private/tmp/quire-cargo bun run test:storage-crash` passes the 12 process-kill checks. Storage all-target Clippy with `-D warnings`, native build, formatting, prose lint, and scoped whitespace pass. The existing debug unwind warning remains.

Sequential review checked observation timing, rejected/duplicate batches, rollback atomicity, safe publication selection, conservative migration, unchanged schemas, and deletion restrictions. The simplification pass retains existing save/import/publication paths and adds only the persistent protection needed by the reproduced failure. Migration prevents future coalescing loss; it cannot recover revisions already deleted by an older build. The 10,000 schedule gate and parent feature work remain open.
Status: done

##### NR09.3.3: Verify at least 10,000 reproducible replica schedules across runtimes

What to do: Connect a persistent native storage/replica probe to the frozen TypeScript replica model and seeded delivery scheduler. Exercise real native save/select/publish/import/rescan operations and independent delivered-folder views without starting one process per action. Check saved contents and final heads for native/native and mixed replicas.
Constraints: Use the production native store and folder loop, retain reproducible seeds/traces, and keep frozen sources unchanged. Include delayed, duplicate, partial, and reordered delivery, missing parents/attachments, concurrent edits, stale windows, merges/tombstones, uncertain publication/reopen, and writer collisions. Assert frozen-selection safety during edits in flight even where the reference needs a separately recorded stronger native guarantee. Simulated/provider schedules do not establish real cloud or power-loss behavior.
Verify: At least 10,000 deterministic schedules finish with the expected saved content and graph heads; failing seeds replay independently. Retain the real-filesystem crash gates and add any schedule-discovered regression to focused tests. Record runtime and storage work counts; keep long-running checks observable while working on independent features.
Proof notes: pending.
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
Constraints: Shared buffers keyed by notebook/page identity; expected-version checks on asynchronous operations. Opening a notebook loads metadata and the needed page, not every body. Refresh mounted-page heads after a rescan error as well as success, because already committed batches can precede a later failure.
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
