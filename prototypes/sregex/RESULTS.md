# sregex-like 多模式 matcher 原型实测

本报告由 `scripts/report.py` 从 `output/` 原始 JSON 生成；可复跑。所有吞吐是固定 context 的逐行 microbenchmark，尚未运行完整 Sublime/syntect parser，也未测 Sublime 本体。

## 主要结果

32 个所选 contexts 的合计时间比：B/A-Onig 加速 **1.36×**，B/A-fancy 加速 **2.44×**。逐 context 的 B/A-Onig 范围 0.46–10.27×，几何平均 1.92×；28/32 个 contexts 比缓存后的 Onig 快。这里的合计时间是每个 context 三次完整扫描时间的中位数之和。

正确性检查覆盖 740 个 contexts，去重实际执行 222,857,936 个 query，对应完整语料×contexts 的 1,970,092,128 个参考 query。546 处差异全部由六种字符的 Unicode word-boundary 语义造成，独立重放已逐 context 全部归因；除此之外成功查询的 spans、pattern priority 和 captures 相同。Markdown 另有七个起点使 A/B 的 Onig 都触发 retry limit，未能验证后续该行查询，不能声称所有输入严格等价。

| 语言 | contexts | 累计扫描 MB | A-Onig MB/s | A-fancy MB/s | B MB/s | B/A-Onig | B/A-fancy |
|---|---|---|---|---|---|---|---|
| rust | 4 | 40.00 | 2.65 | 4.12 | 1.43 | 0.54× | 0.35× |
| javascript | 4 | 40.00 | 1.51 | 0.40 | 2.50 | 1.65× | 6.19× |
| python | 4 | 20.15 | 1.04 | 0.99 | 4.07 | 3.90× | 4.11× |
| cpp | 4 | 20.47 | 1.63 | 0.67 | 2.21 | 1.35× | 3.29× |
| json | 4 | 80.47 | 2.43 | 3.56 | 3.53 | 1.45× | 0.99× |
| markdown | 4 | 20.00 | 1.70 | 0.83 | 4.37 | 2.57× | 5.29× |
| yaml | 4 | 20.02 | 1.20 | 0.58 | 1.43 | 1.18× | 2.45× |
| html | 4 | 20.01 | 5.26 | 2.59 | 8.45 | 1.61× | 3.27× |

Rust statements 的完整 trajectory 存在上述已知 Unicode 差异，以上 32-context 全量比值包含它。仅取 query/match/checksum 全部相同的 31 个 contexts，B/A-Onig 为 **1.69×**、B/A-fancy 为 **3.23×**；这只是等价工作量子集，不能替代全量结果或真实 parser 的 context 频率分布。

## 环境与版本

Apple M1 Pro，16 GB，proc 8:6:2；macOS 26.1 / arm64。`rustc 1.99.1 (58cf8f9c3 2026-10-13)`；`cargo 1.99.1 (5f94df478 2026-08-27)`。吞吐单线程，release，默认优化，无 `target-cpu=native`，`debug=1` 仅保留符号。差分的 contexts 用四个 worker 并行，吞吐测量时无差分任务运行。

| crate | Cargo.lock 版本 |
|---|---|
| regex-automata | 0.4.18 |
| regex-syntax | 0.8.11 |
| fancy-regex | 0.20.2 |
| onig | 6.5.3 |
| onig_sys | 69.9.3 |
| yaml-rust2 | 0.13.0 |
| memchr | 2.8.3 |
| aho-corasick | 1.1.5 |

语法来源：本机官方 Packages，commit `3f8b2c544e8f74a76e7212da449dcfe3f01b2245`。直接依赖由本次 `cargo search` 核对 stable，确切依赖锁定在 `Cargo.lock`；环境记录在 `output/environment.json`。

## 方法与覆盖边界

Loader 展开递归变量、同文件 includes，并保持 ordered priority；环只截断当前递归栈上的边。支持匿名 push/set/branch contexts；支持同目录继承和 `meta_prepend`/`meta_append`，因此加载的是完整 JavaScript/HTML Plain 基类加覆盖。跳过 prototype 自动注入、显式 prototype include、跨文件/`scope:` include、embed 的语法切换及 escape、with_prototype 传播、branch/fail 状态。保留 embed 规则自己的 match。C++ 通过 scope:source.c 引入的 C 规则因此缺席，不能将这里的 C++ 数值当作完整 C++ 高亮器表现。

A-Onig 为原始 pattern 的 `Syntax::default()`（Ruby）逐条搜索；A-fancy 使用相同 Onig 语法归一化后的 fancy-regex。二者保留每条规则在当前行的下一次 match/no-match，`pos <= cached_start` 时复用；发现当前 pos 的 winner 后跳过低优先级规则。B 使用同样的 fallback cache，另有已验证 union winner 的缓存。`\G`/`\K` 规则不复用 search cache。所有引擎都只为最终赢家提取 captures，B 用原始 Onig anchored captures，以完整保留 lookahead 内的 captures。

