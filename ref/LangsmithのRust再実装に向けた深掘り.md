# **LangSmith内部アーキテクチャおよびRust再実装のための包括的技術仕様書**

## **1\. 序論：可観測性プラットフォームのRustによる再構築**

大規模言語モデル（LLM）アプリケーションの複雑性が増大する中、LangSmithはLLMオーケストレーションの可観測性（Observability）、評価（Evaluation）、およびデプロイメント（Deployment）を担う中核的なインフラストラクチャとして位置づけられている。本レポートは、LangSmith、LangGraph、LangChainのスタック全体をRustで再実装するという野心的なプロジェクトにおいて、特に**LangSmith**の内部構造、データプロトコル、およびバックエンドアーキテクチャを徹底的に解剖し、高パフォーマンスかつ型安全なRust実装への移行指針を示すものである。

LangSmithは単なるログ収集ツールではない。それは、非決定論的な生成プロセスを持つLLMの挙動を追跡するための分散トレーシングシステムであり、複雑なチェーンやエージェントの実行パス（Run Tree）を可視化し、定量的な評価を行うための分析基盤である 1。既存のPython/TypeScript実装は柔軟性が高い反面、高負荷時のオーバーヘッドや型安全性に課題が残る場合がある。Rustによる再構築は、メモリ安全性の確保、並行処理性能の向上、および低レイテンシなトレーシング処理を実現する上で極めて有効な戦略となる。

本稿では、LangSmithの公開API仕様、SDKのソースコード挙動、およびセルフホスト版のDocker構成から得られた情報を総合し、システムを「データプレーン」「コントロールプレーン」「ストレージ層」「SDKロジック」の4つの層に分解して詳細に分析する。

## ---

**2\. システムアーキテクチャ概観とコンポーネント分析**

LangSmithのアーキテクチャは、大量のトレースデータを高速に受け入れるための\*\*Ingestion Pipeline（データ取り込みパイプライン）**と、蓄積されたデータを分析・管理するための**Application Server（アプリケーションサーバー）\*\*に大別される。これらはマイクロサービスとして疎結合に保たれており、Rustでの再実装においてもこの分離原則を維持することが推奨される。

### **2.1 ハイレベルアーキテクチャ構成**

解析されたDocker ComposeファイルおよびHelmチャートの情報に基づくと、LangSmithの標準的なデプロイメント構成は以下の主要コンポーネント群で成立している 3。

| コンポーネント | 役割と機能要件 | Rust実装における推奨技術スタック |
| :---- | :---- | :---- |
| **LangSmith Backend** (Control Plane) | APIリクエストの認証、プロジェクト管理、アノテーションキューの操作、メタデータのCRUDを担当する。GoおよびPythonで実装されているが、RESTfulなインターフェースを持つ。 | Axum (Webフレームワーク), Tower (ミドルウェア), Jsonwebtoken (認証) |
| **Ingestion Pipeline** (Data Plane) | SDKから送信される大量のRunデータを受け取り、バッファリングし、非同期でデータベースへ書き込む。高スループットと低レイテンシが絶対条件となる。 | Tokio (非同期ランタイム), Rdkafka (Kafka統合), Flume (メモリ内チャネル) |
| **PostgreSQL** | プロジェクト情報、ユーザー、APIキー、アノテーションデータなどのリレーショナルデータを管理する。 | Sqlx (非同期SQLクライアント), Sea-ORM (ORM) |
| **ClickHouse** | トレースデータ（Runs）、ログ、フィードバックなどの時系列データおよび分析対象データを格納する。OLAP（Online Analytical Processing）に最適化されている。 | Clickhouse-rs (ネイティブクライアント) |
| **Redis** | APIのレートリミット管理、短期的なタスクキュー、キャッシュ層として機能する。 | Redis (Fred または redis-rs) |
| **Queue System** | インジェスチョンと書き込みの間のバッファ。セルフホスト版ではRedisまたはメモリ内キューが使われるが、大規模環境ではKafkaが想定される。 | Rdkafka |

### **2.2 データフローの内部メカニズム**

LangSmithにおけるデータのライフサイクルは、クライアントサイド（SDK）での発生から始まり、最終的な永続化に至るまで、以下のような厳密なステップを経る。

