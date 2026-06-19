//! Embeds the Windows executable icon from `assets/icon.ico` if present.
//!
//! The runtime tray/window icon is generated in code (`src/icon.rs`), so the app
//! always has an icon. This only sets the icon shown by Explorer/taskbar for the
//! `.exe` itself, which needs a real `.ico`. Generate one from `assets/icon.svg`
//! (see README "App icon"); until then this is a no-op and the build is unaffected.

fn main() {
    #[cfg(windows)]
    {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
        let ico = std::path::Path::new(&manifest).join("../../assets/icon.ico");
        println!("cargo:rerun-if-changed=../../assets/icon.ico");
        if ico.exists() {
            let mut res = winresource::WindowsResource::new();
            res.set_icon(&ico.to_string_lossy());
            if let Err(e) = res.compile() {
                println!("cargo:warning=could not embed exe icon: {e}");
            }
        }
    }
}