Onig 搜索需要 Region 来获取 group 0 的 end，内部仍会维护 captures；这里“只为赢家提取”指不为败者复制/返回全部 capture spans。B 的 union NFA 禁用了 explicit captures，fallback 的 Region 用法与 A 一致。

B 每 context 一个 `regex_automata::meta::Regex` 多模式 union，`MatchKind::LeftmostFirst`，只保留 implicit group 0，32 MiB NFA limit，16 MiB hybrid cache 上限。meta 是 DFA/NFA/PikeVM 的自适应组合，不能把每次查询都称为纯 DFA；Unicode、容量限制或短文本都可能触发其他执行路径。额外 guard matcher 只做 anchored 验证。

边界 lookahead 从 core 剥离，分别在 match.start / match.end 验证。失败时按 context priority 在同一 start 做原始 Onig anchored 检查，再决定推进，处理 lower-priority tie、alternative 和更短 core。纯正向 lookahead 的零宽 core 默认用一个 positive guard body 定位候选 start，再还原零宽 end；`--strict-empty-cores` 可关闭此优化。它避免空 core 每字符遮蔽其他规则。

Driver 为 `pos=0; find(line,pos); pos=max(end,pos+1)`，无匹配换行；保留换行符并传完整行与 byte offset。空匹配后若 pos 落在 UTF-8 字符中，推进到下一个字符边界。benchmark 对每条实际 corpus 行执行，先用最多 64 KiB 预热同一 engine cache，再对完整语料扫描三次取中位数；contexts 间轮换引擎顺序。计时包含缓存 reset、赢家 captures、driver 和 checksum，排除 YAML/I/O/编译。

全语料差分按完整行去重，按频次恢复查询总数；缓存逐行清空、driver 无跨行状态，所以相同行具有完全相同的所有 query 轨迹。相同 ordered pattern-ID 列表的 contexts 复用同一差分结果。这是确定性压缩覆盖，不是抽样；性能计时不使用去重。差分在 A-Onig 的每个 query 起点对比 `(start,end,pattern_index,全部 capture spans)`，包括 no-match。异常独立统计，不伪装成 no-match 或语义差异。

### 语料

| 语言 | MB | 来源 | SHA-256（前 16 位） |
|---|---|---|---|
| rust | 10.000 | existing generated corpus | d5f3d0c59a12ba34 |
| javascript | 10.000 | existing generated corpus | 400c06f10bba70bb |
| python | 5.037 | official syntax tests, scope assertions stripped, repeated whole seeds | afbb08566b90b602 |
| cpp | 5.117 | official syntax tests, scope assertions stripped, repeated whole seeds | c0eb8c7c021829f6 |
| json | 20.118 | existing generated corpus | ddf09db9da7c68a1 |
| markdown | 5.000 | existing generated corpus | 62fde7c6ddbd39eb |
| yaml | 5.006 | official syntax tests, scope assertions stripped, repeated whole seeds | da7202d0739a1dee |
| html | 5.002 | official syntax tests, scope assertions stripped, repeated whole seeds | 75d5a85f7957eace |

Rust/JavaScript/JSON/Markdown 使用现有生成语料；其余来自官方 Packages 的 syntax_test 源代码，去除 scope assertion 行，再按完整 seed 重复到至少 5 MB。保留注释和测试中的不完整/错误代码，适合语法高亮场景，但不是自然项目频率分布。完整 hash、seed 和文件清单见 `output/corpora.json`；生成文件在 gitignored `data/`。

## 分类

| 语言 | 全部 contexts | ≥5 patterns | 唯一展开 regex | (a) DFA | (b) 边界 lookahead | (c) fallback | Onig 编译失败 |
|---|---|---|---|---|---|---|---|
| rust | 200 | 110 | 193 | 148 | 39 | 6 | 2 |
| javascript | 323 | 73 | 255 | 69 | 140 | 46 | 0 |
| python | 533 | 231 | 257 | 177 | 65 | 15 | 0 |
| cpp | 263 | 127 | 224 | 119 | 92 | 13 | 1 |
| json | 24 | 9 | 32 | 27 | 3 | 2 | 0 |
| markdown | 341 | 106 | 267 | 114 | 106 | 47 | 5 |
| yaml | 62 | 28 | 87 | 31 | 15 | 41 | 2 |
| html | 125 | 56 | 62 | 28 | 18 | 16 | 0 |

计数是去重后的展开 pattern 字符串，包含无法独立编译的动态 backref 模式；并非原始 match 规则次数，也不是全仓库 13,813 条规则的重新统计。各 context 的展开规则数与实际 fast/boundary 数在原始 JSON 中。前端保守地将内部 lookaround、lookbehind、backrefs、subroutine、原子组、possessive quantifier、\G/\K、不支持的 POSIX 类/flags 留给 Onig。

## 差分正确性

