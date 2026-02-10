use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single evaluation example with input and optional expected output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    /// Unique identifier for this example.
    pub id: String,
    /// Input to the system under test.
    pub input: Value,
    /// Expected/reference output (for comparison evaluators).
    #[serde(default)]
    pub expected: Option<Value>,
    /// Additional metadata (tags, category, difficulty, etc.)
    #[serde(default)]
    pub metadata: std::collections::HashMap<String, Value>,
}

/// A collection of examples for evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dataset {
    /// Dataset name.
    pub name: String,
    /// Description of what this dataset tests.
    #[serde(default)]
    pub description: String,
    /// The examples.
    pub examples: Vec<Example>,
}

impl Dataset {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            examples: Vec::new(),
        }
    }

    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = desc.into();
        self
    }

    pub fn add_example(&mut self, example: Example) -> &mut Self {
        self.examples.push(example);
        self
    }

    pub fn len(&self) -> usize {
        self.examples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.examples.is_empty()
    }

    /// Load from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_example(id: &str) -> Example {
        Example {
            id: id.into(),
            input: json!({"question": "What is 2+2?"}),
            expected: Some(json!("4")),
            metadata: Default::default(),
        }
    }

    #[test]
    fn dataset_creation() {
        let ds = Dataset::new("test-dataset").with_description("A test dataset");
        assert_eq!(ds.name, "test-dataset");
        assert_eq!(ds.description, "A test dataset");
        assert!(ds.is_empty());
        assert_eq!(ds.len(), 0);
    }

    #[test]
    fn add_example() {
        let mut ds = Dataset::new("test");
        ds.add_example(sample_example("ex1"));
        ds.add_example(sample_example("ex2"));
        assert_eq!(ds.len(), 2);
        assert!(!ds.is_empty());
        assert_eq!(ds.examples[0].id, "ex1");
        assert_eq!(ds.examples[1].id, "ex2");
    }

    #[test]
    fn serde_roundtrip() {
        let mut ds = Dataset::new("roundtrip").with_description("test roundtrip");
        ds.add_example(Example {
            id: "ex1".into(),
            input: json!({"q": "hello"}),
            expected: Some(json!("world")),
            metadata: [("tag".into(), json!("easy"))].into_iter().collect(),
        });

        let json_str = ds.to_json().unwrap();
        let ds2 = Dataset::from_json(&json_str).unwrap();
        assert_eq!(ds2.name, ds.name);
        assert_eq!(ds2.description, ds.description);
        assert_eq!(ds2.len(), ds.len());
        assert_eq!(ds2.examples[0].id, "ex1");
        assert_eq!(ds2.examples[0].expected, Some(json!("world")));
        assert_eq!(ds2.examples[0].metadata["tag"], json!("easy"));
    }

    #[test]
    fn from_json() {
        let json_str = r#"{
            "name": "from-json",
            "description": "loaded from json",
            "examples": [
                {
                    "id": "1",
                    "input": "test input",
                    "expected": "test output"
                }
            ]
        }"#;
        let ds = Dataset::from_json(json_str).unwrap();
        assert_eq!(ds.name, "from-json");
        assert_eq!(ds.len(), 1);
        assert_eq!(ds.examples[0].id, "1");
    }

    #[test]
    fn empty_dataset() {
        let ds = Dataset::new("empty");
        assert!(ds.is_empty());
        assert_eq!(ds.len(), 0);

        let json_str = ds.to_json().unwrap();
        let ds2 = Dataset::from_json(&json_str).unwrap();
        assert!(ds2.is_empty());
    }
}