1. **Trace Generation (生成)**: アプリケーション内のLLM呼び出しや関数実行が、Runオブジェクトとしてキャプチャされる。この時点でUUID（id）と親ID（parent\_run\_id）が生成され、実行ツリーの構造が決定する。  
2. **Buffering & Batching (SDK側バッファ)**: パフォーマンスへの影響を最小限にするため、SDKは即座に送信を行わず、バックグラウンドスレッドで一定量（デフォルト100件または10MB）または一定時間（1秒）データを蓄積する 6。  
3. **Ingestion (取り込み)**: バッチ化されたデータは POST /runs/batch または POST /runs/multipart エンドポイントを通じてサーバーへ送信される 7。  
4. **Queuing (キューイング)**: サーバーはリクエストを受け取ると、バリデーションを行った後、即座にメッセージキュー（Redis/Kafka）へ投入し、クライアントへは200 OKを返す（Fire-and-Forgetパターン）。  
5. **Processing & Storage (保存)**: ワーカープロセスがキューからデータを取り出し、構造化データをClickHouseへ、メタデータ関係をPostgreSQLへ振り分けて保存する。

## ---

**3\. データモデル詳細解析：Rust構造体へのマッピング**

LangSmithの中核をなすのは「Run」と呼ばれる実行単位のデータモデルである。これはOpenTelemetryのSpanに類似しているが、LLM開発に特化したフィールド（Prompt, Completion, Tokens, Tools）を含んでいる。Rustでの再実装においては、このデータ構造を正確かつ効率的に定義することが不可欠である。

### **3.1 The Run Object (実行モデル)**

Runオブジェクトは、トレーシングの最小単位である。APIドキュメントおよびSDKのスキーマ定義 8 に基づき、Rustにおける最適な構造体定義を以下に提案する。

Rust

use serde::{Deserialize, Serialize};  
use serde\_json::Value;  
use uuid::Uuid;  
use chrono::{DateTime, Utc};  
use std::collections::HashMap;

/// LangSmithにおける実行の基本単位 (Run Tree Node)  
\#  
pub struct Run {  
    /// 一意な識別子 (UUID v4)  
    pub id: Uuid,

    /// 実行の名前 (例: "ChatOpenAI", "RAG Chain")  
    /// 検索やフィルタリングの主要なキーとなる。  
    pub name: String,

    /// 実行開始時刻 (UTC)  
    pub start\_time: DateTime\<Utc\>,

    /// 実行タイプ。enumで厳密に管理することを推奨。  
    pub run\_type: RunType,

    /// 実行終了時刻。ストリーミング中や実行中はNoneとなる。  
    /// 完了時にPATCHリクエストで更新されることが多い。  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub end\_time: Option\<DateTime\<Utc\>\>,

    /// 入力パラメータ (JSONオブジェクト)  
    /// LLMの場合は {"messages": \[...\]}, Chainの場合は {"input":...} など柔軟な構造。  
    \#\[serde(default)\]  
    pub inputs: HashMap\<String, Value\>,

    /// 出力結果 (JSONオブジェクト)  
    /// 実行完了までNoneの場合がある。  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub outputs: Option\<HashMap\<String, Value\>\>,

    /// 親RunのID。ルートRunの場合はNone。  
    /// これにより木構造が形成される。  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub parent\_run\_id: Option\<Uuid\>,

    /// トレース全体のルートRun ID。分散トレーシングの文脈維持に使用される。  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub trace\_id: Option\<Uuid\>,

    /// 階層構造を表現するためのドット区切りの順序文字列 (例: "1.20230101.1")  
    /// DBインデックス最適化のために使用される。  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub dotted\_order: Option\<String\>,

    /// 実行時エラーが発生した場合のエラーメッセージ  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub error: Option\<String\>,

    /// シリアライズされた実行オブジェクト（再実行用）  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub serialized: Option\<HashMap\<String, Value\>\>,

    /// ストリーミングイベントや中間イベントのリスト  
    \#\[serde(default)\]  
    pub events: Vec\<RunEvent\>,

    /// タグのリスト (UIでのフィルタリング用)  
    \#\[serde(default)\]  
    pub tags: Vec\<String\>,

    /// 追加メタデータ、システム情報  
    /// SDKバージョンやプラットフォーム情報はここに格納される。  
    \#\[serde(default)\]  
    pub extra: Option\<HashMap\<String, Value\>\>,

    /// 関連付けられたセッション（プロジェクト）ID  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub session\_id: Option\<Uuid\>,  
      
