```text
【マスタープロンプト】Step 2-1｜多数仮説生成→Top {HYPOTHESIS_COUNT}選定＋handoff JSON

P0 契約（Non-negotiables）

- 言語：日本語（固有名詞・規格名の英語併記可）

- 入力はFile Searchからのみ参照（`target_specification.txt`／`technical_assets.json`／`previous_hypotheses.txt`）

- 実行形態：ノンインタラクティブ。Skeleton→Fillを同一応答内で完了

- 出力＝ナラティブ本文（任意）＋handoff JSON（必須・単一ブロック）

- handoff JSONは本文内のどこかに単一の```jsonフェンスで囲んで出力（複数フェンス禁止）

- ナラティブ本文は自由記述可（handoff JSONの構造・値は本文に依存せず厳守）

- 選抜重み（I/M/C/L/U）：I 0.25／M 0.30／C 0.25／L 0.10／U 0.10

- 合成スコア（composite） = 0.25·I + 0.30·M + 0.25·C + 0.10·L + 0.10·U（各軸0.00〜1.00）

- 選抜スコア（selection_score） = 下記「スコア式」で定義される score(i) を0.00〜1.00レンジに正規化した値（TopN採用決定に用いる内部スコア）

- KPIはTop各仮説につき3件以上（「数値＋単位」のみ、プレースホルダ不可）

- Negative Scope：近似重複の除外を実施

- Cap-ID指紋は「Cap-XX + Cap-YY (+ ...)」形式（SPPのstructureはCap-IDを2件以上）

- Skeleton敷設後は追加検索禁止（Skeleton敷設前の事前検索＝可）

- 配列キーは「topN」に統一（固定）

- stage／roleの記法：単一文字列。複数は「; 」連記可

- 本出力全文は`hypothesis_context`（拡張子なし）として保存し、Step 2-2に引き渡す

0. 実行プロトコル

- 必須確認：`target_specification.txt`／`technical_assets.json` の2ファイルが存在。無ければ停止し「エラー: 必須ファイルが不足しています。」を出力

- 任意：`previous_hypotheses.txt`（無ければ空扱い）

- CASE A：必須2ファイルあり→実行／CASE B：不足→エラー停止

1. 入力（Role）

- Role A: `target_specification.txt`（市場・顧客ニーズ：MKT-ID付きリスト）

- Role B: `technical_assets.json`（技術シーズ：Cap-ID／material_system／function 等）

- Role C: `previous_hypotheses.txt`（既出仮説；“なし”や空は空リストとして扱う）

- パラメータ: {HYPOTHESIS_COUNT}=Top選定件数（自然数N）

2. 評価アルゴリズム（Internal Execution Logic）

記号定義

- Pool P＝当回の候補集合（例：50〜70件）、Selected S＝逐次選抜で採用済みの集合

- 周内頻度は freq_P(·) を用いる（Cap単体・Capペアの偏り抑制は freq_P）

- selected未出・連発判定は freq_S(·)

- Cap-ID変数は z（例：z1,z2はペア）

- クラスタID変数は cid（当回Pool内のラテント概念クラスタID）

- Role C頻度：freq_C(z)、ρ_C(z)=freq_C(z)/|Role C|

- 補助定義：MI-likeキー＝mechanism@interface（抽出・正規化はGate Mに定義）

- 軽量素材クラスタキー：cluster_key(i)=`raw_materials`＋`material_system`主要名詞（小文字・単数化・2語連結）。Role C側のcluster_key頻度 ρC_cluster(key)=freq_RC(key)/|Role C|

- MKT-ID：`target_specification.txt`内の市場・顧客ニーズ項目に付与されているID（例：MKT-001, MKT-002, ...）。各項目は少なくとも {id, title, background, trade_off or why_unresolved} を持つこと

- 主MKT m(i)：仮説 i に割り当てる主市場軸。Role AのMKTリストから「既に存在するMKT-ID」を取得して割り当てる。生成した仮説の `trade_off.bind + why_legacy_fails + cost_of_inaction` テキストと、各MKT項目の「trade_off／why_unresolved／背景」などの問題文脈との sim_problem を計算し、類似度が最大のMKT-IDを m(i) とする

- Role CにおけるMKT頻度：freq_C_MKT(m)＝Role C内で主MKT＝m の仮説数／ρ_C_MKT(m)=freq_C_MKT(m)/|Role C|

- 未出MKTフラグ（Role C基準）：new_MKT_RC(i)=1 if |Role C|≥40 かつ freq_C_MKT(m(i))=0, else 0

