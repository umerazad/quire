# NR06: PDF and code-formatting selection

<!-- Audience: the NR06 coordinator and whoever implements NR21, NR22 and NR35. They know GPUI and the rt app, may assume GPUI or Zed already covers PDFs and formatting, and fear a dependency that breaks licensing, packaging, or parity late. By the end they should be able to approve or reject the two choices below and know what each costs. -->

rt 0.9.57 gets both features from its runtime. Inline PDFs are an `<iframe>` (an embedded browser frame) that Chromium fills with its built-in viewer, and "Format code" runs Prettier 3.9.10 in the renderer process. Neither exists in a GPUI application, and Zed does not ship a PDF renderer or a self-contained formatter. This document covers small, isolated spikes of four PDF renderers and four formatter routes on this machine, and recommends **pdfium through `pdfium-render`, with a pinned prebuilt library bundled per platform**, for PDFs and **unmodified Prettier 3.9.10 running in an embedded QuickJS-ng engine (`rquickjs`)** for formatting.

All numbers come from macOS 27.0 on Apple silicon (arm64), release builds, measured on 2026-10-31. Linux and Windows were not measured. Raw results are in `results/`.

## Question and parity bar

The question is which implementation gives each feature rt parity on all three native targets, without requiring a developer runtime on the user's machine (the NR06 constraint), at an acceptable size, speed, memory, and license cost.

**PDF parity.** rt renders `![[file.pdf]]`, standard image syntax pointing at a `.pdf`, `{{pdf: ...}}` and `{{[[pdf]]: ...}}` targets, `data:application/pdf` URLs, and HTTPS URLs (`src/editor/MediaEmbed.tsx`, `src/core/attachments.ts`, `src/core/markdown.ts`). The frame is 650 px tall, capped at 80% of the viewport, and scrolls internally; a toolbar offers "Open PDF" or, for data URLs, "Save PDF". Chromium's viewer is built on PDFium, so in rt the user also gets text selection and copy, find within the PDF, zoom, page navigation, fillable-form display, and `#page=N` fragments (rt's own test uses `paper.pdf#page=3`). Which of those the native view must reproduce is an open question below; the renderer choice should not rule them out.