| 语言 | 检查 contexts | 实际去重 query | 全语料×contexts query | B 不同 query | fancy 差异 / 检查范围 | Onig 异常 | B 异常（A 成功时） |
|---|---|---|---|---|---|---|---|
| rust | 110 | 198,388,733 | 400,965,658 | 546 | 6 / 4 contexts | 0 | 0 |
| javascript | 73 | 15,578,656 | 350,881,183 | 0 | 0 / 4 contexts | 0 | 0 |
| python | 231 | 3,126,076 | 652,938,735 | 0 | 0 / 4 contexts | 0 | 0 |
| cpp | 127 | 2,861,303 | 141,252,775 | 0 | 0 / 4 contexts | 0 | 0 |
| json | 9 | 1,447 | 88,470,570 | 0 | 0 / 5 contexts | 0 | 0 |
| markdown | 106 | 2,586,124 | 148,457,574 | 0 | 0 / 4 contexts | 7 | 0 |
| yaml | 28 | 117,120 | 69,192,825 | 0 | 0 / 4 contexts | 0 | 0 |
| html | 56 | 198,477 | 117,932,808 | 0 | 0 / 6 contexts | 0 | 0 |

不同 query/异常列按 context×唯一行计，重复语料还原后的差异计数另存 `represented_mismatches_*`。实际去重 query 还消除了具有相同 ordered pattern 列表的 contexts；其余列保留每个 context 的独立覆盖含义。异常后的剩余该行查询无法完整验证，不能据此声称所有输入严格等价。

B 共检查 740 个 ≥5 patterns contexts。fancy 仅检查 benchmark 所选 contexts 及 ordered patterns 完全相同的 contexts，因此 JSON/HTML 的检查数可超过四个；未检查的 fancy context 不计作零差异证明。

Rust 的全部 546 个 B 差异来自六条含 `²`、`³`、`¹`、`¼`、`½`、`¾` 的源代码行，均为 Unicode word-boundary 导致的 pattern priority 变化。例：`    163, // '²'` 的 byte pos=13，A-Onig 的 `\b(?:|Box|Option|...)\b` 命中空串，而 Rust 的 `\b` 不把该字符视作 word，B 找到后面的引号。它们属于已明确识别的 Onig-vs-Rust 语义差异，没有用“Unicode”泛指其他未知错误。`statements` 的 fancy 对照也出现相同六处差异。

六条源代码行的独立完整 driver 重放，逐 context 的 mismatch 计数与全语料计数完全相等，复现 546 / 未归因 0，记录在 `output/recheck.json`。因此这些语义差异以外，所有成功比较的 query 的 spans、priority 和 captures 均相同。

Markdown 的七个异常都来自 `markdown` context 的 rule 39 / 全局 pattern gid 258（展开的表格行 lookahead，含多层 inline-code/emphasis 分支），原始 Onig 报 `retry-limit-in-match over`。例如 `let hir = parse("a|b").unwrap();`，pos=0。差分 driver 在 A 报错后结束该行，所以表中 B 异常只统计 A 成功的查询；随后独立复测这七个起点，B 的 Onig fallback 也全部触发相同 retry-limit，详见 `output/recheck.json`。没有改大 retry limit，也未把异常算成 mismatch；这些行后续 query 未能验证。十条动态 context backref 无法独立编译，A/B 一致排除，列在上一节和 JSON 中。

| 语言 | context | B mismatch | fancy mismatch | B 原因类别 | fancy 原因类别 |
|---|---|---|---|---|---|
| rust | block-body | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | closure-explicit-body | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | closure-implicit-body | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | declaration-identifier | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | declaration-type | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | enum-discriminant | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | fn-body | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | fn-where | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | generic-angles-body | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | group-body | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | group-tail | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | hrtb/@0:push | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | identifier-names | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | identifier-path | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | identifier-path-second | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | identifiers | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | impl-body | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | impl-trait-types | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | impl-where | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | in-path-identifier-names | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | in-path-type-names | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | inside-struct-tuple | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | macro-identifiers/@1:push | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | macro-transcriber-block/@1:set | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | macro-transcriber-block/@2:set | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | macro-transcriber-block/@3:set | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | module-block/@0:set | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | pattern-param/@12:push | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | return-type/@0:push | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | statements | 6 | 6 | {'pattern_priority': 6} | {'pattern_priority': 6} |
| rust | statements/@14:push | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | static-identifier-path | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | static-identifier-path-second | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | struct-classic-body/@5:push | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | support-type-names | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | trait-block/@0:set | 6 | 0 | {'pattern_priority': 6} | {} |
| rust | type-declaration-body | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | type-names | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | type-names-relaxed | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | type-path | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | type-path-second | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | type-slice-or-array/@0:push | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | types | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | types-relaxed/@2:push | 12 | 0 | {'pattern_priority': 12} | {} |
| rust | types/@13:push | 18 | 0 | {'missing_or_extra_match': 18} | {} |
| rust | types/@6:push | 18 | 0 | {'missing_or_extra_match': 18} | {} |

| 语言 | context | A-Onig 异常 | B 异常 | fancy 异常 |
|---|---|---|---|---|
| markdown | markdown | 7 | 0 | 未测 |

### 语法与语义修正