- Role Cにおける MKT×Cap頻度（部分一致）：freq_C_pair(m,z)＝Role C内で主MKT＝m かつ Cap z を`spp.structure`に含む仮説数／ρ_{C,m}(z)=freq_C_pair(m,z)/max(freq_C_MKT(m),1)

- Cap type：`technical_assets.json` 内の `cap_type` フィールド（baseline/unlocker/design/platform のいずれか）を参照し、フィールドが存在しないCapについては説明文から最尤のtypeを推定する

- type別重み w_type(z)：baseline=1.0／unlocker=1.0／design=1.5／platform=1.3

Phase 0: Skeleton

- Role Map抽出（A/B/C）

- 代表クエリ／情報源カテゴリの列挙（内部保持、出力非掲載）

- Capの役割と選定順序：

- Capには baseline／unlocker／design／platform の4タイプがある。

- baseline：当社が既に持つ材料・技術のうち、世の中一般に知られている性質・技術水準を１つに凝縮した「標準グレード」のカタログ。

- unlocker：baselineで到達しにくい性能・用途領域に踏み込むためのブレイクスルー技術。

- design：baseline／unlockerを前提に、物性・構造・界面・プロセス条件などを連続的に設計・最適化できる「技術的地力（設計ノブ）」。

- platform：高純度・特殊ガス・クリーン運転・高温高圧など、実際のプロセスを安定・安全に量産運転するためのインフラ。

- Ideationでは、まず baseline Cap を１つの出発点として「標準グレードを前提にした場合、このProblemにどうアプローチし得るか」を軽く検討せよ。そのうえで、baselineのみでは仕様・競争優位の観点から十分でないと判断される場合は、unlocker を主軸候補として自由に検討してよい。最終的な仮説では、baseline／unlocker のいずれを主役とした場合でも、baseline（標準グレード）との関係（延長か打破か）を簡潔に言語化すること。

- design／platform Cap は単独で仮説を構成してはならない。必ず baseline／unlocker を核にした技術案を先に構成し、その案に対して「どの特性をどこまで設計・最適化できるか（design）／それを実際にどう安定運転できるか（platform）」を補強するために限定的に用いること。特に design Cap は、「困ったときに何でも足す」汎用手段ではなく、そのProblemで主要なボトルネックとなる物性・プロセスに対してピンポイントで適用せよ。

- type別重み w_type(z) は、baseline=1.0／unlocker=1.0／design=1.5／platform=1.3 とし、Cap頻度ペナルティ P_cap_RC および MKT×Cap頻度ペナルティ P_MKTCap_RC の計算で用いる。

Phase 1: Ideationと選抜

- 発散生成：Ideation総数≥50（領域別件数の内訳を保持）

- 生成マインドセット：

- 「ちょっとずらし」厳禁（用語置換・薄い属性変更は破棄）

- Role Cと「Pain × Core Mechanism」一致は形態が異なっても重複として破棄

- 採用前提：Structure／Mechanism／Problemの少なくとも1要素が本質差分

- 装置・ソフトで解ける案（素材必然性なし）は除外

- 常連抑制（Cap／コンボ／メカ）：Role C頻出のCap-ID・Capペア・機構は原則回避。再利用時は 1) Cap組み合わせが未出 2) MechanismまたはProblemが根本的に異なる 3) `comparative_advantage.delta_kpi`で主要KPI差分≥30% を全て満たす。`moat_factor`を明示できない案は破棄

- 直交生成ガイド：同一市場要件に対し、別素材×別プロセスの直交案を最低1本併走生成（例：素材系変更／真空→非真空／バッチ→連続）

- MKT×Design/Platform Capの偏り抑制：同じMKTに対して、同じDesign/Platform Capを軸とした案を連続・大量に生成しない。初期の数件で同じMKT×Capを多用した場合、その後は意図的に別のCap、特に別種のDesign/PlatformやBaseline/Unlockerを試す（MKT×Capバリエーションの幅を広げる）

- Negative Scope（類似カーネル）：

- sim_mech：`spp.causal_chain + material_solution + device_process_solution` の埋め込み類似（cosine; [0,1]）

- sim_cap：0.5·Jaccard(`spp.structure`のCap-ID集合)+0.5·cosine（`material_system`/`function`）

- sim_kpi：KPIカテゴリ一致のF1 × 値近接（±20%＝1.0、±50%＝0.6、他0.0）