    /// 参照データセット例のID（評価実行時）  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub reference\_example\_id: Option\<Uuid\>,  
}

\#  
\#\[serde(rename\_all \= "snake\_case")\]  
pub enum RunType {  
    Tool,  
    Chain,  
    Llm,  
    Retriever,  
    Embedding,  
    Prompt,  
    Parser,  
    /// 将来的な拡張に対応するためのフォールバック  
    \#\[serde(untagged)\]  
    Custom(String),  
}

\#  
pub struct RunEvent {  
    pub name: String,  
    pub time: DateTime\<Utc\>,  
    pub kwargs: HashMap\<String, Value\>,  
}

**考察と洞察:**

* **非構造化データの取り扱い**: inputs と outputs は serde\_json::Value （または HashMap）として定義されている。これはLangChainがあらゆる種類のデータ（テキスト、画像、JSON、カスタムオブジェクト）を入出力として扱うためである。Rust側では、これらを可能な限り型安全に扱うために、Message 型（HumanMessage, AIMessageなど）への変換ロジックをTraitとして実装すべきである。  
* **extra フィールドの重要性**: ここには metadata（ユーザー定義）だけでなく、runtime（OS、Python/Rustバージョン）や invocation\_params（LLMのtemperatureやmodel\_name）が含まれる 9。クエリの効率化のため、サーバーサイドではこのJSON内の特定フィールドをClickHouseのMaterialized Columnとして昇格させることが望ましい。

### **3.2 Feedback Object (評価データモデル)**

LangSmithのもう一つの柱は評価（Evaluation）である。フィードバックはRunに対して非同期に付与されるアノテーションであり、以下の構造を持つ 8。

Rust

\#  
pub struct Feedback {  
    pub id: Uuid,  
    pub run\_id: Uuid,  
    pub created\_at: DateTime\<Utc\>,  
    pub modified\_at: DateTime\<Utc\>,  
      
    /// 評価指標のキー (例: "correctness", "latency\_score")  
    pub key: String,  
      
    /// 数値スコア (正規化された値など、集計計算に使用)  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub score: Option\<f64\>,  
      
    /// 任意の評価値 (カテゴリカルデータ、文字列、JSONなど)  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub value: Option\<Value\>,  
      
    /// コメントや理由説明  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub comment: Option\<String\>,  
      
    /// 修正案 (Correction) \- 正解データとして使用可能  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub correction: Option\<Value\>,  
      
    /// フィードバックの発生源 (Model, User, API)  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub feedback\_source: Option\<FeedbackSource\>,  
}

\#  
\#\[serde(tag \= "type", rename\_all \= "snake\_case")\]  
pub enum FeedbackSource {  
    Api { metadata: Option\<HashMap\<String, Value\>\> },  
    Model { metadata: Option\<HashMap\<String, Value\>\> }, // LLM-as-a-Judge  
    User { metadata: Option\<HashMap\<String, Value\>\> },  // 人手によるアノテーション  
}

## ---

**4\. Ingestion Pipeline：APIプロトコルと通信仕様**

SDKとサーバー間の通信プロトコルは、高スループットを実現するために高度に最適化されている。Rustクライアントを実装する際は、これらのエンドポイントの仕様を厳密に遵守する必要がある。

### **4.1 データ投入エンドポイント (POST /runs)**

最もトラフィックが集中する箇所である。単一リクエストでの投入も可能だが、実運用ではバッチ処理が基本となる。

* **Endpoint**: POST /runs  
* **Header**:  
  * x-api-key: 認証キー  
  * Content-Type: application/json  
* **Body**: 上記 Run 構造体のJSON表現。

#### **4.1.1 Batch Ingestion (POST /runs/batch)**

ネットワークラウンドトリップを削減するため、SDKはデフォルトでこのエンドポイントを使用する。リクエストボディは、新規作成（post）と更新（patch）を一度に送信できる構造になっている 7。

Rust

\#  
pub struct BatchRunRequest {  
    /// 新規作成されるRunのリスト  
    pub post: Vec\<Run\>,  
    /// 既存Runに対する更新（Patch）のリスト  
    pub patch: Vec\<RunPatch\>,  
}

