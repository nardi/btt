//! Checks that the files the web viewer tests read match what this crate writes.
//!
//! The viewer decodes the links in `ui/viewer/src/fixtures` and compares the result to the JSON
//! next to each link, which proves that its reader follows the encoder here. Run with
//! `UPDATE_VIEWER_FIXTURES=1` to rewrite the files instead.

mod common;

use std::path::PathBuf;

use btt_model::link::vocabulary;
use btt_model::{
    SNAPSHOT_SCHEMA_VERSION, StoredSnapshot, TripSnapshot, decode_snapshot_link,
    encode_snapshot_link,
};
use common::{demo_snapshot, stress_snapshot};

/// Returns the path of a file in the folder of the viewer's sources.
fn viewer_source(relative_path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../ui/viewer/src")
        .join(relative_path)
}

/// Returns whether the files are to be rewritten instead of checked.
fn updating() -> bool {
    std::env::var_os("UPDATE_VIEWER_FIXTURES").is_some()
}

/// Returns the document of a snapshot as the file route writes it.
fn document_of(snapshot: &TripSnapshot) -> StoredSnapshot {
    StoredSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        snapshot: snapshot.clone(),
    }
}

/// Checks the fixture of a snapshot, or writes it. The fixture holds a link and the document the
/// link stands for. The large snapshot is not indented, to keep its file small.
///
/// The check reads the committed link back instead of comparing it to a fresh one, because the
/// bytes of a deflate stream depend on the compressor, and a build with another crate may pick
/// another compressor than a build of this crate alone.
fn check_or_write_fixture(relative_path: &str, snapshot: &TripSnapshot, pretty: bool) {
    let path = viewer_source(relative_path);
    if updating() {
        let link = encode_snapshot_link(snapshot).unwrap();
        let fixture = serde_json::json!({
            "payload": link.payload,
            "document": document_of(snapshot),
        });
        let mut text = if pretty {
            serde_json::to_string_pretty(&fixture).unwrap()
        } else {
            serde_json::to_string(&fixture).unwrap()
        };
        text.push('\n');
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let advice = format!(
        "ui/viewer/src/{relative_path} is out of date. Run `UPDATE_VIEWER_FIXTURES=1 cargo test -p btt-model --test viewer_fixtures`."
    );
    let committed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect(&advice)).expect(&advice);
    let payload = committed["payload"].as_str().expect(&advice);
    assert_eq!(
        decode_snapshot_link(payload).expect(&advice),
        *snapshot,
        "{advice}"
    );
    // JSON objects have no order, and another crate in the build can make `serde_json` keep the
    // order of the fields instead of sorting them, so the check compares data.
    let fresh_document = serde_json::to_value(document_of(snapshot)).unwrap();
    assert_eq!(committed["document"], fresh_document, "{advice}");
}

#[test]
fn the_vocabularies_of_the_viewer_are_current() {
    let path = viewer_source("generated/vocabularies.ts");
    let fresh = vocabulary::typescript_module();
    if updating() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, fresh).unwrap();
        return;
    }
    // Git may check the file out with Windows line endings.
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        fresh == committed,
        "ui/viewer/src/generated/vocabularies.ts is out of date. Run `UPDATE_VIEWER_FIXTURES=1 cargo test -p btt-model --test viewer_fixtures`."
    );
}

#[test]
fn the_demo_link_of_the_viewer_is_current() {
    check_or_write_fixture("fixtures/demo.json", &demo_snapshot(), true);
}

#[test]
fn the_stress_link_of_the_viewer_is_current() {
    check_or_write_fixture("fixtures/stress.json", &stress_snapshot(), false);
}
