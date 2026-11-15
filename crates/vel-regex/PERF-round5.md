# Lane B，第五轮：生产多模式匹配与 parser 热路径

本报告由 scripts/report.py 从原始 JSON 生成。未提交 git commit；未调用 sysctl。

## 预算结果

| 指标 | 本轮实测 | 目标 |
| --- | ---: | --- |
| Rust 10 MB，实际后台 parse+style | 2.124 MB/s；中位 4.708506 s / P95 4.790598 s | ≥2.5 MB/s |
| 八语言独立 backend 加速 | 最低 1.044×；1/8 达到 2× | 每种 ≥2× |
| 最差已测 context | Markdown::__main：0.324× | ≥1× |
| 冷进程首次 context 编译 | pooled P50 0.0412 ms / P95 0.5887 ms | P95 ≤2 ms |
| 八语料完成后的 DFA/NFA/cache 实际驻留 | 16.954 MiB；峰值 16.954 MiB | ≤32 MiB |
| edit → exact visible batch | 500 samples P95 0.11500 ms | ≤0.2 ms |

预算按实测逐项判断；表中未达标项仍是未完成的性能目标，不能用总体平均值替代。

## 八语言吞吐：同一 parser，关闭双方的稳定行 memoization

release，Darwin arm64 / macOS 26.1，rustc 1.99.1；单线程；每引擎先完整预热一次，随后交替先后顺序执行五次。
计时包含真实 context 切换、prototype/embed、branch replay、regex 搜索、赢家 captures 和 LineOps；排除文件 I/O、YAML 加载和预热编译。
双方都使用 parse_line_into 重用输出；双方都关闭稳定行 memoization，以隔离 matcher 差异。后台服务则开启默认 memoization，不能混称同一指标。
P95 为 nearest rank；五次样本的 P95 等于其中最大值。其他 lane 可能活动，未删除噪声或异常值。

| 语言 | MB | 新 backend MB/s | Onig MB/s | 加速 | 新 median / P95 s | Onig median / P95 s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| rust | 10.000 | 2.032 | 1.511 | 1.345× | 4.920104 / 5.006419 | 6.616142 / 6.811257 |
| javascript | 10.000 | 1.875 | 1.765 | 1.062× | 5.334672 / 5.386525 | 5.666322 / 5.764382 |
| python | 5.037 | 2.514 | 1.150 | 2.186× | 2.003740 / 2.044511 | 4.379700 / 4.583226 |
| cpp | 5.117 | 2.225 | 2.131 | 1.044× | 2.300248 / 2.328324 | 2.401217 / 2.434357 |
| json | 20.118 | 6.746 | 3.482 | 1.937× | 2.982263 / 2.999355 | 5.777686 / 5.875841 |
| markdown | 5.000 | 4.271 | 2.251 | 1.897× | 1.170707 / 1.196737 | 2.221069 / 2.252363 |
| yaml | 5.006 | 5.361 | 3.345 | 1.603× | 0.933689 / 0.942740 | 1.496392 / 1.520651 |
| html | 5.002 | 9.438 | 5.775 | 1.634× | 0.529974 / 0.540209 | 0.866085 / 0.894228 |

## Context 剖析与原型慢路径

独立的 instrumented run 使用每语言最前约 200 KB，五次完整 parser 流程，检查 890 个具有五次相同 query 数的 context。
每次 context 调用用 Instant 单独累计；不把这组带计时开销的数据当成完整文件吞吐。低频 context 的固定计时/锁开销占比可能很高，仍保留其最差值。
每次至少 100 queries 的 388 个 context 中，最差为 Rust::type-path，0.394×。

| context | 每次 queries | 新 median ms | Onig median ms | 加速 |
| --- | ---: | ---: | ---: | ---: |
| Markdown::__main | 1 | 0.002958 | 0.000958 | 0.324× |
| Rust::impl-trait-types | 60 | 0.148209 | 0.054378 | 0.367× |
| Rust::type-path | 186 | 0.237707 | 0.093551 | 0.394× |
| JavaScript::__main | 1 | 0.000208 | 0.000083 | 0.399× |
| JSON::__main | 1 | 0.001417 | 0.000625 | 0.441× |
| C++::#anon_data-structures-struct-definition_0 | 5 | 0.005624 | 0.002500 | 0.445× |
| Rust::declaration-identifier | 1820 | 8.686217 | 3.931953 | 0.453× |
| Rust::union-identifier | 3 | 0.003625 | 0.001709 | 0.471× |
| Python::type-parameter-bound-continue | 10 | 0.007292 | 0.003583 | 0.491× |
| Python::function-return-type | 75 | 0.061920 | 0.033878 | 0.547× |
| Rust::statements | 3740 | 5.303418 | 12.150266 | 2.291× |

另对原型点名的两个 context 做独立固定扫描：每种前 200 KB、完整 C/C++ includes、五次、先逐 query 核对 captures；不执行 parser context 切换或自动 prototype 注入。这组数据不冒充整文件 parser 吞吐。

