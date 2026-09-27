//! Checks that the committed TypeScript bindings match the Rust types.
//!
//! Run with `UPDATE_BINDINGS=1` to rewrite the bindings file instead.

use std::path::PathBuf;

#[test]
fn typescript_bindings_are_current() {
    let builder = btt_app_lib::specta_builder();
    let committed_path = PathBuf::from(btt_app_lib::TYPESCRIPT_BINDINGS_PATH);
    if std::env::var_os("UPDATE_BINDINGS").is_some() {
        builder
            .export(btt_app_lib::typescript_exporter(), &committed_path)
            .unwrap();
        return;
    }
    let fresh_path = std::env::temp_dir().join(format!("btt-bindings-{}.ts", std::process::id()));
    builder
        .export(btt_app_lib::typescript_exporter(), &fresh_path)
        .unwrap();
    let fresh = std::fs::read_to_string(&fresh_path).unwrap();
    std::fs::remove_file(&fresh_path).unwrap();
    let committed = std::fs::read_to_string(&committed_path).unwrap_or_default();
    // Git may check the file out with Windows line endings.
    assert!(
        fresh == committed.replace("\r\n", "\n"),
        "ui/shared/src/bindings.ts is out of date. Run `UPDATE_BINDINGS=1 cargo test -p btt-app`."
    );
}