\#  
pub struct RunPatch {  
    pub id: Uuid,  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub end\_time: Option\<DateTime\<Utc\>\>,  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub outputs: Option\<HashMap\<String, Value\>\>,  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub error: Option\<String\>,  
    \#\[serde(skip\_serializing\_if \= "Option::is\_none")\]  
    pub events: Option\<Vec\<RunEvent\>\>,  
}

**プロトコルの洞察**:

LangSmithのトレーシングライフサイクルは「Create (POST)」と「Update (PATCH)」に分かれている。

1. **開始時**: start\_time と inputs を持った Run を post リストに入れて送信。  
2. **実行中**: SDKはRun IDをメモリまたはコンテキスト内に保持。  
3. **終了時**: end\_time と outputs を持った RunPatch を patch リストに入れて送信。 この分離により、長時間実行されるLLMタスク（例えば数分かかる生成）でも、開始したという事実（Start）が即座にサーバーに記録され、途中でプロセスがクラッシュしても「未完了のRun」として追跡が可能になる。Rust SDKの実装では、Drop トレイトを活用して、スコープを抜けた際に自動的にPATCHリクエストをキューに入れる設計が推奨される 12。

#### **4.1.2 Multipart Ingestion (POST /runs/multipart)**

さらにスループットを高めるため、Gzip圧縮されたJSONをマルチパートフォームデータとして送信するエンドポイントが存在する 7。

* **メリット**: JSONのパース負荷を軽減し、帯域幅を節約できる。  
* **Rust実装**: reqwest の multipart 機能を使い、データを圧縮ストリームとして送信する。

### **4.2 Query API とフィルタリング**

データの取得には POST /runs/query が使用される。GETパラメータではなくPOSTボディにフィルタ条件を含めることで、複雑なクエリ（例：「トークン数が1000以上」かつ「タグに'prod'を含む」かつ「エラーが存在する」）を表現する。

## ---

**5\. SDK内部ロジックの詳細：クライアントサイドアーキテクチャ**

RustでSDK (langsmith-rust 等) を実装する場合、単にAPIを叩くだけでは不十分である。Python SDKが持っている高度な並行処理、コンテキスト伝播、再試行ロジックを移植する必要がある。

### **5.1 コンテキスト伝播とRunTreeの構築**

トレーシングの核心は、コードの実行フローに合わせて自動的に親子関係（Parent-Child Relationship）を構築することである。

* **課題**: 非同期処理（async/await）において、関数呼び出しをまたいで現在の「親Run ID」をどう引き継ぐか。  
* **Pythonの解法**: contextvars モジュールを使用し、スレッドセーフかつ非同期タスクセーフにコンテキストを保持。  
* **Rustでの解法**: tokio::task\_local\! マクロが最適解である。

Rust

use tokio::task\_local;  
use std::cell::RefCell;  
use uuid::Uuid;

// 現在のRunコンテキストを保持するタスクローカルストレージ  
task\_local\! {  
    static CURRENT\_RUN\_ID: Option\<Uuid\>;  
    static CURRENT\_TRACE\_ID: Option\<Uuid\>;  
    static CURRENT\_RUN\_TREE: RefCell\<Option\<RunTree\>\>; // よりリッチな情報を保持する場合  
}

// 使用イメージ  
async fn trace\_wrapper\<F, Fut\>(name: &str, f: F) \-\> Fut::Output   
where  
    F: FnOnce() \-\> Fut,  
    Fut: Future,  
{  
    let new\_run\_id \= Uuid::new\_v4();  
    // 親IDの取得（存在する場合）  
    let parent\_id \= CURRENT\_RUN\_ID.try\_with(|id| \*id).unwrap\_or(None);  
      
    //... Run作成とPOST処理...

    // 新しいIDをコンテキストにセットして関数を実行  
    CURRENT\_RUN\_ID.scope(Some(new\_run\_id), f()).await  
}

### **5.2 非同期バックグラウンドキュー (Background Queue)**

メインのアプリケーションスレッドをブロックしないために、API送信は完全に分離された非同期タスクで行われる 6。

1. **Producer (生成側)**: トレース関数内。Run オブジェクトを作成し、メモリ内チャネル（flume や tokio::sync::mpsc）に送信する。  
2. **Consumer (送信側)**: バックグラウンドで永続的に動作するループ。  
   * チャネルからデータを取り出し、内部バッファ（Vec\<Run\>）に追加。  
   * **フラッシュ条件**:  
     * バッファサイズが上限（例: 10MB）を超えた場合。  
     * 最後の送信から一定時間（例: 1000ms）が経過した場合 (tokio::time::interval)。  
     * アプリケーション終了シグナルを受け取った場合。  
