//! Test helpers for the code-delivery landing tests.
#![allow(dead_code)]

use std::path::Path;

pub(crate) fn landed_evidence(s: &pulse_store::Store) -> pulse_codedelivery::Evidence {
    let detail = crate::test_helpers_daemon::must_rows(
        s,
        "SELECT detail FROM events WHERE action = 'land.landed' ORDER BY seq DESC LIMIT 1",
        &[],
    )[0]
    .str("detail");
    let detail: serde_json::Value =
        serde_json::from_str(&detail).unwrap_or_else(|err| panic!("{err}"));
    let (evidence, ok) = pulse_codedelivery::decode_evidence(detail.get("code_delivery"));
    if !ok {
        panic!("landing detail has no code_delivery: {detail:#?}");
    }
    evidence
}

pub(crate) fn write_file(root: impl AsRef<Path>, rel: &str, text: &str) {
    let path = root.as_ref().join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap_or_else(|err| panic!("{err}"));
    std::fs::write(path, text.as_bytes()).unwrap_or_else(|err| panic!("{err}"));
}
