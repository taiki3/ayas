use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use tracing::{info, warn};

use ayas_core::error::{AyasError, ModelError, Result};

use crate::types::{FileSearchStore, Operation, UploadFileResponse, UploadedFile};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
const DEFAULT_UPLOAD_BASE_URL: &str = "https://generativelanguage.googleapis.com/upload/v1beta";

/// Client trait for the Gemini File Search Store API.
#[async_trait]
pub trait FileSearchClient: Send + Sync {
    /// Files API: upload a file (resumable protocol).
    async fn upload_file(
        &self,
        display_name: &str,
        mime_type: &str,
        content: &[u8],
    ) -> Result<UploadedFile>;

    /// FileSearchStores: create a new store.
    async fn create_store(&self, display_name: &str) -> Result<FileSearchStore>;

    /// FileSearchStores: import a file into a store.
    async fn import_file(&self, store_name: &str, file_name: &str) -> Result<Operation>;

    /// FileSearchStores: get store metadata.
    async fn get_store(&self, store_name: &str) -> Result<FileSearchStore>;

    /// FileSearchStores: delete a store.
    async fn delete_store(&self, store_name: &str) -> Result<()>;

    /// Operations: get operation status.
    async fn get_operation(&self, operation_name: &str) -> Result<Operation>;

    /// Wait for store to be ready (all documents indexed).
    async fn wait_for_store_ready(
        &self,
        store_name: &str,
        poll_interval: Duration,
    ) -> Result<FileSearchStore> {
        loop {
            let store = self.get_store(store_name).await?;
            let pending = store
                .pending_documents_count
                .as_deref()
                .unwrap_or("0")
                .parse::<u64>()
                .unwrap_or(0);

            if pending == 0 {
                info!(store = %store_name, "Store ready");
                return Ok(store);
            }

            info!(store = %store_name, pending, "Store not ready, polling...");
            tokio::time::sleep(poll_interval).await;
        }
    }
}

/// Gemini File Search Store API client.
pub struct GeminiFileSearchClient {
    api_key: String,
    base_url: String,
    upload_base_url: String,
    client: reqwest::Client,
}

impl GeminiFileSearchClient {
    /// Create a client with default API URLs.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.into(),
            upload_base_url: DEFAULT_UPLOAD_BASE_URL.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Create a client with custom base URLs (for testing).
    pub fn with_base_urls(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        upload_base_url: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into(),
            upload_base_url: upload_base_url.into(),
            client: reqwest::Client::new(),
        }
    }

    fn files_url(&self) -> String {
        format!("{}/files?key={}", self.upload_base_url, self.api_key)
    }

    fn stores_url(&self) -> String {
        format!("{}/fileSearchStores?key={}", self.base_url, self.api_key)
    }

    fn store_url(&self, store_name: &str) -> String {
        format!("{}/{}?key={}", self.base_url, store_name, self.api_key)
    }

    fn import_url(&self, store_name: &str) -> String {
        format!(
            "{}/{}:importFile?key={}",
            self.base_url, store_name, self.api_key
        )
    }

    fn operation_url(&self, operation_name: &str) -> String {
        format!(
            "{}/{}?key={}",
            self.base_url, operation_name, self.api_key
        )
    }

    fn map_status_error(status: StatusCode, body: String) -> AyasError {
        match status.as_u16() {
            401 | 403 => AyasError::Model(ModelError::Auth(body)),
            429 => AyasError::Model(ModelError::RateLimited {
                retry_after_secs: None,
            }),
            _ => AyasError::Model(ModelError::ApiRequest(format!(
                "HTTP {}: {}",
                status, body
            ))),
        }
    }
}