- sim_problem：`trade_off.bind + why_legacy_fails + cost_of_inaction` の埋め込み類似

- k(i,j)=w_mech·sim_mech + w_cap·sim_cap + w_kpi·sim_kpi + w_prob·sim_problem（w_mech=0.40／w_cap=0.30／w_kpi=0.15／w_prob=0.15）※MKT IDは主軸に含めない／titleは用いない

- Role C早期除外（ハード棄却）：

- Role Cが非空のとき、各iについて max_{p∈RoleC} k(i,p) ≥ Tdup なら即除外（Tdup=0.77）

- 監査：`duplicates_discarded`（Role C早期棄却件数）、`negative_scope_status`（「OK（重複x案破棄: Sしきい値判定）」または「重大重複なし」）

- Gate S（署名距離ガード；構造重複の早期除外）：

- signature(i)={Sig_struct（`spp.structure`のCap-ID集合）, Sig_mech（`spp.causal_chain`要約）, Sig_proc（`device_process_solution`要約）, Sig_prob（`trade_off.bind + why_legacy_fails`要約）, Sig_kpi（KPIカテゴリ集合）}

- sim_signature(i,j)=幾何平均{ Jaccard(Sig_struct), cosine(embed(Sig_mech)), cosine(embed(Sig_proc)), cosine(embed(Sig_prob)), Jaccard(Sig_kpi) }

- Role Cが非空のとき、max_{p∈RoleC} sim_signature(i,p) ≥ T_sigdup なら即除外（T_sigdup=0.83）

- Gate S棄却数は監査に計上しない。Gate後にP<50なら追加生成（上限2バッチ）でideation_total≥50を保証

- Gate M（MI-likeガード；構造×適用面の同型排除／常連抑制強化）：

- MI-likeキーの抽出・正規化

- Mechanism：`device_process_solution` または `spp.causal_chain` から主動詞＋補語1を抽出し、原形・小文字へ正規化

- Interface：`stage`の主要名詞1〜2を抽出し単数・小文字へ正規化

- MI-like＝mechanism@interface（連結文字は `@`）

- Resource集合（離散比較用）

- `spp.structure`のCap-ID集合 ∪ `material_solution`の主要名詞（最大3語）

- Jaccardで比較

- Hard Gate条件（Role Cが非空のときのみ適用）：

- MI-like一致 ∧ Resource集合のJaccard≥0.5 ∧ KPIカテゴリ一致 → 即除外

- 常連MI検出と例外強化（固定化抑制）：

- Role C内でのMI-like頻度比 ρ_MI(k)=freq_RC(MI-like k)/|RoleC|

- 「常連」定義：ρ_MI(k)≥0.05 または MI-like頻度上位5件

- 候補のMI-likeが常連と一致した場合、Hard Gateの例外許容は「主要KPI差分≥30%」に加え、SPP/工程スロット（`spp.structure`／`spp.causal_chain`／`material_solution`／`stage`）のうち3スロット以上の明確差分を必須。未達は棄却

- 内部監査（内部保持）：`rc_regular_hits`（常連一致でHard Gate棄却件数）、`rc_hotspots`（常連MIキー上位5と採否内訳）

- Pool内重複抑止（生成段階）：

- 生成時、max_{p∈P} k(i,p) ≥ T_pool なら破棄・再生成（T_pool=0.76）

- Cap使用上限：任意のCap zで freq_P(z) ≤ 1、任意のCapペア(z1,z2)で freq_P(z1,z2) ≤ 1（超過は破棄・再生成）

- Role C遠距離比率：{i∈P | max_{p∈RoleC} k(i,p) ≤ 0.55} 比率が85%未満なら追加生成（上限2バッチ）

- ラテント概念クラスタ付与（Pool完成後）：

- sim_iface：`stage`/`role`由来テキストを小文字化・原形化・停止語除去したうえでの軽量cos類似

- k_cluster(i,j)=0.40·sim_mech + 0.22·sim_cap + 0.13·sim_problem + 0.13·sim_kpi + 0.12·sim_iface

- t_cluster=0.72で連結成分を形成し、各iにクラスタID cid(i) を付与。freq_P_lt(c)=Pool内クラスタcの件数

- Role Cが空の場合の分岐：

- Role C早期除外／遠距離比率／近接枠・ペナルティ（max_sim_P・P_cap_RC・P_resource_cluster_RC・P_MKT_RC・P_MKTCap_RC）／Gate M（Role C比較が前提）はスキップ。Pool充足とクラスタ分散に専念

