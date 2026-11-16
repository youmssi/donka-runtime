use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use zen_engine::model::DecisionContent;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDecisionGraph {
    #[serde(default)]
    pub meta: DecisionContentMeta,

    #[serde(default)]
    pub title: Option<Arc<str>>,

    #[serde(default)]
    pub description: Option<Arc<str>>,

    #[serde(flatten)]
    pub content: Arc<DecisionContent>,
}

#[derive(Default, Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionContentMeta {
    pub version_id: Option<Arc<str>>,

    /// Original-cased path from the source (zip entry / relative file path);
    /// set by the loader after parsing, never read from the file itself.
    #[serde(skip)]
    pub display_path: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTestContent {
    pub file_path: Option<Arc<str>>,

    #[serde(default)]
    pub disabled: bool,

    #[serde(default)]
    pub test_cases: Vec<FileTestCase>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTestCase {
    #[serde(default)]
    pub name: Option<Arc<str>>,

    #[serde(default)]
    pub input: Value,

    #[serde(default)]
    pub disabled: bool,
}

/// New file types need to be also added below in TaggedFileContent
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "contentType")]
pub enum FileContent {
    Graph(FileDecisionGraph),
    Test(FileTestContent),
    Unknown,
}

impl<'de> Deserialize<'de> for FileContent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// As serde doesn't support tagged union with default - we need to duplicate the content here
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", tag = "contentType")]
        enum TaggedFileContent {
            Graph(FileDecisionGraph),
            Test(FileTestContent),
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Either {
            Tagged(TaggedFileContent),
            Untagged(FileDecisionGraph),
        }

        match Either::deserialize(deserializer) {
            Ok(file) => match file {
                Either::Tagged(TaggedFileContent::Graph(g)) => Ok(FileContent::Graph(g)),
                Either::Tagged(TaggedFileContent::Test(t)) => Ok(FileContent::Test(t)),
                Either::Untagged(g) => Ok(FileContent::Graph(g)),
            },
            Err(_) => Ok(FileContent::Unknown),
        }
    }
}

#[cfg(test)]
#[path = "extended_decision/tests.rs"]
mod tests;
