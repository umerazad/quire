// Inspects a built "Quire Native.app": Info.plist identity, architecture and minimum macOS,
// library linkage and rpath, PDFium where quire_media looks for it, the third-party notices, the
// signature, the `--mcp-server` entry, and the hashes recorded by tools/bundle-mac.ts.
// Usage: bun tools/verify-bundle-mac.ts [path/to/Quire Native.app] [--launch]
//   --launch  also start the app with an empty data folder (opens a window for 10 s)
//        bun tools/verify-bundle-mac.ts --compare <first.inputs.json> <second.inputs.json>
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { libraryPath } from "./pdfium.ts";
import { buildInputsChangedSince, fail, hashTree, identity, isFile, licensesFile, mcpServer, megabytes, quireVersion, root, sha256, treeBytes } from "./package.ts";

// Text the notices must contain: the components earlier tasks said packages must credit.
export const requiredNotices = [
  "GNU GENERAL PUBLIC LICENSE",
  "## PDFium",
  "licenses/freetype.txt",
  "pdfium-render",
  "hayro-interpret",
  "LICENSE_FOXIT",
  "CGATS_LICENSE.txt",
  "vello_cpu",
  "pic-scale",
  "Prettier",
  "THIRD-PARTY-NOTICES.md",
  "quickjs/LICENSE",
  "rquickjs ",
  "tree-sitter-typescript",
  "tree-sitter Markdown grammars",
  "async_zip",
  "quick-xml",
  "- editor 0.1.0",
  "Lilex font",
  "@lezer/markdown",
];

function capture(command: string, args: string[]) {
  const result = spawnSync(command, args, { encoding: "utf8", timeout: 30_000, maxBuffer: 1 << 28 });
  return { ok: result.status === 0, out: `${result.stdout ?? ""}`, err: `${result.stderr ?? ""}` };
}

