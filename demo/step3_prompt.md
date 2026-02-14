```text
【マスタープロンプト】Step 2-2｜単一仮説・深掘り（File Search参照／補完禁止）

P0 契約（Non-negotiables）
- 言語：日本語（固有名詞・規格名は必要に応じて英語併記可）
- 見出しロック：ホワイトリスト完全一致／ブラックリスト禁止（行頭一致）
- Skeleton→Fill（二段進行；同一応答内で連続出力／承認待ち禁止）
- 出力はFill済み本文のみ（レポート本文）。Skeleton（テンプレート／プレースホルダ）の印字禁止（本文への混入禁止）
- Phase Integrity Guard（厳守）：
  - 第5章先頭段落に 選定仮説の handoff_selected.title をそのまま1回出現
  - MKT選定一致（第4章[Selected]のMKTコードがRole AのMKT一覧に存在）
  - StructureにCap-ID≥2（`Cap-XX（material_system） × Cap-YY（material_system）…`の括弧併記形式；material_systemはRole Bから厳密引当）
  - 必達KPI≥3（数値＋単位）
- Bibliography-first：Skeleton敷設時に参考文献URL≥20をバッファ化（本文の引用[n]と一意対応；欠番・重複なし）。出力は最後（第6章）
- URL印字ポリシー：本文（第1〜第5章）に外部URLは印字しない。第6章（参考文献）に限り完全URLを印字する。
- Skeleton敷設後は追加検索禁止（Skeleton敷設前の事前検索＝可）
- 比較表は空欄／N/A禁止（全セル充填・可能な限り定量）

0. 実行プロトコル（Execution Protocol）
以下ファイル・パラメータがFile Search／アプリ環境に存在することを確認し、不足時は即停止。
- 必須ファイル：
  - `hypothesis_context`（Step 2-1のアウトプット全文：ナラティブ＋handoff JSON；拡張子なし）
  - `technical_assets.json`（Role B：Cap-ID→material_system対照）
  - `target_specification.txt`（Role A：市場・顧客ニーズ／MKT一覧）
- 必須パラメータ：{HYPOTHESIS_TITLE}（深掘り対象の仮説タイトル）

【分岐ルール】
- CASE A: 上記必須が全て揃っている → パース→選定→必須フィールド検査→Skeleton→Fillの順で出力
- CASE B: 不足 → 「エラー: 必須ファイルが不足しています。」と出力して停止

1. インプット処理（handoff抽出・選定・整合・補完禁止）
- `hypothesis_context`本文から唯一の```jsonフェンス内のhandoff JSONを検出・パース
  - 複数検出時： 「エラー: hypothesis_context内のhandoff JSONブロックが複数検出されました。単一ブロックのみを許容します。」
  - 未検出時： 「エラー: hypothesis_context内にhandoff JSONが検出できません。処理を停止します。」
- タイトル選定ポリシー（シンプル照合）：
  - 対象は handoff.topN[*].title
  - 次の順で“一致するもの”を1件だけ選ぶ（曖昧な同点処理は行わない）：
    1) 文字列正規化（前後空白除去・連続空白圧縮・大文字小文字非区別・全角半角正規化）後の一致が1件 → 採用
    2) 1)で見つからない場合、正規化文字列の部分一致（{HYPOTHESIS_TITLE}がtitleに含まれる、または逆）が1件だけ存在 → 採用
    3) 上記いずれも0件、または複数件ヒット → エラー停止
  - エラー文言（0件または複数件）： 「エラー: 指定タイトルに一致する仮説が特定できません（title={HYPOTHESIS_TITLE}）。処理を停止します。」
- 選定した要素を`handoff_selected`として内部保持（補完禁止）
- Structureのmaterial_system引当（Role B必須）：
  - `technical_assets.json`のCap-ID対照から厳密引当（同義語展開・推測補完は禁止）
  - 引当不能なCap-ID／material_systemがある場合： 「エラー: technical_assets.jsonへの引当不整合（Cap-IDまたはmaterial_systemが未特定）。処理を停止します。」
- MKT選定整合（Role A必須）：
  - 第4章で用いる[Selected]のMKTコードは、`target_specification.txt`内のitems[*].id（例：MKT-XXX）に存在するものを使用
  - 不整合の場合： 「エラー: MKTコード整合性エラー（指定コードがtarget_specification.txtに不存在）。処理を停止します。」

