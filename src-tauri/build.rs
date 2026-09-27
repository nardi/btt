//! Generates the Tauri context from `tauri.conf.json` and the capability files.

fn main() {
    // Tauri needs version 6 of the Windows common controls, which only an application manifest
    // selects. `tauri-build` embeds that manifest in the app binary only, so test binaries that
    // link Tauri fail to start. Embedding the manifest through linker arguments covers both.
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    tauri_build::try_build(attributes).expect("tauri-build failed");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        let manifest_directory = std::env::var("CARGO_MANIFEST_DIR").expect("set by Cargo");
        let manifest_path = format!("{manifest_directory}/windows-app-manifest.xml");
        for target_kind in ["bins", "tests"] {
            println!("cargo:rustc-link-arg-{target_kind}=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg-{target_kind}=/MANIFESTINPUT:{manifest_path}");
        }
        println!("cargo:rerun-if-changed=windows-app-manifest.xml");
    }
}
