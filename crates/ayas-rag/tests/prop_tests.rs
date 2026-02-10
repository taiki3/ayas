use proptest::prelude::*;

use ayas_rag::types::EmbeddingVector;

/// Strategy to generate non-zero f32 vectors of a given dimension.
fn non_zero_vec(dim: usize) -> impl Strategy<Value = Vec<f32>> {
    proptest::collection::vec(-100.0f32..100.0f32, dim).prop_filter(
        "vector must not be all zeros",
        |v| v.iter().any(|x| *x != 0.0),
    )
}

proptest! {
    #[test]
    fn cosine_self_similarity_is_one(v in non_zero_vec(8)) {
        let emb = EmbeddingVector::new(v);
        let sim = emb.cosine_similarity(&emb);
        prop_assert!((sim - 1.0).abs() < 1e-5, "self-similarity was {}", sim);
    }

    #[test]
    fn cosine_similarity_is_symmetric(
        a in non_zero_vec(8),
        b in non_zero_vec(8),
    ) {
        let ea = EmbeddingVector::new(a);
        let eb = EmbeddingVector::new(b);
        let ab = ea.cosine_similarity(&eb);
        let ba = eb.cosine_similarity(&ea);
        prop_assert!((ab - ba).abs() < 1e-5, "sim(a,b)={} != sim(b,a)={}", ab, ba);
    }

    #[test]
    fn cosine_similarity_in_range(
        a in non_zero_vec(8),
        b in non_zero_vec(8),
    ) {
        let ea = EmbeddingVector::new(a);
        let eb = EmbeddingVector::new(b);
        let sim = ea.cosine_similarity(&eb);
        prop_assert!(sim >= -1.0 - 1e-5 && sim <= 1.0 + 1e-5, "sim={} out of [-1,1]", sim);
    }

    #[test]
    fn cosine_similarity_scale_invariant(
        v in non_zero_vec(8),
        scale in 0.1f32..100.0f32,
    ) {
        let ev = EmbeddingVector::new(v.clone());
        let scaled: Vec<f32> = v.iter().map(|x| x * scale).collect();
        let es = EmbeddingVector::new(scaled);
        let sim = ev.cosine_similarity(&es);
        prop_assert!((sim - 1.0).abs() < 1e-4, "scale-invariance failed: sim={}", sim);
    }
}

mod store_props {
    use ayas_rag::memory::InMemoryVectorStore;
    use ayas_rag::store::VectorStore;
    use ayas_rag::types::{Document, EmbeddingVector, SearchOptions};
    use std::collections::HashMap;

    fn make_doc(id: &str, content: &str) -> Document {
        Document {
            id: id.into(),
            content: content.into(),
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn add_then_search_finds_document() {
        let store = InMemoryVectorStore::new();
        let emb = EmbeddingVector::new(vec![1.0, 0.0, 0.0]);
        store
            .add_documents(vec![(make_doc("d1", "text"), emb.clone())])
            .await
            .unwrap();

        let results = store
            .similarity_search(&emb, SearchOptions { k: 10, score_threshold: None })
            .await
            .unwrap();

        assert!(!results.is_empty());
        assert_eq!(results[0].document.id, "d1");
        assert!((results[0].score - 1.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn add_then_delete_then_search_empty() {
        let store = InMemoryVectorStore::new();
        let emb = EmbeddingVector::new(vec![1.0, 0.0]);
        store
            .add_documents(vec![(make_doc("d1", "text"), emb.clone())])
            .await
            .unwrap();

        store.delete(&["d1".into()]).await.unwrap();

        let results = store
            .similarity_search(&emb, SearchOptions { k: 10, score_threshold: None })
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_k_zero_returns_empty() {
        let store = InMemoryVectorStore::new();
        let emb = EmbeddingVector::new(vec![1.0]);
        store
            .add_documents(vec![(make_doc("d1", "text"), emb.clone())])
            .await
            .unwrap();

        let results = store
            .similarity_search(&emb, SearchOptions { k: 0, score_threshold: None })
            .await
            .unwrap();

        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_high_threshold_filters() {
        let store = InMemoryVectorStore::new();
        store
            .add_documents(vec![
                (make_doc("d1", "close"), EmbeddingVector::new(vec![1.0, 0.0])),
                (make_doc("d2", "far"), EmbeddingVector::new(vec![0.0, 1.0])),
            ])
            .await
            .unwrap();

        let query = EmbeddingVector::new(vec![1.0, 0.0]);
        let results = store
            .similarity_search(
                &query,
                SearchOptions {
                    k: 10,
                    score_threshold: Some(0.99),
                },
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].document.id, "d1");
    }
}
