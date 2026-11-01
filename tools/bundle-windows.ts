// Builds a Windows zip: `quire-native.exe` with pdfium.dll beside it (where quire_media looks
// first after QUIRE_PDFIUM_LIBRARY), the icon, and the generated third-party notices. The
// executable carries its icon (crates/quire/build.rs embeds resources/quire.rc) and, in release,
// Zed's `windows_subsystem = "windows"`, so it opens no console window.
// Unverified: no Windows machine has run it; the Rust code and the resource script were only
// cross-checked from macOS. Zed ships an Inno Setup installer (script/bundle-windows.ps1); Quire
// has no installer yet and no signing.
// Usage: bun tools/bundle-windows.ts [--skip-build]
import { copyFileSync, existsSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { linkedPackages, render } from "./generate-licenses.ts";
import { hostPlatform } from "./pdfium.ts";
import { fail, hashTree, identity, inputs, licensesFile, mcpServer, megabytes, pdfium, quireVersion, resources, root, run, treeBytes } from "./package.ts";

if (process.platform !== "win32") fail("tools/bundle-windows.ts runs on Windows. Use tools/bundle-mac.ts or tools/bundle-linux.ts elsewhere.");
const skipBuild = process.argv.includes("--skip-build");
const platform = hostPlatform();
const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
const triple = `${arch}-pc-windows-msvc`;
const { dir: pdfiumDir, library } = pdfium(platform);
const targetDir = process.env.CARGO_TARGET_DIR ?? join(root, "target");
const binary = join(targetDir, "release", "quire.exe");

if (!skipBuild) run("cargo", ["build", "--release", "--locked", "-p", "quire", "--bin", "quire"], { stdio: "inherit" });
else if (!existsSync(binary)) fail(`${binary} does not exist. Run without --skip-build.`);

const name = `quire-native-${quireVersion()}-windows-${arch}`;
const out = join(targetDir, "release/bundle/windows");
const dir = join(out, name);
rmSync(dir, { recursive: true, force: true });
mkdirSync(dir, { recursive: true });
copyFileSync(binary, join(dir, `${identity.command}.exe`));
copyFileSync(library, join(dir, "pdfium.dll"));
copyFileSync(join(resources, "app-icon.ico"), join(dir, "quire-native.ico"));
writeFileSync(join(dir, licensesFile), render(linkedPackages(triple), pdfiumDir));

const zip = join(out, `${name}.zip`);
rmSync(zip, { force: true });
// Windows' own bsdtar writes zip files; a GNU tar earlier on PATH (Git Bash, MSYS) cannot.
run(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe"), ["-a", "-cf", zip, "-C", out, name]);
const record = {
  inputs: inputs({ target: triple, features: [], platform, library }),
  sizes: { releaseBinaryBytes: statSync(binary).size, bundleBytes: treeBytes(dir), zipBytes: statSync(zip).size },
  outputs: hashTree(dir),
};
writeFileSync(join(out, `${name}.inputs.json`), `${JSON.stringify(record, null, 2)}\n`);
console.log(`Built ${zip} (${megabytes(record.sizes.zipBytes)}; unpacked ${megabytes(record.sizes.bundleBytes)}). Unsigned.`);
const server = mcpServer(join(dir, `${identity.command}.exe`));
if (server) fail(`The package was written but failed its check: quire-native.exe --mcp-server ${server}`);
console.log("--mcp-server answered with no app running.");
