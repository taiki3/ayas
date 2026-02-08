# 設計レビューとMCP戦略

## 1. 当初設計 vs 実装：総合レビュー

### 設計文書一覧（ref/ディレクトリ）

| # | 文書 | 対応範囲 |
|---|------|----------|
| 1 | `LangChain Rust再実装の内部調査.md` | Runnable, Message, ChatModel, Tool, Callback |
| 2 | `RustでLangChainエコシステムを再実装.md` | 全体アーキテクチャ, ADL, Pregel, LangSmith, Web API |
| 3 | `LangsmithのRust再実装に向けた深掘り.md` | 可観測性基盤, Ingestion, Storage, SDK, 評価 |
| 4 | `LangGraph Rust実装のための内部構造調査.md` | Channel, Pregel, Checkpoint, Send/Command, Streaming |

---

### 1.1 LangChain-rs (Core Primitives) — `ayas-core` / `ayas-chain`

#### 実装済み

| 設計項目 | 状態 | 実装内容 |
|----------|------|----------|
| Runnableトレイト (Associated Types) | 完了 | `Input`/`Output` 関連型。`invoke`, `batch`, `stream` メソッド (`crates/ayas-core/src/runnable.rs`) |
| RunnableSequence (型安全な連鎖) | 完了 | `Second: Runnable<Input = First::Output>` の型制約 (`crates/ayas-chain/src/sequence.rs`) |
| Message Enum | 完了 | `System`/`User`/`AI`/`Tool` の4バリアント。`ToolCall` 構造体 (`crates/ayas-core/src/message.rs`) |
| ChatModel トレイト | 完了 | `generate(&self, messages, options) -> Result<ChatResult>` (`crates/ayas-core/src/model.rs`) |
| Tool トレイト + JSON Schema | 完了 | `ToolDefinition` + `parameters: Value`。schemars活用 (`crates/ayas-core/src/tool.rs`) |
| RunnableConfig | 完了 | `tags`, `metadata`, `recursion_limit`, `run_id`, `configurable` (`crates/ayas-core/src/config.rs`) |
| PromptTemplate | 完了 | `{variable}` 形式の変数置換 (`crates/ayas-chain/src/prompt.rs`) |
| OutputParser | 完了 | `StringOutputParser`, `MessageContentParser` (`crates/ayas-chain/src/parser.rs`) |
| RunnableLambda | 完了 | クロージャをRunnableとしてラップ (`crates/ayas-chain/src/lambda.rs`) |
| RunnableParallel | 完了 | 複数Runnableの並列実行 (`crates/ayas-chain/src/parallel.rs`) |
| マルチプロバイダLLM | 完了 | OpenAI, Claude, Gemini の3プロバイダ (`crates/ayas-llm/src/`) |
| マルチモーダル対応 | 完了 | テキスト+画像のContentBlock対応 |

#### 当初設計から変更された点

| 設計項目 | 設計 | 実装 | 理由・備考 |
|----------|------|------|------------|
| Runnableの`Error`関連型 | `type Error` を定義 | なし。共通の `AyasError` を使用 | 統一エラー型に一本化して簡素化 |
| BitOr演算子 (パイプ構文) | `std::ops::BitOr` で `prompt \| model \| parser` | `.pipe()` メソッド方式 | Rustの型推論との相性から `.pipe()` を採用。機能的には同等 |
| UserContent の分離 | `UserContent::Text` / `UserContent::Multimodal` のEnum | `User(String)` の単一バリアント | 簡略化。マルチモーダルは別経路で対応 |
| RunnableConfig の callbacks | `callbacks: Vec<Arc<dyn CallbackHandler>>` | callbacksフィールドなし | `ayas-smith` のTracedデコレータで代替 |

#### 未実装

| 設計項目 | 優先度 | 備考 |
|----------|--------|------|
| VectorStore トレイト | 中 | RAG関連機能。設計書§7に記載 |
| ストリーミングJSONパーサー | 低 | 部分JSONの差分パース。設計書§2.2.4 |
| キャッシュ付きChatModel | 低 | ミドルウェアラッパー。設計書§3.2.2 |
| DynamicRunnable (型消去版) | 低 | `Box<dyn Runnable<Input=Value, Output=Value>>`。設計書§2.2.3 |

---

### 1.2 LangGraph-rs (Orchestration Engine) — `ayas-graph`

