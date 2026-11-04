use super::*;
use serde_json::json;

#[test]
fn graph_with_metadata_is_parsed() {
    let value = json!({
        "contentType": "graph",
        "title": "Pricing",
        "description": "Cart pricing rules",
        "meta": { "versionId": "3a5b" },
        "nodes": [], "edges": []
    });

    let FileContent::Graph(graph) = serde_json::from_value::<FileContent>(value).unwrap() else {
        panic!("expected graph");
    };
    assert_eq!(graph.title.as_deref(), Some("Pricing"));
    assert_eq!(graph.description.as_deref(), Some("Cart pricing rules"));
    assert_eq!(graph.meta.version_id.as_deref(), Some("3a5b"));
    assert_eq!(graph.meta.display_path, None);
}

#[test]
fn legacy_content_type_is_still_graph() {
    let value = json!({
        "contentType": "application/vnd.gorules.decision",
        "nodes": [], "edges": []
    });
    assert!(matches!(
        serde_json::from_value::<FileContent>(value).unwrap(),
        FileContent::Graph(_)
    ));
}

#[test]
fn test_file_is_parsed() {
    let value = json!({
        "contentType": "test",
        "filePath": "Pricing Rule",
        "testCases": [
            { "id": "t1", "name": "small cart", "input": { "cartTotal": 10 } },
            { "name": "off", "input": {}, "disabled": true }
        ]
    });

    let FileContent::Test(test) = serde_json::from_value::<FileContent>(value).unwrap() else {
        panic!("expected test file");
    };
    assert_eq!(test.file_path.as_deref(), Some("Pricing Rule"));
    assert!(!test.disabled);
    assert_eq!(test.test_cases.len(), 2);
    assert_eq!(test.test_cases[0].name.as_deref(), Some("small cart"));
    assert_eq!(test.test_cases[0].input, json!({ "cartTotal": 10 }));
    assert!(!test.test_cases[0].disabled);
    assert!(test.test_cases[1].disabled);
}

#[test]
fn unparseable_content_is_unknown() {
    let value = json!({ "foo": "bar" });
    assert!(matches!(
        serde_json::from_value::<FileContent>(value).unwrap(),
        FileContent::Unknown
    ));
}
