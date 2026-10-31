// The pinned PDFium build and where tools/fetch-pdfium.ts unpacks it, shared with the packaging
// scripts. Mirrors crates/quire_media/src/pdf/library.rs.
import { homedir } from "node:os";
import { join } from "node:path";

// Must match PDFIUM_BUILD and the `pdfium_7881` feature of pdfium-render in crates/quire_media.
export const build = "chromium/7881";
export const archives: Record<string, string> = {
  "mac-arm64": "52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40",
  "mac-x64": "6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b",
  "linux-x64": "1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d",
  "linux-arm64": "ee7f7b7d5468958336a818c1cd580bdd20972846b7377b13f9a923d92d1d4674",
  "win-x64": "73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08",
  "win-arm64": "d3035d4d2cacac6ecd1a2ece197a3d702a1b2a58466276b9f870b8cb278a9d84",
};

export function hostPlatform(): string {
  const arm = process.arch === "arm64";
  return process.platform === "darwin"
    ? arm ? "mac-arm64" : "mac-x64"
    : process.platform === "win32"
      ? arm ? "win-arm64" : "win-x64"
      : arm ? "linux-arm64" : "linux-x64";
}

export function cacheDir(platform: string): string {
  if (process.env.QUIRE_PDFIUM_DIR !== undefined) return process.env.QUIRE_PDFIUM_DIR;
  const base =
    process.platform === "darwin"
      ? join(homedir(), "Library/Caches")
      : process.platform === "win32"
        ? process.env.LOCALAPPDATA ?? join(homedir(), "AppData/Local")
        : process.env.XDG_CACHE_HOME ?? join(homedir(), ".cache");
  return join(base, "quire/pdfium", build.replace("/", "-"), platform);
}

export function libraryName(platform: string): string {
  return platform.startsWith("win") ? "pdfium.dll" : platform.startsWith("mac") ? "libpdfium.dylib" : "libpdfium.so";
}

export function libraryPath(platform: string): string {
  return join(cacheDir(platform), platform.startsWith("win") ? "bin" : "lib", libraryName(platform));
}
