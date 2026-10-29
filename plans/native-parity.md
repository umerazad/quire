# Native rewrite parity ledger

This is a release checklist, not evidence that the native features exist. All rows are **pending**. Record a native test/report and result against every row before NR37. [Architecture](native-rewrite.md) and [task queue](native-rewrite-tasks.md) define implementation boundaries.

The 47 S-rows below preserve each row of the [source storage inventory](../../rt/plans/storage-engine-inventory.md), in the same order. Source test names refer to `rt/tests/<name>.test.ts` or `.tsx`, its `tests/e2e/` directory, or its named scripts. They are behavioral references; a passing legacy test does not prove native parity. Cache mechanisms can change while their correctness/performance contract stays covered.

## Storage and notebook behavior

| ID | Existing behavior | Native acceptance | Source test references | Owning tasks |
| --- | --- | --- | --- | --- |
| S01 | Notebook open at startup | Open only the restored page and required embeds; record first editable state and bounded body loads. | vault, notebook-scale, bootstrap, production-renderer, renderer-smoke | NR11, NR12, NR34 |
| S02 | Open another folder | Native folder dialog switches safely after flushing; cancel leaves the current notebook intact. | Manual only: driven by a native folder dialog | NR13, NR33, NR35 |
| S03 | Analysis cache | Incremental analysis matches a full parse and invalidates on parser/schema changes. | analysis-cache, analyze-pages | NR10, NR12 |
| S04 | Startup text cache | Replace full-text startup caches with indexed metadata and bounded on-demand text; no stale startup state. | vault | NR12, NR34 |
| S05 | Shared edit buffer, undo, redo | One page buffer/history across Source, Write, splits and embeds; undo from either view. | store, e2e editor | NR03, NR04, NR11, NR15 |
| S06 | Save with revision check | Expected-version save refuses stale content; acknowledgment follows durable local commit. | store, vault, window-ui, e2e editor | NR08, NR11 |
| S07 | Conflict resolution | Both versions remain available; resolving checks the observed version and retains later edits. | store, e2e editor | NR09, NR25 |
| S08 | Recovery drafts | Latest acknowledged draft survives kill/restart and is cleared only by a matching save or explicit resolution. | store, vault | NR08, NR25 |
| S09 | Other devices' drafts | Preserve other-device drafts independently; do not overwrite the main page while adopting them. | notebook-storage | NR13, NR25, NR36 |
| S10 | Legacy storage migration | Import legacy names, storage and metadata without deleting the source; migration is resumable. | notebook-storage, vault, legacy-names | NR13, NR36 |
| S11 | External change events | Refresh clean buffers and preserve dirty ones as conflicts; rescan recovers missed watcher events. | vault-monitor, store, e2e editor | NR09, NR13 |
| S12 | Flush on quit | Close waits for local durability and attempts publication; failures expose retry or a safe explicit quit. | close-coordinator, desktop-close, e2e editor | NR11, NR33 |
| S13 | One instance per data folder | Prevent competing writers to one local database; handle activation and protect legacy folder ownership. | single-instance | NR33, NR36 |
| S14 | Create page | Create pages with stable identity and collision rules, including simultaneous named/daily creation. | editor, file-actions, vault | NR11, NR16 |
| S15 | Rename or move page | Keep IDs, titles, history and affected links; interrupted moves recover and case-only names work. | relocate, file-actions | NR25 |
| S16 | Delete page | Preserve external-delete and relocation behavior; a general delete UI remains a stated non-feature. | Covered through relocation and external events | NR13, NR25 |
| S17 | Create folder | Create and display empty folders; match the final source rule for local-only folders in format 1. | file-actions, relocate | NR13, NR16 |
| S18 | Move block to another page | Move descendants and references with local atomicity; failures cannot remove the source prematurely. | editor, vault, notebook-storage | NR14, NR25 |
| S19 | File tree | Virtualized physical tree includes attachments/empty folders while hiding app metadata. | page-list, workspace, renderer-smoke | NR16 |
| S20 | Page browser and namespaces | Logical titles/namespaces stay independent of physical paths; unloaded pages remain discoverable. | notebook-browser, notebook-indexes, renderer-smoke | NR12, NR16 |
| S21 | Insert attachment | Retain attachment bytes durably and enforce notebook boundaries, including symlink escapes. | vault ("copies an attachment into the notebook and refuses a linked attachments folder") | NR21 |
| S22 | Render attachments | Render image/PDF syntax, preserve external links, and recover gracefully from missing/late files. | media-code, editor, readme | NR21 |
| S23 | Page history | Show retained revisions and restore into a new page; match the agreed coalescing/history policy. | history-store, page-history, vault, store | NR25 |
| S24 | Full-text search | Match ranking/snippets and short/Unicode queries, including unsaved edits, within the search budget. | index, editor | NR12, NR34 |
| S25 | Backlinks and linked references | Correct grouping, ordering, ancestor context and source navigation for loaded and unloaded pages. | notebook-indexes, linked-tasks | NR15, NR17 |
| S26 | Mentions of a title | Find mentions even without an existing target page; replace stale overlay contributions. | notebook-indexes ("lists mentions of a title, including one with no page yet") | NR12, NR17 |
| S27 | Tags and tag pages | Exact normalized/nested/Unicode tags, frontmatter exclusions, filters and bounded context; no incidental IDs. | tags, tag-page, tag-navigation, notebook-browser, editor, renderer-smoke | NR17 |
| S28 | Task views | Keep all task states, Due, Projects, next-action indicators, paging and source navigation. | tasks, tasks-page, linked-tasks | NR18, NR19 |
| S29 | Task changes and dates | Preserve stamps, inherited due dates/project links, reopen semantics, bulk edits, IDs and shared undo. | tasks, tasks-page, editor, renderer-smoke | NR18, NR19 |
| S30 | Task capture | Capture into Inbox or today's daily page with current state/stamp rules and local draft retention. | tasks, tasks-page, daily-layout, linked-tasks | NR18, NR19 |
| S31 | Templates | Expand supported templates/macros with fresh referenced IDs and one undo; reject unsupported commands before edits. | templates, template-input, editor | NR20 |
| S32 | Daily notes | Open/create today and resolve supported date aliases, configured layouts, month filters and duplicates. | daily-layout, workspace, renderer-smoke | NR11, NR16 |
| S33 | Notebook settings and daily layout | Validate notebook settings and carry them in the format 1 log; keep machine preferences separate. | daily-layout, notebook-settings, vault ("replaces notebook settings in one step and rejects invalid JSON") | NR09, NR16 |
| S34 | Block ids | Preserve existing IDs; allocate only when needed; copies use fresh IDs where current semantics require them. | store, tasks, editor, markdown | NR10, NR14, NR15 |
| S35 | Block references | Resolve wiki/Roam forms and completion without losing delimiters, descendants or source identity. | markdown, store, editor | NR15 |
| S36 | Editable embeds | Editable full subtrees share the original buffer; inline previews remain compact; cycles and bounds are safe. | editor, e2e editor, renderer-smoke | NR04, NR15 |
| S37 | Profiles | Keep Current/Work/Personal isolation, overlap rules, locations and separate operation status. | profiles, data-settings, navigator-app | NR33 |
| S38 | Backups and restore | Verified complete snapshots restore into new roots/identities, including drafts, history, hidden metadata and attachments. | backups, data-settings | NR26 |
| S39 | Git sync | Personal-only Git matches the final source algorithm; failed pushes preserve work and exclusions remain correct. | git-sync, profiles, data-settings, notebook-storage | NR27 |
| S40 | Roam import | JSON/EDN imports retain IDs/order/metadata, original archive bytes and detached-block recovery. | import, roam-edn, roam-files, roam-worker, renderer-smoke | NR24 |
| S41 | Roam export | Round-trip metadata and references from either backend; verify the new engine rather than assuming parity. | import, roam-files, renderer-smoke | NR24 |
| S42 | Page export | Flatten embeds and links correctly; Markdown works offline; AI PDF/Word export validates and cancels safely. | page-export | NR23, NR31 |
| S43 | AI notebook tools | Read the live graph including unsaved content; keep write policies, stale checks and tool schemas. | notebook-tools, ai-changes, ai-page | NR12, NR29, NR30 |
| S44 | Tool server and MCP | Token-checked bridge and stdio protocol work from installed terminal clients; closure yields actionable errors. | notebook-bridge, mcp-server, ai-runner, e2e editor | NR29 |
| S45 | AI run logs | Preserve private prompt/answer transcripts in the correct notebook and exclude them from Git as before. | ai-runner | NR28, NR30, NR31 |
| S46 | Workspace state | Persist notebook-scoped tabs/favorites/history/scroll/filter state locally; migrate settings explicitly. | workspace, tag-page, ai-page, renderer-smoke | NR16, NR32, NR33, NR36 |
| S47 | Reveal in Finder or Explorer | Reveal real files for folder notebooks and a read-only exported page for format 1, matching the final source behavior. | vault ("reveals items inside the notebook only"), tab-menu | NR13, NR16, NR23 |

