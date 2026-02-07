# **LangChainアーキテクチャの深層解剖とRust移植に向けた技術仕様書**

## **1\. 序論：動的オーケストレーションから静的安全性へのパラダイムシフト**

本レポートは、PythonベースのLLMアプリケーション開発フレームワークであるLangChainの内部構造を徹底的に解剖し、その機能をRust言語で再実装するための詳細な技術仕様とアーキテクチャ設計を提供するものである。LangChainのエコシステムは、LangSmith（可観測性）、LangGraph（エージェントのグラフ制御）、そしてコアライブラリであるLangChain自体から構成されるが、本稿ではコアライブラリに焦点を絞り、その構成要素であるLCEL（LangChain Expression Language）、モデルI/O、エージェント実行ループ、ツール、そしてコールバックシステムの内部ロジックを分析する。

PythonによるLangChainの実装は、同言語の柔軟性（動的型付け、モンキーパッチ、実行時イントロスペクション）に大きく依存している。これに対し、Rustへの移植は、これらの動的なメカニズムを、所有権システム、トレイト境界、静的型付けといったRustの厳格な安全性の中にマッピングするという、根本的なパラダイムシフトを要求する 1。特に、Pythonの「何でも受け入れる」辞書型（dict）を中心としたデータフローから、Rustの厳密な型定義（StructやEnum）への移行は、設計上の最大の課題となる。

本分析では、単なるコードの翻訳ではなく、LangChainが提供する抽象化の本質的な「意図」を汲み取り、それをRustのイディオム（慣用句）を用いて、より高性能かつ堅牢な形で再構築するための指針を示す。具体的には、Runnableプロトコルにおける型消去（Type Erasure）と静的ディスパッチのトレードオフ、非同期実行時のコンテキスト伝播の設計、そしてエージェントの推論ループにおける状態管理の厳密化について詳述する。

## ---

**2\. Runnableプロトコル（LCEL）の内部構造とRust実装戦略**

現在のLangChainの根幹を成すのが、**LangChain Expression Language (LCEL)** であり、その基盤となるのが Runnable プロトコルである。これは、プロンプト、モデル、出力パーサーといった異なるコンポーネントを統一的なインターフェースで扱い、パイプ演算子（|）を用いて宣言的にチェーンを構成するための仕組みである。

### **2.1 Pythonにおける Runnable の内部ロジック**

Pythonにおける Runnable は、ジェネリクスを用いた抽象基底クラス Runnable\[Input, Output\] として定義されている。このクラスは、すべてのLCELコンポーネントが実装すべき標準的なメソッド群を規定している 2。

#### **2.1.1 実行メソッド群の仕様**

Runnable が提供する主要なメソッドは以下の通りである。

* **invoke(input, config)**: 同期実行のエントリーポイント。単一の入力を受け取り、変換された出力を返す。config 引数を通じて、コールバックや実行時パラメータを受け渡す。  
* **ainvoke(input, config)**: 非同期実行のエントリーポイント。Pythonの asyncio を利用し、I/O待ちの間に他のタスクを実行可能にする。  
* **batch(inputs, config)**: リスト形式の入力を並列処理する。デフォルト実装では ThreadPoolExecutor を用いて invoke を並列化するが、多くのLLMプロバイダ向けの実装では、APIのバッチエンドポイントを利用するようにオーバーライドされている 3。  
* **stream(input, config)**: 出力をチャンク（断片）として順次生成するイテレータを返す。これにより、LLMのトークン生成ごとのリアルタイム表示が可能となる。

#### **2.1.2 構成メカニズム（RunnableSequence と演算子オーバーロード）**

LCELの最大の特徴である prompt | model | parser という構文は、Pythonの \_\_or\_\_ マジックメソッドのオーバーロードによって実現されている 3。