#### 実装済み

| 設計項目 | 状態 | 実装内容 |
|----------|------|----------|
| Pregel実行エンジン (BSP) | 完了 | Plan → Execute → Update のスーパーステップ (`crates/ayas-graph/src/compiled.rs`) |
| StateGraphビルダー | 完了 | `add_node`, `add_edge`, `add_conditional_edges`, `compile()` (`crates/ayas-graph/src/state_graph.rs`) |
| Channel トレイト | 完了 | `update`, `get`, `checkpoint`, `restore`, `reset` (`crates/ayas-graph/src/channel.rs`) |
| LastValueチャネル | 完了 | 最新値のみ保持。複数書き込み時のエラーハンドリング |
| AppendChannel (Topic相当) | 完了 | `Vec<Value>` による値の追記 |
| 条件付きエッジ | 完了 | ルーティング関数 + path_map (`crates/ayas-graph/src/edge.rs`) |
| START/END 定数 | 完了 | `__start__`, `__end__` (`crates/ayas-graph/src/constants.rs`) |
| グラフ検証 (compile) | 完了 | 到達可能性、孤立ノード検出 |
| チェックポイント基本機能 | 完了 | `checkpoint()`/`restore()` メソッド |

#### 当初設計から変更された点

| 設計項目 | 設計 | 実装 | 理由・備考 |
|----------|------|------|------------|
| AppendChannelの`accumulate`フラグ | `accumulate: bool` でPub/Sub or 蓄積を切替 | 常に蓄積モード | `consume()` メソッド未実装のためエフェメラル動作なし |
| ノードの並列実行 | `tokio::spawn` / `JoinSet` で並列実行 | 逐次実行 | 将来の並列化に対応可能な構造は保持 |

#### 未実装

| 設計項目 | 優先度 | 備考 |
|----------|--------|------|
| チェックポイント永続化 (CheckpointSaver) | **高** | DB保存のCheckpointSaverトレイト。設計書§5 |
| タイムトラベル (Fork/分岐) | 高 | 過去チェックポイントからの分岐再実行 |
| Send API (動的並列/Map-Reduce) | 中 | 設計書§4.3。動的タスク生成 |
| Command API | 中 | 設計書§4.4。ノード戻り値で状態更新+goto統合 |
| EphemeralValueチャネル | 中 | 設計書§3.5。1ステップだけ生存する一時値 |
| BinaryOperatorAggregateチャネル | 中 | 設計書§3.4。カスタムリデューサー (Sum, Max等) |
| consume() メソッド | 中 | Topicチャネルのクリアに必要 |
| Managed Values (Context注入) | 低 | 設計書§3.6。DB接続やStreamWriterのランタイム注入 |
| ヒューマン・イン・ザ・ループ (interrupt) | **高** | 設計書§6。中断→チェックポイント保存→再開フロー |
| ストリーミングモード (4種) | 中 | values/updates/messages/debug。現在はobserverパターンのみ |
| ノードレベルRetryPolicy | 低 | 設計書§4.1 |
| パニック安全性 (catch_unwind) | 低 | 設計書§8.3 |

---

### 1.3 LangSmith-rs (Observability) — `ayas-smith`

#### 実装済み

| 設計項目 | 状態 | 実装内容 |
|----------|------|----------|
| Run データモデル | 完了 | `id`, `name`, `run_type`, `start_time`, `end_time`, `inputs`, `outputs`, `parent_run_id`, `trace_id`, `error`, `tags`, `extra` (`crates/ayas-smith/src/types.rs`) |
| Feedback データモデル | 完了 | `id`, `run_id`, `key`, `score`, `value`, `comment`, `correction`, `feedback_source` |
| Fire-and-Forget Ingestion | 完了 | バックグラウンドキューによる非同期トレース書き込み (`crates/ayas-smith/src/writer.rs`) |
| 非同期バックグラウンドキュー | 完了 | flumeチャネル + バッファリング + フラッシュ条件 |
| コンテキスト伝播 (task_local) | 完了 | `tokio::task_local!` で `parent_run_id`/`trace_id` を自動伝播 (`crates/ayas-smith/src/context.rs`) |
| Tracedデコレータ | 完了 | `TracedModel`, `TracedTool`, `TracedRunnable` (`crates/ayas-smith/src/traced/`) |
| SmithClient (APIクライアント) | 完了 | `create_run`, `update_run`, `create_feedback`, バッチ操作 (`crates/ayas-smith/src/client.rs`) |
| クエリAPI | 完了 | `list_runs`, `get_run`, `get_trace`, `get_children`, `token_usage_summary`, `latency_percentiles` (`crates/ayas-smith/src/query.rs`) |

