use super::*;
use crate::config::TsgoConfig;
use crate::rules_spec::RuleKind;
use serde_json::{Value, json};

fn document(value: Value) -> Arc<DecisionContent> {
    Arc::new(serde_json::from_value(value).unwrap())
}

fn entry(path: &str) -> SpecEntry {
    SpecEntry {
        path: Arc::from(path),
        kind: RuleKind::Graph,
        title: None,
        description: None,
        content_hash: None,
        input_schema: None,
        output_schema: None,
        skeleton: None,
        examples: vec![],
    }
}

/// Nothing declared on the input node — the shape can only come from the
/// engine's analysis of what the expression reads.
fn expression_graph() -> Value {
    json!({
        "nodes": [
            { "id": "in", "name": "request", "type": "inputNode", "content": {} },
            {
                "id": "expr", "name": "compute", "type": "expressionNode",
                "content": { "expressions": [
                    { "id": "e1", "key": "total", "value": "cart.price * cart.quantity" }
                ]}
            },
            { "id": "out", "name": "response", "type": "outputNode", "content": {} }
        ],
        "edges": [
            { "id": "e-a", "sourceId": "in", "targetId": "expr" },
            { "id": "e-b", "sourceId": "expr", "targetId": "out" }
        ]
    })
}

/// The output shape exists only inside TypeScript, so deriving it requires
/// tsgo to have actually type-checked the handler.
fn function_graph() -> Value {
    json!({
        "nodes": [
            { "id": "in", "name": "request", "type": "inputNode", "content": {
                "schema": "{\"type\":\"object\",\"properties\":{\"amount\":{\"type\":\"number\"}},\"required\":[\"amount\"]}"
            }},
            { "id": "fn", "name": "score", "type": "functionNode", "content": {
                "source": "export const handler = async (input) => ({ score: input.amount * 2, tier: 'gold' as const });"
            }},
            { "id": "out", "name": "response", "type": "outputNode", "content": {} }
        ],
        "edges": [
            { "id": "e-a", "sourceId": "in", "targetId": "fn" },
            { "id": "e-b", "sourceId": "fn", "targetId": "out" }
        ]
    })
}

fn enrich_one(path: &str, content: Value) -> SpecEntry {
    let mut entries = vec![entry(path)];
    enrich(&mut entries, vec![(Arc::from(path), document(content))]);
    entries.remove(0)
}

#[test]
fn derives_input_schema_from_expression_references() {
    let entry = enrich_one("Pricing Rule", expression_graph());
    let input = entry
        .input_schema
        .as_deref()
        .expect("input schema derived from expression references");

    assert_eq!(input["$id"], json!("Pricing Rule"));
    assert_eq!(input["required"], json!(["cart"]));
    assert!(
        input["properties"]["cart"]["properties"]
            .get("price")
            .is_some(),
        "dotted reference cart.price nests: {input}"
    );
}

#[test]
fn derives_skeleton_from_inferred_inputs() {
    let entry = enrich_one("Pricing Rule", expression_graph());

    assert_eq!(
        entry.skeleton.as_deref(),
        Some(&json!({ "cart": { "price": null, "quantity": null } }))
    );
}

/// The end-to-end proof that tsgo is wired in: `tier` is only a `"gold"`
/// literal because the TypeScript compiler said so.
#[test]
fn derives_output_schema_through_tsgo() {
    tsgo::init(&TsgoConfig::default());

    let entry = enrich_one("Scoring", function_graph());
    let output = entry
        .output_schema
        .as_deref()
        .expect("output schema derived from the function node's return type");

    assert_eq!(output["properties"]["score"], json!({ "type": "number" }));
    assert_eq!(output["properties"]["tier"], json!({ "const": "gold" }));
    assert_eq!(output["required"], json!(["score", "tier"]));
}

#[test]
fn declared_schemas_are_not_overwritten() {
    let declared = Arc::new(json!({ "type": "object", "properties": { "declared": {} } }));
    let mut entries = vec![SpecEntry {
        input_schema: Some(declared.clone()),
        ..entry("Pricing Rule")
    }];

    enrich(
        &mut entries,
        vec![(Arc::from("Pricing Rule"), document(expression_graph()))],
    );

    assert_eq!(
        entries[0].input_schema.as_deref(),
        Some(declared.as_ref()),
        "an author's declared schema wins over the derived one"
    );
}

/// An output the engine cannot pin down must stay absent rather than
/// become an object that forbids every key.
#[test]
fn uninformative_types_leave_the_schema_unset() {
    let mut graph = expression_graph();
    graph["nodes"][1]["content"]["expressions"] = json!([]);
    let entry = enrich_one("Pricing Rule", graph);

    assert_eq!(entry.output_schema, None);
}

/// From zen-engine 2.1 (DNK-62), an expression node's outputs are listed
/// by key, and arithmetic types its result as a number.
#[test]
fn expression_outputs_are_listed_by_key() {
    let entry = enrich_one("Pricing Rule", expression_graph());
    let output = entry.output_schema.as_deref().expect("output schema");
    assert_eq!(output["properties"]["total"], json!({ "type": "number" }));
    assert_eq!(output["required"], json!(["total"]));

    // A value passed through keeps its key; its type is the caller's.
    let mut graph = expression_graph();
    graph["nodes"][1]["content"]["expressions"][0]["value"] = json!("cart.label");
    let entry = enrich_one("Pricing Rule", graph);
    let output = entry.output_schema.as_deref().expect("output schema");
    assert_eq!(output["properties"]["total"], json!({}));
}

#[test]
fn unknown_paths_are_left_alone() {
    let mut entries = vec![entry("missing-rule")];
    enrich(&mut entries, vec![]);

    assert_eq!(entries[0].input_schema, None);
    assert_eq!(entries[0].output_schema, None);
    assert_eq!(entries[0].skeleton, None);
}

#[test]
fn empty_entries_short_circuit() {
    let mut entries: Vec<SpecEntry> = vec![];
    enrich(&mut entries, vec![]);
}
