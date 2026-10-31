// Writes the third-party notices a Quire package ships: every crate linked into the `quire`
// binary for one target (from `cargo tree` and `cargo metadata`, with the license files each
// crate ships), the vendored Zed, Prettier, and tree-sitter sources, and the downloaded PDFium.
// Zed generates its notices with cargo-about (Zed's script/generate-licenses); this reads
// the same Cargo data without installing a tool.
// Usage: bun tools/generate-licenses.ts --target <triple> --pdfium <dir> --out <file>
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative, resolve, sep } from "node:path";
import { fail, root, run } from "./package.ts";


// License texts that live below a crate's top level, where the scan below does not look.
const nestedNotices: Record<string, string[]> = {
  "rquickjs-sys": ["quickjs/LICENSE"],
  "hayro-interpret": ["assets/LICENSE_FOXIT", "assets/CGATS_LICENSE.txt", "assets/README.md"],
};

// Sources compiled or embedded into the binary that Cargo does not describe.
const vendoredNotices: [string, string[]][] = [
  ["Zed (vendor/zed, GPL-3.0-or-later; some crates Apache-2.0)", ["vendor/zed/LICENSE-GPL", "vendor/zed/LICENSE-APACHE"]],
  ["Zed themes (vendor/zed/assets/themes)", ["vendor/zed/assets/themes/LICENSES"]],
  ["Zed icons (vendor/zed/assets/icons)", ["vendor/zed/assets/icons/LICENSES"]],
  ["IBM Plex Sans font (vendor/zed/assets/fonts/ibm-plex-sans)", ["vendor/zed/assets/fonts/ibm-plex-sans/license.txt"]],
  ["Lilex font (vendor/zed/assets/fonts/lilex)", ["vendor/zed/assets/fonts/lilex/OFL.txt"]],
  ["Prettier 3.9.10 standalone bundles (vendor/prettier)", ["vendor/prettier/LICENSE", "vendor/prettier/THIRD-PARTY-NOTICES.md"]],
  ["tree-sitter Markdown grammars (vendor/quire-markdown)", ["vendor/quire-markdown/LICENSE", "vendor/quire-markdown/LICENSE-tree-sitter"]],
];

const mitText = `Permission is hereby granted, free of charge, to any person obtaining a copy of this software
and associated documentation files (the "Software"), to deal in the Software without
restriction, including without limitation the rights to use, copy, modify, merge, publish,
distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.`;

const noticeName = /^(licen[cs]e|copying|notice|copyright|unlicense|ofl|authors)/i;

type Package = { name: string; version: string; license: string | null; dir: string; files: string[] };


function option(name: string): string {
  const at = process.argv.indexOf(name);
  const value = at < 0 ? undefined : process.argv[at + 1];
  if (!value) fail(`Missing ${name}. Usage: bun tools/generate-licenses.ts --target <triple> --pdfium <dir> --out <file>`);
  return value;
}

function read(path: string): string {
  return readFileSync(path, "utf8").replace(/\r\n?/g, "\n").trimEnd();
}

function noticeFiles(dir: string, licenseFile: string | null): string[] {
  const found = readdirSync(dir)
    .filter((name) => noticeName.test(name) && statSync(join(dir, name)).isFile())
    .map((name) => join(dir, name));
  if (licenseFile) found.push(resolve(dir, licenseFile));
  return [...new Set(found)].sort();
}

// `cargo tree` resolves features and platform conditions for `quire` alone, which `cargo metadata`
// does not; metadata supplies each package's directory and license fields.
export function linkedPackages(target: string): Package[] {
  const tree = run("cargo", ["tree", "-p", "quire", "--locked", "-e", "normal", "--target", target, "--prefix", "none", "--format", "{p}"]);
  const ids = new Set(
    tree
      .split("\n")
      .map((line) => line.replace(/ \(\*\)$/, "").trim())
      .filter(Boolean),
  );
  const metadata = JSON.parse(run("cargo", ["metadata", "--format-version", "1", "--locked"])) as {
    packages: { name: string; version: string; license: string | null; license_file: string | null; manifest_path: string; source: string | null }[];
  };
  const packages: Package[] = [];
  for (const id of ids) {
    const [name, version] = id.split(" ");
    const matches = metadata.packages.filter((p) => p.name === name && `v${p.version}` === version);
    if (matches.length === 0) fail(`cargo tree listed ${id}, which cargo metadata does not describe. Run \`cargo metadata --locked\` to check the lockfile.`);
    for (const p of matches) {
      const dir = dirname(p.manifest_path);
      if (dir.startsWith(join(root, "crates") + sep)) continue;
      const files = noticeFiles(dir, p.license_file);
      for (const nested of nestedNotices[p.name] ?? []) {
        const path = join(dir, nested);
        if (!existsSync(path)) fail(`${p.name} ${p.version} no longer ships ${nested}. Update nestedNotices in tools/generate-licenses.ts.`);
        files.push(path);
      }
      packages.push({ name: p.name, version: p.version, license: p.license, dir, files });
    }
  }
  return packages.sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version));
}