#### 当初設計から変更された点

| 設計項目 | 設計 | 実装 | 理由・備考 |
|----------|------|------|------------|
| ストレージ | ClickHouse (OLAP) + PostgreSQL (Metadata) | **DuckDB** 単体 + Parquet | 軽量な組み込みDBに変更。セルフホスト/開発環境向けに簡素化 |
| メッセージキュー | Kafka or Redis でIngestion ↔ Storage を分離 | flumeメモリ内チャネルのみ | プロセス内バッファリングに簡素化 |
| RunType のバリアント | Tool, Chain, Llm, Retriever, Embedding, Prompt, Parser, Custom | より少ないバリアントセット | 実用上必要な型に絞り込み |
| Dropトレイトによる自動PATCH | Runスコープ終了時に自動でend_time記録 | 明示的な`update_run`呼び出し | Tracedデコレータ内で明示的に管理 |

#### 未実装

| 設計項目 | 優先度 | 備考 |
|----------|--------|------|
| PostgreSQL メタデータ管理 | 中 | projects, api_keys, datasets テーブル |
| ClickHouse 大規模トレース保存 | 中 | MergeTree, パーティショニング, Materialized Column |
| マルチパート Ingestion (gzip圧縮) | 低 | `POST /runs/multipart` |
| リトライロジック (指数バックオフ) | 低 | SDKのネットワークエラー時再試行 |
| 評価(Evaluation)サブシステム | 中 | オフライン評価、LLM-as-a-Judge |
| アノテーションキュー | 低 | ヒューマンレビュー用キュー管理 |
| レートリミット (Redis) | 低 | API利用量制限 |
| Backpressure / Drop戦略 | 低 | 高負荷時の古いトレース破棄 |

---

### 1.4 ADL (Agent Design Language) — `ayas-adl`

#### 実装済み

| 設計項目 | 状態 | 実装内容 |
|----------|------|----------|
| YAML/JSON ADL仕様 | 完了 | `version`, `agent`, `channels`, `nodes`, `edges` (`crates/ayas-adl/src/types.rs`) |
| AdlBuilder (ADL → StateGraph) | 完了 | YAML解析 → グラフ構築 (`crates/ayas-adl/src/builder.rs`) |
| ComponentRegistry | 完了 | 文字列ID → ファクトリ関数 (`crates/ayas-adl/src/registry.rs`) |
| Rhai式評価 | 完了 | 条件付きエッジの式評価 (`crates/ayas-adl/src/expression.rs`) |
| ADLバリデーション | 完了 | スキーマ検証 (`crates/ayas-adl/src/validation.rs`) |

#### 未実装

| 設計項目 | 優先度 | 備考 |
|----------|--------|------|
| サブグラフ参照 | 低 | ADL内で他エージェント定義をサブグラフとして参照 |
| Rhaiサンドボックス制限 | 低 | 無限ループ防止、メモリ制限 |

---

### 1.5 エージェントパターン — `ayas-agent`

#### 実装済み

| 設計項目 | 状態 | 実装内容 |
|----------|------|----------|
| ReActエージェント | 基本実装 | `create_react_agent()` でStateGraph上にAgent→Tool→Agentループ (`crates/ayas-agent/src/react.rs`) |

#### 未実装

| 設計項目 | 優先度 | 備考 |
|----------|--------|------|
| handle_parsing_errors | 中 | LLMフォーマットエラー時の自己修正フィードバック |
| max_iterations / early_stopping | 中 | recursion_limitでの代替はあるが専用パラメータなし |
| 並列ツール実行 | 中 | `buffer_unordered` による複数ToolCall同時実行 |
| StepResult Enum | 低 | AgentAction/AgentFinish の明示的ステートマシン |

---

### 1.6 Web API / サーバー — `ayas-server`

#### 実装済み

