use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use ayas_llm::provider::Provider;

use crate::error::AppError;

/// API keys extracted from request headers.
#[derive(Debug, Clone, Default)]
pub struct ApiKeys {
    pub gemini_key: Option<String>,
    pub anthropic_key: Option<String>,
    pub openai_key: Option<String>,
}

impl ApiKeys {
    pub fn get_key_for(&self, provider: &Provider) -> Result<String, AppError> {
        let key = match provider {
            Provider::Gemini => &self.gemini_key,
            Provider::Claude => &self.anthropic_key,
            Provider::OpenAI => &self.openai_key,
        };
        key.clone()
            .ok_or_else(|| AppError::MissingApiKey(format!("{:?}", provider)))
    }
}

impl<S> FromRequestParts<S> for ApiKeys
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let gemini_key = parts
            .headers
            .get("X-Gemini-Key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| std::env::var("GEMINI_API_KEY").ok());
        let anthropic_key = parts
            .headers
            .get("X-Anthropic-Key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok());
        let openai_key = parts
            .headers
            .get("X-OpenAI-Key")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok());

        Ok(ApiKeys {
            gemini_key,
            anthropic_key,
            openai_key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_all_keys() {
        let keys = ApiKeys {
            gemini_key: Some("gk".into()),
            anthropic_key: Some("ak".into()),
            openai_key: Some("ok".into()),
        };
        assert_eq!(keys.gemini_key.as_deref(), Some("gk"));
        assert_eq!(keys.anthropic_key.as_deref(), Some("ak"));
        assert_eq!(keys.openai_key.as_deref(), Some("ok"));
    }

    #[test]
    fn extract_partial_keys() {
        let keys = ApiKeys {
            gemini_key: Some("gk".into()),
            anthropic_key: None,
            openai_key: None,
        };
        assert!(keys.gemini_key.is_some());
        assert!(keys.anthropic_key.is_none());
        assert!(keys.openai_key.is_none());
    }

    #[test]
    fn extract_no_keys() {
        let keys = ApiKeys::default();
        assert!(keys.gemini_key.is_none());
        assert!(keys.anthropic_key.is_none());
        assert!(keys.openai_key.is_none());
    }

    #[test]
    fn get_key_for_gemini() {
        let keys = ApiKeys {
            gemini_key: Some("gk".into()),
            ..Default::default()
        };
        assert_eq!(keys.get_key_for(&Provider::Gemini).unwrap(), "gk");
    }

    #[test]
    fn get_key_missing_returns_err() {
        let keys = ApiKeys::default();
        let result = keys.get_key_for(&Provider::Gemini);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn env_var_fallback_when_no_header() {
        // SAFETY: test runs single-threaded (--test-threads=1)
        unsafe {
            std::env::set_var("GEMINI_API_KEY", "env-gemini");
            std::env::set_var("ANTHROPIC_API_KEY", "env-anthropic");
            std::env::set_var("OPENAI_API_KEY", "env-openai");
        }

        let req = axum::http::Request::builder()
            .body(())
            .unwrap();
        let (mut parts, _body) = req.into_parts();

        let keys = ApiKeys::from_request_parts(&mut parts, &())
            .await
            .unwrap();

        assert_eq!(keys.gemini_key.as_deref(), Some("env-gemini"));
        assert_eq!(keys.anthropic_key.as_deref(), Some("env-anthropic"));
        assert_eq!(keys.openai_key.as_deref(), Some("env-openai"));

        unsafe {
            std::env::remove_var("GEMINI_API_KEY");
            std::env::remove_var("ANTHROPIC_API_KEY");
            std::env::remove_var("OPENAI_API_KEY");
        }
    }

    #[tokio::test]
    async fn header_takes_priority_over_env_var() {
        unsafe { std::env::set_var("GEMINI_API_KEY", "env-gemini"); }

        let req = axum::http::Request::builder()
            .header("X-Gemini-Key", "header-gemini")
            .body(())
            .unwrap();
        let (mut parts, _body) = req.into_parts();

        let keys = ApiKeys::from_request_parts(&mut parts, &())
            .await
            .unwrap();

        assert_eq!(keys.gemini_key.as_deref(), Some("header-gemini"));

        unsafe { std::env::remove_var("GEMINI_API_KEY"); }
    }
}