function isVendoredZed(dir: string): boolean {
  return dir.startsWith(join(root, "vendor/zed") + sep);
}

export function render(packages: Package[], pdfiumDir: string): string {
  const out: string[] = [
    "# Quire third-party notices",
    "",
    "Quire is licensed under GPL-3.0-or-later; the full text follows. It includes the software below,",
    "each under its own license. This file is generated by tools/generate-licenses.ts.",
    "",
    "## Quire",
    "",
    "```",
    read(join(root, "LICENSE")),
    "```",
    "",
  ];

  for (const [title, files] of vendoredNotices) {
    out.push(`## ${title}`, "");
    for (const file of files) out.push(`### ${file}`, "", "```", read(join(root, file)), "```", "");
  }

  const pdfiumLicenses = join(pdfiumDir, "licenses");
  if (!existsSync(join(pdfiumDir, "LICENSE")) || !existsSync(pdfiumLicenses))
    fail(`PDFium at ${pdfiumDir} has no LICENSE or licenses/. Run \`bun tools/fetch-pdfium.ts\` and pass its directory.`);
  const pdfiumVersion = existsSync(join(pdfiumDir, "VERSION")) ? read(join(pdfiumDir, "VERSION")).replace(/\n/g, ", ") : "unknown";
  out.push(`## PDFium (bblanchon/pdfium-binaries, ${pdfiumVersion})`, "", "BSD-3-Clause with Apache-2.0 parts; the bundled third-party code follows.", "");
  for (const file of ["LICENSE", ...readdirSync(pdfiumLicenses).sort().map((name) => `licenses/${name}`)])
    out.push(`### ${file}`, "", "```", read(join(pdfiumDir, file)), "```", "");

  const zed = packages.filter((p) => isVendoredZed(p.dir));
  out.push(
    "## Vendored Zed crates",
    "",
    "These crates come from vendor/zed and are covered by the Zed licenses above.",
    "",
    ...zed.map((p) => `- ${p.name} ${p.version}: ${p.license ?? "GPL-3.0-or-later"}`),
    "",
  );

  const others = packages.filter((p) => !isVendoredZed(p.dir));
  const unlicensed = others.filter((p) => !p.license && p.files.length === 0);
  if (unlicensed.length > 0)
    fail(`These crates declare no license and ship no license file: ${unlicensed.map((p) => `${p.name} ${p.version}`).join(", ")}. Review them before packaging.`);

  const groups = new Map<string, { text: string; packages: Set<string> }>();
  const withoutFiles: Package[] = [];
  for (const p of others) {
    if (p.files.length === 0) {
      withoutFiles.push(p);
      continue;
    }
    for (const file of p.files) {
      const text = read(file);
      const key = createHash("sha256").update(text).digest("hex");
      const group = groups.get(key) ?? { text, packages: new Set<string>() };
      group.packages.add(`${p.name} ${p.version} (${p.license ?? "see text"}): ${relative(p.dir, file)}`);
      groups.set(key, group);
    }
  }

  out.push("## Rust crates", "", `${others.length} crates from crates.io and Git, grouped by identical license text.`, "");
  const sorted = [...groups.values()].sort((a, b) => [...a.packages].sort()[0].localeCompare([...b.packages].sort()[0]));
  for (const group of sorted) {
    out.push(...[...group.packages].sort().map((line) => `- ${line}`), "", "```", group.text, "```", "");
  }

  if (withoutFiles.length > 0) {
    const shipped = new Map(others.filter((p) => p.files.length > 0).map((p) => [p.license, p] as const));
    const permissive = /(^|\W)(MIT|Apache-2\.0)(\W|$)/;
    const lines = withoutFiles.map((p) => {
      const twin = shipped.get(p.license);
      if (twin) return `- ${p.name} ${p.version}: ${p.license}, text as shipped by ${twin.name} ${twin.version} above`;
      if (p.license && permissive.test(p.license) && !/WITH| AND /.test(p.license)) return `- ${p.name} ${p.version}: ${p.license}`;
      fail(`${p.name} ${p.version} (${p.license}) ships no license file and no other crate ships that license's text. Add the text to nestedNotices or vendoredNotices in tools/generate-licenses.ts.`);
    });
    out.push(
      "## Rust crates without a license file",
      "",
      "These crates ship no license text; their declared licenses apply, with the standard texts below or the text named.",
      "",
      ...lines,
      "",
      "### MIT",
      "",
      "```",
      mitText,
      "```",
      "",
      "### Apache-2.0",
      "",
      "See vendor/zed/LICENSE-APACHE above.",
      "",
    );
  }
  return out.join("\n");
}

if (import.meta.main) {
  const target = option("--target");
  const pdfium = resolve(option("--pdfium"));
  const outFile = resolve(option("--out"));
  const packages = linkedPackages(target);
  writeFileSync(outFile, render(packages, pdfium));
  console.log(`Wrote notices for ${packages.length} crates and the vendored sources to ${outFile}.`);
}
