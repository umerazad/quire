//! Finding the PDFium library. Quire binds a prebuilt PDFium from `bblanchon/pdfium-binaries`
//! at runtime. The build is pinned to the API the `pdfium-render` feature in this crate's
//! manifest binds (`pdfium_7881`), and `tools/fetch-pdfium.ts` downloads the hash-pinned archive
//! for this platform into the cache directory below. Packages place the library in the macOS
//! bundle's Frameworks, the Linux tarball's `lib`, or beside the Windows executable (`tools/bundle-*.ts`).

use std::{
    env,
    path::{Path, PathBuf},
};

/// The `bblanchon/pdfium-binaries` release this build binds.
pub const PDFIUM_BUILD: &str = "chromium/7881";

/// The archive name for this platform, as `tools/fetch-pdfium.ts` names it.
pub fn platform() -> &'static str {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => "mac-arm64",
        ("macos", _) => "mac-x64",
        ("windows", "aarch64") => "win-arm64",
        ("windows", _) => "win-x64",
        (_, "aarch64") => "linux-arm64",
        _ => "linux-x64",
    }
}

/// Where `tools/fetch-pdfium.ts` unpacks the library: `QUIRE_PDFIUM_DIR`, or the user cache.
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("QUIRE_PDFIUM_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = if cfg!(target_os = "macos") {
        env::var_os("HOME").map(|home| Path::new(&home).join("Library/Caches"))
    } else if cfg!(windows) {
        env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".cache")))
    }?;
    Some(
        base.join("quire/pdfium")
            .join(PDFIUM_BUILD.replace('/', "-"))
            .join(platform()),
    )
}

fn library_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else if cfg!(windows) {
        "pdfium.dll"
    } else {
        "libpdfium.so"
    }
}

/// Places to look, in order: `QUIRE_PDFIUM_LIBRARY`, beside the executable (and in a macOS
/// bundle's Frameworks or a Linux package's `lib`), then the fetched cache.
pub fn candidates() -> Vec<PathBuf> {
    let name = library_name();
    let mut found = Vec::new();
    if let Some(path) = env::var_os("QUIRE_PDFIUM_LIBRARY") {
        found.push(PathBuf::from(path));
    }
    if let Some(dir) = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_owned))
    {
        found.push(dir.join(name));
        found.push(dir.join("../Frameworks").join(name));
        found.push(dir.join("../lib").join(name));
    }
    if let Some(cache) = cache_dir() {
        found.push(
            cache
                .join(if cfg!(windows) { "bin" } else { "lib" })
                .join(name),
        );
    }
    found
}

/// The first library that exists.
pub fn locate() -> Option<PathBuf> {
    candidates().into_iter().find(|path| path.is_file())
}
