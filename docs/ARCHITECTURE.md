# Ayas Architecture

Ayas は LangChain / LangGraph エコシステムを Rust で再実装したプロジェクトです。
型安全な合成可能パイプライン、グラフベースのワークフロー実行、エージェント構築を提供します。

## 技術スタック

- **Rust 2024 Edition**
- **async-trait** — 非同期トレイト抽象化
- **Tokio** — 非同期ランタイム
- **serde / serde_json** — シリアライズ / JSON 操作
- **thiserror v2** — エラー型定義
- **reqwest** (rustls-tls) — HTTP クライアント
- **futures / tokio-stream** — ストリーム処理
- **uuid** — 実行 ID 生成
- **schemars** — JSON Schema 生成

## クレート依存グラフ

```
ayas-core
  ^       ^
  |        \
ayas-chain  ayas-graph
               ^
               |
            ayas-agent

ayas-core
  ^
  |
ayas-deep-research

ayas-examples (各クレートを利用)
```

## テスト概要 (155 テスト)

| クレート | ユニット | 統合/E2E | 合計 |
|---|---|---|---|
| ayas-core | 41 | — | 41 |
| ayas-chain | 22 | 4 | 26 |
| ayas-graph | 37 | 10 + 5 | 52 |
| ayas-agent | 6 | 3 | 9 |
| ayas-deep-research | 24 | 3 | 27 |
| **合計** | | | **155** |

---

## 1. ayas-core — 基盤型・トレイト

全クレートが依存する基盤レイヤー。パイプライン合成の核となるトレイト群と共通型を提供します。

### Runnable トレイト

```rust
#[async_trait]
pub trait Runnable: Send + Sync {
    type Input: Send + 'static;
    type Output: Send + 'static;

    async fn invoke(&self, input: Self::Input, config: &RunnableConfig) -> Result<Self::Output>;
    async fn batch(&self, inputs: Vec<Self::Input>, config: &RunnableConfig) -> Result<Vec<Self::Output>>;
    async fn stream(&self, input: Self::Input, config: &RunnableConfig)
        -> Result<Pin<Box<dyn Stream<Item = Result<Self::Output>> + Send>>>;
}
```

Ayas のすべてのコンポーネント（プロンプト、モデル、パーサー、グラフ）はこのトレイトを実装します。

- **`invoke()`** — 単一入力を処理して結果を返す
- **`batch()`** — 複数入力を順次処理（デフォルト実装）
- **`stream()`** — 出力チャンクをストリーミング（デフォルト実装は単一アイテムを yield）

### RunnableExt — パイプ合成

```rust
pub trait RunnableExt: Runnable + Sized {
    fn pipe<R>(self, next: R) -> RunnableSequence<Self, R>
    where R: Runnable<Input = Self::Output>;
}
```

`.pipe()` で任意の Runnable を型安全に直列接続できます。型の不一致はコンパイル時に検出されます。

```rust
let chain = AddOne.pipe(MultiplyTwo).pipe(ToString);
// Input: i32 → i32 → i32 → String
```

### RunnableSequence

`pipe()` で生成される2段合成 Runnable。自身も `Runnable` を実装するためチェーン可能です。

### IdentityRunnable

入力をそのまま出力するパススルー Runnable。

### Message 型

4 バリアントの会話メッセージ enum。`#[serde(tag = "type")]` で JSON タグ付き。

| バリアント | フィールド |
|---|---|
| `System` | `content: String` |
| `User` | `content: String` |
| `AI` | `AIContent { content, tool_calls, usage }` |
| `Tool` | `content: String, tool_call_id: String` |

ファクトリメソッド: `Message::system()`, `Message::user()`, `Message::ai()`, `Message::ai_with_tool_calls()`, `Message::tool()`

**ToolCall**: `{ id, name, arguments: Value }` — AI メッセージに含まれるツール呼び出しリクエスト。

**UsageMetadata**: `{ input_tokens, output_tokens, total_tokens }` — トークン使用量。

### ChatModel トレイト

```rust
#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn generate(&self, messages: &[Message], options: &CallOptions) -> Result<ChatResult>;
    fn model_name(&self) -> &str;
}
```

LLM プロバイダを抽象化するトレイト。`CallOptions` で `max_tokens`, `temperature`, `tools` (ToolDefinition のリスト), `stop` シーケンスを制御します。`ChatResult` は生成された `Message` と `UsageMetadata` を保持します。

