// Windows: embeds the app icon (resource 1, shown by Explorer, the taskbar and the
// settings window) and a manifest asking for Common Controls v6, so the settings
// window gets the modern Windows look instead of the Windows 95 one.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/dictate.rc");
    println!("cargo:rerun-if-changed=assets/dictate.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    embed_resource::compile("assets/dictate.rc", embed_resource::NONE).manifest_optional().unwrap();
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bin=dictate=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=dictate=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