| 設計項目 | 状態 | 実装内容 |
|----------|------|----------|
| Axum ベース REST API | 完了 | `crates/ayas-server/src/main.rs` |
| SSE ストリーミング | 完了 | グラフ/エージェント実行のリアルタイム配信 (`crates/ayas-server/src/sse.rs`) |
| POST /api/graph/validate | 完了 | グラフ構造の検証 |
| POST /api/graph/execute | 完了 | ADLグラフの実行 (SSE) |
| POST /api/chat/invoke | 完了 | 単一LLM呼び出し |
| POST /api/agent/invoke | 完了 | ReActエージェント実行 (SSE) |
| POST /api/research/invoke | 完了 | Gemini Deep Research |
| POST /api/graph/generate | 完了 | LLMによるグラフ自動生成 |
| 組み込みツール | 完了 | calculator, datetime, web_search |

#### 未実装

| 設計項目 | 優先度 | 備考 |
|----------|--------|------|
| POST /resume (HITL再開) | **高** | 中断されたエージェントの再開 |
| チェックポイント管理API | 高 | タイムトラベル用のGET/PUT checkpoint |
| 認証 (JWT/APIキー) | 中 | Tower ミドルウェア |
| CORS設定 | 低 | tower-http |

---

### 1.7 Webフロントエンド — 未着手

| 設計項目 | 優先度 | 備考 |
|----------|--------|------|
| ReactFlow エディタ | 高 | ノードのドラッグ&ドロップ、ADLとの相互変換 |
| リアルタイム実行可視化 | 高 | SSEイベントに基づくノードハイライト表示 |
| タイムトラベルデバッガ | 中 | チェックポイント履歴の可視化・フォーク |
| 変数インスペクター | 中 | state_update イベントによる変数表示 |

`docs/FRONTEND_DESIGN_BRIEF.md` に設計は記載済みだがコードは未着手。

---

### 1.8 設計外の追加実装

| 機能 | crate | 備考 |
|------|-------|------|
| Gemini Deep Research | `ayas-deep-research` | Gemini 2.0 Chat + Deep Research APIの統合 |
| LLMによるグラフ自動生成 | `ayas-server` | LLMにADL仕様を渡してグラフ構造を自動生成 |
| DuckDB + Parquet分析 | `ayas-smith` | 設計のClickHouse+PostgreSQLに代わる組み込み分析 |
| MockChatModel | `ayas-chain` | テスト用モック |

---

### 1.9 全体サマリー

| カテゴリ | 設計項目数 | 実装済 | 変更あり | 未実装 | 実装率 |
|----------|-----------|--------|----------|--------|--------|
| Core (Runnable/Message/Tool) | 16 | 12 | 4 | 4 | 75% |
| Graph (Pregel/Channel) | 18 | 9 | 2 | 11 | 50% |
| Smith (Observability) | 16 | 8 | 5 | 8 | 50% |
| ADL | 7 | 5 | 0 | 2 | 71% |
| Agent | 5 | 1 | 0 | 4 | 20% |
| Web API | 10 | 7 | 1 | 3 | 70% |
| Frontend | 4 | 0 | 0 | 4 | 0% |
| **合計** | **76** | **42** | **12** | **36** | **55%** |

---

## 2. 開発者体験の戦略: MCP + Skill

### 2.1 背景と方針

ayas はエージェント開発の自動化を目的としたフレームワークであり、開発者はClaude Code (CLI/IDE) および claude.ai (ブラウザ) からこのフレームワークを利用してエージェントを構築する。そのため、ライブラリ単体ではなく**ライブラリの使い方を知ったツール群**をセットで提供する必要がある。

### 2.2 Skill vs MCP の比較

| 観点 | Skill (.claude/skills/) | MCP Server |
|------|------------------------|------------|
| 利用環境 | Claude Code CLI/IDE のみ | CLI, IDE, **claude.ai (Remote MCP)**, 任意のMCPクライアント |
| 提供するもの | 知識・ワークフロー指示 (テキスト) | **実行可能なツール** + リソース |
| ブラウザ対応 | 不可 | **可能** (Remote MCP / Streamable HTTP) |
| 状態保持 | なし (毎回テキスト注入) | サーバー側でセッション・状態管理可能 |
| ayas との親和性 | 低 (Rustコードは実行できない) | **高** (ayas crateを直接呼べる) |

**結論**: MCP Server をメイン、Skill を「MCPツールの使い方ガイド」として薄く添える。

### 2.3 アーキテクチャ