3. **Backpressure & Dropping**: 送信が追いつかない場合、メモリ枯渇を防ぐために古いトレースを捨てるか、新規トレースを拒否する戦略（Drop Strategy）を実装する。デフォルトでは警告ログを出してドロップするのが一般的である。

### **5.3 リトライロジックと指数バックオフ**

ネットワークの瞬断やAPIの一時的な過負荷（429 Too Many Requests）に対応するため、堅牢なリトライロジックが必要である 13。

* **アルゴリズム**: Exponential Backoff with Jitter（ジッター付き指数バックオフ）。  
  * 待機時間 \= min(max\_delay, initial\_delay \* multiplier^attempt) \+ random\_jitter  
  * 推奨値: initial\_delay=100ms, multiplier=2.0, max\_retries=3 6。  
* **Rust実装**: reqwest-middleware と reqwest-retry クレートを組み合わせることで、宣言的に実装可能である。

## ---

**6\. ストレージ層とクエリエンジン（サーバーサイド実装）**

セルフホスト版LangSmithをRustで実装する場合、データの特性に応じた適切なデータベース選定とスキーマ設計が重要となる。

### **6.1 PostgreSQL: メタデータとリレーション管理**

ACID特性が必要なデータ、整合性が重視されるデータに使用する。

| テーブル名 | 用途 | 主要カラム |
| :---- | :---- | :---- |
| projects | プロジェクト管理 | id (PK), name, created\_at, workspace\_id |
| api\_keys | 認証キー管理 | prefix (PK), hash (SHA256), user\_id, scopes |
| datasets | 評価用データセット | id, name, description |
| examples | データセットの各行 | id, dataset\_id, inputs (JSONB), outputs (JSONB) |
| feedback\_configs | フィードバック定義 | id, project\_id, key, type (continuous/categorical) |

Rustでは sqlx を使用し、非同期かつ型安全にクエリを発行する。JSONB 型へのマッピングには serde\_json を利用する。

### **6.2 ClickHouse: トレースデータの保存と分析**

秒間数千〜数万のトレース書き込みと、巨大なデータセットに対する集計クエリを処理するために、列指向データベースであるClickHouseが採用されている 5。

* **Engine**: MergeTree ファミリー。  
* **Partitioning**: toYYYYMM(start\_time) などで月次パーティションを作成し、古いデータの管理（TTL）を容易にする。  
* **Ordering Key**: (project\_id, start\_time, id)。  
  * ほとんどのクエリは「特定のプロジェクト」の「最新の実行」を取得するため、この順序が最も検索効率が良い。  
* **カラム設計**:  
  * inputs, outputs は文字列化されたJSONとして格納するか、ClickHouseの Map(String, String) 型、あるいは新しい実験的な JSON 型を使用する。  
  * クエリパフォーマンス向上のため、total\_tokens、latency\_ms、status、error などの頻出フィールドは、JSONから抽出して個別のカラム（Materialized Column）として保持すべきである。

## ---

**7\. 評価（Evaluation）とフィードバックサブシステム**

LangSmithの価値の源泉は、蓄積されたトレースデータを用いた評価機能にある。Rustでこのサブシステムを構築する際の要点を述べる。

### **7.1 オンライン評価とオフライン評価**

* **オフライン評価**: 事前に定義された Dataset（入力と期待される出力のペア）に対して、アプリケーション（Chain/Agent）を一括実行し、結果をスコアリングする。  
  * API: client.evaluate() に相当。  
  * 処理フロー: データセット取得 \-\> 並列実行 \-\> 評価関数実行 \-\> 結果集計。  
* **オンライン評価（モニタリング）**: 本番環境で流れてくる実際のトレースに対して、サンプリングまたは全数検査で自動評価を実行する。  
  * これを実現するには、Ingestion Pipelineから分岐したストリーム（例: Kafka topic）を監視し、非同期に評価ロジック（LLM-as-a-Judgeなど）を走らせるコンシューマーが必要になる。

### **7.2 フィードバックAPI**

ユーザーからの「Good/Bad」ボタンや、修正（Correction）を受け付けるAPI。