**Formatting parity.** `src/editor/code-tools.ts` calls Prettier's standalone build with seven parsers and one option, `tabWidth: 2`. Fence tags map as follows: `js`/`jsx`/`javascript` to `babel`, `ts`/`tsx`/`typescript` to `typescript`, `json`, `css`, `html` and `xml` to `html`, `yaml`/`yml` to `yaml`, and `md`/`markdown` to `markdown`. A user notices any difference from Prettier's default output (80 columns, double quotes, semicolons, trailing commas "all", Prettier's HTML whitespace rules). The error text the user sees also embeds Prettier's own message. Byte-identical output is therefore the bar.

## Method

Each spike is a standalone Cargo package with an empty `[workspace]` table, or a Bun script, so nothing enters the quire workspace, `Cargo.lock`, or the `--locked` gates. Downloads, build output, and `node_modules` are ignored by `spikes/nr06/.gitignore`.

**PDF.** `pdf/` contains one benchmark per renderer (`src/bench_*.rs`, `pdfkit-bench.swift`). Each opens a file, renders every page to a 1600-pixel-wide BGRA (blue, green, red, alpha) buffer, which is GPUI's `RenderImage` layout, and keeps either the last 6 or the last 1 rendered pages, the way a scrolled inline view would. pdfium, mupdf, and PDFKit then extract all text and count matches for "tax". Peak resident memory (RSS) comes from `/usr/bin/time -l`. `pdf/diff.sh` compares the first two pages of each file against pdfium, because pdfium is the engine behind rt's viewer. `fetch-fixtures.sh` downloads the fixtures and records their SHA-256 hashes:

- `irs-p17.pdf`: IRS Publication 17, 142 pages, 3.1 MB, tables and embedded fonts. This is the 100+ page case.
- `attention.pdf`: arXiv 1706.03762v7, 15 pages, figures and math fonts.
- `irs-f1040.pdf`: IRS Form 1040, 2 pages with AcroForm fields (fillable form widgets).
- `irs-f1040-aes256-empty-user.pdf`: the same form re-encrypted with AES-256 and an empty user password by qpdf 12.4.2.
- `rt-readme-sample.pdf`: the one-page sample embedded in rt's README, which uses the non-embedded Helvetica font.

**Formatting.** `format/corpus.ts` holds 21 unformatted snippets covering every fence tag rt accepts, including rt's own test case `const x={a:1}`. `format/prettier-ref.ts` makes the exact rt call to produce `format/expected/prettier/`. Each candidate formats the same files into `format/out-*`, and `format/compare.ts` counts identical files and changed lines. Two large real inputs check behavior at scale: rt's `src/editor/Editor.tsx` (2,577 lines) and `src/ui/styles.css` (4,010 lines). Both are already Prettier-formatted in rt, so any change is a parity difference.

## PDF results

Render time is per page at 1600 px wide. RSS is for the whole process, keeping 6 rendered pages (about 76 MB of bitmaps), and for pdfium and mupdf it includes extracting all text.

| Renderer | p17 mean / p95 / max ms | p17 total s | p17 RSS MB (6 pages / 1 page) | attention mean / p95 ms | 1040 mean ms | open ms (p17) | text + search, p17 |
|---|---|---|---|---|---|---|---|
| pdfium (chromium/8125) | 6.8 / 8.4 / 14.1 | 0.97 | 218 / 154 | 9.9 / 30.0 | 27.6 | 12.1 | 988,190 chars, 3,133 hits, 504 ms |
| hayro 0.8.0 | 11.7 / 14.8 / 17.3 | 1.66 | 153 / 88 | 20.8 / 93.5 | 12.0 | 3.3 | no API |
| mupdf 0.8.0 | 9.4 / 14.9 / 41.7 | 1.34 | 155 / 92 | 18.8 / 78.5 | 22.8 | 7.1 | 1,009,903 chars, 3,133 hits, 668 ms |
| PDFKit (macOS only) | 6.0 / 11.3 / 41.6 | 0.86 | 327 / not run | 22.0 / 77.4 | 62.1 | 37.7 | 954,839 chars, 3,118 hits, 468 ms |

Every renderer drew every page of every fixture, including the encrypted file and the non-embedded Helvetica sample. hayro's render of the encrypted form is pixel-identical to its render of the original (`results/pdf-encrypted.txt`). Against pdfium, the other renderers differ in 0.5% to 2.3% (hayro), 0.6% to 2.1% (mupdf), and 0.8% to 4.7% (PDFKit) of pixels by more than 64 of 255 in any channel (`results/pdf-pixel-diff.jsonl`). I inspected the Form 1040 page from pdfium, hayro, and PDFKit side by side: all three show every field, box, and line of text. The differences are glyph antialiasing and hinting, not missing content.

The fixtures do not cover scanned pages, JBIG2 or JPEG 2000 images, CJK (Chinese, Japanese, Korean) fonts without embedded glyphs, or transparency groups. hayro's own crate documentation lists blending, isolation, and knockout groups as unsupported, so those cases would likely separate hayro from the others; this spike did not test them.

Binary cost (`results/sizes.txt`): pdfium adds a 7.4 MB library on macOS arm64. The prebuilt archives are 3.5 to 3.9 MB for macOS, Linux, and Windows on x64 and arm64, so all six target triples exist. The `pdfium-render` wrapper adds about 2 MB. hayro adds about 5 MB including embedded fonts and character maps. mupdf adds about 4.7 MB and compiles its C sources in 25 s. PDFKit adds nothing but exists only on Apple platforms.

GPUI can draw any of these outputs. `RenderImage::new` takes BGRA frames, `img()` accepts an `Arc<RenderImage>`, and `Window::drop_image` evicts a texture from the atlas when a page scrolls away (`vendor/zed/crates/gpui/src/assets.rs`, `window.rs`). The wgpu atlas sizes textures up to the device's `max_texture_dimension_2d`. A 1600 by 2071 page is 13 MB of BGRA, so bounded memory is a cache-policy question: keeping 6 pages costs about 76 MB, and the 1-page runs show the renderer's own floor.

## Formatting results

| Route | Identical (of 21) | Changed lines, 21 snippets | Editor.tsx / styles.css changed lines | Per-snippet latency, warm median / worst first call | Editor.tsx latency | Size added | Peak RSS |
|---|---|---|---|---|---|---|---|
| Prettier 3.9.10 in QuickJS-ng (`rquickjs` 0.14) | 21 | 0 | 0 / 0 | 3.6 ms / 11.6 ms, plus 99 ms one-time init | 783 ms | 3.4 MB | 29 MB |
| Prettier 3.9.10 in a `bun build --compile` helper | 21 | 0 | not compared, same Prettier | 1.8 ms / 49 ms, plus 70 ms spawn (1.38 s on first launch) | 130 ms first, 69 ms warm | 64.5 MB | 97 MB |
| Biome 2.6.4 CLI | 15 | 34 | 0 / 4 | about 120 ms per process | not timed | 77.5 MB | not measured |
| dprint family (Rust crates) | 7 | 102 | 684 / 30 | 0.04 ms / 1.9 ms | 19 ms | 8.2 MB | not measured |

The QuickJS route evaluates Prettier's published UMD bundles (2.3 MB of JavaScript, embedded with `include_str!`) and calls `prettier.format` with rt's options. Because it is the same code, every output is byte-identical, including the 2,577-line TSX file and the 4,010-line stylesheet. QuickJS is an interpreter, so large inputs are about 11 times slower than in Bun's JIT (just-in-time compiler): 772 ms against 69 ms on Editor.tsx. Code blocks in notes are usually short, and every corpus snippet finished in under 12 ms.

Biome matches Prettier exactly on all 12 JavaScript, TypeScript, JSX, and JSON cases and on rt's Editor.tsx. It differs on HTML (0 of 3; Biome documents its HTML formatter as experimental), joins a comment onto the following CSS rule, keeps repeated spaces inside a Markdown heading, and converts a setext heading to `#` style. Its Rust crates are not published as a stable library, so shipping it means shipping the 77.5 MB CLI per platform. Biome's language-support table is at <https://biomejs.dev/internals/language-support/>.

The dprint family (`dprint-plugin-typescript` 0.96.1, `dprint-plugin-json` 0.25.2, `dprint-plugin-markdown` 0.28.2, `malva` 0.16.0, `markup_fmt` 0.27.6, `pretty_yaml` 0.6.0) is the fastest and smallest pure-Rust route. Matching Prettier is not its goal, though. After tuning to Prettier-like options (`format/rust/src/fmt_dprint.rs`), it still breaks method chains, one-per-line number arrays, JSX attributes, CSS multi-value transitions, and HTML inline elements differently. It keeps `1.50`, adds `;` inside type literals, and rewrites setext headings. By default it also removes braces and sorts imports, which the tuned run disables. YAML matched on both cases.

Zed's own route does not fit. Its `prettier` crate starts Prettier through `node_runtime`, which downloads Node.js from nodejs.org and installs Prettier with npm into the user's data directory (`vendor/zed/crates/node_runtime/src/node_runtime.rs`, `project/src/prettier_store.rs`). That is the undeclared runtime installation NR06 forbids, and it fails offline.

## Recommendation: PDF

Use pdfium through `pdfium-render` 0.9.x, loading a pinned prebuilt library from `bblanchon/pdfium-binaries` (`chromium/8125`, archive hash recorded in `fetch-fixtures.sh`) that is shipped inside each application package. Render pages on one background worker into BGRA buffers sized to the view width times the scale factor. Keep a small page cache and evict textures with `drop_image`.

pdfium is the parity reference rather than a proxy for it: rt's viewer already uses PDFium, so this choice reproduces rt's rendering by definition. It was also the fastest cross-platform renderer on the 142-page file, at 6.8 ms mean and 14.1 ms worst per page, which is 1.7 times faster than hayro. It is the only cross-platform candidate with text extraction, search, and per-character boxes already exposed, which a text-selection or find feature needs. It renders AcroForm fields when `render_form_data(true)` is set.

Operating envelope and cost:

- **Size.** 3.5 to 3.9 MB compressed and 7.4 MB installed per platform, plus about 2 MB of wrapper code.
- **Threads.** pdfium is not thread-safe. With its default `thread_safe` feature, `pdfium-render` serializes every call behind one process-wide lock, so all PDF work belongs on a single worker. A 142-page document needs about 1 s of CPU to render every page once, so views must render only visible pages.
- **Memory.** About 154 MB RSS with one cached page while rendering and extracting text from all 142 pages; each extra cached 1600 px page adds about 13 MB.
- **Security.** Chromium runs PDFium inside a sandboxed process. In quire it would parse remote, untrusted PDFs inside the application process with the same privileges as the notebook. This is the largest risk, and an open question below.
- **Supply chain.** The prebuilt binaries come from a third-party project's CI. Building pdfium from source in quire's CI needs Google's `gn` and depot_tools toolchain, which this spike did not attempt. Security fixes arrive through Chromium's release cadence and need a bump-and-verify step.
- **Version skew.** `pdfium-render` 0.9.4's `pdfium_latest` feature targets pdfium build 7881, but the spike loaded build 8125. Every call used here worked, because the bindings resolve symbols at load time. NR21 should pin a matching pair, or enable the explicit version feature and test it.
- **Packaging.** NR35 must place and sign the dynamic library inside each package (macOS requires a signed library before notarization).

Put the renderer behind a small trait: open, page count, page size, render page to BGRA, page text, and search. The renderer itself is then a two-way door. hayro remains the drop-in fallback if pdfium's packaging or security posture becomes unacceptable.

## Recommendation: formatting

Run Prettier 3.9.10's own standalone bundles in QuickJS-ng through `rquickjs` 0.14, in process, on a dedicated formatter thread that starts the engine lazily on the first "Format code". Vendor the eight bundle files with a version and hash, and turn `format/corpus.ts` with the Prettier reference outputs into NR22's fixture test.

This is the only route that measured byte-identical output and adds a single-digit megabyte cost (3.4 MB). It needs no external process and no installed runtime. QuickJS supports the NR22 cancellation requirement through `Runtime::set_interrupt_handler` and memory bounds through `set_memory_limit`. Upgrading Prettier means replacing the bundles and re-running the parity corpus. That is the same work rt does when it bumps its `package.json`.

Operating envelope and cost:

- **Works** for snippet-sized blocks: 0.8 to 11.6 ms per call after a 99 ms one-time engine load, and 29 MB peak RSS.
- **Degrades** on multi-thousand-line blocks: about 0.3 ms per line, so 0.7 to 0.8 s for the 2,600- to 4,000-line rt files. Format must stay asynchronous, show progress, and be cancellable; NR22 already requires stale-result rejection.
- **Does not** speed up later; QuickJS has no JIT. Precompiling the bundles to QuickJS bytecode could shorten the 99 ms load, but this spike did not measure it.
- **Rests on** Prettier's standalone bundles continuing to run without Node APIs. They do today, because rt itself runs them in a browser renderer.

## One-way doors

**Licensing.** quire is GPL-3.0-or-later (`LICENSE`, `README.md`). Every recommended component is permissively licensed and GPL-3.0 compatible:

- PDFium: BSD-3-Clause with some Apache-2.0 files (<https://pdfium.googlesource.com/pdfium/+/refs/heads/main/LICENSE>).
- PDFium's bundled third parties, notices shipped in the archive's `licenses/` folder: FreeType (FreeType License), ICU (Unicode License V3), libjpeg-turbo, OpenJPEG, Little CMS, zlib, libpng, Abseil and Highway (Apache-2.0).
- `pdfium-render`: MIT or Apache-2.0 (<https://github.com/ajrcarey/pdfium-render>).
- Prettier: MIT (<https://github.com/prettier/prettier/blob/main/LICENSE>).
- QuickJS-ng: MIT (<https://github.com/quickjs-ng/quickjs>).
- `rquickjs`: MIT (<https://github.com/DelSkayn/rquickjs>).

The FSF lists Apache-2.0 and the FreeType License as compatible with GPLv3 (<https://www.gnu.org/licenses/license-list.html>). NR35 must ship these notices. The choice is a one-way door only in its packaging pipeline and notices, not in its code; neither component constrains quire's own license.

**Binary size.** The two recommendations add about 13 MB uncompressed per platform: 7.4 MB for pdfium, about 2 MB for its wrapper, and 3.4 MB for the formatter. The rejected helper routes would add 64.5 MB (Bun) or 77.5 MB (Biome) each.

**Platform.** pdfium prebuilt archives exist for all six target triples. QuickJS-ng compiles with the `cc` crate and has Windows MSVC (Microsoft Visual C++) handling in `rquickjs-sys`'s build script. Neither was built on Linux or Windows here.

## Alternatives considered

**mupdf (`mupdf` crate 0.8.0).** It rendered everything, at 9.4 ms mean per page with 92 MB RSS on one cached page. Rejected for licensing: MuPDF and the crate are AGPL-3.0, and Artifex sells a commercial license as the alternative (<https://artifex.com/licensing>). The GNU licenses let GPLv3 code be combined with AGPLv3 code (GPLv3 section 13), so it is legal for quire. It would, however, put AGPL terms on the combined work, including the network-interaction clause if quire ever serves notebooks remotely, and block any later relicensing. It offers no measured advantage over pdfium.

**hayro 0.8.0 (pure Rust).** Its strengths are real: safe Rust (`#![forbid(unsafe_code)]`), MIT or Apache-2.0, no native library, the lowest memory (88 MB on one cached page), and correct output on every fixture, including AES-256 encryption. Rejected as the primary renderer because it is 1.7 times slower than pdfium, and its own documentation says it is not yet feature-complete (blending, isolation, knockout groups). It also has no text extraction or search API: text would have to be rebuilt from glyph callbacks in `hayro-interpret`, without reading order or word segmentation. Keep it as the fallback behind the renderer trait. It becomes the better choice if quire decides that remote PDFs must never reach native C++ in process.

**PDFKit through objc2 (macOS only).** Fast per page (6.0 ms mean) and free in size, but it opens files 3 times slower (32 to 38 ms) and uses the most memory (327 MB with 6 cached pages). It also covers only Apple platforms, so quire would still need pdfium or hayro for Linux and Windows and would maintain two renderers with different output.

**pdf-rs plus a rasterizer.** `pdf` 0.10.0 parses but does not rasterize; building a rasterizer is what hayro already is. Not benchmarked.

**Bun-compiled Prettier helper.** It is the same Prettier, so output matches, and its JIT is 11 times faster on large files. Rejected for cost: 64.5 MB per platform, a second process with its own lifecycle and signing, 1.38 s on the very first launch, and Bun statically links JavaScriptCore under LGPL-2 (<https://bun.com/docs/project/licensing>), which adds relinking notice obligations. It is the fallback if QuickJS proves too slow on real notebooks.

**Biome.** Exact for JavaScript, TypeScript, and JSON, but not for HTML, CSS comments, or Markdown, so it fails the parity bar for 3 of the 7 languages. It also ships only as a 77.5 MB CLI.

**dprint family.** Fast and pure Rust, but 7 of 21 identical and 684 changed lines on rt's Editor.tsx. Users would see their code reformatted differently from rt.

**Zed's Prettier integration.** Downloads Node.js and npm packages at runtime; violates the NR06 constraint.

## Not measured

- Linux and Windows builds and timings for every candidate.
- Rendering inside GPUI: texture upload cost, scrolling frame times, and HiDPI scale factors. The spike stops at the BGRA buffer.
- Building pdfium from source, and signing and notarizing the library.
- Scanned, JBIG2, JPEG 2000, CJK-without-embedded-font, and transparency-group PDFs; password-protected PDFs with a non-empty user password.
- Peak RSS for the Biome and dprint routes; the QuickJS RSS on the large inputs (its JavaScript heap reached 25.4 MB).
- QuickJS bytecode precompilation, and the interrupt handler's cancellation latency.

## Open questions and decisions requested

1. **Approve pdfium via `pdfium-render` with pinned prebuilt libraries** as the NR21 renderer, behind a renderer trait with hayro as the fallback. Also decide whether NR35 may consume `bblanchon/pdfium-binaries` archives pinned by hash, or must build pdfium from source.
2. **Approve Prettier 3.9.10 in QuickJS-ng** as the NR22 formatter, with the 21-snippet corpus as its parity gate.
3. **Decide the trust boundary for remote PDFs.** Accept in-process pdfium parsing of untrusted HTTPS and data-URL PDFs, or require an out-of-process renderer helper (or hayro for untrusted sources). This changes NR21's design more than the library choice does.
4. **Decide the native PDF view's scope.** rt users currently get Chromium's selection, find, zoom, page navigation, and `#page=N`. NR21 needs a parity ruling on which of these are required, and whether "Open PDF" in the system viewer covers the rest.

## Reproducing

```sh
bash spikes/nr06/fetch-fixtures.sh
(cd spikes/nr06/pdf && cargo build --release --features pdfium,hayro,mupdf && swiftc -O pdfkit-bench.swift -o target/pdfkit-bench)
bash spikes/nr06/pdf/run.sh; KEEP=1 RENDERERS="pdfium hayro mupdf" bash spikes/nr06/pdf/run.sh
bash spikes/nr06/pdf/diff.sh; bash spikes/nr06/pdf/encrypted.sh
cd spikes/nr06/format && bun install && bun corpus.ts && bun prettier-ref.ts
(cd rust && cargo build --release --features dprint,quickjs)
./rust/target/release/fmt_quickjs corpus out-quickjs 10; ./rust/target/release/fmt_dprint corpus out-dprint 50
bun run-biome.ts; bun build --compile --minify helper/prettier-helper.ts --outfile bin/prettier-helper
bun run-helper.ts "$PWD/bin/prettier-helper" out-bun-helper 20
for d in out-quickjs out-bun-helper out-biome out-dprint; do bun compare.ts $d; done
```
