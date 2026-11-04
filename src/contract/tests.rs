use super::*;

fn violation(field: &str, message: &str) -> Option<Violation> {
    Some(Violation {
        field: field.into(),
        message: message.into(),
    })
}

#[test]
fn names_the_field_from_the_validators_pointer() {
    assert_eq!(
        parse("/applicant/age: 12 is less than the minimum of 18"),
        violation("applicant.age", "12 is less than the minimum of 18")
    );
    assert_eq!(
        parse(r#"/applicant/age: "x" is not of type "integer""#),
        violation("applicant.age", r#""x" is not of type "integer""#)
    );
}

#[test]
fn a_missing_field_is_named_rather_than_its_parent() {
    assert_eq!(
        parse(r#": "applicant" is a required property"#),
        violation("applicant", r#""applicant" is a required property"#)
    );
    assert_eq!(
        parse(r#"/applicant: "age" is a required property"#),
        violation("applicant.age", r#""age" is a required property"#)
    );
}

#[test]
fn other_errors_are_not_violations() {
    assert_eq!(parse("Failed to evaluate expression: \"a +\""), None);
    assert_eq!(parse("boom"), None);
}
