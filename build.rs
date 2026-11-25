// Embeds the icon, res/app.manifest (Windows 10 compatibility - required for the taskbar strip's
// layered child window - per-monitor DPI awareness, common controls 6) and version info.
#[path = "res/resources.rs"]
mod resources;

fn main() {
    if std::env::var_os("CARGO_FEATURE_EXPERIMENTAL_XAML").is_some() {
        let publisher = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("src/experimental_xaml/vendor/publisher");
        println!("cargo:rerun-if-changed={}", publisher.join("widget_bridge_protocol.h").display());
        println!("cargo:rerun-if-changed={}", publisher.join("widget_bridge_publisher.h").display());
        println!("cargo:rerun-if-changed={}", publisher.join("widget_bridge_publisher.cpp").display());
        println!("cargo:rerun-if-changed={}", publisher.join("publisher_ffi.cpp").display());
        cc::Build::new()
            .cpp(true)
            .std("c++17")
            .flag("/EHsc")
            .include(&publisher)
            .file(publisher.join("widget_bridge_publisher.cpp"))
            .file(publisher.join("publisher_ffi.cpp"))
            .compile("batteryhub_xaml_publisher");
        println!("cargo:rustc-link-lib=Advapi32");
    }
    // The app version lives in the VERSION file (Cargo versions cannot have four parts).
    let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let version = std::fs::read_to_string(root.join("VERSION")).unwrap().trim().to_string();
    println!("cargo:rerun-if-changed=VERSION");
    println!("cargo:rustc-env=BH_VERSION={version}");
    // Public half of the release-signing key (scripts/new-signing-key.ps1): when present, the
    // updater only starts setups whose SHA256SUMS.txt carries a valid signature from it.
    let key = root.join("release-signing.pub");
    println!("cargo:rerun-if-changed={}", key.display());
    if let Ok(hex) = std::fs::read_to_string(&key) {
        let hex = hex.trim();
        assert!(hex.len() == 128 && hex.chars().all(|c| c.is_ascii_hexdigit()), "release-signing.pub must hold 64 bytes (X || Y) as hex");
        println!("cargo:rustc-env=BH_RELEASE_KEY={hex}");
    }
    let dir = root.join("res");
    resources::embed(&dir, "BatteryHub - battery levels of wireless devices");
}
