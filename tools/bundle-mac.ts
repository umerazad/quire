// Builds "Quire Native.app" for this Mac, after Zed's script/bundle-mac: a release build with
// Zed's link flags, PDFium in Contents/Frameworks, the generated third-party notices, and a
// signature. Writes a sidecar recording the hashed inputs and outputs, then verifies the bundle.
// Usage: bun tools/bundle-mac.ts [--runtime-shaders] [--skip-build] [--dmg]
//   --runtime-shaders  compile GPUI's Metal shaders at launch (when the Metal toolchain is missing)
//   --skip-build       package the existing target/release/quire
//   --dmg              also make a disk image, and notarize it when credentials are configured
// Signing: QUIRE_SIGNING_IDENTITY names a Developer ID Application identity in the keychain.
// Without it the bundle is signed ad hoc, which runs only on the Mac that built it.
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { linkedPackages, render } from "./generate-licenses.ts";
import { notarize } from "./notarize-mac.ts";
import { hostPlatform } from "./pdfium.ts";
import {
  fail,
  hashTree,
  identity,
  inputs,
  licensesFile,
  megabytes,
  pdfium,
  quireVersion,
  resources,
  root,
  run,
  sha256,
  source,
  treeBytes,
} from "./package.ts";
import { verify } from "./verify-bundle-mac.ts";

const flags = new Set(process.argv.slice(2));
for (const flag of flags)
  if (!["--runtime-shaders", "--skip-build", "--dmg"].includes(flag))
    fail(`Unknown option ${flag}. Usage: bun tools/bundle-mac.ts [--runtime-shaders] [--skip-build] [--dmg]`);
if (process.platform !== "darwin") fail("tools/bundle-mac.ts runs on macOS. Use tools/bundle-linux.ts or tools/bundle-windows.ts elsewhere.");

const runtimeShaders = flags.has("--runtime-shaders");
if (!runtimeShaders && spawnSync("xcrun", ["-sdk", "macosx", "metal", "--version"]).status !== 0)
  fail(
    "The Metal toolchain is missing, so GPUI cannot precompile its shaders. Run `xcodebuild -downloadComponent MetalToolchain`, " +
      "or pass --runtime-shaders for a development bundle that compiles shaders at launch.",
  );
const features = runtimeShaders ? ["gpui_platform/runtime_shaders"] : [];
const platform = hostPlatform();
const triple = process.arch === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
const { dir: pdfiumDir, library } = pdfium(platform);
const targetDir = process.env.CARGO_TARGET_DIR ?? join(root, "target");
const binary = join(targetDir, "release", "quire");

if (!flags.has("--skip-build")) {
  console.log(`Building quire for ${triple}${runtimeShaders ? " with runtime shaders" : ""}.`);
  run(
    "cargo",
    [
      "rustc",
      "--release",
      "--locked",
      "-p",
      "quire",
      "--bin",
      "quire",
      ...(features.length > 0 ? ["--features", features.join(",")] : []),
      "--",
      "-C",
      "link-arg=-Wl,-unexported_symbol,__R*",
      "-C",
      "link-arg=-Wl,-rpath,@executable_path/../Frameworks",
    ],
    { env: { ...process.env, MACOSX_DEPLOYMENT_TARGET: identity.minimumMacOS }, stdio: "inherit" },
  );
} else if (!existsSync(binary)) fail(`${binary} does not exist. Run without --skip-build.`);

// Where the binary came from. A skipped build keeps the provenance an earlier bundle recorded for
// the same binary; any other binary is marked unknown, which the verifier rejects.
const out = join(targetDir, "release/bundle/macos");
const sidecar = join(out, `${identity.name}.inputs.json`);
const binarySha256 = sha256(binary);
const previous = existsSync(sidecar) ? JSON.parse(readFileSync(sidecar, "utf8")).build : undefined;
const build = !flags.has("--skip-build")
  ? { ...source(), features, shaders: runtimeShaders ? "runtime" : "precompiled", binarySha256 }
  : previous?.binarySha256 === binarySha256
    ? previous
    : { provenance: "unknown", binarySha256 };
if (build.provenance === "unknown")
  console.log(`${binary} was not built by this script, so its source and shader mode are unknown and verification will fail. Run without --skip-build.`);

console.log("Extracting debug symbols to quire.dSYM.");
run("dsymutil", [binary, "-o", `${binary}.dSYM`]);

