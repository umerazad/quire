fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=resources/quire.rc");
    println!("cargo:rerun-if-changed=resources/app-icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && let Err(error) = embed_resource::compile(
            "resources/quire.rc",
            embed_resource::ParamsIncludeDirs(["resources"]),
        )
        .manifest_optional()
    {
        panic!(
            "Cannot embed the Windows icon from resources/quire.rc: {error}. Install the Windows SDK resource compiler (rc.exe) or llvm-rc, then build again."
        );
    }
}