1. **演算子の評価**: ユーザーが A | B を記述すると、A.\_\_or\_\_(B) が呼び出される。  
2. **シーケンスの構築**: このメソッドは RunnableSequence オブジェクトを返す。この際、A が既に RunnableSequence である場合は、その内部リストに B を追加するという「平坦化（Flattening）」処理が行われる。これにより、((A | B) | C) のようなネストした構造が \`\` というフラットなリストとして保持され、実行時の再帰呼び出しによるオーバーヘッドを削減している 3。  
3. **データフローの制御**: RunnableSequence.invoke が呼び出されると、内部リストの各ステップが順次実行される。ステップ ![][image1] の出力がステップ ![][image2] の入力として渡される。

| 特徴 | Python (LangChain) | Rust (提案) |
| :---- | :---- | :---- |
| **合成方法** | \_\_or\_\_ (実行時にオブジェクト生成) | BitOr トレイト (コンパイル時に型合成) |
| **型チェック** | 実行時 (Duck Typing) | コンパイル時 (Associated Types) |
| **データ構造** | リスト List | ネストされた構造体 Sequence\<A, B\> |
| **並列実行** | RunnableParallel (dict ベース) | マクロまたは構造体による join\! |

#### **2.1.3 RunnableConfig の伝播とイントロスペクション**

LangChainの隠れた複雑性は、RunnableConfig の伝播にある。この設定オブジェクトには、コールバックハンドラ（callbacks）、タグ（tags）、メタデータ（metadata）、再帰制限（recursion\_limit）などが含まれる 5。

Python実装では、RunnableSequence が各ステップを呼び出す際、単に入力を渡すだけでなく、この config オブジェクトも伝播させる必要がある。しかし、すべての関数やRunnableが config を引数として受け取るわけではない（例えば、単純な lambda x: x \+ 1 など）。

ここでLangChainは、Pythonの inspect モジュールを用いた**実行時イントロスペクション**を行っている。RunnableLambda などは、ラップする関数のシグネチャを検査し、config という名前の引数が存在する場合にのみ、設定オブジェクトを注入する 6。この動的な依存性注入は、ユーザーがボイラープレートコード（定型句）を書かずに済む利便性を提供する反面、静的解析を困難にしている。

### **2.2 Rustにおける再実装戦略**

Runnable プロトコルをRustに移植する際、最大の障壁となるのは「異種型の連鎖（Heterogeneous Chain）」の問題である。プロンプト（Mapを受け取りStringを返す）、モデル（Stringを受け取りMessageを返す）、パーサー（Messageを受け取りStructを返す）は、それぞれ全く異なる入出力型を持つ。これらを一つの「チェーン」として表現するには、高度な型システムの設計が必要となる。

#### **2.2.1 トレイト定義と関連型（Associated Types）**

Rustではジェネリクスではなく、関連型（Associated Types）を持つトレイトとして Runnable を定義することが推奨される。これにより、入力型と出力型の関係をコンパイル時に確定させることができる。

Rust

use async\_trait::async\_trait;  
use serde\_json::Value;

// コンフィグ構造体の定義（PythonのRunnableConfigに相当）  
\#  
pub struct RunnableConfig {  
    pub callbacks: Vec\<Arc\<dyn CallbackHandler\>\>,  
    pub tags: Vec\<String\>,  
    pub metadata: HashMap\<String, Value\>,  
    pub recursion\_limit: usize,  
}

// Runnableトレイトの定義  
\#\[async\_trait\]  
pub trait Runnable: Send \+ Sync {  
    type Input;  
    type Output;  
    type Error;

    async fn invoke(&self, input: Self::Input, config: \&RunnableConfig) \-\> Result\<Self::Output, Self::Error\>;  
}

この定義により、Runnable を実装する各構造体は、自身の入力型と出力型を厳密に宣言する必要がある。例えば、PromptTemplate は Input=HashMap\<String, String\>, Output=String となる。

#### **2.2.2 静的ディスパッチによるチェーン合成**

Pythonの | 演算子による柔軟な合成を再現するために、Rustでは std::ops::BitOr トレイトを実装する。ここで重要なのは、**前のステップの出力型が、次のステップの入力型と一致していること**を where 節で制約として課すことである。

Rust

pub struct RunnableSequence\<First, Second\> {  
    first: First,  
    second: Second,  
}

impl\<First, Second\> Runnable for RunnableSequence\<First, Second\>  
where  
    First: Runnable,  
    Second: Runnable\<Input \= First::Output\>, // 型の一致を強制  
{  
    type Input \= First::Input;  
    type Output \= Second::Output;  
    type Error \= Box\<dyn std::error::Error \+ Send \+ Sync\>;

    async fn invoke(&self, input: Self::Input, config: \&RunnableConfig) \-\> Result\<Self::Output, Self::Error\> {  
        let first\_out \= self.first.invoke(input, config).await.map\_err(|e| e.into())?;  
        self.second.invoke(first\_out, config).await.map\_err(|e| e.into())  
    }  
}

このアプローチ（静的ディスパッチ）の利点は、コンパイラがチェーン全体を単一の関数として最適化（インライン展開など）できるため、実行時のオーバーヘッドが極小化される点にある 7。一方で、チェーンが長くなると型定義が RunnableSequence\<RunnableSequence\<A, B\>, C\> のように深くネストし、型エラーが難解になる欠点がある。

#### **2.2.3 動的ディスパッチと型消去（Type Erasure）**

アプリケーションによっては、ユーザーの設定ファイルに基づいて実行時にチェーンを構築する必要がある。この場合、コンパイル時に型が決定する静的ディスパッチは使用できない。ここで必要となるのが、**トレイトオブジェクト（Trait Object）** を用いた動的ディスパッチである。

しかし、Rustの非同期トレイトメソッドは、その戻り値が impl Future であるため、そのままではオブジェクト安全（Object Safe）ではない 8。これを解決するには、async-trait クレートを使用するか、手動で Pin\<Box\<dyn Future...\>\> を返すように設計する必要がある。

さらに、入力型と出力型が異なるコンポーネントを同一の Vec\<Box\<dyn Runnable\>\> に格納することはできない。これを解決するには、以下の2つのアプローチが考えられる。

1. **Enumによるラップ**: AnyRunnable というEnumを定義し、その中に Prompt, Model, Parser などのバリアントを持たせる。これにより単一の型として扱えるが、拡張性は損なわれる。  
2. **汎用型（serde\_json::Value）の使用**: 入出力をすべて serde\_json::Value に統一した DynamicRunnable トレイトを別途定義し、アダプター層を設ける。これは実行時のオーバーヘッドを生むが、Pythonのような柔軟性を提供する。

#### **2.2.4 ストリーミングの実装：Generator vs Async Iterator**

Pythonの stream メソッドはジェネレータ（yield）を使用している。Rustでは、futures::stream::Stream トレイトがこれに相当する。

Pythonの JsonOutputParser におけるストリーミング処理は、部分的なJSON文字列を受け取りながら、可能な限りパースして差分（パッチ）を生成するという非常に複雑なロジックを持っている 9。Rustでこれを再現するには、単なる文字列操作ではなく、**ストリーミング対応のJSONパーサー**（例えば struson クレートなど）を利用したステートマシンとして実装する必要がある。不完全なトークンが来た場合にエラーとせず、バッファリングして次のトークンを待つという制御フローが不可欠である。

## ---

**3\. モデルI/Oの抽象化：メッセージとチャットモデル**

LangChainの核となるのは、多様なLLMプロバイダ（OpenAI, Anthropic, Google Vertex AIなど）を統一的に扱うための抽象化層である。これには、入出力データの形式である「メッセージ」と、実行主体である「チャットモデル」が含まれる。

### **3.1 Pythonにおける BaseMessage と BaseChatModel**

#### **3.1.1 メッセージ階層構造**

Pythonでは、クラス継承を用いてメッセージの種類を表現している 11。

* **BaseMessage**: 基底クラス。content（内容）、additional\_kwargs（プロバイダ固有の追加情報）、response\_metadata（トークン使用量など）を持つ。  
* **HumanMessage**: ユーザーからの入力。  
* **AIMessage**: AIからの応答。tool\_calls（ツール呼び出し要求）を含む点が重要である。  
* **SystemMessage**: システムプロンプト。  
* **ToolMessage**: ツールの実行結果。tool\_call\_id を持ち、どの呼び出しに対する応答かを紐付ける。

LangChain 0.1以降、AIMessage には tool\_calls という標準化された属性が追加され、プロバイダごとの独自の関数呼び出し形式（OpenAIの function\_call やAnthropicの tool\_use など）が統一された形式に変換されるようになった 13。

#### **3.1.2 チャットモデルの実行フロー**

BaseChatModel は、共通のロジック（コールバックの発火、キャッシュの確認、タグの付与）を invoke メソッドで処理し、実際のAPI呼び出しは抽象メソッド \_generate に委譲するテンプレートメソッドパターンを採用している 15。

Python

\# Pythonの概念コード  
class BaseChatModel(Runnable):  
    def invoke(self, input, config):  
        \# 1\. コールバックマネージャの設定  
        run\_manager \= callback\_manager.on\_chat\_model\_start(...)  
          
        \# 2\. キャッシュの確認  
        if result := self.check\_cache(input):  
            return result  
              
        \# 3\. 実際の生成（サブクラスで実装）  
        result \= self.\_generate(input, run\_manager)  
          
        \# 4\. 終了コールバック  
        run\_manager.on\_chat\_model\_end(result)  
        return result

### **3.2 Rustによる型安全な実装：EnumとSerde**

Rustにおいて、メッセージの種類をクラス継承で表現するのは不適切である。メッセージの種類は有限かつ既知であるため、**Enum（列挙型）** を用いるのが最もRustらしい設計であり、メモリ効率も高い。

#### **3.2.1 Message Enumの設計**

Rust

use serde::{Deserialize, Serialize};

\#  
\#\[serde(tag \= "type", content \= "data")\]  
pub enum Message {  
    System(String),  
    User(UserContent),  
    AI(AIContent),  
    Tool(ToolResult),  
}

\#  
pub enum UserContent {  
    Text(String),  
    Multimodal(Vec\<ContentBlock\>), // 画像などをサポート  
}

\#  
pub struct AIContent {  
    pub content: Option\<String\>,  
    pub tool\_calls: Vec\<ToolCall\>,  
    pub usage: Option\<UsageMetadata\>,  
}

\#  
pub struct ToolCall {  
    pub id: String,  
    pub name: String,  
    pub args: serde\_json::Value, // 引数は動的なJSON  
}

このようにEnumを使用することで、match 式を用いた網羅的なパターンマッチングが可能となり、「未知のメッセージタイプが来た場合の処理漏れ」といったバグをコンパイル時に防ぐことができる。

#### **3.2.2 ChatModel トレイトとミドルウェアパターン**

Rustでは継承ができないため、Pythonの BaseChatModel のような「基底クラスによる共通処理の強制」は難しい。代わりに、**コンポジション（包含）** または **デコレータパターン** を用いる。

1. **Core Trait**: 純粋な生成ロジックのみを持つ ChatModel トレイトを定義する。  
2. **Middleware Wrappers**: キャッシュやトレーシング機能を持つラッパー構造体を定義する。

Rust

// コアとなるモデルのインターフェース  
\#\[async\_trait\]  
pub trait ChatModel: Send \+ Sync {  
    async fn generate(&self, messages: &\[Message\], options: \&CallOptions) \-\> Result\<ChatResult, Error\>;  
}

// トレーシング機能を追加するラッパー  
pub struct TracedModel\<M: ChatModel\> {  
    inner: M,  
}

impl\<M: ChatModel\> ChatModel for TracedModel\<M\> {  
    async fn generate(&self, messages: &\[Message\], options: \&CallOptions) \-\> Result\<ChatResult, Error\> {  
        // ここでon\_llm\_start相当の処理  
        tracing::info\_span\!("llm\_generate", model \=?self.inner).in\_scope(|| async {  
            self.inner.generate(messages, options).await  
        }).await  
    }  
}

この設計により、機能の追加・削除が型レベルで柔軟に行えるようになり、単一継承の制約から解放される。

## ---

**4\. エージェント実行エンジン：推論ループと状態管理**

LangChainのエージェント機能の中核を担うのが AgentExecutor である。これは、LLMを用いた推論（Reasoning）とツール実行（Action）のループを管理するランタイムである。

### **4.1 Pythonにおける AgentExecutor の詳細ロジック**

AgentExecutor の \_call メソッドには、ReAct（Reason \+ Act）パターンを実現するための制御ループが実装されている 16。

#### **4.1.1 実行ループのステートマシン**

1. **初期化**: intermediate\_steps（中間ステップの履歴）を空リストで初期化。iterations カウンタを0に設定。  
2. **プランニング（Plan）**: エージェント（通常はRunnableチェーン）を呼び出す。入力は「ユーザーのクエリ」＋「これまでの intermediate\_steps」。  
3. **出力の解析**: エージェントの出力を受け取り、以下のいずれか（Union型）に分類する。  
   * **AgentFinish**: 最終回答が得られた場合。ループを終了し、結果を返す。  
   * **AgentAction**: ツールを使用すべきと判断された場合。ツール名と引数を持つ。  
4. **ツール実行（Execute）**:  
   * AgentAction に含まれるツール名を self.tools マップから検索。  
   * ツールを実行し、結果（Observation）を取得。  
   * intermediate\_steps に (AgentAction, Observation) のタプルを追加。  
5. **反復**: ループの先頭に戻り、更新された intermediate\_steps を用いて再度プランニングを行う。

#### **4.1.2 エッジケースの処理**

* **最大反復回数（Max Iterations）**: ループが無限に続くのを防ぐため、max\_iterations に達すると強制終了する。この際、early\_stopping\_method に応じて、「強制的に終了」するか、「最後にもう一度LLMを呼び出して結論を出させる（generate）」かを選択できる 18。  
* **パースエラー**: LLMが不正なフォーマットを出力した場合、例外が発生する。handle\_parsing\_errors=True の場合、Executorはこの例外を捕捉し、「フォーマットエラーが発生しました。修正してください」という指示をObservationとしてLLMに送り返すことで、自己修正（Self-Correction）を促す 20。

### **4.2 Rustによる実装：厳密なステートマシン**

Pythonの while ループはシンプルだが、状態変数の変更が暗黙的になりがちである。Rustでは、状態遷移をEnumで明示的に表現することで、より堅牢な実装が可能となる。

#### **4.2.1 ステップ結果のEnum化**

Rust

pub enum StepResult {  
    Action(AgentAction),  
    Finish(AgentFinish),  
}

pub struct AgentAction {  
    pub tool: String,  
    pub tool\_input: serde\_json::Value,  
    pub log: String,  
}

pub struct AgentFinish {  
    pub return\_values: HashMap\<String, Value\>,  
    pub log: String,  
}

#### **4.2.2 実行ループの実装**

Rustの非同期ランタイム（Tokio）を活用することで、特にツール実行フェーズにおける並列性を高めることができる。

Rust

impl\<A: Agent\> AgentExecutor\<A\> {  
    pub async fn invoke(&self, input: String) \-\> Result\<String, AgentError\> {  
        let mut steps \= Vec::new();  
        let mut iterations \= 0;

        while iterations \< self.max\_iterations {  
            // プランニングステップ  
            let step\_result \= self.agent.plan(\&input, \&steps).await?;  
              
            match step\_result {  
                StepResult::Finish(finish) \=\> return Ok(finish.return\_values\["output"\].to\_string()),  
                StepResult::Action(action) \=\> {  
                    // ツールの検索  
                    let tool \= self.tools.get(\&action.tool)  
                       .ok\_or\_else(|| AgentError::ToolNotFound(action.tool.clone()))?;  
                      
                    // ツールの実行（エラーハンドリングを含む）  
                    let observation \= match tool.call(\&action.tool\_input).await {  
                        Ok(output) \=\> output,  
                        Err(e) \=\> {  
                            if self.handle\_parsing\_errors {  
                                format\!("Error executing tool: {}", e) // 自己修正用フィードバック  
                            } else {  
                                return Err(AgentError::ToolError(e));  
                            }  
                        }  
                    };  
                      
                    steps.push((action, observation));  
                }  
            }  
            iterations \+= 1;  
        }  
        Err(AgentError::MaxIterationsReached)  
    }  
}

#### **4.2.3 並列ツール実行による高速化**

OpenAIの新しいモデルなどは、一度に複数のツール呼び出し（Parallel Tool Calls）を返すことができる。Pythonの AgentExecutor は標準ではこれらをシーケンシャルに処理するか、スレッドプールを用いて並列化する 22。

Rustでは、futures::stream::iter と buffer\_unordered を用いることで、スレッドのオーバーヘッドなしに、軽量なタスク（Green Threads）として多数のツール呼び出しを真に並列実行できる。これは、API待ち時間が支配的なエージェント処理において、劇的なパフォーマンス向上をもたらす。

## ---

**5\. ツール定義とスキーマ検証**

ツールは、LLMが外部世界（検索エンジン、データベース、API）と対話するためのインターフェースである。

### **5.1 Python内部ロジック：Pydanticによる動的検証**

Pythonの BaseTool は Pydantic の BaseModel を継承している 23。

* **スキーマ推論**: ツールの引数定義（型ヒント）から、自動的に JSON Schema が生成される。これがLLMへのプロンプト（Function Calling定義）として使用される。  
* **実行時検証**: ツールが呼び出されると、渡された辞書型の引数は Pydantic によって検証される。型が合わない場合、実行前に ValidationError が発生する 21。

### **5.2 Rust実装：schemars とコンパイル時スキーマ生成**

Rustには実行時のイントロスペクションがないため、Pydanticのように関数の引数定義から動的にスキーマを生成することはできない。代わりに、マクロとコード生成を活用する。

#### **5.2.1 schemars クレートの活用**

Rustの構造体から JSON Schema を生成するための標準的なライブラリである schemars を使用する。

Rust

use schemars::JsonSchema;  
use serde::Deserialize;

// ツールの入力引数を定義する構造体  
\#  
struct CalculatorInput {  
    expression: String,  
}

\#\[async\_trait\]  
impl Tool for CalculatorTool {  
    fn name(&self) \-\> &str { "calculator" }  
      
    // スキーマ情報の提供  
    fn parameters(&self) \-\> serde\_json::Value {  
        schemars::schema\_for\!(CalculatorInput).into()  
    }

    // 実行ロジック  
    async fn call(&self, input: serde\_json::Value) \-\> Result\<String, ToolError\> {  
        // ここで自動的にバリデーションが行われる（デシリアライズ失敗＝バリデーションエラー）  
        let args: CalculatorInput \= serde\_json::from\_value(input)?;  
        let result \= eval(\&args.expression);  
        Ok(result.to\_string())  
    }  
}

このアプローチにより、PythonのPydanticが実行時に行っていたスキーマ生成とバリデーションを、Rustの強力な型システムと serde エコシステムに自然に統合できる。デシリアライズの成功は、入力がスキーマに適合していることを保証するため、メソッド内部では型安全な args を安心して使用できる。

## ---

**6\. コールバックシステムと可観測性（Observability）**

LangChainの最大の強みの一つが、実行のトレース（追跡）を可能にする強力なコールバックシステムである。これはLangSmithなどのモニタリングツールと連携する基盤となる。

### **6.1 Python内部ロジック：CallbackManager と階層構造**

* **ツリー構造の形成**: CallbackManager は実行時に親子関係を形成する。チェーン（親）がツール（子）を呼び出す際、親の run\_id を継承した子マネージャが生成される 25。  
* **ContextVarsの使用**: 非同期実行において、スレッドやタスクをまたいで CallbackManager を伝播させるために、Python 3.7以降の contextvars が利用されることがある 27。しかし、明示的に config 引数として渡すことが推奨されており、これが RunnableConfig の伝播ロジックと密接に関わっている。

### **6.2 Rust実装：tracing エコシステムの採用**

Rustには tracing というデファクトスタンダードの計測ライブラリが存在する。LangChain独自のコールバックシステムを再発明するのではなく、tracing の仕組みに乗ることがRustにおける最適解である。

| 機能 | LangChain (Python) | Rust (tracing) |
| :---- | :---- | :---- |
| **実行単位** | Run オブジェクト | Span (スパン) |
| **コンテキスト伝播** | CallbackManager の受け渡し / contextvars | tokio のタスクローカルストレージ (TLS) |
| **データ出力** | CallbackHandler (APIコールなど) | Subscriber / Layer |

#### **6.2.1 LangChainLayer の実装**

Rust版LangChainでは、すべての invoke 呼び出しを tracing::instrument マクロで装飾する。そして、tracing-subscriber のレイヤーとして LangChainLayer を実装し、スパンの開始・終了イベントをフックしてLangSmithのAPIにデータを送信する。

Rust

// 擬似コード：トレーシングの実装  
\#\[tracing::instrument(name \= "ChainInvoke", skip(self, config), fields(run\_id))\]  
async fn invoke(&self, input: Input, config: \&Config) \-\> Result\<Output, Error\> {  
    // 実行ロジック...  
}

この設計により、ユーザーは config オブジェクトを手動でバケツリレーのように渡す必要がなくなり（tokio が暗黙的にコンテキストを伝播してくれるため）、APIが大幅にクリーンになる。これは、Python版で頻発する「コンテキスト伝播漏れによるトレースの分断」問題 27 を根本的に解決する。

## ---

**7\. ベクトルストアと非同期I/O**

RAG（検索拡張生成）の要となるのが、埋め込みベクトルを管理するベクトルストアである。

### **7.1 Pythonの非同期Shim問題**

多くのPythonベクトルライブラリ（FAISSや古いバージョンのクライアント）は同期的である。そのため、LangChainの adelete や aadd\_documents といった非同期メソッドの実体は、同期メソッドを loop.run\_in\_executor（スレッドプール）でラップしただけの「偽の非同期（Async Shim）」であることが多い 22。これはGIL（Global Interpreter Lock）の影響を受け、CPUバウンドな処理（埋め込み計算など）において並列性が制限される要因となる。

### **7.2 Rustによるネイティブ非同期実装**

Rustのデータベースドライバ（sqlx for Postgres, qdrant-client, mongodb）は、最初から非同期（Async-First）で設計されている。

Rust

\#\[async\_trait\]  
pub trait VectorStore: Send \+ Sync {  
    async fn add\_documents(&self, docs: &) \-\> Result\<Vec\<String\>, VectorStoreError\>;  
    async fn similarity\_search(&self, query: &str, k: usize) \-\> Result\<Vec\<Document\>, VectorStoreError\>;  
}

Rust実装では、スレッドプールへのオフロードではなく、真の非同期I/Oを活用できるため、大量のドキュメントのインジェスト（取り込み）や並列検索において、Python版を圧倒するスループットを実現できる。また、Send \+ Sync 境界により、スレッドセーフであることがコンパイル時に保証されるため、Webサーバーへの組み込みも容易である。

## ---

**8\. 結論と提言**

LangChainをRustで書き直すという試みは、単なる言語の置き換えにとどまらず、アプリケーションのアーキテクチャをより堅牢で高性能なものへと進化させる機会である。

1. **静的型付けの恩恵**: メッセージやエージェントの状態をEnumで定義することで、実行時エラーを激減させることができる。  
2. **並列性の最大化**: PythonのGILやスレッドプールの制約から解放され、Tokioランタイムによる数千規模の並列タスク実行が可能となる。  
3. **エコシステムとの調和**: 独自のコールバックシステムではなく tracing を、独自のバリデーションではなく serde/schemars を採用することで、Rustの豊かなエコシステムとシームレスに連携できる。

Python版LangChainが提供していた「柔軟性」の一部（動的なチェーン構築など）は、Rustの厳格さとトレードオフになるが、本番環境における信頼性とパフォーマンスを重視するシステムにおいては、Rust版LangChain（仮称 langchain-rs）は極めて強力な基盤となるだろう。

#### **引用文献**

1. Rust vs Python for AI: Is Rig better than Langchain? \- YouTube, 1月 31, 2026にアクセス、 [https://www.youtube.com/watch?v=cyZXVgzy7DA](https://www.youtube.com/watch?v=cyZXVgzy7DA)  
2. LCEL Interface | LangChain OpenTutorial \- GitBook, 1月 31, 2026にアクセス、 [https://langchain-opentutorial.gitbook.io/langchain-opentutorial/01-basic/07-lcel-interface](https://langchain-opentutorial.gitbook.io/langchain-opentutorial/01-basic/07-lcel-interface)  
3. Runnables | LangChain Reference \- LangChain Docs, 1月 31, 2026にアクセス、 [https://reference.langchain.com/python/langchain\_core/runnables/](https://reference.langchain.com/python/langchain_core/runnables/)  
4. Why the Pipe Character “|” Works in LangChain's LCEL | by Michael Hashimoto \- Medium, 1月 31, 2026にアクセス、 [https://medium.com/@MichaelHashimoto/why-the-pipe-character-works-in-langchains-lcel-b4e8685855f5](https://medium.com/@MichaelHashimoto/why-the-pipe-character-works-in-langchains-lcel-b4e8685855f5)  
5. RunnableConfig — LangChain documentation, 1月 31, 2026にアクセス、 [https://reference.langchain.com/v0.3/python/core/runnables/langchain\_core.runnables.config.RunnableConfig.html](https://reference.langchain.com/v0.3/python/core/runnables/langchain_core.runnables.config.RunnableConfig.html)  
6. Langchain: How to let an AgentExecutor Propagate RunnableConfig\["configurable"\] to @tool? \- Stack Overflow, 1月 31, 2026にアクセス、 [https://stackoverflow.com/questions/79773741/langchain-how-to-let-an-agentexecutor-propagate-runnableconfigconfigurable](https://stackoverflow.com/questions/79773741/langchain-how-to-let-an-agentexecutor-propagate-runnableconfigconfigurable)  
7. Rust Static vs. Dynamic Dispatch \- SoftwareMill, 1月 31, 2026にアクセス、 [https://softwaremill.com/rust-static-vs-dynamic-dispatch/](https://softwaremill.com/rust-static-vs-dynamic-dispatch/)  
8. Trait \+ async function that returns another trait \- The Rust Programming Language Forum, 1月 31, 2026にアクセス、 [https://users.rust-lang.org/t/trait-async-function-that-returns-another-trait/104975](https://users.rust-lang.org/t/trait-async-function-that-returns-another-trait/104975)  
9. Output parsers | LangChain Reference, 1月 31, 2026にアクセス、 [https://reference.langchain.com/python/langchain\_core/output\_parsers/](https://reference.langchain.com/python/langchain_core/output_parsers/)  
10. langchain/libs/core/langchain\_core/output\_parsers/json.py at master \- GitHub, 1月 31, 2026にアクセス、 [https://github.com/langchain-ai/langchain/blob/master/libs/core/langchain\_core/output\_parsers/json.py](https://github.com/langchain-ai/langchain/blob/master/libs/core/langchain_core/output_parsers/json.py)  
11. langchain/libs/core/langchain\_core/messages/ai.py at master \- GitHub, 1月 31, 2026にアクセス、 [https://github.com/langchain-ai/langchain/blob/master/libs/core/langchain\_core/messages/ai.py](https://github.com/langchain-ai/langchain/blob/master/libs/core/langchain_core/messages/ai.py)  
12. BaseMessage — LangChain documentation, 1月 31, 2026にアクセス、 [https://reference.langchain.com/v0.3/python/core/messages/langchain\_core.messages.base.BaseMessage.html](https://reference.langchain.com/v0.3/python/core/messages/langchain_core.messages.base.BaseMessage.html)  
13. Tool Calling with LangChain, 1月 31, 2026にアクセス、 [https://www.blog.langchain.com/tool-calling-with-langchain/](https://www.blog.langchain.com/tool-calling-with-langchain/)  
14. Messages | LangChain Reference, 1月 31, 2026にアクセス、 [https://reference.langchain.com/python/langchain/messages/](https://reference.langchain.com/python/langchain/messages/)  
15. langchain/libs/core/langchain\_core/language\_models/chat\_models.py at master \- GitHub, 1月 31, 2026にアクセス、 [https://github.com/langchain-ai/langchain/blob/master/libs/core/langchain\_core/language\_models/chat\_models.py](https://github.com/langchain-ai/langchain/blob/master/libs/core/langchain_core/language_models/chat_models.py)  
16. LangChain: How an Agent works. Deep dive into Agent and AgentExecutor \- Masato Naka, 1月 31, 2026にアクセス、 [https://nakamasato.medium.com/langchain-how-an-agent-works-7dce1569933d](https://nakamasato.medium.com/langchain-how-an-agent-works-7dce1569933d)  
17. How does LangChain actually implement the ReAct pattern on a high level? \- Reddit, 1月 31, 2026にアクセス、 [https://www.reddit.com/r/LangChain/comments/17puzw9/how\_does\_langchain\_actually\_implement\_the\_react/](https://www.reddit.com/r/LangChain/comments/17puzw9/how_does_langchain_actually_implement_the_react/)  
18. early\_stopping\_method parameter of AgentExecutor doesn't work in expected way · Issue \#16374 \- GitHub, 1月 31, 2026にアクセス、 [https://github.com/langchain-ai/langchain/issues/16374](https://github.com/langchain-ai/langchain/issues/16374)  
19. LangChain Agent Executor Deep Dive \- Aurelio AI, 1月 31, 2026にアクセス、 [https://www.aurelio.ai/learn/langchain-agent-executor](https://www.aurelio.ai/learn/langchain-agent-executor)  
20. AgentExecutor stopping before reaching the set max\_iteration and max\_execution\_time limits without meeting the stop condition · Issue \#13897 · langchain-ai/langchain \- GitHub, 1月 31, 2026にアクセス、 [https://github.com/langchain-ai/langchain/issues/13897](https://github.com/langchain-ai/langchain/issues/13897)  
21. Issue: How to validate Tool input arguments without raising ValidationError \#13662 \- GitHub, 1月 31, 2026にアクセス、 [https://github.com/langchain-ai/langchain/issues/13662](https://github.com/langchain-ai/langchain/issues/13662)  
22. How does LangChain support multi-threaded processing? \- Milvus, 1月 31, 2026にアクセス、 [https://milvus.io/ai-quick-reference/how-does-langchain-support-multithreaded-processing](https://milvus.io/ai-quick-reference/how-does-langchain-support-multithreaded-processing)  
23. Tools | LangChain Reference, 1月 31, 2026にアクセス、 [https://reference.langchain.com/python/langchain/tools/](https://reference.langchain.com/python/langchain/tools/)  
24. Structured Tools \- LangChain Blog, 1月 31, 2026にアクセス、 [https://www.blog.langchain.com/structured-tools/](https://www.blog.langchain.com/structured-tools/)  
25. DOC: Clarify how to handle runs and linked calls with run\_managers \#13390 \- GitHub, 1月 31, 2026にアクセス、 [https://github.com/langchain-ai/langchain/issues/13390](https://github.com/langchain-ai/langchain/issues/13390)  
26. LangSmith Tracing Deep Dive — Beyond the Docs | by aviad rozenhek | Medium, 1月 31, 2026にアクセス、 [https://medium.com/@aviadr1/langsmith-tracing-deep-dive-beyond-the-docs-75016c91f747](https://medium.com/@aviadr1/langsmith-tracing-deep-dive-beyond-the-docs-75016c91f747)  
27. Troubleshoot trace nesting \- Docs by LangChain, 1月 31, 2026にアクセス、 [https://docs.langchain.com/langsmith/nest-traces](https://docs.langchain.com/langsmith/nest-traces)  
28. Storage (LangGraph) | LangChain Reference, 1月 31, 2026にアクセス、 [https://reference.langchain.com/python/langgraph/store/](https://reference.langchain.com/python/langgraph/store/)

[image1]: <data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAcAAAAZCAYAAAD9jjQ4AAAAiUlEQVR4XmNgGOpAEIgZ0QVBgBWI/wOxJ7oEDHCgCxAEzEAsjC4IAipAfBqIXwOxPLIEDxCvAGIzIDYF4iJkSZDLQIKcQLwDiBWRJWFAB4jfM+DwYwMQ/0MXBAF+ID4BxNeBWBmIA5ElbYD4NxBPAuJSII5AlpRhgOjaD8QmyBIwAAo2SXTBQQUAlhoPwfFnHOEAAAAASUVORK5CYII=>

[image2]: <data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAZCAYAAABD2GxlAAABG0lEQVR4Xu2Vv2oCQRCHJ5iAYsCgIAhp7H0ESSVICquUgo2NhZ2Fffo8QgpL8QXs7CTgW4h1grWF+Q3DgTe33u6drIjsB18zc39+3M7uEQUCgfviAVbhk25cCX53XRdPmcAj/NINz5ThEO7hVPVi8Jer6KIjPV1w5AUW4Izk46QGvIS8ASOsAXn+irqYAa8BX+GKZAa6queKt4AluIAfcAfnJDORFW8B3+EbyRIfYDveTvAMGwYHhhrL17twNmBEE/6QfRd/wq3BX0ONHcltVqwB+5TSdMDbEjM8h0vYIlkSPjT58MyC14Ac7I/kmOEZ/IaPsSvseA1Yg2u4gWOSDZOVvAE5EAfTJp7HvznXHWci8cBbo6MLgcA98A+CFjwJ139HGwAAAABJRU5ErkJggg==>