- Gate A：

- KPIが「数値＋単位」で2件以上なら候補保持（2件は内部採点でL/U軽微減点）。2件未満は破棄

- 採点：各仮説のI/M/C/L/Uを0.00〜1.00で採点し、合成スコア（composite）を算出（重みはP0）

逐次選抜（2パス＋スコア・制約）

- 初期選抜（Pass-1：クラスタ別チャンピオン先取り）：

- 各cidについて composite(i) 最大の1件をチャンピオン候補とする。ただし候補が max_sim_P(i) ≥ T_high の場合は当該cidのチャンピオンから除外（Pass-2で補完）

- チャンピオン同点時のタイブレーク：① max_sim_P(i)が低い方 → ② Σ freq_P(z)が小さい方 → ③ 仮説ID昇順

- |Champ| ≥ {HYPOTHESIS_COUNT} の場合：Champからcomposite順に上位{HYPOTHESIS_COUNT}件を仮TopNとし、「TopN相互制約（k<0.75）／Role C近接枠／クラスタクォータ」の最終検査を適用。違反があればcomposite次点に順次置換し、全制約を満たすよう調整して選抜終了

- それ以外：S=Champ としてPass-2へ

- Pass-2（逐次選抜）：

- N=1：compositeトップ1件を採用（減点・加点は使用しない）

- Regime A（N=2〜5）：β=0.24／γ=0.50／q=2.3／η=0.10／μ_cap=0.30／ν_pair=0.15／α=0.20／ρ=0.15

- Regime B（N=6〜10）：β=0.34／γ=0.50／q=2.3／η=0.12／μ_cap=0.30／ν_pair=0.15／α=0.15／ρ=0.12

定義：

- max_sim_S(i)=max_{s∈S} k(i,s)、max_sim_P(i)=max_{p∈RoleC} k(i,p)

- 主MKT割当：Role Aの既存MKTリストから、sim_problem最大のMKT-IDを m(i) として割り当てる

- n_MKT(i,S)：S内で主MKT＝m(i)の仮説数

- N_S(i)：TopN内での新規MKT・新規MI・新規Cap登場による多様性加点用カウンタ

- C_S(i)：クラスタ多様性加点用カウンタ

- freq_P(z)、freq_P(z1,z2)

- max_sigSim_S(i)=max_{s∈S} sim_signature(i,s)

- MI_like_match_S(i)= [既選S中にMI-like一致が存在する場合は1、それ以外は0]

- クラスタRC圧：P_cluster_RC(i)=λ · max_{p∈RoleC, j∈cluster(i)} k(j,p)（Role Cが非空のときに適用）

- Role CにおけるCap頻度：ρ_C(z)=freq_C(z)/|Role C|

- Role CにおけるMKT頻度：ρ_C_MKT(m)=freq_C_MKT(m)/|Role C|

- Role CにおけるMKT×Cap頻度（部分一致）：ρ_{C,m}(z)=freq_C_pair(m,z)/max(freq_C_MKT(m),1)

ペナルティ・加点：

- P_cap(i)=Σ_{z∈i} μ_cap·max(0,freq_P(z)−1)^2

- P_pair(i)=Σ_{(z1,z2)∈i} ν_pair·max(0,freq_P(z1,z2)−1)^1.5

- Cap頻度ペナルティ（Role C由来；type別重み付き）：

- P_cap_RC(i)=Σ_{z∈Z(i)} μ_RC·w_type(z)·[max(0,ρ_C(z)−ρ0_cap_RC)]^{p_cap_rc}

- Z(i)：仮説iの`spp.structure`に含まれるCap集合

- ρ0_cap_RC=0.04、p_cap_rc=2、μ_RC=4.5

- 同系素材クラスタ頻度ペナルティ（Role C由来）：

- P_resource_cluster_RC(i)=Σ_{key∈cluster_key(i)} μ_cluster_RC·[max(0,ρC_cluster(key)−ρ0_cluster_RC)]^2

- μ_cluster_RC=6.0、ρ0_cluster_RC=0.02

- MKT単体頻度ペナルティ（Role C由来；MKT偏り抑制）：

- P_MKT_RC(i)=μ_MKT·[max(0,ρ_C_MKT(m(i))−ρ0_MKT)]^2

- ρ0_MKT=0.04、μ_MKT=8.0

- 未出MKT加点（Role C由来；ロングテールMKTの探索促進）：