* **Endpoint**: POST /feedback  
* **Rust実装**: フィードバックはRun IDに紐付くが、Runとは独立したライフサイクルを持つため、ClickHouseの別テーブル（feedbacks）に保存し、クエリ時に JOIN またはアプリケーション側で結合する設計となる。

## ---

**8\. LangGraphおよびLangChainとの統合ポイント**

LangSmithはLangGraph/LangChainと密結合しているように見えるが、実際にはAPIを通じた疎結合な関係である。しかし、Rustでスタック全体を書き直す場合、以下の統合ポイントを意識する必要がある。

* **LangGraphのステート管理**: LangGraphは実行状態（State）を保存・復元する機能（Checkpointer）を持つ。このチェックポイントデータと、LangSmithのトレースデータは概念的に異なるが、リンクされている必要がある。  
  * **戦略**: LangGraphのチェックポイント保存時に、その時点の run\_id をメタデータとして保存し、LangSmith側からそのRun IDを通じてステートを参照できるようにする。  
* **Callback Handler**: LangChain/LangGraphの各ノード実行時にフックされるコールバックシステム。Rust版LangChain (langchain-rust) においては、このコールバックメカニズムが Run オブジェクト生成のトリガーとなる。

## ---

**9\. デプロイメントと運用上の考慮事項**

### **9.1 Docker Composeによる構成**

セルフホスト版の構成 4 を参考に、Rust版サービスのDocker構成を定義する。

YAML

services:  
  langsmith-rust-backend:  
    image: my-rust-langsmith:latest  
    environment:  
      \- DATABASE\_URL=postgres://user:pass@postgres:5432/langsmith  
      \- CLICKHOUSE\_URL=http://clickhouse:8123  
      \- REDIS\_URL=redis://redis:6379  
    ports:  
      \- "1984:1984"  
    depends\_on:  
      \- postgres  
      \- clickhouse  
      \- redis

  postgres:  
    image: postgres:14  
    \#...

  clickhouse:  
    image: clickhouse/clickhouse-server:latest  
    \#...

  redis:  
    image: redis:7  
    \#...

### **9.2 パフォーマンス・チューニング**

* **メモリ管理**: Rustの強みであるが、大量のトレースをバッファリングする際はメモリリークに注意する。jemalloc などのアロケータを使用することでフラグメンテーションを防ぐ。  
* **非同期ランタイム**: tokio のマルチスレッドランタイムを使用し、I/Oバウンドな処理（DB書き込み）とCPUバウンドな処理（JSONパース）を適切にスケジューリングする。

## ---

**10\. 実装ロードマップと推奨Rustライブラリ選定**

最後に、LangSmithをRustで再実装するための具体的なロードマップと、各工程で採用すべきクレート（ライブラリ）を提示する。

### **10.1 フェーズ1: コアSDKとデータモデルの実装**

まずはクライアントサイドのライブラリを作成し、Pythonサーバーに対してデータを送信できる状態を目指す。

* **必須クレート**:  
  * serde, serde\_json: データモデル定義。  
  * reqwest: HTTPクライアント。  
  * tokio: 非同期処理。  
  * uuid: ID生成。  
  * chrono: 日時管理。  
  * flume: 高速なMPMCチャネル（バッファリング用）。  
  * tracing: 自身のログ出力用。

### **10.2 フェーズ2: サーバーサイド（Ingestion API）の実装**

データを受け取り、保存する最小限のサーバーを構築する。

* **必須クレート**:  
  * axum: Webフレームワーク。  
  * clickhouse-rs: ClickHouseドライバ。  
  * sqlx: PostgreSQLドライバ。  
  * tower-http: CORS、Trace、Compressionなどのミドルウェア。

### **10.3 フェーズ3: クエリとUIバックエンドの実装**

保存されたデータを検索し、フロントエンド（別途必要だが、既存のLangSmith UIと互換性を持たせることも理論上可能）に返すAPIを実装する。

## **結論**

LangSmithのRustによる再構築は、単なる言語の置き換え以上の意味を持つ。それは、**LLMアプリケーションのための高信頼・低レイテンシな専用テレメトリ基盤**を構築する試みである。Python版SDKが抱えるGC停止やスレッド競合のリスクを排除し、GoやJavaで書かれたバックエンドと同等以上のスループットを、より少ないリソースで実現できる可能性が高い。