## Editor and application behavior beyond the storage inventory

| ID | Required behavior and acceptance | Source evidence | Owning tasks |
| --- | --- | --- | --- |
| E01 | Write and Source share text, selection intent and undo; Markdown presentation retains headings, quotes, tables, rules, links, code and frontmatter behavior. | `markdown`, `editor`, `readme`, bundled `src/content/README.md` | NR04, NR10, NR14, NR32 |
| E02 | Enter/Tab/Shift-Tab, move/join/paste/renumber, folding, zoom, hidden-marker caret boundaries and subtree selection match current behavior. | `block-commands`, `outline-caret`, `list-numbers`, `block-wrapping`, `scripts/check-outline-editing.mjs` | NR04, NR14 |
| E03 | Vim modes, motions/operators/registers/search, prompts, shared undo/redo, :w, outline overrides, task shortcuts and :ai work in all editor views. | `src/editor/vim-bindings.ts`, `editor`, `editor-mac`, `scripts/check-vim-tasks.mjs` | NR03, NR14, NR19, NR28 |
| E04 | English, Arabic and Urdu retain suitable fonts, diacritics, shaping, mixed-direction selection, grapheme editing and wrapping; composition input works on each OS. | `script-lang`, `script-lines.ts`, `script-fonts.css`, `readme`; native coverage is incomplete in the source | NR05, NR32, NR35 |
| E05 | Inline image/PDF presentation, local/remote/data sources, missing-media handling, Open PDF and large-document memory remain usable. | `media-code`, `readme`, `MediaEmbed.tsx` | NR06, NR21, NR35 |
| E06 | Code highlight/copy and formatting for all seven supported languages work; stale formatting cannot overwrite newer edits and applies as one undo. | `media-code`, `src/editor/code-tools.ts` | NR06, NR22, NR35 |
| E07 | Supported Roam template controls and task/date/random/breadcrumb commands keep current output semantics; unsupported scripts remain inert source. | `templates`, `template-input`, `plans/roam-template-commands.md` | NR20 |
| E08 | Task shortcuts, project membership, date controls, inherited links, linked-task filters and capture defaults match editor and card behavior. | `tasks`, `tasks-page`, `linked-tasks`, `task-dates` plan | NR18, NR19 |
| E09 | AI prompt/range transforms, streaming proposals, accept/reject/undo, blank-line drafting, synthesis and Tidy preserve current limits and stale checks. | `ai-commands`, `editor-ai`, `ai-changes`, `ai-page`, `tidy` | NR28, NR30 |
| E10 | AI Off/Review/Apply, terminal changes, CLI testing/cancellation/version errors, citations, keep-in-daily, transcript privacy, PDF/Word export remain available. | `ai-runner`, `ai-settings`, `notebook-tools`, `notebook-bridge`, `mcp-server`, `page-export` | NR28, NR29, NR30, NR31 |
| E11 | System/light/dark, font/spacing/Vim/tag settings, semantic tag colors, reduced motion, keyboard focus, native menus/window controls and dialogs work. | `appearance`, `dark-theme`, `editor-preferences`, `tag-tones`, `window-ui`, `data-settings` | NR05, NR32, NR33, NR35 |
| E12 | Help and shortcuts are offline; README is editable isolated sample content with reset, media and multilingual examples. Neither enters real notebook queries or exports. | `help-page`, `readme`, `src/ui/help-content.ts`, `src/content/README.md` | NR32 |
| E13 | Page relocation preserves title, IDs, children and affected qualified/relative links; interrupted multi-file changes are recoverable. | `relocate`, `file-actions`, `vault`, `notebook-storage` | NR25 |
| E14 | Format 1 matches the finished source across Rust and TypeScript, including replaced unpublished saves, uncertain publication, page-name collisions and newer formats. | `sync-format`, `sync-harness`, `engine`, `git-sync`, `tests/e2e/quire-notebook.spec.ts` | NR07, NR08, NR09, NR27 |
| E15 | First editable state, viewport query latency, input/frame timing, memory and concurrent-background-work budgets pass on controlled release builds. | Source `plans/storage-engine.md`, `scripts/synthetic-notebook.ts`, `scripts/bench-startup.mjs`; new native measurements required | NR01, NR11, NR12, NR34 |
| E16 | Install and run independently of source checkouts and developer runtimes; native input, media, MCP, close and restore pass in packaged apps. | `production-renderer`, `desktop-close`, `scripts/test-packaged.ts`; replace Electron-specific checks | NR35, NR36, NR37 |

## Evidence to record

For each row, record the native test names, exact build revision and command, pass/fail result, and a link to any required OS/provider report. Use deterministic content/ID/query assertions for semantics, process tests for durability, native event tests for interaction, and traces for timing. Screenshots supplement those checks.

Track folder and format 1 results separately for storage-dependent rows. Track each OS separately for input, dialogs, PDF, packaging and accessibility. Unavailable hardware or providers remain explicit gaps. Reconcile this ledger once more with the final `rt` handoff before calling feature parity complete.

## Known source limitations to preserve honestly

The source README lists incomplete native composition/accessibility/Windows verification, no general delete UI, no cross-file undo, no standalone fenced-code references, and no complete rich-formatting toolbar. Tasks have no recurrence, priority, schedule engine, or arbitrary query blocks. Imported Roam JavaScript is not executed. These are not accidental native regressions, and source limitations do not waive this plan's explicit native input and platform acceptance gates.

The source storage plan still contains incomplete lazy queries, real-provider measurements, and export/failure evidence despite some tasks being marked done. Those are covered by the queue rather than inherited as verified behavior.