| 固定 context | 每次 queries | 新 median / P95 ms | Onig median / P95 ms | 加速 |
| --- | ---: | ---: | ---: | ---: |
| rust::statements | 54848 | 128.6694 / 135.9089 | 139.8766 / 156.4977 | 1.087× |
| cpp::global | 54750 | 143.9691 / 147.1993 | 145.1753 / 160.0258 | 1.008× |

原型在 Rust statements 的主要问题是 Unicode 边界/PikeVM、重复 guard matcher 和失败后的逐规则 anchored 修复。
本轮使用 ASCII hybrid DFA 和 exact Onig Unicode fallback；边界小谓词直接判断，独立 lookahead 绝不顺序消费；纯正向、无 captures 的 lookahead 在自动机定位后直接还原零宽 span；没有有效定位式的 epsilon 候选留给 Onig。拒绝候选后仅按原始顺序修复该 query。
C++ 的全局候选包含不规则 lookaround 时，先生成保守候选，再用原式核实；固定捕获布局可直接生成 inline spans。
DFA 与 fallback 先比较 span/priority，再为最终赢家物化 captures；query 起点的赢家不再搜索不可能抢先的低优先级 fallback。
少于四条规则的上下文直接建立原 Onig matcher，避免 DFA 与额外 dispatch 开销。其他 context 仍可能不及 Onig；上表完整披露。可选 adaptive 会比较 64 个 warm queries，但短前缀会误判后续流量，默认关闭；未将其不同工作量混入主表。
共享机器争用时注册/搜索使用 try_lock 并退回 Onig，避免一个后台文档阻塞另一个文档的前台高亮；有专门的并发回归测试。

## Lazy 编译与内存

五次独立进程各从新 SyntaxSet 开始，依次进入八语言的前 64 KiB 完整行。计时覆盖前端、NFA、forward/reverse DFA 与 literal set；只编译实际进入的 context。
Onig 赢家/不支持规则的单模式编译仍按需发生，其成本不冒充 DFA 编译。编译数组含未能建立 DFA 而选择 fallback 的尝试。

| 独立进程 | contexts 尝试 | compile P50 / P95 ms | 首次编译合计 ms | peak RSS MiB |
| --- | ---: | ---: | ---: | ---: |
| 1 | 422 | 0.0415 / 0.5500 | 74.461 | 112.938 |
| 2 | 422 | 0.0383 / 0.4935 | 71.934 | 104.141 |
| 3 | 422 | 0.0409 / 0.4476 | 71.828 | 107.953 |
| 4 | 422 | 0.0464 / 0.7738 | 87.749 | 90.391 |
| 5 | 422 | 0.0439 / 0.5201 | 73.700 | 95.297 |

主吞吐进程：507 个 ordered lists，507 次 lazy 尝试，0 次 LRU 驱逐。
32 MiB 限额覆盖整个 backend 的所有语言，包含 forward/reverse NFA、DFA transition caches 和 literal automata；共享列表不重复持有机器。
SearchCache/checkpoint 不持有编译机器，因而不会阻止驱逐。小内存配置、LRU 重编译、共享与并发均有测试。
API memory 不包含源模式/HIR、Onig、allocator、语法索引、parser/checkpoints 或短暂编译工作区。RSS 由 wait4/rusage 单独读取每个子进程，包含进程与冷前缀解析，不能和纯 DFA 预算混为一谈；没有调用被 sandbox 禁止的 sysctl。
未增加磁盘 DFA cache；编译延迟和内存证据保留在 compile.json / throughput.json。

## Parser 热路径与服务

parse_line_into 复用 operations/corrections；Region 与常见 capture ops 使用 inline storage；删除了每 token 未使用的 MatchOperation clone。
稳定、非 speculative 的 (完整输入行，语义起始状态) → (结束状态，interned operations) 按文档缓存，最多 4096 条及 4 MiB 源文本；hash 命中仍核对完整文本与状态。
cache 不参与 checkpoint 的语义相等性；命中不会丢失绝对行计数，下一次真实解释前恢复行号；rewind/corrections/warnings 不缓存。
分支 replay、动态捕获、Unicode/换行编辑、缓存 suffix 收敛、scheme 切换及跨行修正均保留测试。
一个含捕获的稳定流式 fixture，在关闭 memoization 和启用 memoization 两种模式下，各 10,000 行均为 0 次 Rust allocation；这不测 Onig 的 C allocator，也不声称冷 miss、分支快照、styled spans 和返回 batch 永不分配。

Rust 10 MB core parse+style/checkpoints：中位 4.681539 s / P95 5.148992 s。
实际 worker：中位 4.708506 s / P95 4.790598 s；对比上轮 7.444 s 的中位数为 1.581×。
稳定编辑服务 500 samples P95 0.11500 ms，最坏 2.13958 ms。计时包括 ChangeSet 提交与获得 exact 100-line viewport；文本替换在计时外。
该编辑场景是稳定 token 编辑，不代表任意多行 delimiter 修改；单个病态 Onig 调用仍不可抢占，UI 请求沿用非阻塞降级协议。

