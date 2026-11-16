use super::*;
use crate::data::extended_decision::FileContent;
use serde_json::json;

fn graph(value: serde_json::Value, display_path: &str) -> (String, FileDecisionGraph) {
    let FileContent::Graph(mut graph) = serde_json::from_value::<FileContent>(value).unwrap()
    else {
        panic!("expected graph");
    };
    graph.meta.display_path = Some(Arc::from(display_path));
    (display_path.to_lowercase(), graph)
}

fn test_file(value: serde_json::Value) -> FileTestContent {
    let FileContent::Test(test) = serde_json::from_value::<FileContent>(value).unwrap() else {
        panic!("expected test file");
    };
    test
}

#[test]
fn collect_examples_caps_and_skips_disabled() {
    let files = vec![
        (
            "Pricing Rule.test".to_string(),
            test_file(json!({
                "contentType": "test",
                "filePath": "Pricing Rule",
                "testCases": [
                    { "name": "one", "input": { "n": 1 } },
                    { "name": "off", "input": { "n": 0 }, "disabled": true },
                    { "input": { "n": 3 } },
                    { "name": "four", "input": { "n": 4 } },
                    { "name": "five", "input": { "n": 5 } }
                ]
            })),
        ),
        (
            "disabled.test".to_string(),
            test_file(json!({
                "contentType": "test",
                "filePath": "Pricing Rule",
                "disabled": true,
                "testCases": [{ "name": "ignored", "input": {} }]
            })),
        ),
        (
            "orphan.test".to_string(),
            test_file(json!({ "contentType": "test", "testCases": [{ "input": {} }] })),
        ),
    ];

    let examples = collect_examples(files);
    assert_eq!(examples.len(), 1);

    let list = examples.get("pricing rule").unwrap();
    assert_eq!(
        list.len(),
        3,
        "capped at MAX_EXAMPLES_PER_RULE, disabled skipped"
    );
    assert_eq!(list[0].name.as_ref(), "one");
    assert_eq!(
        list[1].name.as_ref(),
        "Pricing Rule.test",
        "name falls back to test file path"
    );
    assert_eq!(list[1].input, json!({ "n": 3 }));
    assert_eq!(list[2].name.as_ref(), "four");
    assert_eq!(list[0].source_test_file.as_ref(), "Pricing Rule.test");
}

#[test]
fn spec_entries_extracts_schemas() {
    let (key_b, graph_b) = graph(
        json!({
            "contentType": "graph",
            "title": "Pricing",
            "description": "Cart pricing rules",
            "meta": { "versionId": "3a5b" },
            "nodes": [
                { "id": "in", "name": "request", "type": "inputNode",
                  "content": { "schema": "{\"type\":\"object\",\"properties\":{\"cartTotal\":{\"type\":\"number\"}}}" } },
                { "id": "out", "name": "response", "type": "outputNode",
                  "content": { "schema": "{\"type\":\"object\"}" } }
            ],
            "edges": [{ "id": "e1", "sourceId": "in", "targetId": "out" }]
        }),
        "Pricing Rule",
    );
    let (key_a, graph_a) = graph(
        json!({ "contentType": "graph", "nodes": [], "edges": [] }),
        "alpha-rule",
    );

    let mut examples = HashMap::new();
    examples.insert(
        "pricing rule".to_string(),
        vec![SpecExample {
            name: Arc::from("one"),
            input: json!({ "n": 1 }),
            source_test_file: Arc::from("Pricing Rule.test"),
        }],
    );

    let loader = ImmutableLoader::new(
        HashMap::from([(key_a, graph_a), (key_b, graph_b)]),
        examples,
        None,
    );

    let entries = loader.spec_entries();
    assert_eq!(entries.len(), 2);

    let pricing = entries
        .iter()
        .find(|e| e.path.as_ref() == "Pricing Rule")
        .expect("expected an entry for Pricing Rule");
    assert_eq!(pricing.title.as_deref(), Some("Pricing"));
    assert_eq!(pricing.description.as_deref(), Some("Cart pricing rules"));
    assert_eq!(pricing.content_hash.as_deref(), Some("3a5b"));
    assert_eq!(
        pricing.input_schema.as_deref(),
        Some(&json!({ "type": "object", "properties": { "cartTotal": { "type": "number" } } }))
    );
    assert_eq!(
        pricing.output_schema.as_deref(),
        Some(&json!({ "type": "object" }))
    );
    assert_eq!(pricing.examples.len(), 1);

    let alpha = entries
        .iter()
        .find(|e| e.path.as_ref() == "alpha-rule")
        .expect("expected an entry for alpha-rule");
    assert_eq!(alpha.title, None);
    assert_eq!(alpha.input_schema, None);
    assert_eq!(alpha.examples.len(), 0);
}