【handoff_selected 必須フィールド検査（補完禁止）】
- rank／title／tag（Core|Strategic|Moonshot）
- scores（I／M／C／L／U／composite；0.00〜1.00）
- cap_id_fingerprint（形式「Cap-XX + Cap-YY（＋…）」）
- industry／domain／stage／role
- raw_materials[]／form_factor
- hypothesis_summary
- trade_off.bind／trade_off.why_legacy_fails／trade_off.cost_of_inaction
- spp.structure[]（≥2）／spp.property／spp.performance／spp.causal_chain（1〜2文）
- device_process_solution／material_solution
- kpi[]（≥3；各「数値＋単位」）
- comparative_advantage.alt_refs[]／basis／delta_kpi／moat_factor
- moat_outline
- competitors[]（1〜3）
- risk_seeds[]（≥1）
- killer_experiment_outline[]（≥1；各「閾値：数値＋単位」）
- 任意：target_slice／mkt_candidates
- 欠落時：固定文言で停止
  「エラー: 選択された仮説（handoff_selected）の必須フィールドが不足しています（missing=フィールド名一覧）。処理を停止します。」

2. Instruction（Skeleton→Fill）
- Skeleton：内部敷設のみ（印字禁止）。ホワイトリストの見出し枠と第5章カード枠／参考文献バッファを内部で用意
- Fill：Skeleton直後に本文を一度だけ印字（見出し重複印字禁止／見出しは変更不可）

3. ホワイトリスト（見出し・順序固定）
- 【レポートタイトル】
- 【第1章：エグゼクティブサマリー】
- 【第2章：事業機会を創出する構造的変曲点 (Why Now?)】
- 【第3章：市場機会とエコシステム分析 (Where to Play?)】
- 【第4章：技術的ボトルネックと未解決の顧客課題 (What is the Problem?)】
- 【第5章：事業仮説（The Business Hypothesis）】
- 【第6章：参考文献 (References)】

4. ブラックリスト（行頭一致で禁止）
- 「#」「##」「###」「I.」「II.」「序論」「結論」「Strategic Context」
- 「Executive Summary（英語表記）」「Part」「Chapter」「Table Title」「監査ストリップ（Phase 2内）」
- 前後注釈や「保存完了」等の非本文テキスト

5. Skeleton（出力テンプレート）
【レポートタイトル】
[handoff_selected.title] における [handoff_selected.target_slice] 向け事業仮説

【第1章：エグゼクティブサマリー】
- The Shift／The Pain／The Solution／The Value
- タイトル整合（handoff_selected.title）

【第2章：事業機会を創出する構造的変曲点 (Why Now?)】
- 技術的限界／産業構造の変化／無理難題

【第3章：市場機会とエコシステム分析 (Where to Play?)】
- サプライチェーンのTier構造（川上→川中→川下→規制）と各Tierの主要プレイヤー（社名例）
- パワーバランス（採用ゲート強度／供給集中度・代替可用性・切替摩擦／必須パートナーアクセス）
- 市場機会（定性要約）

【第4章：技術的ボトルネックと未解決の顧客課題 (What is the Problem?)】
- 4.1 市場のマクロトレンドと変曲点
- 4.2 新しいプロセス・デバイス構造の要件
- 4.3 従来・一般アプローチの困難（Methods級具体性）
- 4.4 The Trade-off（二律背反の定義）［[Selected]にMKT-XXXを併記；Role A由来のコードのみ使用］

【第5章：事業仮説（The Business Hypothesis）】
- ターゲット:
- 顧客の「解決不能なジレンマ」 (The Trade-off):
- Inevitability (Must-have根拠):
- Material Necessity (素材必然性の根拠):
- 当社ソリューションの物理化学的メカニズム (The Mechanism):
- Structure: ［Cap-XX（material_system） × Cap-YY（material_system）…；Role Bから厳密引当］
- Property:
- Performance:
- Causal chain（S→P→Performance）:
- 必達KPI:
- 比較優位の可視化（競合手法・材料の比較表）:
| 評価軸 (Criteria) | 既存技術A (Competitor) | 代替手法B (Alternative) | 当社ソリューション (Champion) |
| :--- | :--- | :--- | :--- |
| 物理化学的メカニズム | | | |
| 主要トレードオフ | | | |
| 性能（定量） | | | |
| プロセス適合性 | | | |
| コスト・単位経済（Unit Economics） | | | |
| 判定 | | | |
- 技術的競争優位性 (Technical Moat):
- 対象事業領域:
- キラー実験:
- 暫定撤退ライン:
- 主要リスクの種（技術／市場／規制）:

【第6章：参考文献 (References)】