本レポートで示したデータモデル（Run/Feedback）、APIプロトコル（Batch/Multipart）、およびアーキテクチャ設計（Postgres/ClickHouseハイブリッド）は、その実現のための確固たる設計図となるものである。開発者はまず、Run 構造体の厳密な型定義と、tokio をベースとした堅牢なインジェスチョンパイプラインの構築から着手すべきである。

#### **引用文献**

1. LangSmith Explained: Debugging and Evaluating LLM Agents | DigitalOcean, 1月 31, 2026にアクセス、 [https://www.digitalocean.com/community/tutorials/langsmith-debudding-evaluating-llm-agents](https://www.digitalocean.com/community/tutorials/langsmith-debudding-evaluating-llm-agents)  
2. LangSmith \- Observability \- LangChain, 1月 31, 2026にアクセス、 [https://www.langchain.com/langsmith/observability](https://www.langchain.com/langsmith/observability)  
3. Self-host LangSmith with Docker \- Docs by LangChain, 1月 31, 2026にアクセス、 [https://docs.langchain.com/langsmith/docker](https://docs.langchain.com/langsmith/docker)  
4. helm/charts/langsmith/docker-compose/docker-compose.yaml at ..., 1月 31, 2026にアクセス、 [https://github.com/langchain-ai/helm/blob/main/charts/langsmith/docker-compose/docker-compose.yaml](https://github.com/langchain-ai/helm/blob/main/charts/langsmith/docker-compose/docker-compose.yaml)  
5. Upgrade an installation \- Docs by LangChain, 1月 31, 2026にアクセス、 [https://docs.langchain.com/langsmith/self-host-upgrades](https://docs.langchain.com/langsmith/self-host-upgrades)  
6. Beta LangSmith Collector-Proxy \- Docs by LangChain, 1月 31, 2026にアクセス、 [https://docs.langchain.com/langsmith/collector-proxy](https://docs.langchain.com/langsmith/collector-proxy)  
7. Cloud \- Docs by LangChain, 1月 31, 2026にアクセス、 [https://docs.langchain.com/langsmith/cloud](https://docs.langchain.com/langsmith/cloud)  
8. Schemas (LangSmith) | LangChain Reference, 1月 31, 2026にアクセス、 [https://reference.langchain.com/python/langsmith/observability/sdk/schemas/](https://reference.langchain.com/python/langsmith/observability/sdk/schemas/)  
9. Dataset transformations \- Docs by LangChain, 1月 31, 2026にアクセス、 [https://docs.langchain.com/langsmith/dataset-transformations](https://docs.langchain.com/langsmith/dataset-transformations)  
10. Interface Feedback \- langchain.js, 1月 31, 2026にアクセス、 [https://reference.langchain.com/javascript/interfaces/langsmith.schemas.Feedback.html](https://reference.langchain.com/javascript/interfaces/langsmith.schemas.Feedback.html)  
11. langsmith-cookbook/tracing-examples/rest/rest.ipynb at main ..., 1月 31, 2026にアクセス、 [https://github.com/langchain-ai/langsmith-cookbook/blob/main/tracing-examples/rest/rest.ipynb](https://github.com/langchain-ai/langsmith-cookbook/blob/main/tracing-examples/rest/rest.ipynb)  
12. langsmith-rust \- Lib.rs, 1月 31, 2026にアクセス、 [https://lib.rs/crates/langsmith-rust](https://lib.rs/crates/langsmith-rust)  
13. \[FEATURE\] Add retry logic with exponential backoff · Issue \#2018 \- GitHub, 1月 31, 2026にアクセス、 [https://github.com/langchain4j/langchain4j/issues/2018](https://github.com/langchain4j/langchain4j/issues/2018)  
14. Built-in middleware \- Docs by LangChain, 1月 31, 2026にアクセス、 [https://docs.langchain.com/oss/javascript/langchain/middleware/built-in](https://docs.langchain.com/oss/javascript/langchain/middleware/built-in)  
15. Function toolRetryMiddleware \- LangChain Docs, 1月 31, 2026にアクセス、 [https://reference.langchain.com/javascript/functions/langchain.index.toolRetryMiddleware.html](https://reference.langchain.com/javascript/functions/langchain.index.toolRetryMiddleware.html)  
16. ClickHouse (self-hosted) \- Langfuse, 1月 31, 2026にアクセス、 [https://langfuse.com/self-hosting/deployment/infrastructure/clickhouse](https://langfuse.com/self-hosting/deployment/infrastructure/clickhouse)