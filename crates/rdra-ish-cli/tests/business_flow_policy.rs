//! Flow condition references are traceability, never an implicit runtime guard.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn declared_flow_structure_and_condition_references_survive_direct_exports() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_rdra-ish"))
            .current_dir(&root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let sample = "samples/python-domain/flow.rdra";
    run(&["check", sample]);
    let diagram = run(&["diagram", sample, "--kind", "flow"]);
    assert!(
        diagram.contains("step__InspectApplication -.-> condition__Approved"),
        "{diagram}"
    );
    assert!(
        diagram.contains("step__InspectApplication -->|precedes| step__FinishReview"),
        "{diagram}"
    );
    let policy: serde_json::Value =
        serde_json::from_str(&run(&["codegen", sample, "--lang", "policy"])).unwrap();
    assert!(diagram.contains("step__InspectApplication -.-> variation__ReviewChoice"));
    assert_eq!(policy["guards"], serde_json::json!([]));
    let flow = policy["flows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|flow| flow["id"] == "ReviewFlow")
        .unwrap();
    assert_eq!(flow["id"], "ReviewFlow");
    assert_eq!(flow["bucs"], serde_json::json!(["ReviewApplications"]));
    let step = flow["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|step| step["id"] == "InspectApplication")
        .unwrap();
    assert_eq!(step["performers"], serde_json::json!(["Reviewer"]));
    assert!(step["covers"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!({"kind":"condition","id":"Approved"})));
    assert!(step["covers"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!({"kind":"variation","id":"ReviewChoice"})));
    assert_eq!(
        flow["links"],
        serde_json::json!([{"kind":"precedes","from":"InspectApplication","to":"FinishReview"}])
    );
}
