use crate::rules_spec::SpecEntry;
use crate::schema;
use crate::tsgo;
use std::sync::Arc;
use zen_engine::model::DecisionContent;
use zen_engine::policy::{ScopeRequest, Workspace};

/// Fills in the schemas a rule does not declare, by analysing the release as
/// one workspace. Input/output properties come from the engine; the TypeScript
/// return types of function nodes come from tsgo, which is what lets a graph
/// whose shape is only decided inside a function still publish an output
/// schema.
///
/// Declared schemas win — an author who wrote one on an input node meant it.
/// Every step degrades to leaving the entry as it was, so a rule the engine
/// cannot analyse still appears in the document with its declared shape.
pub fn enrich(entries: &mut [SpecEntry], documents: Vec<(Arc<str>, Arc<DecisionContent>)>) {
    if entries.is_empty() {
        return;
    }

    let mut workspace = Workspace::new();
    for (path, content) in documents {
        workspace.set_document_arc(path, content);
    }

    if let Some(analyzer) = tsgo::analyzer() {
        workspace.set_function_resolver(move |source, input| {
            analyzer
                .resolve(source, &schema::to_typescript(input))
                .map(|resolved| resolved.to_string())
        });
    }

    for entry in entries {
        let request = ScopeRequest::for_policy(entry.path.clone());
        let signature = workspace
            .graph_analysis(&entry.path)
            .map(|analysis| analysis.signature.clone());

        if entry.input_schema.is_none() {
            let inputs = workspace.inputs(&request);
            entry.input_schema = if inputs.is_empty() {
                signature
                    .as_ref()
                    .and_then(|signature| schema::from_type(&signature.input, &entry.path))
            } else {
                Some(schema::from_properties(
                    inputs
                        .iter()
                        .map(|input| (input.path.as_ref(), &input.resolved_type)),
                    &entry.path,
                ))
            }
            .map(Arc::new);
        }

        if entry.output_schema.is_none() {
            // Graphs have no per-property output list; their whole output type
            // is the graph signature, which is where a function node's tsgo
            // resolved type lands.
            let outputs = workspace.outputs(&request);
            entry.output_schema = if outputs.is_empty() {
                signature
                    .as_ref()
                    .and_then(|signature| schema::from_type(&signature.output, &entry.path))
            } else {
                Some(schema::from_properties(
                    outputs
                        .iter()
                        .map(|output| (output.path.as_ref(), &output.resolved_type)),
                    &entry.path,
                ))
            }
            .map(Arc::new);
        }

        // An unknown path still skeletons to `{}`, which says nothing.
        let skeleton = workspace.input_skeleton(&request);
        if skeleton.as_object().is_some_and(|map| !map.is_empty()) {
            entry.skeleton = Some(Arc::new(skeleton));
        }
    }
}

#[cfg(test)]
#[path = "spec_derive/tests.rs"]
mod tests;
