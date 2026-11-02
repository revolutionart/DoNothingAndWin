fn main() {
    println!("cargo:rerun-if-changed=assets/brand_icon.png");
    println!("cargo:rerun-if-changed=assets/brand_icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // Keep the same multi-resolution icon for the executable and installer.
    // Regenerate from the window's PNG with scripts/generate-app-icon.py.
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/brand_icon.ico");
    resource.set("ProductName", "DoNothing&Win Launcher");
    resource.set("FileDescription", "DoNothing&Win Launcher");
    resource.set("InternalName", "DoNothingAndWinLauncher");
    resource.set("OriginalFilename", "DoNothingAndWinLauncher.exe");
    resource
        .compile()
        .expect("Compile application icon resource");
}