### Tool トレイト

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;
    async fn call(&self, input: serde_json::Value) -> Result<String>;
}
```

ツールは JSON 入力を受け取り文字列を返します。`ToolDefinition` は `{ name, description, parameters: Value(JSON Schema) }` でモデルに渡すメタデータを保持します。

### RunnableConfig

パイプラインを通して伝搬する実行コンフィグ。

| フィールド | 型 | デフォルト |
|---|---|---|
| `tags` | `Vec<String>` | `[]` |
| `metadata` | `HashMap<String, Value>` | `{}` |
| `recursion_limit` | `usize` | `25` |
| `run_id` | `Uuid` | 自動生成 |
| `configurable` | `HashMap<String, Value>` | `{}` |

ビルダーメソッド: `with_tag()`, `with_metadata()`, `with_recursion_limit()`, `with_run_id()`

### AyasError — エラー階層

```
AyasError
├── Model(ModelError)       — ApiRequest / InvalidResponse / Auth / RateLimited
├── Tool(ToolError)         — NotFound / InvalidInput / ExecutionFailed
├── Chain(ChainError)       — Template / Parse / MissingVariable
├── Graph(GraphError)       — InvalidGraph / RecursionLimit / Channel / NodeExecution
├── Serialization(serde_json::Error)
└── Other(String)
```

`thiserror` v2 による `#[from]` 自動変換。`pub type Result<T> = std::result::Result<T, AyasError>`。

---

## 2. ayas-chain — チェーン合成プリミティブ

LangChain の LCEL (LangChain Expression Language) に相当する合成ブロック群。

### RunnableLambda

任意の async クロージャを `Runnable` として利用可能にします。

```rust
let double = RunnableLambda::new(|x: i32, _config| async move { Ok(x * 2) });
```

- `Fn(I, RunnableConfig) -> Future<Output = Result<O>>`
- `Arc<dyn Fn>` で内部保持し `Send + Sync`

### RunnableParallel

2 ブランチを `tokio::join!` で並行実行します。

```rust
let parallel = RunnableParallel::new(branch_a, branch_b);
// Input が Clone される。Output は (A::Output, B::Output) タプル。
```

パイプと組み合わせて fan-out / fan-in パターンを構築できます:

```rust
let chain = RunnableParallel::new(double, triple).pipe(sum);
```

### PromptTemplate

`{variable}` 構文のテンプレートエンジン。

- **`from_template(template)`** — 単一ユーザーメッセージ
- **`from_messages(vec![("system", "..."), ("user", "...")])`** — 複数メッセージ

`Runnable<Input = HashMap<String, String>, Output = Vec<Message>>` を実装。

### StringOutputParser

`Vec<Message>` から最後の AI メッセージのテキストを抽出。

`Runnable<Input = Vec<Message>, Output = String>`

### MessageContentParser

単一 `Message` からテキストコンテンツを抽出。

`Runnable<Input = Message, Output = String>`

### MockChatModel

テスト用モック LLM。プリセットのレスポンスリストをサイクル的に返し、呼び出し回数を追跡します。

`Runnable<Input = Vec<Message>, Output = Vec<Message>>`

### RunnableSequence (re-export)

ayas-core の `RunnableSequence` を re-export。`.pipe()` で自動生成されます。

### 典型的な合成パターン

```rust
use ayas_chain::prelude::*;
use ayas_core::prelude::*;

let chain = PromptTemplate::from_messages(vec![
    ("system", "You are a {role}."),
    ("user", "Tell me about {topic}."),
])
.pipe(MockChatModel::with_response("Rust is great!"))
.pipe(StringOutputParser);

let mut vars = HashMap::new();
vars.insert("role".into(), "helpful assistant".into());
vars.insert("topic".into(), "Rust".into());

let result = chain.invoke(vars, &config).await?;
// result == "Rust is great!"
```

---

## 3. ayas-graph — グラフ実行エンジン

LangGraph に相当するステートフルなグラフ実行エンジン。

### Channel トレイト

グラフ状態の各キーを管理するチャネル。ノードの出力が `update()` を通じてマージされます。

```rust
pub trait Channel: Send + Sync {
    fn update(&mut self, values: Vec<Value>) -> Result<bool>;
    fn get(&self) -> &Value;
    fn checkpoint(&self) -> Value;
    fn restore(&mut self, data: Value);
    fn reset(&mut self);
}
```

#### LastValue チャネル

最後に書き込まれた値のみを保持。1ステップで複数値を受信するとエラー。