- new_MKT_RC(i)=1 if |Role C|≥40 かつ freq_C_MKT(m(i))=0, else 0

- A_MKT_RC(i)=α_MKT_RC·new_MKT_RC(i)

- α_MKT_RC=0.10

- MKT×Cap頻度ペナルティ（Role C由来；部分一致・type別重み付き）：

- ベース関数（二乗型）：

- P_MKTCap_base(ρ)=μ_MKTCap·max(0,ρ−ρ0_MKTCap)^2

- ρ0_MKTCap=0.06、μ_MKTCap=6.0

- 実際のペナルティ：

- P_MKTCap_RC(i)=Σ_{z∈Z(i)} w_type(z)·P_MKTCap_base(ρ_{C,m(i)}(z))

- 頻度系ペナルティ合算上限（安定化）：

- pen_freq_total_capped(i)=min( P_cap_RC(i) + P_resource_cluster_RC(i) + P_MKT_RC(i) + P_MKTCap_RC(i), 0.80 )

- P_cluster(i)=ω_cluster·max(0,freq_P_lt(cid(i))−1)^1.6（ω_cluster=0.18）

- 追加減点（冗長抑制）：γ_sig·max_sigSim_S(i)、γ_MI·MI_like_match_S(i)（ともに減点項として用いる）

スコア式：

- score(i)= composite(i)

+ β·(1 − max_sim_S(i))

− γ·(max_sim_P(i))^q

− η·[n_MKT(i,S)]^2

+ α·N_S(i) + ρ·C_S(i) + A_MKT_RC(i)

− P_cap(i) − P_pair(i) − pen_freq_total_capped(i) − P_cluster(i)

− γ_sig·max_sigSim_S(i) − γ_MI·MI_like_match_S(i)

− P_cluster_RC(i)

係数設定値：

- γ_sig=0.18、γ_MI=0.30、λ=0.15

- ρ0_MKT=0.04、μ_MKT=8.0、α_MKT_RC=0.10

- ρ0_MKTCap=0.06、μ_MKTCap=6.0

- ρ0_cap_RC=0.04、μ_RC=4.5

- 逐次選抜で最終的にTopNに採用した各仮説について、算出した score(i) を0.00〜1.00レンジに正規化したうえで、handoff JSONの`scores.selection_score`として書き戻すこと（小数第3位程度まででよい）。

逐次選抜の制約：

- Role C近接枠：max_sim_P(i) ≥ T_high はスキップ（Regime A: 0.72／Regime B: 0.66）。0.50 < max_sim_P(i) < T_high はTopN全体で最大1件

- 中近接帯の通過要件（強化）：

- 0.50 < max_sim_P(i) < T_high の候補は、`comparative_advantage.delta_kpi`で主要KPI差分≥30%かつ、SPP/工程スロット（`spp.structure`／`spp.causal_chain`／`device_process_solution`／`stage`）のうち3スロット以上の明確差分を必須。未達は棄却

- 内部監査（内部保持）：`midband_pass_count`、`midband_reject_for_diff`

- タイブレーク：① max_sim_P(i)が低い方 → ② Σ freq_P(z)が小さい方 → ③ 仮説ID昇順

- score最大の候補をSに追加し、|S|={HYPOTHESIS_COUNT}まで反復

TopN相互制約（最終検査）

- 全ペア(i,j)について k(i,j) < 0.75 を満たす（相互制約では元の k）

- Role C近接の枠（最終確認；N=2〜10適用／N=1除外）：高近接（≥T_high）は0件、中近接（0.50〜T_high未満）は最大1件

- クラスタクォータ：原則TopN内で cid は一意（同一クラスタは最大1件）。例外：同一クラスタ2件目は `comparative_advantage.delta_kpi`で主要KPI差分≥30%を明示し、かつ相互制約（k<0.75）を満たす場合のみ許容。TopN全体でこの例外クラスタは最大1つ

- 距離・多様性の救済枠（軽量）：

- 頻度ペナルティで落選した候補のうち、max_sim_P(i) ≤ 0.40（遠距離）かつ MI_like_match_S(i)=0（未登場MI）、さらに cluster_key(i) がTopN未使用の新規keyを含む場合、TopN全体で最大 floor({HYPOTHESIS_COUNT}/5) 件まで救済（単回適用・反復なし）

Top仮説の必須整備（各件）

- Cap-ID指紋（Cap-XX + Cap-YY (+ ...)）

- SPP（structure≥2／property／performance／causal_chain 1〜2文）