已处理 `\h/\H` 为十六进制字符、Unicode POSIX classes（Rust 原生 POSIX classes 是 ASCII）、Onig `\w`、`(?x)` 的注释/空格、Onig `(?m)` → Rust `(?s)`、`\Z`、`{,n}` → `{0,n}`、Ruby `\</\>` 为字面尖括号。Onig 的 `^` 不匹配终端换行后的虚拟空行：B 仅对这类边界 candidate 复核 Onig，fancy 用等价的零宽 assertion 改写。Rust 将 `a++` 当作重复量词组合，不能视作 Onig possessive 的等价翻译，明确 fallback。

仍需统一 Unicode tables、`\b` 的 word 集合及多字符 case folding：Rust 的 word 集合包括 Join_Control，Onig Ruby 不包括；Onig 的 ignore-case 可有多字符折叠，Rust 通常为 simple folding。仅编译通过并不证明等价。命中这些特征时生产实现应统一字符属性/边界实现或保守 fallback；具体实测差异以本节上表和原始 samples 为准，不能把任意 mismatch 自动归入 Unicode。

独立探针 `output/semantics.json` 还验证了 `\b` 在 `a<U+200C>b` 上的差异，以及 `(?i:ss)` 在 `ß` 上 A-Onig 成功而 B/fancy 不匹配。这两个探针没有加入吞吐 corpus；本次 corpus 内差异仅为上一段六种字符。

## 逐 context 性能

| 语言 | context | patterns | A-Onig MB/s | A-fancy MB/s | B MB/s | B/A-Onig | B/A-fancy |
|---|---|---|---|---|---|---|---|
| rust | declarations | 15 | 20.52 | 19.53 | 44.33 | 2.16× | 2.27× |
| rust | keywords | 22 | 7.36 | 32.14 | 64.82 | 8.81× | 2.02× |
| rust | numbers | 8 | 22.18 | 23.48 | 227.69 | 10.27× | 9.70× |
| rust | statements | 128 | 0.78 | 1.18 | 0.36 | 0.46× | 0.31× |
| javascript | expression-begin | 41 | 1.04 | 0.28 | 1.64 | 1.58× | 5.80× |
| javascript | expression-end | 20 | 1.62 | 0.67 | 2.63 | 1.62× | 3.91× |
| javascript | literal-variable | 43 | 1.96 | 0.28 | 3.43 | 1.75× | 12.23× |
| javascript | statement | 28 | 1.81 | 0.75 | 3.10 | 1.72× | 4.14× |
| python | class-definition-base-list-body | 54 | 1.06 | 1.02 | 4.00 | 3.76× | 3.94× |
| python | function-call-argument-list-body | 53 | 1.12 | 1.05 | 3.84 | 3.44× | 3.67× |
| python | maybe-set-body | 55 | 1.09 | 1.31 | 4.47 | 4.11× | 3.41× |
| python | statements | 84 | 0.93 | 0.75 | 4.02 | 4.34× | 5.37× |
| cpp | expressions | 58 | 1.83 | 0.82 | 6.64 | 3.63× | 8.14× |
| cpp | global | 72 | 1.52 | 0.68 | 1.38 | 0.91× | 2.04× |
| cpp | preprocessor-if-branch-global | 79 | 1.46 | 0.54 | 1.27 | 0.87× | 2.34× |
| cpp | statements | 61 | 1.80 | 0.71 | 6.74 | 3.75× | 9.51× |
| json | array-body | 11 | 2.06 | 5.17 | 3.73 | 1.81× | 0.72× |
| json | expect-object-value | 9 | 2.03 | 2.11 | 2.98 | 1.47× | 1.41× |
| json | main | 8 | 7.05 | 20.24 | 11.97 | 1.70× | 0.59× |
| json | object-value-end | 10 | 1.92 | 2.45 | 2.24 | 1.17× | 0.91× |
| markdown | fenced-code-block-body | 46 | 0.63 | 4.53 | 2.53 | 4.03× | 0.56× |
| markdown | list-paragraph | 42 | 3.96 | 0.64 | 7.72 | 1.95× | 12.08× |
| markdown | paragraph | 42 | 4.04 | 0.65 | 7.76 | 1.92× | 12.01× |
| markdown | strikethrough-content | 42 | 3.83 | 0.67 | 3.82 | 1.00× | 5.73× |
| yaml | block-node-12 | 26 | 1.17 | 0.56 | 1.38 | 1.18× | 2.49× |
| yaml | flow-mapping-body | 26 | 1.26 | 0.65 | 1.49 | 1.18× | 2.28× |
| yaml | flow-sequence-body | 26 | 1.26 | 0.65 | 1.53 | 1.22× | 2.36× |
| yaml | main | 29 | 1.13 | 0.50 | 1.32 | 1.17× | 2.64× |
| html | main | 24 | 7.67 | 1.67 | 13.12 | 1.71× | 7.84× |
| html | tag | 14 | 9.50 | 2.66 | 12.27 | 1.29× | 4.62× |
| html | tag-attributes | 6 | 2.21 | 4.91 | 3.98 | 1.80× | 0.81× |
| html | tag-html | 10 | 13.63 | 2.71 | 15.53 | 1.14× | 5.74× |

