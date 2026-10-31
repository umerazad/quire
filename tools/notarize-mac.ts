// Notarizes and staples a signed disk image, after Zed's script/bundle-mac. Runs only when the
// owner has configured credentials; otherwise it says what is missing and leaves the file as is.
// Credentials: QUIRE_NOTARY_PROFILE (a keychain profile made with `xcrun notarytool
// store-credentials`), or an App Store Connect API key in APPLE_NOTARIZATION_KEY (the .p8 text),
// APPLE_NOTARIZATION_KEY_ID, and APPLE_NOTARIZATION_ISSUER_ID.
// Usage: bun tools/notarize-mac.ts <signed .dmg>
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fail } from "./package.ts";

export const skipped =
  "Notarization skipped: set QUIRE_SIGNING_IDENTITY to a Developer ID Application identity and either " +
  "QUIRE_NOTARY_PROFILE (from `xcrun notarytool store-credentials`) or APPLE_NOTARIZATION_KEY, " +
  "APPLE_NOTARIZATION_KEY_ID, and APPLE_NOTARIZATION_ISSUER_ID. Without it, macOS asks users to approve the app in System Settings before it opens.";

export function notarize(path: string): boolean {
  const env = process.env;
  const apiKey = env.APPLE_NOTARIZATION_KEY && env.APPLE_NOTARIZATION_KEY_ID && env.APPLE_NOTARIZATION_ISSUER_ID;
  if (!env.QUIRE_SIGNING_IDENTITY || !(env.QUIRE_NOTARY_PROFILE || apiKey)) {
    console.log(skipped);
    return false;
  }
  // The key file must be removed before any exit, so failures here return a message instead of
  // calling fail(), which exits without running `finally`.
  const scratch = mkdtempSync(join(tmpdir(), "quire-notary-"));
  let problem: string | undefined;
  try {
    const auth = env.QUIRE_NOTARY_PROFILE
      ? ["--keychain-profile", env.QUIRE_NOTARY_PROFILE]
      : (() => {
          const key = join(scratch, "key.p8");
          writeFileSync(key, env.APPLE_NOTARIZATION_KEY!, { mode: 0o600 });
          return ["--key", key, "--key-id", env.APPLE_NOTARIZATION_KEY_ID!, "--issuer", env.APPLE_NOTARIZATION_ISSUER_ID!];
        })();
    console.log(`Submitting ${path} for notarization; this waits for Apple's answer.`);
    const submit = spawnSync("xcrun", ["notarytool", "submit", "--wait", "--output-format", "json", ...auth, path], { encoding: "utf8" });
    const answer = (() => {
      try {
        return JSON.parse(submit.stdout) as { id?: string; status?: string };
      } catch {
        return {};
      }
    })();
    if (submit.status !== 0 || answer.status !== "Accepted")
      problem =
        `Notarization of ${path} ended with ${answer.status ?? `exit ${submit.status}`}: ${submit.stderr.trim()}` +
        (answer.id ? `\nRead Apple's reasons with \`xcrun notarytool log ${answer.id}\` and the same credentials, fix them, then rebuild.` : "\nCheck the credentials, then rerun.");
    else {
      const staple = spawnSync("xcrun", ["stapler", "staple", path], { encoding: "utf8" });
      if (staple.status !== 0) problem = `Stapling ${path} failed: ${staple.stderr.trim()}. Retry \`xcrun stapler staple\` once Apple's ticket is published.`;
    }
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
  if (problem) fail(problem);
  return true;
}

if (import.meta.main) {
  const path = process.argv[2];
  if (!path) fail("Usage: bun tools/notarize-mac.ts <signed .dmg>");
  notarize(resolve(path));
}
