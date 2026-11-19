#[path = "../../../tests/support/native_home_cli.rs"]
mod fixture;

use serde_json::{Value, json};

fn run(fixture: &fixture::Fixture, annotations: &std::path::Path) -> Value {
    let output = fixture
        .command(env!("CARGO_BIN_EXE_semon"))
        .args(["sessions", "--all", "--json", "--proc-root"])
        .arg(fixture.root.join("empty-proc"))
        .arg("--cache")
        .arg(fixture.root.join("cache/index.json"))
        .arg("--annotations")
        .arg(annotations)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cli_annotation_metadata_preserves_native_tree_and_input() {
    let fixture = fixture::Fixture::new();
    let path = fixture.root.join("annotations.jsonl");
    let before = run(&fixture, &path);
    let record = json!({"v":1,"harness":"codex","native_id":"00000000-0000-4000-8000-000000000002",
        "system":"ostrom","repository":"owner/repo","ticket":"9","issue":12,"pr":34,
        "title":"Annotation title","updated_at":"2026-11-19T04:52:01Z"});
    let bytes = format!("{record}\nrejected private content\n{record}");
    fixture.write("annotations.jsonl", bytes.as_bytes());
    let after = run(&fixture, &path);
    assert_eq!(
        after["annotations"],
        json!({"path":path,"valid":1,"rejected":1})
    );
    assert!(!after.to_string().contains("rejected private content"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes.as_bytes());
    let mut stripped = after["roots"].clone();
    fn strip(nodes: &mut Value) {
        for node in nodes.as_array_mut().unwrap() {
            if node.get("annotation").is_some() {
                assert_eq!(node["annotation"]["source"], "annotation");
                assert_eq!(node["annotation"]["pr"], 34);
                assert_eq!(node["label"], "Annotation title");
                node.as_object_mut().unwrap().remove("annotation");
                node["label"] = Value::Null;
            }
            // Collection time can advance between the two reads.
            node.as_object_mut()
                .unwrap()
                .remove("last_activity_age_seconds");
            strip(&mut node["children"]);
        }
    }
    strip(&mut stripped);
    let mut baseline = before["roots"].clone();
    strip(&mut baseline);
    assert_eq!(stripped, baseline);
}

#[test]
fn native_names_win_over_annotation_titles() {
    let fixture = fixture::Fixture::new();
    let id = "00000000-0000-4000-8000-000000000002";
    fixture.write(
        "configured-codex/session_index.jsonl",
        format!(
            "{}\n",
            json!({
                "id":id,"thread_name":"Native thread name","updated_at":"2026-11-19T04:52:01Z"
            })
        )
        .as_bytes(),
    );
    fixture.write(
        "annotations.jsonl",
        format!(
            "{}\n",
            json!({"v":1,"harness":"codex","native_id":id,
        "system":"ostrom","repository":"owner/repo","ticket":"9","issue":null,"pr":null,
        "title":"Annotation title","updated_at":"2026-11-19T04:52:01Z"})
        )
        .as_bytes(),
    );
    let output = run(&fixture, &fixture.root.join("annotations.jsonl"));
    let node = output["roots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == id)
        .unwrap();
    assert_eq!(node["label"], "Native thread name");
    assert_eq!(node["annotation"]["title"], "Annotation title");
}
