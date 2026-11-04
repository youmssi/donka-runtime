use super::*;
use crate::data::release_data::{
    ReleaseData, ReleaseDataEnvironment, ReleaseDataProject, ReleaseDataRelease,
};
use serde_json::json;

fn release() -> ReleaseData {
    ReleaseData {
        version: Some(Arc::from("1")),
        project: Some(ReleaseDataProject {
            id: Some(Arc::from("11111111-1111-1111-1111-111111111111")),
            key: Some(Arc::from("rules-project")),
            name: None,
        }),
        access_tokens: vec![Arc::from("secret-token")],
        access_token_hashes: Vec::new(),
        release: Some(ReleaseDataRelease {
            id: Some(Arc::from("22222222-2222-2222-2222-222222222222")),
            version: Some(Arc::from("1.2.3")),
            name: None,
            status: None,
            commit_id: None,
        }),
        environment: None,
    }
}

fn environment_release() -> ReleaseData {
    let mut data = release();
    data.environment = Some(ReleaseDataEnvironment {
        id: Some(Arc::from("77777777-7777-7777-7777-777777777777")),
        key: Some(Arc::from("production")),
        name: Some(Arc::from("Production")),
    });
    data
}

fn entries() -> Vec<SpecEntry> {
    vec![
        SpecEntry {
            path: Arc::from("Pricing Rule"),
            kind: RuleKind::Graph,
            title: Some(Arc::from("Pricing")),
            description: Some(Arc::from("Cart pricing rules")),
            content_hash: Some(Arc::from("3a5b")),
            input_schema: Some(Arc::new(json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": { "cartTotal": { "type": "number" } }
            }))),
            output_schema: Some(Arc::new(json!({ "type": "object" }))),
            skeleton: Some(Arc::new(json!({ "cartTotal": 0 }))),
            examples: vec![SpecExample {
                name: Arc::from("small cart"),
                input: json!({ "cartTotal": 10 }),
                source_test_file: Arc::from("Pricing Rule.test"),
            }],
        },
        SpecEntry {
            path: Arc::from("plain-rule"),
            kind: RuleKind::Policy,
            title: None,
            description: None,
            content_hash: None,
            input_schema: None,
            output_schema: None,
            skeleton: None,
            examples: vec![],
        },
    ]
}

#[test]
fn document_shape_with_release() {
    let release = release();
    let entries = entries();
    let document = build_rules_openapi(RulesDocumentSource {
        project_ref: "rules-project",
        release: Some(&release),
        entries: &entries,
    });

    assert_eq!(document["openapi"], json!("3.0.3"));
    assert_eq!(document["info"]["title"], json!("rules-project rules API"));
    assert_eq!(document["info"]["version"], json!("1.2.3"));
    assert_eq!(
        document["x-gorules"]["source"],
        json!({
            "type": "release",
            "releaseId": "22222222-2222-2222-2222-222222222222",
            "version": "1.2.3"
        })
    );
    assert_eq!(
        document["servers"][0]["url"],
        json!("/api/rules/rules-project")
    );
    assert_eq!(
        document["servers"][0]["description"],
        json!("Deployed release 1.2.3")
    );
    assert_eq!(document["security"], json!([{ "accessToken": [] }]));
    assert_eq!(
        document["components"]["securitySchemes"]["accessToken"],
        json!({ "type": "apiKey", "in": "header", "name": "X-Access-Token" })
    );
}