#[async_trait]
impl FileSearchClient for GeminiFileSearchClient {
    async fn upload_file(
        &self,
        display_name: &str,
        mime_type: &str,
        content: &[u8],
    ) -> Result<UploadedFile> {
        info!(display_name, mime_type, size = content.len(), "Uploading file (resumable)");

        // Step 1: Initiate resumable upload
        let metadata = serde_json::json!({
            "file": {
                "displayName": display_name,
                "mimeType": mime_type,
            }
        });

        let init_response = self
            .client
            .post(self.files_url())
            .header("X-Goog-Upload-Protocol", "resumable")
            .header("X-Goog-Upload-Command", "start")
            .header("X-Goog-Upload-Header-Content-Length", content.len())
            .header("X-Goog-Upload-Header-Content-Type", mime_type)
            .header("Content-Type", "application/json")
            .body(metadata.to_string())
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = init_response.status();
        if !status.is_success() {
            let body = init_response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            return Err(Self::map_status_error(status, body));
        }

        let upload_url = init_response
            .headers()
            .get("x-goog-upload-url")
            .ok_or_else(|| {
                AyasError::Model(ModelError::InvalidResponse(
                    "Missing X-Goog-Upload-URL header".into(),
                ))
            })?
            .to_str()
            .map_err(|e| {
                AyasError::Model(ModelError::InvalidResponse(format!(
                    "Invalid upload URL header: {e}"
                )))
            })?
            .to_string();

        info!("Resumable upload URL obtained, uploading bytes");

        // Step 2: Upload file content
        let upload_response = self
            .client
            .post(&upload_url)
            .header("X-Goog-Upload-Command", "upload, finalize")
            .header("X-Goog-Upload-Offset", "0")
            .header("Content-Type", mime_type)
            .body(content.to_vec())
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = upload_response.status();
        if !status.is_success() {
            let body = upload_response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            return Err(Self::map_status_error(status, body));
        }

        let resp: UploadFileResponse = upload_response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))?;

        info!(name = %resp.file.name, "File uploaded successfully");
        Ok(resp.file)
    }

    async fn create_store(&self, display_name: &str) -> Result<FileSearchStore> {
        info!(display_name, "Creating file search store");

        let body = serde_json::json!({
            "displayName": display_name,
        });

        let response = self
            .client
            .post(self.stores_url())
            .json(&body)
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            return Err(Self::map_status_error(status, body));
        }

        let store: FileSearchStore = response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))?;

        info!(name = %store.name, "Store created");
        Ok(store)
    }

    async fn import_file(&self, store_name: &str, file_name: &str) -> Result<Operation> {
        info!(store = %store_name, file = %file_name, "Importing file to store");

        let body = serde_json::json!({
            "fileName": file_name,
        });

        let response = self
            .client
            .post(self.import_url(store_name))
            .json(&body)
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            return Err(Self::map_status_error(status, body));
        }

        let op: Operation = response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))?;

        info!(op_name = %op.name, done = op.done, "Import operation started");
        Ok(op)
    }

    async fn get_store(&self, store_name: &str) -> Result<FileSearchStore> {
        let response = self
            .client
            .get(self.store_url(store_name))
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            return Err(Self::map_status_error(status, body));
        }

        response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))
    }

    async fn delete_store(&self, store_name: &str) -> Result<()> {
        info!(store = %store_name, "Deleting file search store");

        let response = self
            .client
            .delete(self.store_url(store_name))
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            warn!(store = %store_name, %status, "Delete store failed");
            return Err(Self::map_status_error(status, body));
        }

        info!(store = %store_name, "Store deleted");
        Ok(())
    }

    async fn get_operation(&self, operation_name: &str) -> Result<Operation> {
        let response = self
            .client
            .get(self.operation_url(operation_name))
            .send()
            .await
            .map_err(|e| AyasError::Model(ModelError::ApiRequest(e.to_string())))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "failed to read response body".into());
            return Err(Self::map_status_error(status, body));
        }

        response
            .json()
            .await
            .map_err(|e| AyasError::Model(ModelError::InvalidResponse(e.to_string())))
    }
}