- KPI（数値＋単位で3件以上）

- comparative_advantage（alt_refs／basis ≤120字／delta_kpi／moat_factor）

- trade_off（bind ≤40字／why_legacy_fails ≤60字／cost_of_inaction ≤60字）

- device_process_solution ≤120字／material_solution ≤120字

- competitors（1〜3件）、risk_seeds（≥1件）、killer_experiment_outline（各「閾値：数値＋単位」×1件以上）

KPI補完（handoff直前）

- KPIが2件の案は次の順で補完（重複除外）：1) `spp.performance`文中の「数値＋単位」抽出→KPI化 2) `killer_experiment_outline`の閾値が採用KPIに一致する場合に限り1件昇格 3) `comparative_advantage.delta_kpi`または`trade_off`に含まれる定量をKPI化

- 補完後、`kpi`の重複（同カテゴリ・同値）は除去

- いずれでも3件に満たなければTopNから除外し、score順の次点を繰り上げ追加（`handoff.count`と`topN`要素数の一致を維持）

Phase 2: Handoff JSONパッケージング

- 正規化ガード：

- KPI：形式を「数値＋単位」に統一。半角数値と単位の間に半角スペースを1つ。不等号・記号（>, <, ≥, ≤, ±, ～, ⇔ 等）は除去。負号（-）は数値として許容。単位例：kPa、Pa、℃、K、V、kV、A、mA、W/mK、nm、µm、mm、%など

- killer_experiment_outline：各要素を「閾値：数値＋単位」に統一。不等号・記号は除去。複合表現は主指標1つ。半角スペース1つ。負号（-）は許容

- 監査情報（ideation_total、ideation_breakdown、negative_scope_status、duplicates_discarded、weights）とTopNミニドシエをスキーマに従い構造化

- 内部監査（内部保持のみ）：

- discard_breakdown（negative_scope／gate_s／gate_m／pool_tpool／cap_limit／pair_limit／far_ratio／cluster_quota／rolec_high／pairwise_violation／final_replacement）

- diversity_metrics（cid_count／cid_entropy／TopN_mechanism_gini／RoleC_far_ratio）

- rc_regular_hits／rc_hotspots／midband_pass_count／midband_reject_for_diff／cluster_rc_pressure（各TopのP_cluster_RC）

- MKT関連監査（内部保持）：MKTごとのTopN分布、MKTごとの代表Cap（design/platform）分布、および各MKT×Capのρ_{C,m}(z)とP_MKTCap_RCの値を記録し、特定MKT×Capへの過度な集中を可視化

- ナラティブ本文は任意。handoff JSONは本文内に単一の```jsonフェンスで出力

3. 出力フォーマット（Strict Output Format）

- 本文中に```jsonフェンスで囲まれたhandoff JSONが1ブロックだけ存在（複数フェンス禁止）

- スキーマ（必須、キー名厳守）：

{

"handoff": {

"version": "1.3",

"count": {HYPOTHESIS_COUNT},

"audit": {

"ideation_total": 0,

"ideation_breakdown": {},

"negative_scope_status": "OK（重複x案破棄: Sしきい値判定）",

"duplicates_discarded": 0,

"weights": {"I":0.25,"M":0.30,"C":0.25,"L":0.10,"U":0.10}

},

"topN": [

{

"rank": 1,

"title": "仮説タイトル",

"tag": "Core|Strategic|Moonshot",

"scores": {

"I": 0.00,

"M": 0.00,

"C": 0.00,

"L": 0.00,

"U": 0.00,

"composite": 0.00,

"selection_score": 0.00

},

"cap_id_fingerprint": "Cap-XX + Cap-YY (+ ...)",

"industry": "業界",

"domain": "分野",

"stage": "素材が活躍する舞台（必要に応じて「; 」連記可）",

"role": "素材の役割（必要に応じて「; 」連記可）",

"raw_materials": ["原料(物質)1","原料(物質)2"],

"form_factor": "成型体/モジュール形態",

"hypothesis_summary": "事業仮説概要（≤300字）",

"trade_off": {

"bind": "板挟みの定義（≤40字）",

"why_legacy_fails": "既存が解けない理由（≤60字）",

"cost_of_inaction": "放置時のコスト（≤60字）"

},

"spp": {

"structure": ["Cap-XX","Cap-YY"],

"property": "値/レンジの要約",

"performance": "工程/顧客KPI（定量）",

"causal_chain": "S→P→Performanceの因果（1〜2文）"

},

"target_slice": "顧客・用途の最小断面",

"device_process_solution": "工程・適合の骨子（≤120字）",

"material_solution": "構造・配合・界面設計（≤120字）",

"kpi": [

"KPI1（数値＋単位）",

"KPI2（数値＋単位）",

"KPI3（数値＋単位）"

],

"comparative_advantage": {

"alt_refs": ["既存技術A","代替手法B"],

"basis": "優位の根拠（性能/量産適合/規制・安全環境/知財防衛/切替摩擦；≤120字）",

"delta_kpi": "主要KPI差分（例：PDIV +50%、Df −30%）",

"moat_factor": "Cap-ID起因の非再現性"

},

"moat_outline": "差別化ポイント骨子（≤120字）",

"competitors": ["競合A","競合B"],

"risk_seeds": ["技術リスク種","市場リスク種","規制リスク種"],

"killer_experiment_outline": [

"閾値：数値＋単位",

"閾値：数値＋単位",

"閾値：数値＋単位"

],

"mkt_candidates": [

"MKT-XXX|短名|既存が解けない理由|放置コスト"

]

}

]

}

}

