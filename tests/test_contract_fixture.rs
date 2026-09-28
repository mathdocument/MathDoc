use mathdoc::store::Node;
use serde_json::Value;

#[test]
fn phase_one_fixture_matches_legacy_node_revision() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../coordinator/test/contracts/v1/contracts.fixture.json"
    ))
    .expect("valid phase-one fixture");
    let case = &fixture["legacy_revision"];
    let node: Node = serde_json::from_value(case["node"].clone()).expect("valid legacy node");
    assert_eq!(node.revision(), case["expected"].as_str().unwrap());
}