/// Mock implementation for testing.
pub struct MockFileSearchClient {
    store_name: String,
    pending_polls: std::sync::atomic::AtomicU32,
}

impl MockFileSearchClient {
    /// Create a mock that returns the given store name and becomes ready immediately.
    pub fn ready(store_name: impl Into<String>) -> Self {
        Self {
            store_name: store_name.into(),
            pending_polls: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Create a mock that requires `polls` get_store calls before becoming ready.
    pub fn with_pending(store_name: impl Into<String>, polls: u32) -> Self {
        Self {
            store_name: store_name.into(),
            pending_polls: std::sync::atomic::AtomicU32::new(polls),
        }
    }
}

#[async_trait]
impl FileSearchClient for MockFileSearchClient {
    async fn upload_file(
        &self,
        display_name: &str,
        _mime_type: &str,
        _content: &[u8],
    ) -> Result<UploadedFile> {
        Ok(UploadedFile {
            name: format!("files/mock-{}", display_name.replace('.', "-")),
            uri: String::new(),
            display_name: display_name.to_string(),
            mime_type: "text/markdown".into(),
        })
    }

    async fn create_store(&self, display_name: &str) -> Result<FileSearchStore> {
        Ok(FileSearchStore {
            name: self.store_name.clone(),
            display_name: display_name.to_string(),
            active_documents_count: Some("0".into()),
            pending_documents_count: Some("0".into()),
            failed_documents_count: Some("0".into()),
        })
    }

    async fn import_file(&self, _store_name: &str, _file_name: &str) -> Result<Operation> {
        Ok(Operation {
            name: "operations/mock-import".into(),
            done: true,
            error: None,
        })
    }

    async fn get_store(&self, _store_name: &str) -> Result<FileSearchStore> {
        let remaining = self
            .pending_polls
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |v| if v > 0 { Some(v - 1) } else { Some(0) },
            )
            .unwrap_or(0);

        let pending = if remaining > 0 {
            Some(remaining.to_string())
        } else {
            Some("0".into())
        };

        Ok(FileSearchStore {
            name: self.store_name.clone(),
            display_name: "mock-store".into(),
            active_documents_count: Some("2".into()),
            pending_documents_count: pending,
            failed_documents_count: Some("0".into()),
        })
    }

    async fn delete_store(&self, _store_name: &str) -> Result<()> {
        Ok(())
    }

    async fn get_operation(&self, _operation_name: &str) -> Result<Operation> {
        Ok(Operation {
            name: "operations/mock-op".into(),
            done: true,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_file_search_client_urls() {
        let client = GeminiFileSearchClient::new("test-key");
        assert_eq!(
            client.files_url(),
            "https://generativelanguage.googleapis.com/upload/v1beta/files?key=test-key"
        );
        assert_eq!(
            client.stores_url(),
            "https://generativelanguage.googleapis.com/v1beta/fileSearchStores?key=test-key"
        );
        assert_eq!(
            client.store_url("fileSearchStores/abc"),
            "https://generativelanguage.googleapis.com/v1beta/fileSearchStores/abc?key=test-key"
        );
        assert_eq!(
            client.import_url("fileSearchStores/abc"),
            "https://generativelanguage.googleapis.com/v1beta/fileSearchStores/abc:importFile?key=test-key"
        );
        assert_eq!(
            client.operation_url("operations/op1"),
            "https://generativelanguage.googleapis.com/v1beta/operations/op1?key=test-key"
        );
    }

    #[test]
    fn gemini_file_search_client_custom_urls() {
        let client = GeminiFileSearchClient::with_base_urls(
            "key",
            "http://localhost:8080/v1beta",
            "http://localhost:8080/upload/v1beta",
        );
        assert_eq!(
            client.files_url(),
            "http://localhost:8080/upload/v1beta/files?key=key"
        );
        assert_eq!(
            client.stores_url(),
            "http://localhost:8080/v1beta/fileSearchStores?key=key"
        );
    }

    #[test]
    fn file_search_store_serde() {
        let store = FileSearchStore {
            name: "fileSearchStores/abc".into(),
            display_name: "test-store".into(),
            active_documents_count: Some("2".into()),
            pending_documents_count: Some("1".into()),
            failed_documents_count: None,
        };
        let json = serde_json::to_string(&store).unwrap();
        assert!(json.contains("displayName"));
        assert!(json.contains("activeDocumentsCount"));

        let deserialized: FileSearchStore = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "fileSearchStores/abc");
        assert_eq!(deserialized.display_name, "test-store");
        assert_eq!(deserialized.active_documents_count.as_deref(), Some("2"));
    }

    #[test]
    fn uploaded_file_serde() {
        let file = UploadedFile {
            name: "files/abc".into(),
            uri: "https://example.com/files/abc".into(),
            display_name: "test.md".into(),
            mime_type: "text/markdown".into(),
        };
        let json = serde_json::to_string(&file).unwrap();
        let deserialized: UploadedFile = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "files/abc");
        assert_eq!(deserialized.display_name, "test.md");
    }

    #[test]
    fn operation_serde_done() {
        let op = Operation {
            name: "operations/op1".into(),
            done: true,
            error: None,
        };
        let json = serde_json::to_string(&op).unwrap();
        let deserialized: Operation = serde_json::from_str(&json).unwrap();
        assert!(deserialized.done);
        assert!(deserialized.error.is_none());
    }

    #[test]
    fn operation_serde_with_error() {
        let op = Operation {
            name: "operations/op2".into(),
            done: true,
            error: Some(crate::types::OperationError {
                code: 500,
                message: "internal error".into(),
            }),
        };
        let json = serde_json::to_string(&op).unwrap();
        let deserialized: Operation = serde_json::from_str(&json).unwrap();
        assert!(deserialized.done);
        let err = deserialized.error.unwrap();
        assert_eq!(err.code, 500);
        assert_eq!(err.message, "internal error");
    }

    #[tokio::test]
    async fn mock_client_upload_and_create_store() {
        let client = MockFileSearchClient::ready("fileSearchStores/mock-123");

        let file = client
            .upload_file("test.md", "text/markdown", b"hello")
            .await
            .unwrap();
        assert!(file.name.starts_with("files/"));
        assert_eq!(file.display_name, "test.md");

        let store = client.create_store("test-store").await.unwrap();
        assert_eq!(store.name, "fileSearchStores/mock-123");
    }

    #[tokio::test]
    async fn mock_client_import_and_operation() {
        let client = MockFileSearchClient::ready("fileSearchStores/mock-123");

        let op = client
            .import_file("fileSearchStores/mock-123", "files/abc")
            .await
            .unwrap();
        assert!(op.done);

        let op2 = client.get_operation(&op.name).await.unwrap();
        assert!(op2.done);
    }

    #[tokio::test]
    async fn wait_for_store_ready_immediate() {
        let client = MockFileSearchClient::ready("fileSearchStores/mock-123");
        let store = client
            .wait_for_store_ready("fileSearchStores/mock-123", Duration::from_millis(1))
            .await
            .unwrap();
        assert_eq!(store.pending_documents_count.as_deref(), Some("0"));
    }

    #[tokio::test]
    async fn wait_for_store_ready_after_polls() {
        let client = MockFileSearchClient::with_pending("fileSearchStores/mock-123", 2);
        let store = client
            .wait_for_store_ready("fileSearchStores/mock-123", Duration::from_millis(1))
            .await
            .unwrap();
        assert_eq!(store.pending_documents_count.as_deref(), Some("0"));
    }

    #[tokio::test]
    async fn mock_client_delete_store() {
        let client = MockFileSearchClient::ready("fileSearchStores/mock-123");
        let result = client.delete_store("fileSearchStores/mock-123").await;
        assert!(result.is_ok());
    }
}
