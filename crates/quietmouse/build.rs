//! Gives the Windows executables an icon and version details. Explorer, Task
//! Manager and a file's Properties all read them from resources embedded in the
//! .exe, and without any they show a generic icon and nothing about the program.

fn main() {
    // Resources are a Windows thing, and building them needs the Windows
    // resource compiler, so this only runs when building on Windows for Windows.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        windows_resources();
    }
}

#[cfg(windows)]
fn windows_resources() {
    let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/windows/quietmouse.ico");
    println!("cargo::rerun-if-changed={}", icon.display());
    println!("cargo::rerun-if-changed=build.rs");
    // Version numbers come from Cargo.toml; winresource fills those in itself.
    winresource::WindowsResource::new()
        .set_icon(&icon.to_string_lossy())
        .set("ProductName", "quietmouse")
        .set("FileDescription", "quietmouse")
        .set("CompanyName", "Ben Weaver")
        .set("LegalCopyright", "Copyright (c) 2026 quietmouse contributors")
        .compile()
        .expect("can't embed the icon and version details in the Windows executables");
}