contexts 选择优先指定常见入口/表达式，再补最大且 ordered pattern 列表不同的 contexts，最终每语言四个；不是根据实测速度挑选。原始三次时间保存在 `output/bench.json`。速度比 <1 表示 B 更慢。

### 各语言完整指标

| 语言 | engine | queries 合计 | 时间中位数之和 s | queries/s | MB/s |
|---|---|---|---|---|---|
| rust | onig | 5,374,703 | 15.066351 | 356,736 | 2.65 |
| rust | fancy | 5,374,691 | 9.703644 | 553,884 | 4.12 |
| rust | multi | 5,374,691 | 27.905949 | 192,600 | 1.43 |
| javascript | onig | 27,124,038 | 26.414379 | 1,026,866 | 1.51 |
| javascript | fancy | 27,124,038 | 99.177874 | 273,489 | 0.40 |
| javascript | multi | 27,124,038 | 16.027159 | 1,692,380 | 2.50 |
| python | onig | 15,000,957 | 19.326952 | 776,168 | 1.04 |
| python | fancy | 15,000,957 | 20.341622 | 737,451 | 0.99 |
| python | multi | 15,000,957 | 4.952050 | 3,029,242 | 4.07 |
| cpp | onig | 3,731,475 | 12.527987 | 297,851 | 1.63 |
| cpp | fancy | 3,731,475 | 30.481112 | 122,419 | 0.67 |
| cpp | multi | 3,731,475 | 9.275460 | 402,295 | 2.21 |
| json | onig | 63,999,980 | 33.067820 | 1,935,416 | 2.43 |
| json | fancy | 63,999,980 | 22.601222 | 2,831,704 | 3.56 |
| json | multi | 63,999,980 | 22.804938 | 2,806,409 | 3.53 |
| markdown | onig | 2,269,100 | 11.761845 | 192,920 | 1.70 |
| markdown | fancy | 2,269,100 | 24.176239 | 93,857 | 0.83 |
| markdown | multi | 2,269,100 | 4.574415 | 496,042 | 4.37 |
| yaml | onig | 13,497,861 | 16.648919 | 810,735 | 1.20 |
| yaml | fancy | 13,497,861 | 34.428858 | 392,051 | 0.58 |
| yaml | multi | 13,497,861 | 14.050802 | 960,647 | 1.43 |
| html | onig | 5,020,050 | 3.804533 | 1,319,492 | 5.26 |
| html | fancy | 5,020,050 | 7.739405 | 648,635 | 2.59 |
| html | multi | 5,020,050 | 2.368357 | 2,119,634 | 8.45 |

每语言指标是四个固定 contexts 扫描同一 corpus 四次的合计；总时间为四个 context 的单次完整扫描时间中位数之和，queries 为对应四个计数之和。不是某个真实 parser 在该语言上的动态 context 混合吞吐。

以下 benchmark trajectories/checksum 有差异，不能把速度比当作等价工作量的结论：

| 语言 | context | onig checksum | fancy checksum | B checksum |
|---|---|---|---|---|
| rust | statements | 6649784453327764499 | 8848093029763121935 | 8848093029763121935 |

## 启动编译与内存

| 语言 | YAML/load ms | A-Onig ms | A-fancy+翻译 ms | B union/guards ms | B 总 ms |
|---|---|---|---|---|---|
| rust | 4.23 | 3.80 | 42.76 | 575.96 | 582.58 |
| javascript | 4.52 | 23.75 | 79.48 | 308.69 | 342.67 |
| python | 7.47 | 1.99 | 32.57 | 332.20 | 336.77 |
| cpp | 6.34 | 14.86 | 126.42 | 2938.32 | 2961.39 |
| json | 1.06 | 0.09 | 2.17 | 3.61 | 3.83 |
| markdown | 5.63 | 7.55 | 44.03 | 177.16 | 187.27 |
| yaml | 1.22 | 0.50 | 10.42 | 10.58 | 11.69 |
| html | 1.92 | 1.38 | 7.11 | 46.10 | 48.20 |

启动列是三次独立进程的中位数：每语法唯一展开 regex 共享编译，includes 重复不重复编译 A；B eager 编译所有 contexts 和 guards，B 总计包括 Onig capture/fallback pool、前端、所有 union/guards。A-fancy 在有效规则上若有编译失败会明确 fallback 到 Onig；逐 context 的 `fancy_fallback_rules` 与 runtime fallback 计数保留在 JSON。这里 A-fancy+翻译列未计共同保留的 Onig safety pool，需结合 fallback 计数解释。

