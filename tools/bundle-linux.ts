// Builds a Linux tarball, after Zed's script/bundle-linux: the stripped `quire-native` executable,
// PDFium in lib/ (quire_media looks in ../lib beside the executable), the desktop entry, icons,
// and the generated third-party notices. Unverified: no Linux machine has run it yet.
// Usage: bun tools/bundle-linux.ts [--skip-build]
// Install by unpacking anywhere and linking bin/quire-native into a directory on PATH; the
// desktop entry and icons go under ~/.local/share/applications and ~/.local/share/icons.
// The executable is `quire-native` because the Go terminal app installs `quire`.
import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { linkedPackages, render } from "./generate-licenses.ts";
import { hostPlatform } from "./pdfium.ts";
import { fail, hashTree, identity, inputs, licensesFile, mcpServer, megabytes, pdfium, quireVersion, resources, root, run, treeBytes } from "./package.ts";

if (process.platform !== "linux") fail("tools/bundle-linux.ts runs on Linux. Use tools/bundle-mac.ts or tools/bundle-windows.ts elsewhere.");
const skipBuild = process.argv.includes("--skip-build");
const platform = hostPlatform();
const arch = process.arch === "arm64" ? "aarch64" : "x86_64";
const triple = `${arch}-unknown-linux-gnu`;
const { dir: pdfiumDir, library } = pdfium(platform);
const targetDir = process.env.CARGO_TARGET_DIR ?? join(root, "target");
const binary = join(targetDir, "release", "quire");

if (!skipBuild) run("cargo", ["build", "--release", "--locked", "-p", "quire", "--bin", "quire"], { stdio: "inherit" });
else if (!existsSync(binary)) fail(`${binary} does not exist. Run without --skip-build.`);

const name = `quire-native-${quireVersion()}-linux-${arch}`;
const out = join(targetDir, "release/bundle/linux");
const dir = join(out, name);
rmSync(dir, { recursive: true, force: true });
for (const sub of ["bin", "lib", "share/applications", "share/icons/hicolor/scalable/apps", "share/icons/hicolor/1024x1024/apps", "share/doc/quire-native"])
  mkdirSync(join(dir, sub), { recursive: true });

console.log("Keeping debug symbols in quire.dbg and stripping the packaged executable.");
run("objcopy", ["--only-keep-debug", binary, `${binary}.dbg`]);
const executable = join(dir, "bin", identity.command);
run("objcopy", ["--strip-unneeded", binary, executable]);
copyFileSync(library, join(dir, "lib", "libpdfium.so"));
chmodSync(join(dir, "lib", "libpdfium.so"), 0o755);
copyFileSync(join(resources, `${identity.bundleId}.desktop`), join(dir, "share/applications", `${identity.bundleId}.desktop`));
copyFileSync(join(resources, "app-icon.svg"), join(dir, "share/icons/hicolor/scalable/apps", `${identity.bundleId}.svg`));
copyFileSync(join(resources, "app-icon.png"), join(dir, "share/icons/hicolor/1024x1024/apps", `${identity.bundleId}.png`));
writeFileSync(join(dir, "share/doc/quire-native", licensesFile), render(linkedPackages(triple), pdfiumDir));

const problems: string[] = [];
const libraries = run("ldd", [executable])
  .split("\n")
  .map((line) => line.trim())
  .filter(Boolean);
const missing = libraries.filter((line) => line.includes("not found"));
if (missing.length > 0) problems.push(`the executable needs libraries this system lacks: ${missing.join("; ")}`);
const server = mcpServer(executable);
if (server) problems.push(`quire-native --mcp-server ${server}`);

const tarball = join(out, `${name}.tar.gz`);
run("tar", ["-czf", tarball, "-C", out, name]);
const record = {
  inputs: inputs({ target: triple, features: [], platform, library }),
  sizes: { releaseBinaryBytes: statSync(binary).size, strippedExecutableBytes: statSync(executable).size, bundleBytes: treeBytes(dir), tarballBytes: statSync(tarball).size },
  outputs: hashTree(dir),
  linkedLibraries: libraries,
};
writeFileSync(join(out, `${name}.inputs.json`), `${JSON.stringify(record, null, 2)}\n`);
console.log(`Built ${tarball} (${megabytes(record.sizes.tarballBytes)}; unpacked ${megabytes(record.sizes.bundleBytes)}).`);
console.log(`The executable links:\n  ${libraries.join("\n  ")}`);
if (problems.length > 0) fail(`The package was written but failed its checks:\n  ${problems.join("\n  ")}`);
console.log("ldd found every library, and --mcp-server answered with no app running.");