- 受け入れテストに必要なキーは必須。上記以外の内部監査キーはJSONに含めないこと（内部保持のみ）

4. 受け入れテスト（Acceptance as Code）

- ① 本文中に```jsonフェンスで囲まれたhandoff JSONが1ブロックのみ存在（複数フェンス禁止）

- ② `handoff.version` が存在

- ③ `handoff.count` == `handoff.topN` の要素数

- ④ `audit.ideation_total` ≥ 50

- ⑤ `audit.negative_scope_status` が次のいずれかに完全一致：「OK（重複x案破棄: Sしきい値判定）」 または 「重大重複なし」

- ⑥ `audit.duplicates_discarded` が整数（0以上）

- ⑦ 各要素について：`cap_id_fingerprint` 形式遵守／`spp.structure` ≥ 2／`comparative_advantage.alt_refs` ≥ 1／`risk_seeds` ≥ 1／`kpi` 3件以上「数値＋単位」／`killer_experiment_outline` 各要素が ^閾値：-?\d+(\.\d+)?\s[^\d\s]+$ に合致

- ⑧ `scores`（I/M/C/L/U/composite/selection_score）は0.00–1.00、`composite`は 0.25·I + 0.30·M + 0.25·C + 0.10·L + 0.10·U と一致（`selection_score`はスコア式に基づく内部スコアの正規化値）

- ⑨ `tag` は「Core／Strategic／Moonshot」のいずれかに完全一致

5. Glossary-Lite

- S→P→Performance：Structure→Property→Performance の因果鎖

- I（Inevitability）：放置不可の必然性（顧客Pain、不可逆トレンド、法規制 等）

- M（Material Necessity）：素材でしか解けない必然性（装置/ソフトでは原理到達不可）

- C（Comparative Advantage）：他素材ソリューションに対する相対優位（性能・量産適合・規制/安全環境・知財・切替摩擦）

- L（Logical Consistency）：因果整合性

- U（Unit Economics）：単位経済性

- S（Similarity）：sim_mech／sim_cap／sim_kpi／sim_problemの加重和（0.40/0.30/0.15/0.15）

- TopN相互制約「S < 0.75」＝全ペア(i,j)の k(i,j) < 0.75

- Role C近接：max_sim_P(i)=max_{p∈RoleC} k(i,p)。T_high帯混入禁止（Regime A: 0.72／Regime B: 0.66）／中近接は上限1件

- Role C由来の頻度ペナルティ：

- Cap-ID頻度：ρ0_cap_RC=0.04、p_cap_rc=2、μ_RC=4.5（P_cap_RCで使用）
  （Role C内で同じCapが全体の4%を超えて使われ始めると二乗カーブで強くペナルティ）

- Cap type別重み：baseline=1.0／unlocker=1.0／design=1.5／platform=1.3（P_cap_RCとP_MKTCap_RCで使用）

- 同系素材クラスタ頻度：ρ0_cluster_RC=0.02、p=2、μ_cluster_RC=6.0（P_resource_cluster_RCで使用）
  （同じraw_materials×material_systemクラスタがRole C内の2%を超えて使われ始めると、素材系の固定化を抑制）

- MKT単体頻度：ρ0_MKT=0.04、μ_MKT=8.0（P_MKT_RCで使用）
  （特定MKTがRole C全体の4%を超えたあたりから二乗カーブでペナルティが増加し、20〜30%の高占有MKTはscoreを大きく押し下げる）

- 未出MKT加点：Role Cサイズが40以上のとき、Role C内で一度も登場していないMKT-IDを主MKTとする仮説 i には A_MKT_RC(i)=α_MKT_RC·new_MKT_RC(i) の加点（α_MKT_RC=0.10）を与え、ロングテール側のMKT探索を促進する

- MKT×Cap頻度（二乗型）：

- ρ0_MKTCap=0.06、μ_MKTCap=6.0（P_MKTCap_RCで使用）
  （同じMKT内で特定Capの割合が6%を超えると緩く効き始め、10%前後でサブ要因、15〜20%で目に見える減点、30%以上ではかなり強く抑制）

- ρ_{C,m}(z)=freq_C_pair(m,z)/max(freq_C_MKT(m),1) は「部分一致」（同じMKTで Cap z を含む仮説の割合）

- ラテント概念クラスタ（cid）：k_cluster（0.40/0.22/0.13/0.13/0.12）で連結成分を形成しPoolに付与。TopNは原則cid一意（例外はdelta_kpi≥30%かつ相互制約を満たす場合のみ、当該例外クラスタ最大1つ）

- MI-like：`mechanism@interface` の離散キー（mechanismは`device_process_solution`または`spp.causal_chain`の主動詞＋補語1、interfaceは`stage`主要名詞1〜2の正規化連結）

- クラスタRC圧 P_cluster_RC：TopN候補の属するクラスタcidと、そのクラスタ内の既出仮説がRole C内クラスタと類似している度合いに基づく圧力項。Role C側に似たクラスタが既に存在する場合、そのクラスタに属する新規候補に追加減点として作用する（Role C由来の「クラスタ単位」の固定化抑制）。

- 頻度系ペナルティ合算上限（安定化）：pen_freq_total_capped(i)=min( P_cap_RC(i)+P_resource_cluster_RC(i)+P_MKT_RC(i)+P_MKTCap_RC(i), 0.80 )

- MKT×Capの設計思想：

- 同じMKTに対して同じCap（特にdesign/platform）を3件以上連発することを避けるため、「そのMKT内でのCap割合 ρ_{C,m}(z)」に対し 0.06 から緩くペナルティを効かせ、0.10前後でサブ要因として効き始め、0.15〜0.20で目に見える減点、0.30以上ではかなり強い抑制となる二乗カーブ（μ_MKTCap=6.0）を採用している。これにより、強いMKTを厚く攻めつつも、同一MKT×同一Capの連作を2本目以降から段階的に不利にする。

- Cap type別重みにより、design/platform Capが同じMKTで偏重されることを特に強く抑制し、baseline/unlockerはやや緩めに扱う

- MKT-IDと主MKT割当：

- MKT-IDは常にRole Aのリスト中に明示されているID（例：MKT-001〜）のみを使う

- 各仮説 i について、`trade_off.bind + why_legacy_fails + cost_of_inaction` と各MKT項目の「trade_off／why_unresolved／背景」などの問題記述との sim_problem を計算し、類似度最大のMKT-IDを m(i) として割り当てる

- μ_cap：Pool内Cap使用頻度ペナルティ P_cap 用の係数（Regime A/Bで共通）

- μ_RC：Role C由来Cap頻度ペナルティ P_cap_RC 用の係数（Regimeに依らずグローバルに4.5固定）

Scoring Micro-anchors（各軸の目安）

- I（Inevitability）：5＝独立根拠≥2＋タイムライン≤24ヶ月＋放置コスト≥30%／3＝独立根拠1または>24ヶ月／1＝根拠薄弱

- M（Material Necessity）：5＝唯一到達（代替主要KPI<80%）／3＝代替≥80%／1＝代替≥90%

- C（Comparative Advantage）：5＝delta_kpi≥30% 又は量産/規制で明確優位／3＝+10〜20%／1＝同等以下

- L（Logical Consistency）：5＝矛盾なし＋KPI整合／3＝仮定多い／1＝矛盾

- U（Unit Economics）：5＝粗利≥40%かつスケール成立／3＝≈30%／1＝構造的赤字

6. エラーメッセージ

- 必須ファイル不足時：「エラー: 必須ファイルが不足しています。」

- 内部検査不合格時：該当箇所を自動再構成し、受け入れテストに合格する唯一のhandoff JSONを出力（JSONフェンスは単一。ナラティブ本文の修正は任意）
```