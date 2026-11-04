use super::*;
use std::cell::RefCell;
use std::rc::Rc;

fn object(fields: &[(&str, VariableType)]) -> VariableType {
    let map = fields
        .iter()
        .map(|(key, value)| (Rc::from(*key), value.clone()))
        .collect();
    VariableType::Object(Rc::new(RefCell::new(map)))
}

#[test]
fn scalar_typescript_rendering() {
    assert_eq!(to_typescript(&VariableType::Bool), "boolean");
    assert_eq!(to_typescript(&VariableType::Date), "Date");
    assert_eq!(to_typescript(&VariableType::Interval), "string");
    assert_eq!(to_typescript(&VariableType::Const(Rc::from("a"))), "\"a\"");
    assert_eq!(
        to_typescript(&VariableType::Enum(
            None,
            vec![Rc::from("a"), Rc::from("b")]
        )),
        "\"a\" | \"b\""
    );
    assert_eq!(
        to_typescript(&VariableType::Nullable(Rc::new(VariableType::Number))),
        "(number | null | undefined)"
    );
}

#[test]
fn object_typescript_is_key_sorted_and_quoted() {
    let rendered = to_typescript(&object(&[
        ("b", VariableType::Number),
        ("a", VariableType::String),
        ("needs quoting", VariableType::Bool),
    ]));

    assert_eq!(
        rendered,
        "{ \"a\": string; \"b\": number; \"needs quoting\": boolean; }"
    );
}

#[test]
fn nested_array_typescript() {
    let rendered = to_typescript(&VariableType::Array(Rc::new(object(&[(
        "id",
        VariableType::Number,
    )]))));

    assert_eq!(rendered, "Array<{ \"id\": number; }>");
}

#[test]
fn dotted_paths_nest_into_objects() {
    let cart = VariableType::Number;
    let name = VariableType::String;
    let schema = from_properties(
        [("customer.name", &name), ("cart.total", &cart)],
        "Pricing Rule",
    );

    assert_eq!(schema["$id"], json!("Pricing Rule"));
    assert_eq!(schema["required"], json!(["cart", "customer"]));
    assert_eq!(
        schema["properties"]["customer"]["properties"]["name"],
        json!({ "type": "string" })
    );
    assert_eq!(
        schema["properties"]["cart"]["properties"]["total"],
        json!({ "type": "number" })
    );
    assert_eq!(
        schema["properties"]["customer"]["additionalProperties"],
        json!(false)
    );
}

#[test]
fn nullable_top_level_properties_are_not_required() {
    let optional = VariableType::Nullable(Rc::new(VariableType::String));
    let required = VariableType::Number;
    let schema = from_properties([("note", &optional), ("amount", &required)], "r");

    assert_eq!(schema["required"], json!(["amount"]));
    assert_eq!(
        schema["properties"]["note"],
        json!({ "anyOf": [{ "type": "string" }, { "type": "null" }] })
    );
}

#[test]
fn object_leaf_marks_non_nullable_fields_required() {
    let value = object(&[
        ("id", VariableType::Number),
        (
            "note",
            VariableType::Nullable(Rc::new(VariableType::String)),
        ),
    ]);
    let schema = from_properties([("customer", &value)], "r");

    let customer = &schema["properties"]["customer"];
    assert_eq!(customer["required"], json!(["id"]));
    assert_eq!(customer["properties"]["id"], json!({ "type": "number" }));
}

#[test]
fn any_becomes_an_open_schema() {
    let any = VariableType::Any;
    let schema = from_properties([("passthrough", &any)], "r");

    assert_eq!(schema["properties"]["passthrough"], json!({}));
}
