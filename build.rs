// Windows: embeds the app icon (resource 1, shown by Explorer, the taskbar and the
// settings window) and a manifest asking for Common Controls v6, so the settings
// window gets the modern Windows look instead of the Windows 95 one.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/dictate.rc");
    println!("cargo:rerun-if-changed=assets/dictate.ico");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os == "macos" && std::env::var_os("CARGO_FEATURE_METAL").is_some() {
        link_clang_runtime();
    }
    if os != "windows" {
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

// macOS + Metal: ggml's Objective-C code uses `@available`, which calls
// `__isPlatformVersionAtLeast` from clang's runtime library; rustc's link line
// doesn't include it, so link libclang_rt.osx.a from the active clang.
fn link_clang_runtime() {
    let out = std::process::Command::new("clang").arg("--print-resource-dir").output();
    let Ok(out) = out else { return };
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let lib = std::path::Path::new(&dir).join("lib/darwin");
    if lib.join("libclang_rt.osx.a").exists() {
        println!("cargo:rustc-link-search=native={}", lib.display());
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    } else {
        println!("cargo:warning=libclang_rt.osx.a not found in {}", lib.display());
    }
}