## 差分与单调 baseline

官方：242 文件、202 语法，193,704 行 / 8,831,216 bytes，486 次 rewind。
大文件：1,704,072 行 / 65,279,368 bytes，7,624 次 rewind。均为 0 divergences，无抽样、无文件跳过。
诊断模式逐 query 核对 pattern index、span、所有 capture slots；逐行核对完整 LineOps、corrections、rewind 和 warnings。
额外 raw-Onig 探针修复了原前缀过滤器的 x-mode class 空白差异和 Rust-only class 操作符差异；精确 DFA 赢家先验证原 Onig 源确实可编译。终端换行后的 ^（包括内部 ^）保留 Onig 语义。HIR 可消去 {0} 分支，输出仍按 Native 元数据保留全部未参与 capture slots。
官方文本使用与 assertion harness 相同的 CR 归一化；大文件原样解析。单元测试另覆盖 CRLF、空串、终端换行、exclusions、nonempty、独立 lookahead、短串穷举、²³¹¼½¾ 与 join controls、full folding、search-start 和自定义 regex 限制。
整个官方 release differential 还把启用行 memoization 的结果与 uncached Onig 比较，避免缓存掩盖底层差异。

基线：scopes **125,877/0**，symbols **688/0**，reindent **30/5**，零 skips。
partial-symbols 文件照常评估 scopes；selector 仍使用最长可解析前缀通则，没有特定 suffix 或文件名例外。
本轮修复两种 PHP 变体、Bash case 与 Bash heredoc。一般规则：完整同一行 body 不再延续缩进；preserveIndent 选取前导实际代码而非后方 heredoc；metadata increase 不再错误套用外层括号的基准；clause terminator 使用退出前的作用域，case 结束恢复对应层级。

| 剩余文件 | variant | 首处不同的行 |
| --- | --- | ---: |
| C++/tests/syntax_test_indent_common.c++ | reindent-unchanged | 770 |
| CSS/tests/syntax_test_indentation.css | reindent-unchanged | 77 |
| JavaScript/tests/syntax_test_js_indent_common.js | reindent-unchanged | 682 |
| JavaScript/tests/syntax_test_jsx_indent.jsx | reindent-unchanged | 34 |
| ShellScript/Zsh/tests/syntax_test_reindent_case.zsh | reindent-unchanged | 107 |

详细差异保留在 vel-syntax/tests/baseline.json 和 RESULTS.md；所有已通过文件均维持单调。

## 复跑与来源

```sh
cargo fmt -p vel-regex -p vel-syntax -p vel-highlight --check
cargo clippy -p vel-regex -p vel-syntax -p vel-highlight --all-targets -- -D warnings
cargo test --release -p vel-regex -p vel-syntax -p vel-highlight
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-regex -p vel-syntax -p vel-highlight --no-deps
python3 crates/vel-regex/scripts/measure.py
python3 crates/vel-regex/scripts/report.py
```

官方 Packages commit：3f8b2c544e8f74a76e7212da449dcfe3f01b2245。仅使用公开 permissive 语法与本仓库原型；未读取应用私有实现。
原始 evidence：differential.json、large-differential.json、throughput.json、contexts.json、focused-contexts.json、compile.json，及 vel-highlight/timings.json。

| 输入 | bytes | SHA-256 |
| --- | ---: | --- |
| spikes/data/rust-10mb.rs | 10000000 | d5f3d0c59a12ba34446e540cfd7b5e1a3eab6c03176f3aff048281f12e031380 |
| spikes/data/javascript-10mb.js | 10000000 | 400c06f10bba70bb06b45041cf059fdf14c12b85c65f1471f4d7e46acbf8a79a |
| prototypes/sregex/data/python-5mb.py | 5036973 | afbb08566b90b6026457b81b082954064dfe8ab73ce6b28854aa02907b659721 |
| prototypes/sregex/data/cpp-5mb.cpp | 5117175 | c0eb8c7c021829f6aea6c7fcc9796fd9d79fa9783e4963eacabcbb08ba0f1251 |
| spikes/data/json-20mb.json | 20117640 | ddf09db9da7c68a1b8e072bafb8f74a2de77214e4353661ba81b80884bdfb623 |
| spikes/data/markdown-5mb.md | 5000000 | 62fde7c6ddbd39ebe98050bc446f3259566121e9e10fa68ebebbed0fb8058e6c |
| prototypes/sregex/data/yaml-5mb.yaml | 5005611 | da7202d0739a1dee46dbe7a57c04779ef5ed138655bbca876cb11979f1e7ed14 |
| prototypes/sregex/data/html-5mb.html | 5001969 | 75d5a85f7957eace008b6fdf28a2c820c161385d8d08073bd91aa12a1c2ec445 |
