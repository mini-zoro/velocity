# 语法引擎 v1：官方语料正确性与性能

语料：仓库固定的 `third_party/sublime-packages`，202 个定义、242 个测试文件。
本 lane 的源码和报告仅位于三个新增 crate，另更新根 Cargo.lock；未修改语料、spec、spikes，未提交。公开文档、MIT syntect 源码与 PR #705 为允许参考；许可和归属见 README。

载入 **202/202**，载入/继承警告 **0**；断言跨度通过 **125,452/125,454 (99.9984%)**，失败 **2**，显式跳过 **1,143**。语法/运行错误文件 **0**，语料 panic **0**。

字符列另计：通过 586,575，失败 54。跨度内任一字符失败，整个断言记失败。跳过不计入通过率分母。

> **评审更正（2026-11-14）**：原运行器把头部带 `partial-symbols` 修饰符的文件整文件跳过，但该修饰符只放宽符号断言，
> 作用域断言仍应检查（例如 Diff 测试有 149 个 `^`/`<-` 作用域断言）。修正后只有 `reindent*` 文件整文件跳过：
> **通过 125,875 / 失败 2 / 跳过 720（99.998%）**，新纳入的 423 个作用域断言全部通过；`tests/baseline.json` 已按此更新。
> 评审另做了变异测试：篡改一条测试预期 → 报 1 个失败；把 Rust 语法里的 `keyword.declaration.struct` 改名 → 报 7 个失败；
> 运行器能可靠发现错误。下文其余数字为修正前的记录。

## 与稳定 syntect 5.3 / onig 的比较

旧 spike 只测八个 canonical 语法，不能声称有全 242 文件的 syntect 总体率。旧版只载入 78/202；本实现载入 202/202。

| 语言 | vel 通过 / 失败 | vel 通过率 | syntect 通过 / 失败 / 未执行 | syntect 已执行通过率 |
|---|---:|---:|---:|---:|
| Rust | 2426 / 0 | 100.000% | 2402 / 11 / 0 | 99.54% |
| JSON | 135 / 0 | 100.000% | 133 / 2 / 0 | 98.52% |
| YAML | 607 / 0 | 100.000% | 603 / 4 / 0 | 99.34% |
| Python | 6312 / 0 | 100.000% | 1311 / 399 / 4602 | 76.67% |
| JavaScript | 3011 / 2 | 99.934% | 0 / 0 / 3013 | 未载入 |
| C++ | 1755 / 0 | 100.000% | 1079 / 676 / 0 | 61.48% |
| Markdown | 4869 / 0 | 100.000% | 668 / 4209 / 0 | 13.70% |
| HTML | 1012 / 0 | 100.000% | 0 / 0 / 1012 | 未载入 |

旧八语言合计：6196/11497 = **53.89%** 已执行通过；把 8627 个未执行也计入计划分母，则为 **30.79%**。Python 旧结果只执行了约 27%，不是完整结果。

当前 runner 修正纯断言识别、完整 header 尾 token、跨行回放与过行尾列定位，故某些语言的断言数与旧 spike 略有差别（Rust 2426 对旧 2413，Markdown 4869 对旧 4877）。未通过分母伪装为同一 harness 的逐项差分。

## 剩余问题与跳过原因

唯一剩余失败类别是 **selector 尾缀兼容性（2 个）**：JavaScript 两条官方断言包含 `-comment() {}`；当前严格 selector 解析拒绝这一尾缀。未改写断言或把失败转为跳过。其实际作用域在 JSON 中完整记录。

- `JavaScript/tests/syntax_test_js.js:236`：`-comment() {}`；源行 235、字符列 4，实际 `source.js meta.group.js meta.mapping.js meta.function.js meta.string.js string.quoted.single.js punctuation.definition.string.begin.js`。
- `JavaScript/tests/syntax_test_js.js:239`：`-comment() {}`；源行 238、字符列 4，实际 `source.js meta.group.js meta.mapping.js meta.function.js meta.string.js string.quoted.double.js punctuation.definition.string.begin.js`。

另有 664 条 `@` / `>` 符号或引用标注未实现索引，明确跳过。34 个 modifier 文件跳过，共记录 479 个跳过单位：reindent 文件无 scope 断言时计文件级 1，partial-symbols 按标注数计。它们没有算作成功。

`.tmLanguage` 转换返回明确的 unimplemented 错误；zip 包尚未实现。selector 数值评分的精确公式未经 ST 验证，源码以 `ST-BEHAVIOR: unverified` 标注。

本次 runner 正则诊断：`{"compilations": 19892, "compile_errors": 0, "search_errors": 404}`。命中 Onig 限制按无匹配处理并计数，绝不 panic。

## 性能（每项至少 5 次）

