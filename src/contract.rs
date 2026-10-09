//! Input contracts (DNK-37). A decision's input node may carry a JSON Schema, written by Donka
//! Studio; the engine validates every request against it and stops at the input node when the
//! request breaks it. This module names the field at fault, so a caller reads
//! `{ "field": "applicant.age", "message": "12 is less than the minimum of 18" }` instead of
//! parsing the engine's error text.

use crate::engine_ext::EngineExtension;
use serde::Serialize;
use zen_engine::model::DecisionNodeKind;
use zen_engine::{DecisionEngine, EvaluationError};

/// One way a request breaks the decision's input contract.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Violation {
    /// The field, as a dotted path (`applicant.age`); empty for the request as a whole.
    pub field: String,
    pub message: String,
}

/// The contract violation behind `error`, when the request was stopped by the input node of
/// decision `key`.
pub fn violation(engine: &DecisionEngine, key: &str, error: &EvaluationError) -> Option<Violation> {
    let EvaluationError::NodeError {
        node_id, source, ..
    } = error
    else {
        return None;
    };
    let documents = engine.documents();
    let (_, content) = documents.iter().find(|(path, _)| {
        // Folders of decision files name them with their extension.
        let path = path.strip_suffix(".json").unwrap_or(path);
        path.eq_ignore_ascii_case(key.strip_suffix(".json").unwrap_or(key))
    })?;
    let is_input =
        content.as_graph()?.nodes.iter().any(|node| {
            node.id == *node_id && matches!(node.kind, DecisionNodeKind::InputNode { .. })
        });
    if !is_input {
        return None;
    }
    parse(&source.to_string())
}

/// Reads the validator's `<JSON pointer>: <message>` (`/applicant/age: 12 is less than…`,
/// `: "applicant" is a required property`).
fn parse(text: &str) -> Option<Violation> {
    let (pointer, message) = text.split_once(": ")?;
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return None;
    }
    let mut field: Vec<String> = pointer
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| part.replace("~1", "/").replace("~0", "~"))
        .collect();
    // A missing property is reported on its parent: name the property itself.
    if let Some(name) = message
        .strip_suffix(" is a required property")
        .and_then(|quoted| quoted.strip_prefix('"')?.strip_suffix('"'))
    {
        field.push(name.to_owned());
    }
    Some(Violation {
        field: field.join("."),
        message: message.to_owned(),
    })
}

#[cfg(test)]
mod tests {
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
}