6. Fillガイド（handoffマッピング／フォーマット要件）
- 第1章：handoff_selected.hypothesis_summary／kpi／moat_outline を凝縮（600〜1,000字）。Valueは必達KPIを一言で可
- 第2章：技術的限界／産業構造の変化／無理難題を記述し、最低1件は定量[n]を含める
- 第3章（400〜800字）：Tier／主要プレイヤー／パワーバランス／市場機会
- 第4章（総量800〜1,500字）：MKT課題3〜5件＋[Selected]1件（MKTコードはRole Aの一覧から選択）
- 第5章（1,000〜2,500字）：
  - 先頭段落に handoff_selected.title をそのまま記載（選定済みタイトル）
  - Mechanism：
    - Structure：Cap-ID（material_system併記）×…（≥2件）。material_systemはRole Bから厳密引当
    - Property／Performance／Causal chain：各1〜2文の説明＋定量
  - KPI：数値＋単位、≥3件
  - 比較表：各セル定量。判定は`scores.C`／`comparative_advantage.delta_kpi／basis`要約（≤80字）
  - Moat：`moat_outline`／`comparative_advantage.moat_factor`を S→P→P に紐づけ具体化
  - 対象事業領域／キラー実験／暫定撤退ライン／主要リスクの種：所定形式

7. 受け入れテスト（Acceptance as Code）
- ① 見出しがホワイトリスト通り（【レポートタイトル】〜【第6章：参考文献 (References)】）
- ② ブラックリスト語（#, ##, ###, I., II., 序論, 結論, Strategic Context, Executive Summary（英語）, Part, Chapter, Table Title, 監査ストリップ 等）未出現／前後注釈なし
- ③ タイトル整合：第5章先頭段落に handoff_selected.title が1回出現
- ④ 第3章にサプライチェーンTier構造＋主要プレイヤー（社名例）＋パワーバランス＋市場機会（定性要約）が含まれ、章の分量が400〜800字
- ⑤ 第4章：冒頭にMKT課題3〜5件＋[Selected]1件／総量800〜1,500字に収まる／[Selected]のMKTコードがRole A（target_specification.txt）のitems[*].idに存在
- ⑥ 4.3の記述対象が「従来・一般のアプローチの困難」に限定（チャンピオン／Cap-IDの直接言及なし）
- ⑦ Structure行にCap-IDが≥2、各要素が「Cap-XX（material_system）」の括弧併記形式（material_systemはRole Bから引当済み）
- ⑧ Property／Performanceに「説明」行が存在（各1〜2文）
- ⑨ S→P→Performanceの因果が1〜2文存在
- ⑩ 必達KPIが≥3件、全て「数値＋単位」（プレースホルダなし）
- ⑪ 比較表に空欄／N/Aなし。判定セル要約≤80字
- ⑫ 比較表の判定根拠として本文に`scores.C` または `comparative_advantage.delta_kpi／basis`の言及がある
- ⑬ 第5章のI／Mに最低1件の新規[n]（第4章未出の引用；単位付き）
- ⑭ 第5章末尾に「対象事業領域／キラー実験／暫定撤退ライン／主要リスクの種」4項目が存在
- ⑮ Skeletonのプレースホルダ語句が本文に一切含まれていない（例：The Shift／The Pain／The Solution／The Value 等）
- ⑯ 本文にMarkdown装飾の**や行頭*が一切含まれていない（箇条書きは必要時「・」を使用）
- ⑰ 各章見出しは本文中に1回のみ出現（重複見出し印字なし）
- ⑱ 第6章の参考文献が20件以上・完全URLのみ、本文[n]と一意対応（欠番・重複番号なし）

8. 表記規範・用語
- 章見出しはホワイトリスト文字列に完全一致（語尾・記号・順序の改変禁止）
- Skeletonは印字禁止。本文はFill済みの一回出力のみ
- 太字・斜体などのMarkdown装飾（**、*）は本文で使用しない。箇条書きは必要時「・」を使用（行頭*は禁止）
- コードブロック（```）や前後注釈の印字禁止（レポート本文のみ）
- 規制・安全環境／知財防衛／製造原価・採算性・総コストなどの平易語を使用（英略語の一般用語は禁止）
- 比較表は全セル充填・可能な限り定量（空欄／N/A禁止）
- Structure行のmaterial_systemは、`technical_assets.json`のCap-ID対照から厳密に引く（同義語展開や推測で補完しない）

9. エラーメッセージ（固定）
- 必須ファイル不足時： 「エラー: 必須ファイルが不足しています。」
- handoff JSON複数検出時： 「エラー: hypothesis_context内のhandoff JSONブロックが複数検出されました。単一ブロックのみを許容します。」
- handoff JSON未検出時： 「エラー: hypothesis_context内にhandoff JSONが検出できません。処理を停止します。」
- タイトル選定不可時： 「エラー: 指定タイトルに一致する仮説が特定できません（title={HYPOTHESIS_TITLE}）。処理を停止します。」
- 引当不整合時： 「エラー: technical_assets.jsonへの引当不整合（Cap-IDまたはmaterial_systemが未特定）。処理を停止します。」
- MKT不整合時： 「エラー: MKTコード整合性エラー（指定コードがtarget_specification.txtに不存在）。処理を停止します。」
- handoff_selected不足時： 「エラー: 選択された仮説（handoff_selected）の必須フィールドが不足しています（missing=フィールド名一覧）。処理を停止します。」
```