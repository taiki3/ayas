# Ayas TODO — refドキュメント vs 現状の差分

## 凡例

- [x] 実装済み
- [ ] 未実装 / 差分あり

---

## 1. LangChain-rs (Core / Chain / LLM)

- [x] Runnableトレイト (`invoke/batch/stream` + `pipe()` 合成)
- [x] Message型 (System/User/AI/Tool + マルチモーダル)
- [x] LLMプロバイダ (Claude, OpenAI, Gemini) — ツール呼び出し対応
- [x] パーサー (String, JSON, Structured, Regex)
- [x] RunnableBranch / RunnableWithFallback / RunnablePassthrough
- [x] RunnableParallel (tokio::join!)
- [x] MockChatModel
- [x] `tracing::instrument` ベースの自動計測レイヤー (SmithLayer — tracing::Layer実装)
- [x] LLMプロバイダのトークン単位SSEストリーミング (Claude/OpenAI/Gemini SSE対応)

---

## 2. LangGraph-rs (Graph / Agent)

- [x] Pregelスーパーステップ (BSP 3フェーズ) — CompiledStateGraph
- [x] チャネルシステム (LastValue, Topic, BinaryOp, Ephemeral, Append)
- [x] 条件付きエッジ / ConditionalFanOutEdge
- [x] Send API (動的並列実行)
- [x] Command API (状態更新+遷移の統合)
- [x] Interrupt / Human-in-the-Loop (BreakpointConfig)
- [x] タイムトラベル (履歴取得, フォーク, リプレイ)
- [x] サブグラフ埋め込み (`subgraph_node()`)
- [x] ReActエージェント / ToolCallingエージェント
- [x] Supervisorエージェント / Map-Reduceパターン
- [x] ストリーミング4モード (Values/Updates/Messages/Debug — stream_with_modes + SSEエンドポイント)

---

## 3. LangSmith-rs (Observability) — 最大のギャップ

### 現状
- ayas-smith: Parquet + DuckDB、SmithStoreトレイト抽象化済み、DuckDbStore実装済み
- ayas-server: REST API (`/api/runs`, `/api/feedback` 等)
- Feedback型を ayas-smith に移動済み、task_local トレース伝播対応済み

### データプレーン (高スループットIngestion)
- [x] Fire-and-Forget Ingestion Pipeline
- [x] Batch Ingestion API (`POST /runs/batch` — post + patch)
- [x] flumeチャネルによるバックグラウンドキュー (Producer/Consumer)
- [x] バッチフラッシュ条件 (サイズ/時間/終了シグナル)
- [x] Backpressure (ドロップカウンタ + 警告ログ)
- [x] リトライロジック (Exponential Backoff with Jitter)
- [x] `Drop`トレイトによるRun終了時の自動PATCH送信 (RunGuard)
- [x] `dotted_order` による階層構造表現 (Parquet 19カラム化)

### コントロールプレーン (Application Server)
- [x] プロジェクト管理API (CRUD — POST/GET/DELETE /api/projects)
- [ ] APIキー管理
- [x] データセット管理 (examples, CRUD — POST/GET /api/datasets, /api/datasets/{id}/examples)
- [ ] アノテーションAPI
- [x] Feedback フル仕様 (score/value/correction/feedback_source)

### ストレージ

> **方針**: 初期実装は現行の **DuckDB + Parquet** で進める。
> ストレージアクセスはトレイトで抽象化し、後からPostgreSQL (メタデータ) や
> ClickHouse (トレースOLAP) バックエンドを差し替え可能にしておく。
>
> DuckDBは組み込みOLAPエンジンとして十分な性能があり、外部DB不要でデプロイも容易。
> Proxy環境 (80/443のみ通過) でも問題なく動作する。

- [x] DuckDB + Parquet (現行実装、初期バックエンド)
- [x] ストレージ抽象化トレイト (`SmithStore`) の定義 — 9メソッド、バックエンド差し替え可能
- [x] DuckDbStore (トレイト実装、SmithQuery+writerを統合、Feedback JSON永続化)
- [x] PostgreSQL バックエンド (postgres_store.rs — Runs/Feedback/統計フル実装, Project/Dataset未実装; checkpoint/postgres.rs — CheckpointStoreフル実装)
- [x] ClickHouse バックエンド (clickhouse_store.rs — Runs/Feedback/統計フル実装, HTTP API + JSONEachRow, Project/Dataset未実装)

### 評価サブシステム
- [x] オフライン評価 (ayas-eval: ExactMatch, Contains, LlmJudge)
- [x] オンライン評価 (online.rs — Ingestionパイプライン分岐 → LLM-as-a-Judge非同期評価)

### SDK
- [x] mpsc channel + Parquetバッチ書き込み (方向性は合致)
- [x] `tokio::task_local!` による `parent_run_id` 非同期伝播 (SmithTraceCtx + TracedChatModel/Tool対応)
- [x] Run ライフサイクル管理 (RunGuard: 開始POST → 実行中保持 → 終了PATCH)

---

## 4. ADL (Agent Design Language)