| 语言 | A-Onig RSS MiB | A-fancy RSS MiB | B RSS MiB | B 增量 RSS MiB | B ΣAPI memory_usage MiB | B 初始 cache MiB |
|---|---|---|---|---|---|---|
| rust | 6.7 | 19.0 | 162.7 | 158.7 | 95.9 | 38.2 |
| javascript | 9.3 | 29.4 | 86.7 | 82.1 | 45.8 | 5.5 |
| python | 7.7 | 18.1 | 95.7 | 90.4 | 48.3 | 16.6 |
| cpp | 11.7 | 51.9 | 370.1 | 365.2 | 555.6 | 59.1 |
| json | 3.3 | 5.5 | 5.9 | 3.4 | 0.5 | 0.2 |
| markdown | 7.8 | 20.9 | 47.8 | 42.6 | 20.0 | 4.4 |
| yaml | 4.4 | 9.3 | 9.0 | 6.1 | 1.0 | 0.3 |
| html | 4.6 | 7.6 | 14.4 | 11.3 | 3.6 | 1.1 |

内存分别用新子进程测量，每次只加载一个语法和一个 engine，不载入 corpus；B 保留该语法所有 compiled contexts 和初始 per-context union caches。RSS 含共享库、allocator、YAML/前端；增量是加载 engine 后减去 loader 后的 RSS。API 列是每个 meta/guard 对象 memory_usage 的和，可能重复计算共享结构，并包含未完全驻留的容量，不能视作独占物理 heap；因此 C++ 的 API 之和可以大于 RSS。cold cache 会在扫描时增长，16 MiB 是配置的每个 hybrid DFA cache 上限，forward/reverse caches 可分别占用。fancy 进程保留 Onig 安全副本，故其 RSS 不是纯 fancy 最小值。peak RSS 另存原始 memory.json。

## 对真实引擎的结论与风险

多模式搜索的收益由 context 和候选形态决定。宽关键字集合通常能分享扫描与 literal prefilter；规则少、零宽规则密集、长 Unicode identifiers、复杂 guards 或 Onig fallback 较多时，候选验证、PikeVM/反向定位和 captures 会吞掉收益。不能从总体正则覆盖率推出“高亮一定快六倍”。

本次 Rust statements 只有 0.46×，拖累 Rust 四个 contexts 合计到 0.54×；C++ global / preprocessor-if-branch-global 为 0.91× / 0.87×；Markdown strikethrough-content 约 1.00×，可视为持平。JSON 的 B 虽胜 Onig，合计与 fancy 基本持平（0.99×）。相对而言 Python 合计 3.90×、Markdown 2.57×、Rust numbers 10.27×，显示收益高度依赖工作负载。C++ eager 编译 2.96 秒、RSS 370.1 MiB，相比 A-Onig 14.86 ms / 11.7 MiB，启动和内存成本不能忽略。

应 lazy 编译活跃 context，共享相同 ordered pattern 集合及 guard matcher；按 NFA 大小配置 cache，并对 eager 内存设限。首次命中成本、cache thrashing、非 ASCII 触发的慢路径需独立观测。当前为每 context 重复编译大量相似 guards，适合验证方法，不适合直接放入编辑器启动路径。

生产实现需要完整 Onig front end/HIR 语义验证，按 rule priority 输出可验证的候选集合，避免失败 guard 触发 O(patterns) anchored 修复。需要处理可变长度 core 的回溯、零宽循环、同起点 tie、lookahead captures、动态 delimiter backrefs、\G/\K 与 search-start、Unicode boundaries/full folding、CRLF/多行输入和 retry-limit。可将常见有限 lookahead 编入 automaton/候选 predicate，剩余规则用 Onig。

真正 parser 还要处理 context stack、prototype/include/embed、branch/fail 与 syntax inheritance 的全部规则；本次没有接入 syntect ParseState interception。还应在真实项目文件上按活跃 context 分布测端到端编辑、增量重高亮和 p95/p99 延迟，才能决定自研还是先复用 syntect。

验证：`cargo test --release --locked` 通过 10 项测试，包括短字符串穷举、cache、同位置 priority、core 回缩、lookahead captures、terminal newline 与方言翻译。当前 toolchain 未安装 rustfmt/clippy，未完成这两项可选检查。

## 完整计时指标

