use mathdoc::store::Node;
use serde_json::Value;

#[test]
fn phase_one_fixture_matches_legacy_node_revision() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../coordinator/test/contracts/v1/contracts.fixture.json"
    ))
    .expect("valid phase-one fixture");
    // legacy_revision_key_order: metadata keys whose Rust (UTF-8 byte) order differs from
    // JavaScript object and UTF-16 order: integer-like keys, a comma, BMP vs astral.
    let cases = fixture["legacy_revision"]
        .as_array()
        .unwrap()
        .iter()
        .chain(fixture["legacy_revision_key_order"].as_array().unwrap());
    for case in cases {
        let node: Node = serde_json::from_value(case["node"].clone()).expect("valid legacy node");
        assert_eq!(node.revision(), case["expected"].as_str().unwrap());
    }
}