```rust
let ch = LastValue::new(json!(0)); // デフォルト値 0
```

#### AppendChannel

値を JSON 配列に蓄積。配列値は自動フラット化されます（`[a, b]` → a, b を個別追加）。

```rust
let ch = AppendChannel::new(); // 初期値 []
```

### ChannelSpec

呼び出しごとにチャネルを新規生成するファクトリ。内部可変性（Mutex）を不要にする設計。

```rust
pub enum ChannelSpec {
    LastValue { default: Value },
    Append,
}
```

### NodeFn

async 関数をグラフノードとしてラップ。`Fn(Value, RunnableConfig) -> Future<Result<Value>>` を受け取ります。

```rust
let node = NodeFn::new("my_node", |mut state: Value, _config| async move {
    state["count"] = json!(state["count"].as_i64().unwrap() + 1);
    Ok(state)
});
```

ノードは完全な状態を受け取り、部分的な更新（変更したキーのみ）を返します。

### Edge / ConditionalEdge

- **Edge** — 静的な有向辺 `{ from, to }`
- **ConditionalEdge** — 実行時の状態に基づくルーティング
  - `route_fn: Fn(&Value) -> String` でルーティングキーを決定
  - オプションの `path_map: HashMap<String, String>` でキーをノード名に変換
  - path_map にキーがない場合、キーそのものがターゲットノード名として使用される
  - **条件辺は静的辺より優先**

### StateGraph ビルダー

宣言的にグラフを構築し、`compile()` で実行可能な `CompiledStateGraph` を生成します。

```rust
let mut graph = StateGraph::new();
graph.add_last_value_channel("count", json!(0));
graph.add_append_channel("messages");
graph.add_node(node_a)?;
graph.add_node(node_b)?;
graph.set_entry_point("a");
graph.add_edge("a", "b");
graph.set_finish_point("b");
let compiled = graph.compile()?;
```

**ビルダーメソッド:**
- `add_channel(name, ChannelSpec)` / `add_last_value_channel(name, default)` / `add_append_channel(name)`
- `add_node(NodeFn)` — 重複名・予約名 (`__start__`, `__end__`) はエラー
- `add_edge(from, to)` / `add_conditional_edges(ConditionalEdge)`
- `set_entry_point(node)` / `set_finish_point(node)`

**compile() 時の検証:**
1. エントリポイントが設定されているか
2. エントリポイントのノードが存在するか
3. 辺が参照するノードが存在するか（センチネル `START`/`END` は許可）
4. 条件辺のソースノードが存在するか
5. フィニッシュポイントのノードが存在するか
6. BFS 到達可能性 — 全ノードがエントリポイントから到達可能か

### CompiledStateGraph — Pregel 実行エンジン

`StateGraph::compile()` で生成される実行可能グラフ。`Runnable<Input = Value, Output = Value>` を実装します。

**Pregel スーパーステップループ:**

```
1. チャネル初期化 (ChannelSpec → Channel)
2. 入力値でチャネルを更新
3. while current_nodes が空でない:
   a. recursion_limit チェック
   b. 各ノードに対して:
      - チャネルから状態を構築
      - ノードを実行
      - 部分出力でチャネルを更新
      - 次のノードを決定（条件辺 > 静的辺）
   c. 重複除去
4. 最終状態をチャネルから構築して返す
```

**定数:**
- `START = "__start__"` — グラフエントリポイントのセンチネル
- `END = "__end__"` — グラフ終了のセンチネル

**サポートするトポロジ:**
- 直線 (A → B → C)
- 分岐 (A → B, A → C)
- 合流 / ダイヤモンド (A → B, A → C → D, B → D)
- ループ / サイクル (A → B → A → ...) — `recursion_limit` で制御

---

## 4. ayas-agent — プリビルトエージェント

LangGraph の既製エージェントパターンを提供します。

### create_react_agent()

```rust
pub fn create_react_agent(
    model: Arc<dyn ChatModel>,
    tools: Vec<Arc<dyn Tool>>,
) -> Result<CompiledStateGraph>
```

ReAct (Reasoning + Acting) パターンの実装。内部的に `StateGraph` を構築し `CompiledStateGraph` を返します。

**グラフ構造:**

```
        ┌────────────────────┐
        │                    │
        ▼                    │
START → agent ──(tool_calls)→ tools
          │
          └──(no tool_calls)→ END
```

**状態スキーマ:**
- `messages`: `AppendChannel` — 会話履歴を蓄積

