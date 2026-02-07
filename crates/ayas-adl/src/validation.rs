use std::collections::HashSet;

use crate::error::AdlError;
use crate::registry::ComponentRegistry;
use crate::types::{AdlDocument, AdlEdgeType, normalize_sentinel};

/// Validate an ADL document against the given registry.
pub fn validate_document(doc: &AdlDocument, registry: &ComponentRegistry) -> Result<(), AdlError> {
    validate_version(doc)?;
    validate_node_ids(doc)?;
    validate_node_types(doc, registry)?;
    validate_edges(doc)?;
    validate_start_edge(doc)?;
    validate_conditional_edges(doc)?;
    Ok(())
}

fn validate_version(doc: &AdlDocument) -> Result<(), AdlError> {
    if doc.version != "1.0" {
        return Err(AdlError::Validation(format!(
            "Unsupported ADL version '{}', expected '1.0'",
            doc.version
        )));
    }
    Ok(())
}

fn validate_node_ids(doc: &AdlDocument) -> Result<(), AdlError> {
    let mut seen = HashSet::new();
    for node in &doc.nodes {
        // Check reserved names
        let normalized = normalize_sentinel(&node.id);
        if normalized == "__start__" || normalized == "__end__" {
            return Err(AdlError::Validation(format!(
                "Node ID '{}' is a reserved sentinel name",
                node.id
            )));
        }
        // Check duplicates
        if !seen.insert(&node.id) {
            return Err(AdlError::Validation(format!(
                "Duplicate node ID '{}'",
                node.id
            )));
        }
    }
    Ok(())
}

fn validate_node_types(doc: &AdlDocument, registry: &ComponentRegistry) -> Result<(), AdlError> {
    for node in &doc.nodes {
        if !registry.has_type(&node.node_type) {
            return Err(AdlError::UnknownNodeType {
                node_type: node.node_type.clone(),
            });
        }
    }
    Ok(())
}

fn validate_edges(doc: &AdlDocument) -> Result<(), AdlError> {
    let node_ids: HashSet<&str> = doc.nodes.iter().map(|n| n.id.as_str()).collect();

    for edge in &doc.edges {
        let from = normalize_sentinel(&edge.from);
        // Validate 'from' reference
        if from != "__start__" && from != "__end__" && !node_ids.contains(from.as_str()) {
            return Err(AdlError::Validation(format!(
                "Edge references unknown source node '{}'",
                edge.from
            )));
        }

        // Validate 'to' reference for static edges
        if edge.edge_type == AdlEdgeType::Static {
            let to_str = edge.to.as_deref().unwrap_or("");
            let to = normalize_sentinel(to_str);
            if to != "__start__" && to != "__end__" && !node_ids.contains(to.as_str()) {
                return Err(AdlError::Validation(format!(
                    "Edge references unknown target node '{to_str}'"
                )));
            }
        }

        // Validate condition targets
        for cond in &edge.conditions {
            let cond_to = normalize_sentinel(&cond.to);
            if cond_to != "__start__"
                && cond_to != "__end__"
                && !node_ids.contains(cond_to.as_str())
            {
                return Err(AdlError::Validation(format!(
                    "Condition references unknown target node '{}'",
                    cond.to
                )));
            }
        }
    }
    Ok(())
}

fn validate_start_edge(doc: &AdlDocument) -> Result<(), AdlError> {
    let has_start = doc.edges.iter().any(|e| {
        let from = normalize_sentinel(&e.from);
        from == "__start__"
    });
    if !has_start {
        return Err(AdlError::Validation(
            "No edge from __start__ found; the graph needs an entry point".to_string(),
        ));
    }
    Ok(())
}

fn validate_conditional_edges(doc: &AdlDocument) -> Result<(), AdlError> {
    for edge in &doc.edges {
        if edge.edge_type == AdlEdgeType::Conditional && edge.conditions.is_empty() {
            return Err(AdlError::Validation(format!(
                "Conditional edge from '{}' has no conditions",
                edge.from
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AdlDocument;

    fn registry() -> ComponentRegistry {
        ComponentRegistry::with_builtins()
    }

    fn parse(yaml: &str) -> AdlDocument {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn valid_document_passes() {
        let doc = parse(
            r#"
version: "1.0"
nodes:
  - id: a
    type: passthrough
edges:
  - from: __start__
    to: a
  - from: a
    to: __end__
"#,
        );
        assert!(validate_document(&doc, &registry()).is_ok());
    }

    #[test]
    fn invalid_version_fails() {
        let doc = parse(
            r#"
version: "2.0"
nodes: []
edges:
  - from: __start__
    to: __end__
"#,
        );
        let err = validate_document(&doc, &registry()).unwrap_err();
        assert!(err.to_string().contains("Unsupported ADL version"));
    }

    #[test]
    fn duplicate_node_id_fails() {
        let doc = parse(
            r#"
version: "1.0"
nodes:
  - id: a
    type: passthrough
  - id: a
    type: passthrough
edges:
  - from: __start__
    to: a
"#,
        );
        let err = validate_document(&doc, &registry()).unwrap_err();
        assert!(err.to_string().contains("Duplicate node ID"));
    }

    #[test]
    fn reserved_node_id_fails() {
        let doc = parse(
            r#"
version: "1.0"
nodes:
  - id: __start__
    type: passthrough
edges:
  - from: __start__
    to: __start__
"#,
        );
        let err = validate_document(&doc, &registry()).unwrap_err();
        assert!(err.to_string().contains("reserved sentinel"));
    }

    #[test]
    fn unknown_node_type_fails() {
        let doc = parse(
            r#"
version: "1.0"
nodes:
  - id: a
    type: nonexistent_type
edges:
  - from: __start__
    to: a
"#,
        );
        let err = validate_document(&doc, &registry()).unwrap_err();
        assert!(err.to_string().contains("Unknown node type"));
    }

    #[test]
    fn edge_unknown_source_fails() {
        let doc = parse(
            r#"
version: "1.0"
nodes:
  - id: a
    type: passthrough
edges:
  - from: __start__
    to: a
  - from: nonexistent
    to: a
"#,
        );
        let err = validate_document(&doc, &registry()).unwrap_err();
        assert!(err.to_string().contains("unknown source node"));
    }

    #[test]
    fn edge_unknown_target_fails() {
        let doc = parse(
            r#"
version: "1.0"
nodes:
  - id: a
    type: passthrough
edges:
  - from: __start__
    to: nonexistent
"#,
        );
        let err = validate_document(&doc, &registry()).unwrap_err();
        assert!(err.to_string().contains("unknown target node"));
    }

    #[test]
    fn missing_start_edge_fails() {
        let doc = parse(
            r#"
version: "1.0"
nodes:
  - id: a
    type: passthrough
edges:
  - from: a
    to: __end__
"#,
        );
        let err = validate_document(&doc, &registry()).unwrap_err();
        assert!(err.to_string().contains("No edge from __start__"));
    }

    #[test]
    fn conditional_edge_without_conditions_fails() {
        let doc = parse(
            r#"
version: "1.0"
nodes:
  - id: a
    type: passthrough
edges:
  - from: __start__
    to: a
  - from: a
    type: conditional
"#,
        );
        let err = validate_document(&doc, &registry()).unwrap_err();
        assert!(err.to_string().contains("no conditions"));
    }
}