function linked(binary: string): string[] {
  const self = capture("otool", ["-D", binary]).out.split("\n").slice(1).map((line) => line.trim());
  return capture("otool", ["-L", binary])
    .out.split("\n")
    .slice(1)
    .map((line) => line.trim().replace(/ \(compatibility.*$/, ""))
    .filter((path) => path && !self.includes(path));
}

function loadCommand(binary: string, command: string, field: string): string[] {
  const lines = capture("otool", ["-l", binary]).out.split("\n").map((line) => line.trim());
  const values: string[] = [];
  lines.forEach((line, at) => {
    if (line !== `cmd ${command}`) return;
    const hit = lines.slice(at, at + 6).find((next) => next.startsWith(`${field} `));
    if (hit) values.push(hit.slice(field.length + 1).replace(/ \(offset \d+\)$/, ""));
  });
  return values;
}

// Opens a window, so it runs only with --launch. HOME points at an empty folder so the launch
// neither reads the user's data nor hands over to a running Quire; the folder sits in /tmp
// because the single-instance socket path inside it must fit in SUN_LEN (104 bytes).
function launchSmoke(executable: string): string | undefined {
  const home = mkdtempSync("/tmp/qn-");
  try {
    const result = spawnSync(executable, [], { encoding: "utf8", timeout: 10_000, env: { ...process.env, HOME: home } });
    if (result.error && result.signal !== "SIGTERM") return `could not start: ${result.error.message}`;
    if (result.signal === "SIGTERM") return undefined;
    return `exited after less than 10 s with ${result.status ?? result.signal}: ${(result.stderr ?? "").trim()}`;
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
}

export function verify(appPath: string, launch = false): void {
  const app = resolve(appPath);
  const contents = join(app, "Contents");
  const failures: string[] = [];
  const check = (ok: boolean, what: string, problem: string) => {
    if (ok) console.log(`ok    ${what}`);
    else {
      console.log(`FAIL  ${what}: ${problem}`);
      failures.push(`${what}: ${problem}`);
    }
  };
  const note = (text: string) => console.log(`note  ${text}`);
  if (!existsSync(join(contents, "Info.plist"))) fail(`${app} has no Contents/Info.plist. Build it with \`bun tools/bundle-mac.ts\`.`);

  const plist = JSON.parse(capture("plutil", ["-convert", "json", "-o", "-", join(contents, "Info.plist")]).out);
  const expected: Record<string, unknown> = {
    CFBundleIdentifier: identity.bundleId,
    CFBundleName: identity.name,
    CFBundleDisplayName: identity.name,
    CFBundleExecutable: identity.macExecutable,
    CFBundlePackageType: "APPL",
    CFBundleIconFile: "AppIcon",
    CFBundleShortVersionString: quireVersion(),
    CFBundleVersion: quireVersion(),
    LSMinimumSystemVersion: identity.minimumMacOS,
    NSHighResolutionCapable: true,
  };
  for (const [key, value] of Object.entries(expected))
    check(plist[key] === value, `Info.plist ${key} is ${JSON.stringify(value)}`, `found ${JSON.stringify(plist[key])}`);
  check(plist.CFBundleIdentifier !== identity.rtBundleId, "bundle id differs from rt's", `both use ${identity.rtBundleId}`);
  check(isFile(join(contents, "Resources", "AppIcon.icns")), "Resources/AppIcon.icns present", "missing");

  const executable = join(contents, "MacOS", identity.macExecutable);
  check(isFile(executable) && (statSync(executable).mode & 0o111) !== 0, "executable present", `${executable} is missing or not executable`);
  const arch = process.arch === "arm64" ? "arm64" : "x86_64";
  const archs = (binary: string) => capture("lipo", ["-archs", binary]).out.trim();
  const system = (path: string) => path.startsWith("/usr/lib/") || path.startsWith("/System/Library/");
  check(archs(executable) === arch, `executable is ${arch}`, archs(executable));
  const minos = loadCommand(executable, "LC_BUILD_VERSION", "minos");
  check(minos.includes(identity.minimumMacOS), `executable targets macOS ${identity.minimumMacOS}`, `minos ${minos.join(", ")}`);
  const foreign = linked(executable).filter((path) => !system(path));
  check(foreign.length === 0, "executable links only system libraries", foreign.join(", "));
  const rpaths = loadCommand(executable, "LC_RPATH", "path");
  check(rpaths.includes("@executable_path/../Frameworks"), "executable rpath includes @executable_path/../Frameworks", rpaths.join(", ") || "no LC_RPATH");
  check(
    !capture("nm", ["-u", executable]).out.includes("FPDF_InitLibrary"),
    "PDFium is loaded at run time, not linked",
    "the executable imports FPDF symbols",
  );

  const pdfium = join(contents, "Frameworks", "libpdfium.dylib");
  check(isFile(pdfium), "Frameworks/libpdfium.dylib present (quire_media looks in ../Frameworks)", "missing");
  if (isFile(pdfium)) {
    const installName = capture("otool", ["-D", pdfium]).out;
    check(installName.includes("@rpath/libpdfium.dylib"), "PDFium install name is @rpath/libpdfium.dylib", installName.trim());
    check(archs(pdfium) === arch, `PDFium is ${arch}`, archs(pdfium));
    const pdfiumForeign = linked(pdfium).filter((path) => !system(path));
    check(pdfiumForeign.length === 0, "PDFium links only system libraries", pdfiumForeign.join(", "));
  }

  const notices = join(contents, "Resources", licensesFile);
  check(isFile(notices), `Resources/${licensesFile} present`, "missing");
  if (isFile(notices)) {
    const text = readFileSync(notices, "utf8");
    const missing = requiredNotices.filter((marker) => !text.includes(marker));
    check(missing.length === 0, `notices name ${requiredNotices.length} required components`, `missing ${missing.join(", ")}`);
  }

  const signature = capture("codesign", ["--verify", "--deep", "--strict", app]);
  check(signature.ok, "codesign --verify --deep --strict passes", signature.err.trim());
  const details = capture("codesign", ["-dv", "--verbose=2", app]).err;
  const adhoc = details.includes("Signature=adhoc");
  const authority = /Authority=(.*)/.exec(details)?.[1];
  const hardened = /flags=0x[0-9a-f]+\([^)]*runtime/.test(details);
  note(`signature: ${adhoc ? "ad hoc" : authority ?? "unknown"}; hardened runtime ${hardened ? "on" : "off"}`);
  // A Developer ID bundle must meet notarization's rules, for the bundle and PDFium alike.
  if (!adhoc)
    for (const [what, path] of [["bundle", app], ["PDFium", join(contents, "Frameworks", "libpdfium.dylib")]]) {
      const info = capture("codesign", ["-dv", "--verbose=2", path]).err;
      const missing = [
        !/flags=0x[0-9a-f]+\([^)]*runtime/.test(info) && "hardened runtime",
        !/^Timestamp=/m.test(info) && "secure timestamp",
        (/^TeamIdentifier=not set/m.test(info) || !/^TeamIdentifier=/m.test(info)) && "team identifier",
      ].filter(Boolean);
      check(missing.length === 0, `${what} signature is ready for notarization`, `missing ${missing.join(", ")}; re-sign with QUIRE_SIGNING_IDENTITY`);
    }
  const entitlements = capture("codesign", ["-d", "--entitlements", "-", "--xml", app]).out;
  check(entitlements.includes("com.apple.security.cs.allow-jit"), "entitlements are embedded", "codesign shows no entitlements");
  const gatekeeper = capture("spctl", ["--assess", "--type", "execute", app]);
  note(`Gatekeeper: ${gatekeeper.ok ? "accepted" : `rejected (${gatekeeper.err.trim()})`}; expected to reject unless signed with a Developer ID and notarized`);

  const server = isFile(executable) ? mcpServer(executable) : "no executable";
  check(server === undefined, "`quire --mcp-server` lists 14 tools and reports the closed app", server ?? "");
  if (launch) {
    const problem = launchSmoke(executable);
    check(problem === undefined, "the app starts with an empty data folder and stays up for 10 s", problem ?? "");
  }

  const sidecar = join(dirname(app), `${basename(app, ".app")}.inputs.json`);
  if (existsSync(sidecar)) {
    const record = JSON.parse(readFileSync(sidecar, "utf8"));
    const now = hashTree(app);
    const changed = [...new Set([...Object.keys(now), ...Object.keys(record.outputs)])].filter((file) => now[file] !== record.outputs[file]);
    check(changed.length === 0, "bundle files match the hashes recorded at build time", `changed: ${changed.join(", ")}`);
    const inputs = record.inputs;
    const library = libraryPath(inputs.pdfium.platform);
    const stale = [
      sha256(join(root, "Cargo.lock")) !== inputs.cargoLock && "Cargo.lock changed",
      sha256(join(root, "Cargo.toml")) !== inputs.cargoToml && "Cargo.toml changed",
      JSON.stringify(hashTree(join(root, "crates/quire/resources"))) !== JSON.stringify(inputs.resources) && "crates/quire/resources changed",
      !existsSync(library) && `the PDFium library is missing at ${library}`,
      existsSync(library) && sha256(library) !== inputs.pdfium.librarySha256 && "the PDFium library changed",
    ].filter(Boolean);
    check(stale.length === 0, "packaging inputs unchanged since the bundle was made", `${stale.join("; ")}; rebuild the bundle`);
    const build = record.build;
    const provenance = !build || build.provenance === "unknown" ? ["the binary's source is unknown (packaged with --skip-build)"] : buildInputsChangedSince(build.commit);
    if (build?.dirty) provenance.push("the binary was built with uncommitted changes");
    check(provenance.length === 0, "the binary was built from the current sources", `${provenance.join("; ")}; run \`bun tools/bundle-mac.ts\` without --skip-build`);
    if (build?.commit) note(`binary built from ${build.commit.slice(0, 12)}, shaders ${build.shaders}, features [${build.features.join(", ")}]`);
    const s = record.sizes;
    note(`sizes: release binary ${megabytes(s.releaseBinaryBytes)}, stripped executable ${megabytes(s.strippedExecutableBytes)}, PDFium ${megabytes(s.pdfiumBytes)}, bundle ${megabytes(s.bundleBytes)}`);
  } else {
    note(`no ${sidecar}; build hashes not checked`);
    note(`bundle size ${megabytes(treeBytes(app))}`);
  }

  if (failures.length > 0) fail(`${failures.length} bundle check(s) failed:\n${failures.map((line) => `  ${line}`).join("\n")}`);
  console.log(`All bundle checks passed for ${app}.`);
}

// Reproducibility: two builds whose recorded inputs match should produce the same files.
export function compare(firstSidecar: string, secondSidecar: string): void {
  const [a, b] = [firstSidecar, secondSidecar].map((path) => JSON.parse(readFileSync(path, "utf8")));
  const differ = (x: Record<string, unknown>, y: Record<string, unknown>) =>
    [...new Set([...Object.keys(x), ...Object.keys(y)])].filter((key) => JSON.stringify(x[key]) !== JSON.stringify(y[key]));
  const inputs = differ(a.inputs, b.inputs);
  const outputs = differ(a.outputs, b.outputs);
  console.log(inputs.length === 0 ? "Inputs match." : `Inputs differ in: ${inputs.join(", ")}.`);
  console.log(outputs.length === 0 ? `All ${Object.keys(a.outputs).length} bundle files match.` : `Bundle files differ: ${outputs.join(", ")}.`);
  if (inputs.length === 0 && outputs.length > 0) fail("The same inputs produced different bundles; the build is not reproducible for the files above.");
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const against = args.indexOf("--compare");
  if (against >= 0) {
    if (!args[against + 1] || !args[against + 2]) fail("Usage: bun tools/verify-bundle-mac.ts --compare <first.inputs.json> <second.inputs.json>");
    compare(args[against + 1], args[against + 2]);
  } else {
    const app = args.find((arg) => !arg.startsWith("--"));
    verify(app ?? join(root, "target/release/bundle/macos", `${identity.name}.app`), args.includes("--launch"));
  }
}