**agent ノード:**
1. `messages` から `Vec<Message>` をパース
2. `ChatModel::generate()` にメッセージとツール定義を渡す
3. AI の応答メッセージを `messages` に追加

**tools ノード:**
1. 最後の AI メッセージから `tool_calls` を抽出
2. 各ツールを名前で検索して実行
3. `Message::tool(output, tool_call_id)` を `messages` に追加

**ルーティング:**
- `agent` からの条件辺: `tool_calls` あり → `"tools"`, なし → `END`
- `tools` → `agent` への静的辺（ループ）

**使用例:**

```rust
let graph = create_react_agent(model, tools)?;
let input = json!({"messages": [{"type": "user", "content": "What is 2+2?"}]});
let result = graph.invoke(input, &config).await?;
```

---

## 5. ayas-deep-research — Gemini Deep Research

Google Gemini Interactions API を利用したディープリサーチ機能。

### InteractionsClient トレイト

```rust
#[async_trait]
pub trait InteractionsClient: Send + Sync {
    async fn create(&self, request: &CreateInteractionRequest) -> Result<Interaction>;
    async fn get(&self, interaction_id: &str) -> Result<Interaction>;
    async fn create_and_poll(&self, request: &CreateInteractionRequest, poll_interval: Duration) -> Result<Interaction>;
    async fn create_stream(&self, request: &CreateInteractionRequest)
        -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>>;
}
```

- `create()` — インタラクション作成 (POST)
- `get()` — ステータス取得 (GET)
- `create_and_poll()` — 作成後、完了までポーリング（デフォルト実装あり）
- `create_stream()` — SSE ストリーミング

### GeminiInteractionsClient

Gemini Interactions API の HTTP 実装。Bearer トークン認証、SSE パーシング。

### MockInteractionsClient

テスト用モック。以下のファクトリメソッドを提供:
- `completed(text)` — 即座に完了を返す
- `with_polling(steps, text)` — 指定ステップ数 InProgress 後に完了
- `failing(error)` — 失敗を返す

### 型定義

| 型 | 説明 |
|---|---|
| `CreateInteractionRequest` | インタラクション作成リクエスト |
| `Interaction` | インタラクションのレスポンス |
| `InteractionStatus` | `InProgress` / `Completed` / `Failed` |
| `InteractionInput` | `Text(String)` |
| `InteractionOutput` | `{ text: String }` |
| `AgentConfig` | `{ agent_type, thinking_summaries }` |
| `ToolConfig` | ツール設定 |
| `StreamEvent` | SSE イベント |
| `StreamDelta` | ストリームの差分データ |
| `StreamEventType` | イベント種別 |
| `ContentPart` | コンテンツパーツ |

### DeepResearchRunnable

```rust
pub struct DeepResearchRunnable { ... }

impl Runnable for DeepResearchRunnable {
    type Input = DeepResearchInput;
    type Output = DeepResearchOutput;
}
```

`InteractionsClient` をラップし、`Runnable` として利用可能にします。

**DeepResearchInput:**
- `query: String` — リサーチクエリ（必須）
- `agent: Option<String>` — エージェント名
- `agent_config: Option<AgentConfig>`
- `tools: Option<Vec<ToolConfig>>`
- `previous_interaction_id: Option<String>` — 前回のインタラクション

**DeepResearchOutput:**
- `interaction_id: String`
- `text: String` — リサーチ結果テキスト
- `status: InteractionStatus`

ビルダーパターン: `DeepResearchRunnable::new(client).with_agent("...").with_poll_interval(duration)`

---

## 6. ayas-examples — 使用例

実際の API プロバイダとの統合例。

| ファイル | 内容 |
|---|---|
| `gemini_chat.rs` | Google Gemini API を使ったチャット |
| `claude_chat.rs` | Anthropic Claude API を使ったチャット |
| `openai_chat.rs` | OpenAI API を使ったチャット |

---

## 設計原則

1. **合成可能性** — すべてが `Runnable` トレイトを実装し、`.pipe()` で接続可能
2. **型安全** — 入出力型の不一致はコンパイル時に検出
3. **Send + Sync** — すべてのコンポーネントがスレッド安全
4. **ステートレス呼び出し** — `ChannelSpec` ファクトリにより、各 `invoke()` が独立した状態を持つ
5. **テストファースト** — 155 テストで品質を保証、clippy clean
6. **ワークスペース依存** — 共通依存はワークスペースレベルで管理