macOS 26.1 / arm64，rustc 1.99.1，release，单线程完整解析。缓存批次使用已暖文件系统。没有调用 sysctl；其他并行 lane 的活动可能影响时序。

输入为 `spikes/data/rust-10mb.rs`，10,000,000 字节、297,030 行。预热 regex 后计时，不着色，不去重输入，包含解释器、scope ops、状态 hash 与栈共享的开销。

| 项目 | 五次中位数 | 目标/参照 |
|---|---:|---|
| Rust parse-only | **1.408 MB/s / 41,811 行/s** | syntect 1.02 MB/s / 约 30k 行/s |
| YAML 构建全部 202 个定义 | **816.363 ms** | ≤1500 ms |
| 校验内容哈希并读取 binary index | **11.328 ms** | ≤20 ms；包含 IO、源文件哈希、payload 校验与索引反序列化 |
| 一字符编辑后状态收敛 | **0.0086 ms** | ≤2 ms；五次均重新解析 1 行后收敛 |
| 强制解析完整 100 行窗口 | **2.489 ms** | 超过 2 ms；不依赖收敛时仍有差距 |

这是一次稳定上下文的字符替换，并非所有编辑的最坏界限。跨行 branch、字符串边界变化等可能需要更多行。吞吐相对旧数字约 **1.38×**；不同会话、相同语料的比较，不是同时运行的对照实验。

原始 parse 秒数：`[7.163809042, 7.0995165, 7.068728166, 7.199893125, 7.104121625]`；cache ms：`[12.51775, 11.360833000000001, 11.327584, 11.123917, 11.051458]`；其余逐次数据见 `tests/timings.json`。

吞吐预热及五轮累计的 regex 诊断：`{"compilations": 80296, "compile_errors": 0, "search_errors": 6}`。这些长输入中的资源上限事件如实记为无匹配；不推断为语法断言全部正确。

上下文体按语法独立压缩，首次进入时才解码，索引元数据无需解码全部上下文。这增加 YAML 构建时的一次性压缩开销，但显著减少缓存载入的 IO、校验和复制；两项仍均在预算内。

## 编辑器接入

共享 `Arc<SyntaxSet>`；每个文档持有 `ParseState` 和独立的 `ScopeStack`。行输入保留 `\n`，ops 为字节偏移，作用域选择器可复用解析后的对象。保存行末 checkpoint 后，状态相等即可停止增量重解析。

`LineOps.rewind.from_line` 与 `corrections` 均从 0 开始；按序替换已缓存的 ops，同一行最后一次修正生效，再从最早修正行重建 scope stacks。branch 最多回退 128 行。

`MatcherBackend` / `PatternMatcher` 是实际解析路径的替换点，已经用注入工厂的测试证明。P2 可插入 MultiDfa；必须保留最早起点、同位规则顺序、空匹配策略、排除规则、完整行 lookbehind / \G 和胜者捕获语义。

## 全部 202 个语法

无测试断言的语法不虚报 100%；用“—”标记。按 header 指定的语法归类，不按测试所在目录猜语言。