const app = join(out, `${identity.name}.app`);
const contents = join(app, "Contents");
rmSync(app, { recursive: true, force: true });
for (const dir of ["MacOS", "Frameworks", "Resources"]) mkdirSync(join(contents, dir), { recursive: true });

const version = quireVersion();
writeFileSync(
  join(contents, "Info.plist"),
  readFileSync(join(resources, "Info.plist"), "utf8")
    .replaceAll("@VERSION@", version)
    .replaceAll("@MINIMUM_SYSTEM_VERSION@", identity.minimumMacOS),
);
run("plutil", ["-lint", join(contents, "Info.plist")]);
writeFileSync(join(contents, "PkgInfo"), "APPL????");

const executable = join(contents, "MacOS", identity.macExecutable);
copyFileSync(binary, executable);
run("strip", ["-x", executable]);

const framework = join(contents, "Frameworks", "libpdfium.dylib");
copyFileSync(library, framework);
chmodSync(framework, 0o755);
run("install_name_tool", ["-id", "@rpath/libpdfium.dylib", framework]);

copyFileSync(join(resources, "app-icon.icns"), join(contents, "Resources", "AppIcon.icns"));
console.log("Generating third-party notices from Cargo metadata.");
writeFileSync(join(contents, "Resources", licensesFile), render(linkedPackages(triple), pdfiumDir));

const entitlements = join(resources, "quire.entitlements");
const signer = process.env.QUIRE_SIGNING_IDENTITY;
if (signer) {
  console.log(`Signing with "${signer}" and the hardened runtime.`);
  const sign = ["--force", "--timestamp", "--options", "runtime", "--sign", signer];
  run("codesign", [...sign, framework]);
  run("codesign", [...sign, "--entitlements", entitlements, executable]);
  run("codesign", [...sign, "--entitlements", entitlements, app]);
} else {
  console.log(
    "QUIRE_SIGNING_IDENTITY is not set, so the bundle is signed ad hoc without the hardened runtime. It opens on this Mac only; " +
      "set QUIRE_SIGNING_IDENTITY to a Developer ID Application identity in your keychain to sign for distribution.",
  );
  run("codesign", ["--force", "--sign", "-", framework]);
  run("codesign", ["--force", "--sign", "-", "--entitlements", entitlements, app]);
}
run("codesign", ["--verify", "--deep", "--strict", app]);

let dmg: { file: string; sha256: string; bytes: number; notarized: boolean } | undefined;
if (flags.has("--dmg")) {
  const stage = join(out, "dmg");
  rmSync(stage, { recursive: true, force: true });
  mkdirSync(stage);
  run("ditto", [app, join(stage, `${identity.name}.app`)]);
  symlinkSync("/Applications", join(stage, "Applications"));
  const file = join(out, `Quire-Native-${version}-${process.arch === "arm64" ? "aarch64" : "x86_64"}.dmg`);
  run("hdiutil", ["create", "-volname", identity.name, "-srcfolder", stage, "-ov", "-format", "UDZO", file]);
  rmSync(stage, { recursive: true, force: true });
  if (signer) run("codesign", ["--force", "--timestamp", "--sign", signer, file]);
  const notarized = notarize(file);
  dmg = { file, sha256: sha256(file), bytes: statSync(file).size, notarized };
}

const sizes = {
  releaseBinaryBytes: statSync(binary).size,
  strippedExecutableBytes: statSync(executable).size,
  pdfiumBytes: statSync(framework).size,
  bundleBytes: treeBytes(app),
  dmgBytes: dmg?.bytes,
};
const record = {
  build,
  inputs: inputs({ target: triple, features: build.features ?? features, platform, library }),
  signing: signer ? { identity: signer, hardenedRuntime: true } : { identity: "ad hoc", hardenedRuntime: false },
  sizes,
  outputs: hashTree(app),
  dmg,
};
writeFileSync(sidecar, `${JSON.stringify(record, null, 2)}\n`);

console.log(
  [
    `Built ${app}`,
    `  release binary ${megabytes(sizes.releaseBinaryBytes)}, stripped executable ${megabytes(sizes.strippedExecutableBytes)}, ` +
      `PDFium ${megabytes(sizes.pdfiumBytes)}, bundle ${megabytes(sizes.bundleBytes)}${dmg ? `, disk image ${megabytes(dmg.bytes)}` : ""}`,
    `  inputs and output hashes: ${sidecar}`,
  ].join("\n"),
);
verify(app);
