// Notarizes and staples a signed disk image, after Zed's script/bundle-mac. Runs only when the
// owner has configured credentials; otherwise it says what is missing and leaves the file as is.
// Credentials: QUIRE_NOTARY_PROFILE (a keychain profile made with `xcrun notarytool
// store-credentials`), or an App Store Connect API key in APPLE_NOTARIZATION_KEY (the .p8 text),
// APPLE_NOTARIZATION_KEY_ID, and APPLE_NOTARIZATION_ISSUER_ID.
// Usage: bun tools/notarize-mac.ts <signed .dmg>
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fail, run } from "./package.ts";

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
  const scratch = mkdtempSync(join(tmpdir(), "quire-notary-"));
  try {
    const auth = env.QUIRE_NOTARY_PROFILE
      ? ["--keychain-profile", env.QUIRE_NOTARY_PROFILE]
      : (() => {
          const key = join(scratch, "key.p8");
          writeFileSync(key, env.APPLE_NOTARIZATION_KEY!);
          chmodSync(key, 0o600);
          return ["--key", key, "--key-id", env.APPLE_NOTARIZATION_KEY_ID!, "--issuer", env.APPLE_NOTARIZATION_ISSUER_ID!];
        })();
    console.log(`Submitting ${path} for notarization; this waits for Apple's answer.`);
    run("xcrun", ["notarytool", "submit", "--wait", ...auth, path], { stdio: "inherit" });
    run("xcrun", ["stapler", "staple", path], { stdio: "inherit" });
    return true;
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const path = process.argv[2];
  if (!path) fail("Usage: bun tools/notarize-mac.ts <signed .dmg>");
  notarize(resolve(path));
}