| 语法 | 已载入 | 通过 | 失败 | 跳过 | 通过率 | 主要原因 |
|---|:---:|---:|---:|---:|---:|---|
| ASP | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| ActionScript 2.0 | 是 | 184 | 0 | 0 | 100.0000% | — |
| AppleScript | 是 | 513 | 0 | 0 | 100.0000% | — |
| Bash | 是 | 11090 | 0 | 46 | 100.0000% | — |
| Bash (for Markdown) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Batch File | 是 | 3918 | 0 | 0 | 100.0000% | — |
| Batch File (Compound) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| BibTeX | 是 | 100 | 0 | 7 | 100.0000% | — |
| Binary | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| C | 是 | 876 | 0 | 0 | 100.0000% | — |
| C# | 是 | 9355 | 0 | 70 | 100.0000% | — |
| C++ | 是 | 1755 | 0 | 3 | 100.0000% | — |
| CSS | 是 | 3514 | 0 | 1 | 100.0000% | — |
| CSS (Go) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS (PHP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS (Rails) | 是 | 48 | 0 | 0 | 100.0000% | — |
| CSS (for ASP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS (for HAML) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS (for JS template) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS (for JSP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS (for PHP Interpolated) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS (for PHP double-quoted) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS (for PHP single-quoted) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS (for Ruby) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS (for TS template) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| CSS inside Go backtick string | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Cabal | 是 | 421 | 0 | 0 | 100.0000% | — |
| Cargo Build Results | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Cassandra Query Language | 是 | 399 | 0 | 0 | 100.0000% | — |
| Clojure | 是 | 1699 | 0 | 0 | 100.0000% | — |
| ClojureScript | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| D | 是 | 3110 | 0 | 0 | 100.0000% | — |
| DMD Output | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| DTD | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Diff | 是 | 0 | 0 | 311 | — | 仅显式跳过/无 scope 用例 |
| Diff (Basic) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Diff (for Markdown) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Diff (for PHP Interpolated) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Erlang | 是 | 4350 | 0 | 0 | 100.0000% | — |
| Git Attributes | 是 | 176 | 0 | 0 | 100.0000% | — |
| Git Blame Ignore Revisions | 是 | 37 | 0 | 0 | 100.0000% | — |
| Git Code Owners | 是 | 92 | 0 | 0 | 100.0000% | — |
| Git Commit | 是 | 258 | 0 | 0 | 100.0000% | — |
| Git Commit Message | 是 | 137 | 0 | 0 | 100.0000% | — |
| Git Common | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Git Config | 是 | 566 | 0 | 0 | 100.0000% | — |
| Git Diff | 是 | 88 | 0 | 132 | 100.0000% | — |
| Git Ignore | 是 | 285 | 0 | 0 | 100.0000% | — |
| Git Link | 是 | 53 | 0 | 0 | 100.0000% | — |
| Git Log | 是 | 162 | 0 | 0 | 100.0000% | — |
| Git Mailmap | 是 | 45 | 0 | 0 | 100.0000% | — |
| Git Rebase Todo | 是 | 392 | 0 | 0 | 100.0000% | — |
| Go | 是 | 4402 | 0 | 0 | 100.0000% | — |
| Go (Embedded Backtick String) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Go (Embedded CSS) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Go (Embedded HTML) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Go (Embedded JSON) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Go (Embedded JavaScript) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Go (Embedded Regex) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Go (Embedded SQL) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Go (Embedded T-SQL) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Go (Embedded XML) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Graphviz (DOT) | 是 | 210 | 0 | 16 | 100.0000% | — |
| Groovy | 是 | 295 | 0 | 0 | 100.0000% | — |
| HAML | 是 | 368 | 0 | 0 | 100.0000% | — |
| HTML | 是 | 1012 | 0 | 1 | 100.0000% | — |
| HTML (ASP) | 是 | 993 | 0 | 0 | 100.0000% | — |
| HTML (Erlang) | 是 | 60 | 0 | 0 | 100.0000% | — |
| HTML (Go) | 是 | 169 | 0 | 0 | 100.0000% | — |
| HTML (JSP) | 是 | 701 | 0 | 0 | 100.0000% | — |
| HTML (Plain) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| HTML (Rails) | 是 | 167 | 0 | 0 | 100.0000% | — |
| HTML (Tcl) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| HTML (for HAML) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| HTML (for HSX) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| HTML (for JS template) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| HTML (for PHP Interpolated) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| HTML (for Ruby) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| HTML (for TS template) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| HTML inside Go backtick string | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Haskell | 是 | 3492 | 0 | 1 | 100.0000% | — |
| JSON | 是 | 135 | 0 | 0 | 100.0000% | — |
| JSON (Go) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JSON (PHP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JSON (Rails) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JSON (for ASP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JSON (for HAML) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JSON (for JS template) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JSON (for JSP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JSON (for PHP Interpolated) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JSON (for TS template) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JSON inside Go backtick string | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JSX | 是 | 619 | 0 | 1 | 100.0000% | — |
| JSX (Plain) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Java | 是 | 8903 | 0 | 1 | 100.0000% | — |
| Java (for HTML) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Java (for JSP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Java Properties | 是 | 84 | 0 | 0 | 100.0000% | — |
| JavaDoc | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript | 是 | 3011 | 2 | 1 | 99.9336% | selector 尾缀 |
| JavaScript (Go) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (PHP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (Plain) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (Rails) | 是 | 31 | 0 | 0 | 100.0000% | — |
| JavaScript (for ASP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (for Go Embedded Backtick Strings) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (for HAML) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (for JS template) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (for JSP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (for PHP Interpolated) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (for PHP double-quoted) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (for PHP single-quoted) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (for Ruby) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| JavaScript (for TS template) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| LaTeX | 是 | 1446 | 0 | 381 | 100.0000% | — |
| LaTeX Log | 是 | 80 | 0 | 0 | 100.0000% | — |
| Lisp | 是 | 1995 | 0 | 0 | 100.0000% | — |
| Literate Haskell | 是 | 32 | 0 | 0 | 100.0000% | — |
| Lua | 是 | 915 | 0 | 0 | 100.0000% | — |
| MATLAB | 是 | 624 | 0 | 6 | 100.0000% | — |
| Make Output | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Makefile | 是 | 552 | 0 | 0 | 100.0000% | — |
| Markdown | 是 | 4869 | 0 | 14 | 100.0000% | — |
| Markdown (Go) | 是 | 160 | 0 | 0 | 100.0000% | — |
| Markdown (for HAML) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| MultiMarkdown | 是 | 26 | 0 | 0 | 100.0000% | — |
| MySQL | 是 | 3735 | 0 | 0 | 100.0000% | — |
| OCaml | 是 | 238 | 0 | 0 | 100.0000% | — |
| OCamllex | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| OCamlyacc | 是 | 41 | 0 | 0 | 100.0000% | — |
| Objective-C | 是 | 708 | 0 | 2 | 100.0000% | — |
| Objective-C++ | 是 | 1511 | 0 | 2 | 100.0000% | — |
| PHP | 是 | 4465 | 0 | 2 | 100.0000% | — |
| PHP Source | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| PHP Source (for PHP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Pascal | 是 | 371 | 0 | 26 | 100.0000% | — |
| Perl | 是 | 4590 | 0 | 1 | 100.0000% | — |
| PostgreSQL | 是 | 710 | 0 | 0 | 100.0000% | — |
| Python | 是 | 6312 | 0 | 1 | 100.0000% | — |
| R | 是 | 334 | 0 | 0 | 100.0000% | — |
| R Console | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Rd (R Documentation) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| RegExp (for Python raw-f-string) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| RegExp inside Go backtick string | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regex (for C# literal string) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regex (for C# verbatim string) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regular Expression | 是 | 548 | 0 | 0 | 100.0000% | — |
| Regular Expression (Basic) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regular Expression (Perl inside angle brackets) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regular Expression (Perl inside braces) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regular Expression (Perl inside parens) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regular Expression (Perl inside square brackets) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regular Expression (Perl) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regular Expression Replacement | 是 | 36 | 0 | 0 | 100.0000% | — |
| Regular Expressions (C#) | 是 | 206 | 0 | 0 | 100.0000% | — |
| Regular Expressions (JavaScript) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regular Expressions (PHP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Regular Expressions (Python) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Ruby | 是 | 1151 | 0 | 0 | 100.0000% | — |
| Ruby (Rails) | 是 | 20 | 0 | 0 | 100.0000% | — |
| Ruby (for HAML) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Rust | 是 | 2426 | 0 | 14 | 100.0000% | — |
| SQL | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL (Basic) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL (Rails) | 是 | 37 | 0 | 0 | 100.0000% | — |
| SQL (for C# raw string) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL (for C# verbatim format string) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL (for C# verbatim string) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL (for C#) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL (for JS template) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL (for PHP Interpolated) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL (for PHP) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL (for Ruby) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL (for TS template) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| SQL inside Go backtick string | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Scala | 是 | 1704 | 0 | 0 | 100.0000% | — |
| Shell | 是 | 5 | 0 | 0 | 100.0000% | — |
| Shell (for Makefile) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| ShellScript (for Ruby) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Sublime File Pattern | 是 | 144 | 0 | 0 | 100.0000% | — |
| T-SQL | 是 | 1651 | 0 | 8 | 100.0000% | — |
| T-SQL inside Go backtick string | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| TOML | 是 | 651 | 0 | 9 | 100.0000% | — |
| TOML (for Python) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| TSX | 是 | 662 | 0 | 0 | 100.0000% | — |
| TSX (Plain) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Tcl | 是 | 201 | 0 | 0 | 100.0000% | — |
| TeX | 是 | 794 | 0 | 0 | 100.0000% | — |
| TeX (DocStrip) | 是 | 46 | 0 | 0 | 100.0000% | — |
| Textile | 是 | 608 | 0 | 47 | 100.0000% | — |
| TypeScript | 是 | 2307 | 0 | 0 | 100.0000% | — |
| TypeScript (Plain) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| XML | 是 | 1485 | 0 | 0 | 100.0000% | — |
| XML (for PHP Interpolated) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| XML inside Go backtick string | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| YAML | 是 | 607 | 0 | 0 | 100.0000% | — |
| YAML (Go) | 是 | 95 | 0 | 0 | 100.0000% | — |
| YAML (for PHP Interpolated) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| Zsh | 是 | 8037 | 0 | 39 | 100.0000% | — |
| Zsh (for Markdown) | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| camlp4 | 是 | 0 | 0 | 0 | — | 无 scope 用例 |
| reStructuredText | 是 | 42 | 0 | 0 | 100.0000% | — |

复现：`cargo run --release -p vel-syntax --bin syntax-tests`；`cargo run --release -p vel-syntax --bin syntax-bench`；`python3 crates/vel-syntax/scripts/report.py`。

回归门槛位于 `tests/baseline.json`；`cargo test -p vel-scope -p vel-regex -p vel-syntax --release` 实际运行全 242 文件并逐语法检查不得倒退。