```
+---------------------------------------------------+
|                  Claude (任意の環境)                |
|        claude.ai / Claude Code CLI / IDE           |
+------------------------+--------------------------+
                         | MCP Protocol (Streamable HTTP)
                         v
+---------------------------------------------------+
|              ayas-mcp-server (新crate)             |
|                                                    |
|  +----------+ +-----------+ +-------------------+ |
|  | Tools    | | Resources | | Prompts           | |
|  |          | |           | |                   | |
|  | validate | | adl://    | | create-agent      | |
|  | compile  | | graph://  | | debug-graph       | |
|  | invoke   | | smith://  | | optimize-pipeline | |
|  | trace    | |           | |                   | |
|  +-----+----+ +-----+----+ +-------------------+ |
|        |            |                              |
|  +-----v------------v--------------------------+   |
|  |  ayas-core / ayas-graph / ayas-adl /        |   |
|  |  ayas-smith / ayas-llm / ayas-agent         |   |
|  +---------------------------------------------+   |
+---------------------------------------------------+
```

---

### 2.4 MCP Server 設計: `ayas-mcp-server`

#### Tools (Claude が実行できるアクション)

**ADL 関連**

| Tool | 入力 | 出力 | 用途 |
|------|------|------|------|
| `ayas_validate_adl` | `adl_yaml: string` | `{ valid, errors[], warnings[] }` | ADLの構文・構造検証 |
| `ayas_compile_graph` | `adl_yaml: string` | `{ nodes[], edges[], channels[] }` | ADLからグラフ構造を可視化 |

**グラフ実行**

| Tool | 入力 | 出力 | 用途 |
|------|------|------|------|
| `ayas_invoke_graph` | `adl_yaml, input, provider, api_key` | `{ result, trace_id }` | コンパイル済みグラフの実行 |

**エージェント構築**

| Tool | 入力 | 出力 | 用途 |
|------|------|------|------|
| `ayas_create_react_agent` | `system_prompt, tools[], input, provider` | `{ result, steps[], trace_id }` | ReActエージェントの構築・実行 |

**可観測性**

| Tool | 入力 | 出力 | 用途 |
|------|------|------|------|
| `ayas_query_traces` | `{ filter: { run_type?, status?, name? }, limit }` | `{ runs[] }` | トレース検索・フィルタリング |
| `ayas_get_trace_tree` | `{ trace_id }` | `{ tree }` | 実行ツリーの取得 |

**スキャフォールド**

| Tool | 入力 | 出力 | 用途 |
|------|------|------|------|
| `ayas_scaffold_agent` | `name, description, tools[], pattern` | `{ files: [{ path, content }] }` | 新規エージェントプロジェクトの雛形生成 |

#### Resources (Claude が参照できるデータ)

| URI | 用途 |
|-----|------|
| `adl://schema` | ADL仕様のJSON Schema |
| `graph://{graph_id}/topology` | コンパイル済みグラフのトポロジー |
| `smith://runs/recent` | 最新のトレース実行一覧 |
| `smith://runs/{run_id}` | 特定の実行の詳細 |

#### Prompts (Claude が使えるプロンプトテンプレート)

| Prompt | 引数 | 用途 |
|--------|------|------|
| `create-agent` | `use_case: string` | ユースケースからADLを生成 |
| `debug-graph` | `trace_id: string` | トレース分析して問題特定 |
| `optimize-pipeline` | `adl_yaml: string` | 既存ADLの最適化提案 |

---

### 2.5 Skill は「知識層」として薄く添える

```
.claude/skills/
  ayas-dev/
    SKILL.md          # ライブラリ開発時のガイドライン
                      # (user-invocable: false, 自動読み込み)
  agent-builder/
    SKILL.md           # /agent-builder で手動呼び出し
                       # MCPツールの組み合わせ方を指示
```

**ayas-dev** (自動ロード型, `user-invocable: false`):
- crate間の依存関係と設計原則
- テストパターン
- ADL仕様変更時のルール
- MCPツールの使い方ガイド

**agent-builder** (手動呼び出し型, `/agent-builder`):
- MCPツールを組み合わせたエージェント構築ワークフロー
- パターン別のベストプラクティス (ReAct, Multi-step Graph, RAG)

---

## 3. ライブラリ / MCP 並列開発ワークフロー

### 3.1 原則

**1つの機能を「ライブラリ実装 → MCPツール公開 → テスト」のサイクルで回す。**

