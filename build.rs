fn main() {
    println!("cargo:rerun-if-changed=assets/brand_icon.png");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("brand.ico");
    let icon = image::open("assets/brand_icon.png").expect("Read brand icon");
    // The ICO resource is regenerated from the same PNG used by the window.
    let resized = icon
        .resize(256, 256, image::imageops::FilterType::Lanczos3)
        .to_rgba8();
    let mut square = image::RgbaImage::new(256, 256);
    image::imageops::overlay(
        &mut square,
        &resized,
        ((256 - resized.width()) / 2) as i64,
        ((256 - resized.height()) / 2) as i64,
    );
    square
        .save_with_format(&output, image::ImageFormat::Ico)
        .expect("Encode Windows icon");
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(output.to_str().unwrap());
    resource.set("ProductName", "DoNothing&Win Launcher");
    resource.set("FileDescription", "DoNothing&Win Launcher");
    resource.set("InternalName", "DoNothingAndWinLauncher");
    resource.set("OriginalFilename", "DoNothingAndWinLauncher.exe");
    resource
        .compile()
        .expect("Compile application icon resource");
}
