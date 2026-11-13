// Embeds an application manifest asking for Common Controls v6, so the settings
// window gets the modern Windows look instead of the Windows 95 one.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bin=dictate=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=dictate=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}