#[test]
fn rich_entry_operation() {
    let release = release();
    let entries = entries();
    let document = build_rules_openapi(RulesDocumentSource {
        project_ref: "rules-project",
        release: Some(&release),
        entries: &entries,
    });

    let post = &document["paths"]["/evaluate/Pricing Rule"]["post"];
    assert_eq!(post["operationId"], json!("evaluate_Pricing_Rule"));
    assert_eq!(post["summary"], json!("Pricing"));
    assert_eq!(post["description"], json!("Cart pricing rules"));
    assert_eq!(post["tags"], json!(["graph"]));
    assert_eq!(
        post["x-gorules"],
        json!({
            "path": "Pricing Rule",
            "kind": "graph",
            "contentHash": "3a5b",
            "hasInputSchema": true,
            "hasOutputSchema": true,
            "skeleton": { "cartTotal": 0 }
        })
    );

    let content = &post["requestBody"]["content"]["application/json"];
    assert_eq!(
        content["schema"]["properties"]["context"],
        json!({ "type": "object", "properties": { "cartTotal": { "type": "number" } } }),
        "$schema key stripped, declared schema propagated"
    );
    assert_eq!(content["schema"]["required"], json!(["context"]));
    assert_eq!(
        content["examples"]["example_1"],
        json!({
            "summary": "small cart",
            "value": { "context": { "cartTotal": 10 } },
            "x-source": "Pricing Rule.test"
        })
    );

    let response_schema = &post["responses"]["200"]["content"]["application/json"]["schema"];
    assert_eq!(
        response_schema["properties"]["performance"],
        json!({ "type": "string" })
    );
    assert_eq!(
        response_schema["properties"]["result"],
        json!({ "type": "object" })
    );
    assert_eq!(
        response_schema["required"],
        json!(["performance", "result", "meta"])
    );
    assert_eq!(
        response_schema["properties"]["trace"],
        json!({ "type": "object", "description": "Present when the request body sets trace=true" })
    );
    let meta = &response_schema["properties"]["meta"];
    assert_eq!(meta["properties"]["type"]["enum"], json!(["release"]));
    assert_eq!(
        meta["required"],
        json!(["type", "path", "project", "release"])
    );
    assert!(meta["properties"].get("environment").is_none());
    assert_eq!(meta["properties"]["release"]["required"], json!(["id"]));
}

#[test]
fn environment_config_document() {
    let release = environment_release();
    let entries = entries();
    let document = build_rules_openapi(RulesDocumentSource {
        project_ref: "rules-project",
        release: Some(&release),
        entries: &entries,
    });

    assert_eq!(
        document["x-gorules"]["source"],
        json!({
            "type": "environment",
            "environmentId": "77777777-7777-7777-7777-777777777777",
            "environmentKey": "production",
            "releaseId": "22222222-2222-2222-2222-222222222222"
        })
    );

    let meta = &document["paths"]["/evaluate/plain-rule"]["post"]["responses"]["200"]["content"]["application/json"]
        ["schema"]["properties"]["meta"];
    assert_eq!(meta["properties"]["type"]["enum"], json!(["environment"]));
    assert_eq!(
        meta["required"],
        json!(["type", "path", "project", "release", "environment"])
    );
    assert_eq!(
        meta["properties"]["environment"]["required"],
        json!(["id", "key", "name"])
    );
}

#[test]
fn bare_entry_and_no_release_fallbacks() {
    let entries = entries();
    let document = build_rules_openapi(RulesDocumentSource {
        project_ref: "some-project",
        release: None,
        entries: &entries,
    });

    assert_eq!(document["info"]["version"], json!("1.0.0"));
    assert_eq!(
        document["x-gorules"]["source"],
        json!({ "type": "release", "releaseId": null, "version": null })
    );

    let post = &document["paths"]["/evaluate/plain-rule"]["post"];
    assert_eq!(
        post["summary"],
        json!("plain-rule"),
        "summary falls back to file name"
    );
    assert_eq!(post.get("description"), None);
    assert_eq!(post["x-gorules"]["contentHash"], json!(null));
    assert_eq!(post["x-gorules"]["hasInputSchema"], json!(false));
    assert_eq!(post["x-gorules"]["hasOutputSchema"], json!(false));
    assert_eq!(
        post["requestBody"]["content"]["application/json"]["schema"]["properties"]["context"],
        json!({ "type": "object" }),
        "generic object fallback"
    );
    assert_eq!(
        post["requestBody"]["content"]["application/json"].get("examples"),
        None,
        "no examples key when none exist"
    );
    assert_eq!(
        post["responses"]["200"]["content"]["application/json"]["schema"]["properties"]["result"],
        json!({ "type": "object" }),
        "response schema falls back to generic object"
    );
    let response_schema = &post["responses"]["200"]["content"]["application/json"]["schema"];
    assert_eq!(
        response_schema.get("required"),
        None,
        "no required without meta"
    );
    assert!(response_schema["properties"].get("meta").is_none());
    assert_eq!(
        response_schema["properties"]["trace"]["type"],
        json!("object")
    );
}

#[test]
fn operation_id_sanitization() {
    assert_eq!(
        operation_id("first level/nested-sample"),
        "evaluate_first_level_nested_sample"
    );
    assert_eq!(operation_id("__weird__ path!!"), "evaluate_weird_path");
}