ライブラリだけ先に進めてMCPが後回しになると、MCPツール設計がライブラリAPIに影響を与えられなくなる。逆にMCPだけ先行するとstubだらけになる。並行開発で互いのフィードバックを得る。

```
+----------------------------------------------------------+
|  Feature: チェックポイント永続化                           |
|                                                           |
|  (1) ライブラリ実装                                       |
|     ayas-graph に CheckpointSaver trait を追加             |
|     |                                                     |
|  (2) MCPツール公開                                        |
|     ayas_save_checkpoint / ayas_load_checkpoint            |
|     ayas_list_checkpoints / ayas_fork_from                 |
|     |                                                     |
|  (3) E2Eテスト (Claude自身がMCPツールを使って検証)         |
|     claude.ai 上で checkpoint -> fork -> resume を実行     |
|     |                                                     |
|  (4) Skill更新                                            |
|     チェックポイント関連のベストプラクティスを追記           |
+----------------------------------------------------------+
```

### 3.2 Phase 0: 基盤整備 (最初にやる)

| タスク | 成果物 |
|--------|--------|
| CLAUDE.md 作成 | プロジェクト概要、開発規約、crate構成 |
| ayas-mcp-server crate 作成 | 空のMCPサーバー骨格 (Streamable HTTP) |
| 最小限のSkill (ayas-dev) 配置 | `.claude/skills/ayas-dev/SKILL.md` |

### 3.3 Phase 1: 既存機能のMCP化

| ライブラリ側 | MCP側 | Skill側 |
|-------------|-------|---------|
| (既存コードの安定化) | `ayas_validate_adl` | ayas-dev (基本ガイドライン) |
| | `ayas_compile_graph` | |
| | `ayas_invoke_graph` | |

このフェーズではライブラリの新規開発は行わず、既存の ayas-adl, ayas-graph の機能をMCPツールとして公開することに集中する。MCPサーバーの基盤を固める。

### 3.4 Phase 2: Checkpoint + HITL

| ライブラリ側 | MCP側 | Skill側 |
|-------------|-------|---------|
| CheckpointSaver trait | `ayas_save_checkpoint` | チェックポイント/HITLパターン追記 |
| interrupt/resume 実装 | `ayas_load_checkpoint` | |
| | `ayas_resume` | |
| | `ayas_list_checkpoints` | |

### 3.5 Phase 3: Agent強化

| ライブラリ側 | MCP側 | Skill側 |
|-------------|-------|---------|
| Send/Command API | `ayas_create_react_agent` 強化 | agent-builder Skill 作成 |
| 並列ツール実行 | `ayas_scaffold_agent` | |
| handle_parsing_errors | | |

### 3.6 Phase 4: 可観測性強化

| ライブラリ側 | MCP側 | Skill側 |
|-------------|-------|---------|
| 評価(Evaluation)サブシステム | `ayas_evaluate` | 評価ワークフロー Skill |
| | `ayas_query_traces` 強化 | |
| | `ayas_get_trace_tree` | |

### 3.7 Phase 5: Remote MCP

| ライブラリ側 | MCP側 | Skill側 |
|-------------|-------|---------|
| — | Streamable HTTP対応 | — |
| — | 認証・APIキー管理 | — |
| — | claude.ai からの利用開始 | — |

---

### 3.4 CI/CDへの組み込み

```yaml
# GitHub Actions (イメージ)
test-library:
  - cargo test --workspace

test-mcp-tools:
  - cargo run --bin ayas-mcp-server &
  - cargo test -p ayas-mcp-server --test integration

validate-skills:
  - # SKILL.md のフロントマター構文チェック
  - # 参照するMCPツール名が実際に存在するか検証
```

---

## 4. MCP が最適な理由 (まとめ)

1. **ブラウザ到達性**: claude.ai の Remote MCP でブラウザから直接 ayas のグラフ実行やADL検証ができる
2. **ライブラリとの一体性**: ayas crate を直接使う MCP サーバーなのでラッパーの二重管理が不要
3. **段階的拡張**: ツールを1つずつ追加していける。Skill のようなテキスト全体の書き直しが不要
4. **テスト可能性**: MCPツールは入出力が明確なので自動テストが書きやすい
5. **エコシステム**: 他のMCPクライアント (Cursor, Windsurf等) からも利用可能
