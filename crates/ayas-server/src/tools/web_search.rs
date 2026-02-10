use async_trait::async_trait;
use ayas_core::error::{AyasError, Result};
use ayas_core::tool::{Tool, ToolDefinition};
use serde::{Deserialize, Serialize};

/// Web search tool using Tavily API.
pub struct WebSearchTool {
    api_key: Option<String>,
    client: reqwest::Client,
}

impl WebSearchTool {
    pub fn new() -> Self {
        let api_key = std::env::var("TAVILY_API_KEY").ok();
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Serialize)]
struct TavilyRequest {
    api_key: String,
    query: String,
    search_depth: String,
    max_results: u32,
}

#[derive(Deserialize)]
struct TavilyResponse {
    results: Vec<TavilyResult>,
}

#[derive(Deserialize)]
struct TavilyResult {
    title: String,
    url: String,
    content: String,
    #[allow(dead_code)]
    score: f64,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_search".into(),
            description: "Search the web for current information. Returns relevant web pages with titles, URLs, and content snippets.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 5)",
                        "default": 5
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call(&self, input: serde_json::Value) -> Result<String> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AyasError::Other("Missing 'query' parameter".into()))?;

        let max_results = input
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as u32;

        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| {
                AyasError::Other(
                    "TAVILY_API_KEY environment variable not set. Web search is unavailable."
                        .into(),
                )
            })?;

        let tavily_req = TavilyRequest {
            api_key: api_key.clone(),
            query: query.to_string(),
            search_depth: "basic".into(),
            max_results,
        };

        let response = self
            .client
            .post("https://api.tavily.com/search")
            .json(&tavily_req)
            .send()
            .await
            .map_err(|e| AyasError::Other(format!("Web search request failed: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AyasError::Other(format!(
                "Tavily API error ({status}): {body}"
            )));
        }

        let tavily_resp: TavilyResponse = response
            .json()
            .await
            .map_err(|e| AyasError::Other(format!("Failed to parse search response: {e}")))?;

        // Format results as readable text
        let mut output = String::new();
        for (i, result) in tavily_resp.results.iter().enumerate() {
            output.push_str(&format!(
                "{}. {}\n   URL: {}\n   {}\n\n",
                i + 1,
                result.title,
                result.url,
                result.content
            ));
        }

        if output.is_empty() {
            output = "No results found.".into();
        }

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_correct_schema() {
        let tool = WebSearchTool::new();
        let def = tool.definition();
        assert_eq!(def.name, "web_search");
        assert!(def.description.contains("Search the web"));
    }

    #[tokio::test]
    async fn call_without_api_key_returns_error() {
        let tool = WebSearchTool {
            api_key: None,
            client: reqwest::Client::new(),
        };
        let result = tool.call(serde_json::json!({"query": "test"})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("TAVILY_API_KEY"));
    }

    #[tokio::test]
    async fn call_missing_query_returns_error() {
        let tool = WebSearchTool {
            api_key: Some("fake-key".into()),
            client: reqwest::Client::new(),
        };
        let result = tool.call(serde_json::json!({"not_query": "test"})).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("query"));
    }

    #[tokio::test]
    async fn call_with_invalid_input_returns_error() {
        let tool = WebSearchTool {
            api_key: Some("fake-key".into()),
            client: reqwest::Client::new(),
        };
        let result = tool.call(serde_json::json!("not an object")).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("query"));
    }
}