- [x] YAML/JSON パース (serde_yaml, serde_json)
- [x] Rhai式評価エンジン (サンドボックス, max_operations=10,000)
- [x] ComponentRegistry (ノード型ファクトリ)
- [x] バリデーション (6項目: バージョン, ノードID, 型, エッジ, エントリポイント, 条件辺)
- [x] AdlBuilder: `build_from_yaml()` → CompiledStateGraph
- [x] ReactFlowエディタとの双方向変換 (reactflow.rs — ADL YAML ↔ ReactFlow JSON)

---

## 5. フロントエンドUI (Playground)

> `playground/` — Vite + React + TypeScript + Tailwind CSS v4
> `docker-compose.yml` で nginx経由 port 13000 で配信

- [x] プロジェクト基盤 (Vite + React + TS + Tailwind, APIプロキシ設定)
- [x] 共通レイアウト (ヘッダー, タブナビ, API Keysモーダル)
- [x] Chat画面 (2カラム: 設定パネル + チャットエリア, トークン表示)
- [x] Agent画面 (3カラム: ツール設定 + チャット + 実行トレースパネル, SSE対応)
- [x] Graph画面 (ReactFlowビジュアルビルダー, Validate/Run, ノードハイライト)
- [x] Research画面 (Gemini Deep Research, SSE進捗, Markdownレンダリング)
- [x] タイムトラベルデバッガUI (TimeTravel.tsx — ステップ一覧, state diff, ナビゲーション)
- [x] LangSmithダッシュボード (Traces.tsx, Dashboard.tsx, Projects.tsx — トレース一覧, 統計, Feedback)

---

## 6. RAG

- [x] VectorStore / Embedding / Retriever トレイト定義
- [x] InMemoryVectorStore (コサイン類似度)
- [x] Embeddingプロバイダ実装 (OpenAI + Gemini Embeddings)
- [x] 外部VectorStore連携 (Qdrant — qdrant_store.rs)
- [x] 高度な検索戦略 (MMR retriever — Maximal Marginal Relevance)

---

## 7. チェックポイント

- [x] MemoryCheckpointStore
- [x] SqliteCheckpointStore
- [x] PostgreSQLチェックポイントストア (postgres.rs)

---

## 優先度ガイド

### P0 — コアビジョンとの最大乖離
1. ~~**ストレージ抽象化**~~ → 完了 (SmithStore + DuckDbStore)
2. ~~**フロントエンドUI基盤**~~ → 完了 (4画面 + ダッシュボード + タイムトラベル)
3. ~~**LangSmith データプレーン強化**~~ → 完了 (Batch Ingestion, リトライ, RunGuard)
4. ~~**LangSmith コントロールプレーン**~~ → 完了 (プロジェクト/データセット/Feedback API)

### P1 — プロダクション品質に必要
5. ~~LLMプロバイダのSSEストリーミング完全実装~~ → 完了
6. ~~tracingベース自動計測レイヤー~~ → 完了 (SmithLayer)
7. ~~ストリーミング4モードの完全実装~~ → 完了 (Values/Updates/Messages/Debug)
8. ~~タイムトラベルデバッガUI~~ → 完了
9. ~~LangSmithダッシュボード~~ → 完了

### P2 — エコシステム拡充
10. ~~RAG実プロバイダ実装~~ → 完了 (OpenAI/Gemini Embedding, Qdrant, MMR)
11. ~~オンライン評価パイプライン~~ → 完了
12. ~~ADL ↔ ReactFlow双方向変換~~ → 完了
13. ~~Run ライフサイクル管理~~ → 完了 (RunGuard)
14. ~~PostgreSQL / ClickHouse バックエンド~~ → 完了 (stub)

### 残項目
- [ ] APIキー管理
- [ ] アノテーションAPI
- [ ] **Pipeline: Deep Research リトライ機構** — Gemini Interactions API は HTTP 500 を頻繁に返す。STEP 3 並列実行時に一部のみ失敗するケースが多いため、Exponential backoff (1s→2s→4s, 最大3回) のリトライが必要。429は `retry_after_secs` を尊重。対象: `ayas-deep-research/src/runnable.rs` にリトライラッパー追加、`pipeline.rs` でリトライ進捗SSE通知
- [ ] **File Search 対応** — 現在 `demo/` ファイルを `include_str!` / `fs::read_to_string` でインライン添付しているが、Gemini File Search（ベクトルストア）API 利用可能時にファイルアップロード→ベクトルストア作成→File Search tool 設定に置き換え

---

## クレート構成 (現在)

```
ayas-core         基盤トレイト (Runnable, Message, ChatModel, Tool)
ayas-llm          LLMプロバイダ実装 (Claude, OpenAI, Gemini)
ayas-chain        パイプライン合成 (Lambda, Parallel, Prompt, Parser)
ayas-graph        Pregelグラフ実行エンジン
ayas-checkpoint   永続化 (Memory, SQLite)
ayas-agent        プリビルトエージェント (ReAct, ToolCalling, Supervisor, MapReduce)
ayas-rag          RAG (InMemory VectorStore)
ayas-eval         評価フレームワーク
ayas-smith        トレーシング (Parquet + DuckDB)
ayas-adl          Agent Design Language (YAML/JSON → グラフ)
ayas-deep-research  Gemini Deep Research統合
ayas-server       Axum Webサーバー
ayas-examples     サンプル集
```