| 语言 | context | engine | queries | 总时间中位 s | queries/s | MB/s | runtime fallback |
|---|---|---|---|---|---|---|---|
| rust | declarations | onig | 406,404 | 0.487373 | 833,866 | 20.52 | 0 |
| rust | declarations | fancy | 406,404 | 0.512145 | 793,533 | 19.53 | 0 |
| rust | declarations | multi | 406,404 | 0.225596 | 1,801,466 | 44.33 | 0 |
| rust | keywords | onig | 564,290 | 1.359050 | 415,209 | 7.36 | 0 |
| rust | keywords | fancy | 564,290 | 0.311162 | 1,813,491 | 32.14 | 0 |
| rust | keywords | multi | 564,290 | 0.154278 | 3,657,612 | 64.82 | 0 |
| rust | numbers | onig | 383,681 | 0.450945 | 850,838 | 22.18 | 0 |
| rust | numbers | fancy | 383,681 | 0.425979 | 900,705 | 23.48 | 0 |
| rust | numbers | multi | 383,681 | 0.043919 | 8,736,022 | 227.69 | 0 |
| rust | statements | onig | 4,020,328 | 12.768983 | 314,851 | 0.78 | 0 |
| rust | statements | fancy | 4,020,316 | 8.454358 | 475,532 | 1.18 | 0 |
| rust | statements | multi | 4,020,316 | 27.482155 | 146,288 | 0.36 | 0 |
| javascript | expression-begin | onig | 8,458,461 | 9.603568 | 880,762 | 1.04 | 0 |
| javascript | expression-begin | fancy | 8,458,461 | 35.283856 | 239,726 | 0.28 | 0 |
| javascript | expression-begin | multi | 8,458,461 | 6.079970 | 1,391,201 | 1.64 | 0 |
| javascript | expression-end | onig | 8,483,033 | 6.175725 | 1,373,609 | 1.62 | 0 |
| javascript | expression-end | fancy | 8,483,033 | 14.848262 | 571,315 | 0.67 | 0 |
| javascript | expression-end | multi | 8,483,033 | 3.801950 | 2,231,232 | 2.63 | 0 |
| javascript | literal-variable | onig | 1,837,622 | 5.095766 | 360,617 | 1.96 | 0 |
| javascript | literal-variable | fancy | 1,837,622 | 35.703130 | 51,469 | 0.28 | 0 |
| javascript | literal-variable | multi | 1,837,622 | 2.919470 | 629,437 | 3.43 | 0 |
| javascript | statement | onig | 8,344,922 | 5.539319 | 1,506,489 | 1.81 | 0 |
| javascript | statement | fancy | 8,344,922 | 13.342626 | 625,433 | 0.75 | 0 |
| javascript | statement | multi | 8,344,922 | 3.225768 | 2,586,957 | 3.10 | 0 |
| python | class-definition-base-list-body | onig | 3,600,456 | 4.740005 | 759,589 | 1.06 | 0 |
| python | class-definition-base-list-body | fancy | 3,600,456 | 4.961154 | 725,730 | 1.02 | 0 |
| python | class-definition-base-list-body | multi | 3,600,456 | 1.260163 | 2,857,134 | 4.00 | 0 |
| python | function-call-argument-list-body | onig | 3,773,025 | 4.502477 | 837,989 | 1.12 | 0 |
| python | function-call-argument-list-body | fancy | 3,773,025 | 4.808393 | 784,675 | 1.05 | 0 |
| python | function-call-argument-list-body | multi | 3,773,025 | 1.310467 | 2,879,145 | 3.84 | 0 |
| python | maybe-set-body | onig | 3,830,220 | 4.640382 | 825,410 | 1.09 | 0 |
| python | maybe-set-body | fancy | 3,830,220 | 3.841047 | 997,181 | 1.31 | 0 |
| python | maybe-set-body | multi | 3,830,220 | 1.127980 | 3,395,645 | 4.47 | 0 |
| python | statements | onig | 3,797,256 | 5.444088 | 697,501 | 0.93 | 0 |
| python | statements | fancy | 3,797,256 | 6.731027 | 564,142 | 0.75 | 0 |
| python | statements | multi | 3,797,256 | 1.253439 | 3,029,470 | 4.02 | 0 |
| cpp | expressions | onig | 880,625 | 2.797170 | 314,827 | 1.83 | 0 |
| cpp | expressions | fancy | 880,625 | 6.270581 | 140,438 | 0.82 | 0 |
| cpp | expressions | multi | 880,625 | 0.770218 | 1,143,346 | 6.64 | 0 |
| cpp | global | onig | 985,350 | 3.370485 | 292,347 | 1.52 | 0 |
| cpp | global | fancy | 985,350 | 7.557747 | 130,376 | 0.68 | 0 |
| cpp | global | multi | 985,350 | 3.712128 | 265,441 | 1.38 | 0 |
| cpp | preprocessor-if-branch-global | onig | 985,325 | 3.514389 | 280,369 | 1.46 | 0 |
| cpp | preprocessor-if-branch-global | fancy | 985,325 | 9.429687 | 104,492 | 0.54 | 0 |
| cpp | preprocessor-if-branch-global | multi | 985,325 | 4.033355 | 244,294 | 1.27 | 0 |
| cpp | statements | onig | 880,175 | 2.845944 | 309,274 | 1.80 | 0 |
| cpp | statements | fancy | 880,175 | 7.223097 | 121,856 | 0.71 | 0 |
| cpp | statements | multi | 880,175 | 0.759760 | 1,158,491 | 6.74 | 0 |
| json | array-body | onig | 19,058,817 | 9.781611 | 1,948,433 | 2.06 | 0 |
| json | array-body | fancy | 19,058,817 | 3.890799 | 4,898,432 | 5.17 | 0 |
| json | array-body | multi | 19,058,817 | 5.393751 | 3,533,500 | 3.73 | 0 |
| json | expect-object-value | onig | 20,117,640 | 9.928808 | 2,026,189 | 2.03 | 0 |
| json | expect-object-value | fancy | 20,117,640 | 9.514743 | 2,114,365 | 2.11 | 0 |
| json | expect-object-value | multi | 20,117,640 | 6.747760 | 2,981,381 | 2.98 | 0 |
| json | main | onig | 4,705,883 | 2.852240 | 1,649,890 | 7.05 | 0 |
| json | main | fancy | 4,705,883 | 0.994140 | 4,733,623 | 20.24 | 0 |
| json | main | multi | 4,705,883 | 1.680468 | 2,800,341 | 11.97 | 0 |
| json | object-value-end | onig | 20,117,640 | 10.505161 | 1,915,024 | 1.92 | 0 |
| json | object-value-end | fancy | 20,117,640 | 8.201540 | 2,452,910 | 2.45 | 0 |
| json | object-value-end | multi | 20,117,640 | 8.982960 | 2,239,534 | 2.24 | 0 |
| markdown | fenced-code-block-body | onig | 1,196,515 | 7.954939 | 150,412 | 0.63 | 0 |
| markdown | fenced-code-block-body | fancy | 1,196,515 | 1.104327 | 1,083,479 | 4.53 | 0 |
| markdown | fenced-code-block-body | multi | 1,196,515 | 1.973567 | 606,270 | 2.53 | 0 |
| markdown | list-paragraph | onig | 357,277 | 1.263809 | 282,699 | 3.96 | 0 |
| markdown | list-paragraph | fancy | 357,277 | 7.828172 | 45,640 | 0.64 | 0 |
| markdown | list-paragraph | multi | 357,277 | 0.648017 | 551,339 | 7.72 | 0 |
| markdown | paragraph | onig | 357,325 | 1.237177 | 288,823 | 4.04 | 0 |
| markdown | paragraph | fancy | 357,325 | 7.745340 | 46,134 | 0.65 | 0 |
| markdown | paragraph | multi | 357,325 | 0.644684 | 554,264 | 7.76 | 0 |
| markdown | strikethrough-content | onig | 357,983 | 1.305920 | 274,123 | 3.83 | 0 |
| markdown | strikethrough-content | fancy | 357,983 | 7.498400 | 47,741 | 0.67 | 0 |
| markdown | strikethrough-content | multi | 357,983 | 1.308147 | 273,657 | 3.82 | 0 |
| yaml | block-node-12 | onig | 3,453,177 | 4.265921 | 809,480 | 1.17 | 0 |
| yaml | block-node-12 | fancy | 3,453,177 | 9.012997 | 383,133 | 0.56 | 0 |
| yaml | block-node-12 | multi | 3,453,177 | 3.617761 | 954,507 | 1.38 | 0 |
| yaml | flow-mapping-body | onig | 3,290,430 | 3.977880 | 827,182 | 1.26 | 0 |
| yaml | flow-mapping-body | fancy | 3,290,430 | 7.699739 | 427,343 | 0.65 | 0 |
| yaml | flow-mapping-body | multi | 3,290,430 | 3.370617 | 976,210 | 1.49 | 0 |
| yaml | flow-sequence-body | onig | 3,302,091 | 3.987866 | 828,035 | 1.26 | 0 |
| yaml | flow-sequence-body | fancy | 3,302,091 | 7.734120 | 426,951 | 0.65 | 0 |
| yaml | flow-sequence-body | multi | 3,302,091 | 3.277196 | 1,007,596 | 1.53 | 0 |
| yaml | main | onig | 3,452,163 | 4.417253 | 781,518 | 1.13 | 0 |
| yaml | main | fancy | 3,452,163 | 9.982001 | 345,839 | 0.50 | 0 |
| yaml | main | multi | 3,452,163 | 3.785228 | 912,009 | 1.32 | 0 |
| html | main | onig | 264,453 | 0.651915 | 405,656 | 7.67 | 0 |
| html | main | fancy | 264,453 | 2.987894 | 88,508 | 1.67 | 0 |
| html | main | multi | 264,453 | 0.381129 | 693,868 | 13.12 | 0 |
| html | tag | onig | 287,973 | 0.526356 | 547,107 | 9.50 | 0 |
| html | tag | fancy | 287,973 | 1.883404 | 152,900 | 2.66 | 0 |
| html | tag | multi | 287,973 | 0.407564 | 706,571 | 12.27 | 0 |
| html | tag-attributes | onig | 4,289,754 | 2.259240 | 1,898,760 | 2.21 | 0 |
| html | tag-attributes | fancy | 4,289,754 | 1.019509 | 4,207,668 | 4.91 | 0 |
| html | tag-attributes | multi | 4,289,754 | 1.257557 | 3,411,180 | 3.98 | 0 |
| html | tag-html | onig | 177,870 | 0.367023 | 484,630 | 13.63 | 0 |
| html | tag-html | fancy | 177,870 | 1.848599 | 96,219 | 2.71 | 0 |
| html | tag-html | multi | 177,870 | 0.322106 | 552,209 | 15.53 | 0 |

复跑命令与实现细节见 [README.md](README.md)。原始数据：`output/diff.json`、`output/bench.json`、`output/compile-{0,1,2}.json`、`output/memory.json`；所有编译失败原因、skips、翻译后 core/guard 和 mismatch samples 均保留。无 git commit。
