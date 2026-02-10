use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::RwLock;

/// An active interrupt session waiting for human input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptSession {
    pub session_id: String,
    pub thread_id: String,
    pub checkpoint_id: String,
    pub interrupt_value: Value,
    /// Store the original graph definition (nodes/edges/channels) for resume.
    pub graph_definition: Value,
    pub created_at: DateTime<Utc>,
}

/// In-memory store for interrupt sessions.
#[derive(Debug, Clone, Default)]
pub struct SessionStore {
    sessions: Arc<RwLock<HashMap<String, InterruptSession>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn create(&self, session: InterruptSession) {
        let mut sessions = self.sessions.write().await;
        sessions.insert(session.session_id.clone(), session);
    }

    pub async fn get(&self, session_id: &str) -> Option<InterruptSession> {
        let sessions = self.sessions.read().await;
        sessions.get(session_id).cloned()
    }

    pub async fn delete(&self, session_id: &str) -> Option<InterruptSession> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(session_id)
    }

    pub async fn list_pending(&self) -> Vec<InterruptSession> {
        let sessions = self.sessions.read().await;
        let mut list: Vec<_> = sessions.values().cloned().collect();
        list.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_session(id: &str) -> InterruptSession {
        InterruptSession {
            session_id: id.into(),
            thread_id: format!("thread-{id}"),
            checkpoint_id: format!("cp-{id}"),
            interrupt_value: json!({"question": "approve?"}),
            graph_definition: json!({"nodes": [], "edges": [], "channels": []}),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn create_and_get() {
        let store = SessionStore::new();
        let session = make_session("s1");
        store.create(session.clone()).await;

        let retrieved = store.get("s1").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().session_id, "s1");
    }

    #[tokio::test]
    async fn get_nonexistent() {
        let store = SessionStore::new();
        assert!(store.get("nope").await.is_none());
    }

    #[tokio::test]
    async fn delete_session() {
        let store = SessionStore::new();
        store.create(make_session("s1")).await;

        let deleted = store.delete("s1").await;
        assert!(deleted.is_some());
        assert!(store.get("s1").await.is_none());
    }

    #[tokio::test]
    async fn delete_nonexistent() {
        let store = SessionStore::new();
        assert!(store.delete("nope").await.is_none());
    }

    #[tokio::test]
    async fn list_pending_sessions() {
        let store = SessionStore::new();
        store.create(make_session("s1")).await;
        store.create(make_session("s2")).await;
        store.create(make_session("s3")).await;

        let list = store.list_pending().await;
        assert_eq!(list.len(), 3);
    }

    #[tokio::test]
    async fn list_empty() {
        let store = SessionStore::new();
        assert!(store.list_pending().await.is_empty());
    }
}
