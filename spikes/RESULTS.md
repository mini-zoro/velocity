# Rust 编辑器 building blocks：可行性 spike 与实测

测量日期：2026-11-13（Asia/Shanghai）；报告生成时间：2026-11-13T17:45:29+08:00。全部是本机实际运行得到的结果。没有测量 Sublime Text 本体，因此不能把下列数字解释为“达到 Sublime 的某个百分比”。

## 机器、版本与测量方法

Apple MacBook Pro MacBookPro18,3，M1 Pro，8 核（6P + 2E），16 GB，macOS 26.1 (25B78)，aarch64-apple-darwin。接 AC，100% 电量。rustc 1.99.1 (`58cf8f9c3`, 2026-10-13)，cargo 1.99.1 (`5f94df478`, 2026-08-27)，LLVM 23.1.1。Release profile，默认优化等级 3，debug=1；未开启额外 LTO、target-cpu=native 或自定义 allocator。

版本来自当日 crates.io sparse index，选择未 yanked、无 prerelease 后缀的最新 stable，并在 manifests 中精确固定。`Cargo.lock` 固定全部传递依赖；下表是直接使用的 crate。syntect 的 regex 后端遵循其发布版本的依赖约束，未私自替换成不兼容的新版依赖。

| 程序 | crate = exact version |
| --- | --- |
| bench-buffer | `ropey = 1.6.1`、`crop = 0.4.3` |
| bench-syntax | `syntect = 5.3.0`、`tree-sitter = 0.27.2`、`tree-sitter-rust = 0.24.2`、`tree-sitter-javascript = 0.25.0` |
| bench-search | `regex = 1.13.1`、`ignore = 0.4.34`、`grep-searcher = 0.1.17`、`grep-regex = 0.1.14`、`nucleo-matcher = 0.3.1` |
| bench-text | `cosmic-text = 0.19.0` |
| 公共/辅助 | `anyhow = 1.0.104`、`serde_json = 1.0.154`、`walkdir = 2.5.0`、`libc = 0.2.191` |

关键传递依赖：`fancy-regex 0.16.2`、`fontdb 0.23.0`、`harfrust 0.5.2`、`onig 6.5.3`、`onig_sys 69.9.3`、`skrifa 0.40.0`、`skrifa 0.44.0`、`swash 0.2.10`。完整依赖树可由 `cargo tree --locked` 重建。

计时使用 `std::time::Instant`。独立程序串行运行，排除编译时间；通常 5 次取 median，fuzzy 7 次，完整 viewport 9 次热字体样本，单行/缓存 100 次，多光标 100 个连续事务。P95 为 nearest-rank；5 次样本的 P95 就是最大值。RSS 是整个进程生命周期的高水位，单位 MiB。`/usr/bin/time -l` 在此沙箱因 `sysctl kern.clockrate: Operation not permitted` 失败，已在 `data/raw/rss-method.json` 保存探测结果；改用 Rust 进程内 `libc::getrusage(RUSAGE_SELF).ru_maxrss`（macOS 返回 bytes）。不将测量工具失败误记为被测程序失败。

文件系统缓存没有清除。“cold-ish”只表示该搜索进程第一次遍历，之前生成数据/编译已读取部分文件；不是真正冷磁盘。其他读文件结果以五次混合样本中位数为主；所有逐次样本、错误和执行命令在 `data/raw/*.jsonl`、`*.time.txt`、`runs.json`。固定随机种子；`black_box` 消费输出，编辑后检查长度，排版断言 120 行与 24,000 glyphs。

## 输入数据

官方 Packages 浅克隆 commit：`3f8b2c544e8f74a76e7212da449dcfe3f01b2245`。Rust 来自原有 `~/.cargo/registry/src` 的 802 个真实 `.rs` 文件；记录源文件路径和 SHA256 于 `data/source_manifest.json`。buffer 语料将非 ASCII 字符替成 `?`，使两种索引单位可比较；原始 UTF-8 Rust 文件另存供语法基准。JS 为 lodash 4.17.21 的 unminified + minified 文件重复拼接；Markdown 来自 crate README；JSON 为合法的结构化数组。拼接源码并非可编译的单一程序，末尾也可能截断，因此 tree-sitter 的 ERROR 节点是已观察到的输入特征。

| 文件（data/ 下） | MB（10⁶ bytes） | SHA256 |
| --- | --- | --- |
| text-100mb.txt | 100.000000 | 30c98b601a71debe1f826378658bd20d615c215bfd1fe4990ea031d2c87d0028 |
| text-1gb.txt | 1,000.000000 | 0f3567b9b9aa4cd29070b73a0e21869f312f7782ab955f7f885f9e9ebc44f9f7 |
| rust-10mb.rs | 10.000000 | d5f3d0c59a12ba34446e540cfd7b5e1a3eab6c03176f3aff048281f12e031380 |
| javascript-10mb.js | 10.000000 | 400c06f10bba70bb06b45041cf059fdf14c12b85c65f1471f4d7e46acbf8a79a |
| json-20mb.json | 20.117640 | ddf09db9da7c68a1b8e072bafb8f74a2de77214e4353661ba81b80884bdfb623 |
| markdown-5mb.md | 5.000000 | 62fde7c6ddbd39ebe98050bc446f3259566121e9e10fa68ebebbed0fb8058e6c |

buffer 两个文件的行数分别为 2,894,649、28,923,028，行长混合；两者末尾都不是换行，因此 ropey/crop 的最终空行语义差异不影响这次行数。重复拼接带来很强的内容重复与缓存局部性；不能外推到随机二进制、超长单行或所有 Unicode 文本。

## 1. bench-buffer

加载计时包含文件读取、UTF-8 校验和建树。ropey 使用 `Rope::from_reader`；crop 使用 64 KiB 流式 `RopeBuilder`，避免先保留完整 String 使 RSS 翻倍。String 是可选 baseline，仅测加载和整份 clone；没有实现 gap buffer，也没有进行其 O(N) 随机编辑。String 的进程 RSS 包含 clone 时同时存在的两份正文，不能当成单份加载后的常驻量。

| 实现 | 输入 | 加载 median ms | P95 ms | 进程峰值 RSS MiB | String clone ms |
| --- | --- | --- | --- | --- | --- |
| ropey | 100mb | 88.18 | 159.02 | 110.4 | 见下一表 |
| crop | 100mb | 24.70 | 35.93 | 101.2 | 见下一表 |
| string | 100mb | 15.39 | 18.63 | 192.3 | 2.53 |
| ropey | 1gb | 741.98 | 1,084.30 | 1,089.7 | 见下一表 |
| crop | 1gb | 257.15 | 575.37 | 996.2 | 见下一表 |
| string | 1gb | 169.17 | 762.46 | 1,908.5 | 27.05 |

| 实现 | 输入 | 行数读取×1M ms | char→line×1M ms | line→char×1M ms | 随机插入×10k ms | 随机删除×10k ms | clone ns/次 | ops 峰值 MiB |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| ropey | 100mb | 3.90 | 562.01 | 515.31 | 11.98 | 8.99 | 2.50 | 146.8 |
| crop | 100mb | 0.94 | 267.41 | 410.66 | 15.08 | 7.57 | 2.21 | 154.1 |
| ropey | 1gb | 3.49 | 1,172.28 | 1,011.00 | 17.91 | 13.07 | 2.17 | 1,128.0 |
| crop | 1gb | 0.95 | 578.49 | 787.99 | 18.45 | 10.60 | 2.08 | 1,043.4 |

**crop 不提供与 ropey 同义的 native Unicode-scalar `char_to_line/line_to_char` API**：此处调用 `line_of_byte/byte_of_line`，依赖 ASCII 输入的 byte==char。它不代表 crop 对 Unicode scalar 索引有同样的性能；编辑器需统一 byte/UTF-16/grapheme 坐标或补充索引。snapshot 按 1,000 份 clone 一批、持有至计时结束，7 批取每次摊销 median，排除 drop；纳秒值包含摊销的 Vec 分配，并非完整 undo 事务耗时。随机编辑各 5 组，每组从原始共享 snapshot clone，计入首次 COW 的成本。

| 实现 | 输入 | 光标数 | 每事务 median ms | P95 ms | 事务数 |
| --- | --- | --- | --- | --- | --- |
| ropey | 100mb | 10,000 | 2.446 | 2.916 | 100 |
| ropey | 100mb | 100,000 | 29.923 | 73.990 | 100 |
| crop | 100mb | 10,000 | 1.899 | 2.562 | 100 |
| crop | 100mb | 100,000 | 21.279 | 26.302 | 100 |
| ropey | 1gb | 10,000 | 2.531 | 3.042 | 100 |
| ropey | 1gb | 100,000 | 28.238 | 33.966 | 100 |
| crop | 1gb | 10,000 | 1.856 | 2.300 | 100 |
| crop | 1gb | 100,000 | 22.332 | 29.353 | 100 |

每个事务重新计算前 N 行的行首 offset，再逆序逐个插入 `foo`，计时包含 offset 查找、Vec 分配和全部插入；这是一段连续逻辑事务，没有调用 rope 的 bulk-edit API。连续 100 次不重置正文（分别增加 3 MB / 30 MB）；一直保留最初 snapshot，但没有为每个事务保留一份新的 undo snapshot。当前结果不能代替完整 selections/undo 合并成本。

## 2. bench-syntax：ST4 兼容性（重点）

### 2.1 官方语法载入

| 后端 | 文件数 | 逐文件 load OK | load 失败 | add_from_folder 失败前已加 | 去重未链接引用数 | 逐文件尝试+build ms | 峰值 MiB |
| --- | --- | --- | --- | --- | --- | --- | --- |
| fancy | 202 | 78 | 124 | 1 | 19 | 3,261.80 | 51.1 |
| onig | 202 | 78 | 124 | 1 | 19 | 1,403.24 | 31.4 |

`add_from_folder` 两个后端都在 `ASP/Embeddings/CSS (for ASP).sublime-syntax` 停止，错误为 `Missing mandatory key in YAML file: match`。随后独立尝试全部文件，成功的才加入同一 SyntaxSet。**load OK 只说明 YAML/regex 的表面结构被接受，不代表 ST4 语义兼容**，例如 `extends` 和 `branch_point/fail` 可能被忽略。成功子集的跨语言引用也不完整；`find_unlinked_contexts` 返回的是去重描述集合，19 不是所有引用出现位置的计数。

| 实际错误分类 | fancy | onig | 样本文件 |
| --- | --- | --- | --- |
| InvalidScope (scope depth > 8 in ActionScript) | 1 | 1 | ActionScript/ActionScript.sublime-syntax |
| MainMissing (often inheritance) | 13 | 13 | C#/Embeddings/Regex (for C# verbatim string).sublime-syntax |
| MissingMandatoryKey(contexts) | 8 | 8 | Batch File/Batch File (Compound).sublime-syntax |
| MissingMandatoryKey(match) | 102 | 102 | ASP/Embeddings/CSS (for ASP).sublime-syntax |

`MissingMandatoryKey(match)` 经常来自继承语法中的 `meta_prepend/meta_append` 等规则，不把 102 个错误全部归因于 branch。`MainMissing` / 缺少 contexts 常见于仅声明继承的文件。ActionScript 的 `InvalidScope` 错误文字写 “Max 2^16-2 atoms”，但本例实际有超过 8 段的 scope（如 `keyword.operator.assignment.augmented.bitwise.shift.left.unsigned.actionscript`）；syntect 的 scope 深度上限也是 8。没有观测到顶层 `RegexCompileError`，不代表运行中不会遇到 regex 编译错误。

| 源码关键字（锚定 YAML key） | 使用文件数 | 其中 load OK | 其中 load 失败 | key 出现次数 |
| --- | --- | --- | --- | --- |
| extends | 126 | 3 | 123 | 126 |
| version: 2 | 176 | 53 | 123 | 176 |
| branch_point | 20 | 16 | 4 | 160 |
| fail | 21 | 16 | 5 | 241 |
| embed | 52 | 19 | 33 | 378 |
| escape | 57 | 23 | 34 | 384 |

这些特征互相重叠，不能相加当成失败总数。`version: 2` 本身不会让加载器拒绝文件；embed/escape 在 syntect 中有实现，但依赖语法缺失仍会造成错误。稳定版 5.3.0 的 YAML loader 没有实现 extends/inheritance 与 branch/fail 的回溯语义，必须靠下面的行为测试判断接受后的实际效果。

### 2.2 官方 syntax_test_* 行为测试

适配 released syntect v5.3.0 的 `examples/syntest.rs`：按最近一个不含测试标记的源代码行的 scopes 检查 `^` 范围和 `<-` 行首；与该例程默认一致，`parse_test_lines=true`。每个断言范围全部字符匹配才记作一个 case 通过，同时保留按字符计数。现代 header 支持无空格 `T:SYNTAX TEST`；reindent 等 modifier 不执行；symbol/reference 的 `@` / `>` 标记明确跳过并记录。用人工 grammar 验证正向、负向、`<-` 与包含 Unicode 的行；两个后端自检均通过。运行时错误保留发生前的计数，后续记“未执行”。这测的是官方测试预期 scope，未启动 Sublime 本体。

官方成功载入子集（没有混入 bundled 语法）：

| 后端 | 语言 | 文件 P/F/E/B | 断言 P/F/未执行 | 已执行断言通过率 | 计划断言覆盖率 |
| --- | --- | --- | --- | --- | --- |
| fancy | Rust | 16/7/0/0 | 2402/11/0 | 99.54% | 100.00% |
| fancy | Python | 0/0/4/0 | 1311/399/4602 | 76.67% | 27.09% |
| fancy | JavaScript | 0/0/0/14 | 0/0/3013 | — | 0.00% |
| fancy | C++ | 2/1/0/0 | 1079/676/0 | 61.48% | 100.00% |
| fancy | JSON | 0/1/0/0 | 133/2/0 | 98.52% | 100.00% |
| fancy | Markdown | 0/1/1/0 | 662/4206/9 | 13.60% | 99.82% |
| fancy | HTML | 0/0/0/2 | 0/0/1012 | — | 0.00% |
| fancy | YAML | 6/1/0/0 | 603/4/0 | 99.34% | 100.00% |
| onig | Rust | 16/7/0/0 | 2402/11/0 | 99.54% | 100.00% |
| onig | Python | 0/0/4/0 | 1311/399/4602 | 76.67% | 27.09% |
| onig | JavaScript | 0/0/0/14 | 0/0/3013 | — | 0.00% |
| onig | C++ | 2/1/0/0 | 1079/676/0 | 61.48% | 100.00% |
| onig | JSON | 0/1/0/0 | 133/2/0 | 98.52% | 100.00% |
| onig | Markdown | 0/2/0/0 | 668/4209/0 | 13.70% | 100.00% |
| onig | HTML | 0/0/0/2 | 0/0/1012 | — | 0.00% |
| onig | YAML | 6/1/0/0 | 603/4/0 | 99.34% | 100.00% |

文件列：P=全部 scope cases 通过，F=完成但存在失败，E=解析错误或 panic，B=语法未加载。通过率只针对已执行 cases；覆盖率给出已执行/计划总数，避免 Python 的部分执行被误解为整体通过。JavaScript、HTML 的 B 是载入阻塞，不能伪装成 0 条失败。

同一批官方测试对 syntect bundled defaults 的补充对照（较旧语法快照；不等于成功加载当前 Packages）：

| 语言 | 测试文件数 | fancy 断言 P/F/未执行 (通过率) | onig 断言 P/F/未执行 (通过率) | 文件 P/F/E（fancy ; onig） |
| --- | --- | --- | --- | --- |
| Rust | 23 | 1326/1087/0 (54.95%) | 1326/1087/0 (54.95%) | 0/23/0 ; 0/23/0 |
| Python | 4 | 2601/3711/0 (41.21%) | 2601/3711/0 (41.21%) | 0/4/0 ; 0/4/0 |
| JavaScript | 14 | 673/2340/0 (22.34%) | 673/2340/0 (22.34%) | 0/14/0 ; 0/14/0 |
| C++ | 3 | 733/1022/0 (41.77%) | 733/1022/0 (41.77%) | 1/2/0 ; 1/2/0 |
| JSON | 1 | 19/116/0 (14.07%) | 19/116/0 (14.07%) | 0/1/0 ; 0/1/0 |
| Markdown | 2 | 834/4043/0 (17.10%) | 834/4043/0 (17.10%) | 0/2/0 ; 0/2/0 |
| HTML | 2 | 382/630/0 (37.75%) | 382/630/0 (37.75%) | 0/2/0 ; 0/2/0 |
| YAML | 7 | 305/302/0 (50.25%) | 305/302/0 (50.25%) | 1/6/0 ; 1/6/0 |

全仓共发现 242 个测试文件，其中 56 个纳入八语言 scope tests。排除项：outside the eight canonical target syntax paths = 177；unsupported non-scope test modifier: reindent-unchanged = 7；no scope assertions (symbol-only/fixture) = 2。只纳入上面八个 canonical syntax 路径；按 header 声明归类，而非所在目录或扩展名；例如 Rust 目录中声明 Markdown 的 frontmatter 测试计入 Markdown。TSX、MultiMarkdown、C 等派生/其他语法不混入分母。逐测试文件、具体失败 selector、actual scopes 和错误位置见 `data/syntax-{fancy,onig}-tests.json`。

| 后端 | 语法来源 | 运行错误样本 | 错误 |
| --- | --- | --- | --- |
| fancy | official | Rust/tests/syntax_test_frontmatter.md | runtime panic at line 5: regex string should be pre-tested: ParseError(127, TargetNotRepeatable) |
| fancy | official | Python/tests/syntax_test_conflicts.py | parse at line 208: Tried to use a ContextReference that has not bee resolved yet: ByScope { scope: <source.regexp.python>, sub_context: Some("base-literal"), with_escape: false } |
| onig | official | Python/tests/syntax_test_conflicts.py | parse at line 208: Tried to use a ContextReference that has not bee resolved yet: ByScope { scope: <source.regexp.python>, sub_context: Some("base-literal"), with_escape: false } |

### 2.3 parse 与 parse+style 吞吐

预读整个输入 String，计时排除 IO 与 SyntaxSet/ThemeSet 加载；每次新建 ParseState 或 HighlightLines，从头扫描所有行。parse-only 消费 scope ops，parse+style 用 `base16-ocean.dark` 消费 style spans；这两种模式是分别独立扫描，并不是相减得到 style 成本。每行保留换行。每个成功项 5 次取 median，包含该进程第一次触发的 lazy regex compile，但中位数通常由热缓存主导。

| 后端 | 语言 | parse MB/s | parse lines/s | parse+style MB/s | parse+style lines/s | 来源（fallback 明示） | 进程峰值 MiB |
| --- | --- | --- | --- | --- | --- | --- | --- |
| fancy | Rust | 0.328 | 9,751 | 0.304 | 9,032 | official successfully loaded subset | 34.3 |
| fancy | JavaScript | 0.619 | 17,429 | 0.592 | 16,660 | syntect bundled defaults | 35.0 |
| fancy | JSON | 2.234 | 13,066 | 1.672 | 9,777 | official successfully loaded subset | 26.9 |
| fancy | Markdown | 0.425 | 10,645 | 0.438 | 10,963 | syntect bundled defaults | 34.0 |
| onig | Rust | 1.023 | 30,383 | 0.882 | 26,206 | official successfully loaded subset | 17.8 |
| onig | JavaScript | 1.573 | 44,268 | 1.453 | 40,886 | syntect bundled defaults | 17.3 |
| onig | JSON | 2.155 | 12,604 | 1.457 | 8,519 | official successfully loaded subset | 25.2 |
| onig | Markdown | 16.938 | 423,895 | 16.101 | 402,935 | official successfully loaded subset | 13.9 |

Markdown 的相同 bundled grammar 补充对照（fancy 是必需 fallback，onig 是控制组）：

| 后端 | parse MB/s | parse lines/s | parse+style MB/s | parse+style lines/s |
| --- | --- | --- | --- | --- |
| fancy | 0.425 | 10,645 | 0.438 | 10,963 |
| onig | 0.796 | 19,933 | 1.104 | 27,624 |

| 未完成的原始尝试 | 来源 | 错误/状态 |
| --- | --- | --- |
| throughput-fancy-Markdown-parse | official successfully loaded subset | runtime panic: regex string should be pre-tested: ParseError(127, TargetNotRepeatable) |
| throughput-fancy-Markdown-highlight | official successfully loaded subset | runtime panic: regex string should be pre-tested: ParseError(127, TargetNotRepeatable) |

表中的 MB 是十进制（10⁶ bytes）。成功载入但高亮不能完成时会记录原错误；若有 bundled 重试，速度表明确使用重试来源。不同来源的 grammar 完整度不同，不能把较少 scope ops 的高吞吐当成等价语义下的引擎胜负。fancy 后端内部也会将某些 regex search 的错误当作 non-match，现有 API 无法完整统计此类丢失匹配。

### 2.4 SyntaxSet 启动

| 后端 | 相同子集含 Plain 数量 | YAML→build ms | 同子集 dump→load ms | bundled dump load ms | 生成自制 dump ms | dump KiB |
| --- | --- | --- | --- | --- | --- | --- |
| fancy | 79 | 2,989.61 | 0.771 | 0.338 | 5.139 | 657.5 |
| onig | 79 | 621.45 | 0.819 | 0.356 | 1.239 | 657.5 |

YAML 与自制 binary dump 使用同一批成功语法，均 5 次。YAML 包含 IO、regex 验证与 linking；dump 加载没有预热全部 contexts/regex，也没有加载 custom metadata，启动后的第一次实际解析仍需额外工作。bundled defaults 是 syntect 自带另一套预制语法，只作为常见部署路径的启动参考。不能用失败即停止的 `add_from_folder` 时间冒充全套加载时间。

### 2.5 tree-sitter 对照

| grammar | MB | 全量 median ms | 全量 MB/s | 中部插入 1 char 后 reparse ms | 原文 ERROR | 编辑后 ERROR | changed ranges | 进程峰值 MiB |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Rust | 10.0 | 729.51 | 13.71 | 41.218 | True | True | 1 | 585.4 |
| JavaScript | 10.0 | 544.54 | 18.36 | 2.745 | True | True | 0 | 410.0 |

相同的 10 MB 拼接输入，每项 5 次；在 byte 5,000,000 左右的 UTF-8 边界插入 `x`，正确调用 Tree::edit 和 InputEdit，再以旧树进行 incremental parse。编辑位置构造、String 拷贝与 Tree::edit 不计入 reparse；只测 parser.parse。这些输入含 ERROR 节点，Rust 的一次编辑造成较大重分析，不能据此断言所有正常 Rust 编辑都需要同样耗时。changed_ranges 是结构变化范围；0 不等于跳过解析或文本未变。RSS 包含相邻 full samples 暂时同时持有的旧树/新树以及正文副本，不是单棵 AST 的大小。该对照没有主题着色，也不提供 Sublime syntax 兼容性。

## 3. bench-search

| regex::bytes 搜索 | 输入 MB | median ms | MB/s | match 数 | 进程峰值 MiB |
| --- | --- | --- | --- | --- | --- |
| literal | 1,000 | 51.595 | 19,381.7 | 842,628 | 955.9 |
| complex | 1,000 | 2,365.984 | 422.7 | 1,350,800 | 957.8 |

literal pattern 为 `pub fn`；complex pattern 为 `(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+[A-Za-z_]\w*\s*(?:<[^>\n]{0,160}>)?\s*\(`。已预读 1 GB 输入，计时只扫描和计数，不含磁盘 IO、结果排序或 UI 展示。

| find in files：8 workers | 文件数 | 选中 bytes | 实际扫描 bytes | 匹配行 | 耗时 ms | 扫描 MB/s | 错误 / binary |
| --- | --- | --- | --- | --- | --- | --- | --- |
| first cold-ish | 13306 | 326280980 | 173361595 | 64053 | 438.17 | 395.65 | 0 / 2880 |
| warm（5 次 median） | 13306 | 326280980 | 173361595 | 64053 | 226.44 | 765.60 | 0 / 2880 |

范围为原有 `~/.cargo/registry/src`、本次隔离的 `data/cargo-home/registry/src` 和官方 Packages；部分 crate 在两份 registry 中重复，但路径不同，按两个文件搜索。ignore 默认过滤，隐藏文件/忽略规则生效；grep-regex + grep-searcher 流式读文件，NUL 判 binary 后退出。吞吐以 SinkFinish 实际扫描 bytes 计算，未用包含 binary 未读尾部的 selected bytes 虚增吞吐。fuzzy 的路径采集用普通 WalkDir，包含隐藏路径，因此其真实路径数与此表不同。

| Goto Anything query | 总路径 | 真实 | 合成 | 全量 score+top50 ms | 逐字符总 ms | 最后一键 ms | UTF32 索引构建 ms | 峰值 MiB |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| srcmainrs | 100000 | 14017 | 85983 | 26.43 | 373.80 | 22.32 | 5.16 | 38.7 |
| tokioruntime | 100000 | 14017 | 85983 | 12.73 | 160.44 | 10.04 | 5.16 | 38.7 |

| query | 当前 prefix | 重算候选数 | 剩余候选 | 每键 median ms |
| --- | --- | --- | --- | --- |
| srcmainrs | s | 100000 | 100000 | 5.155 |
| srcmainrs | sr | 100000 | 99576 | 68.328 |
| srcmainrs | src | 99576 | 98958 | 73.395 |
| srcmainrs | srcm | 98958 | 84398 | 58.491 |
| srcmainrs | srcma | 84398 | 53281 | 48.521 |
| srcmainrs | srcmai | 53281 | 43176 | 38.575 |
| srcmainrs | srcmain | 43176 | 33279 | 31.357 |
| srcmainrs | srcmainr | 33279 | 27043 | 26.287 |
| srcmainrs | srcmainrs | 27043 | 23413 | 22.318 |
| tokioruntime | t | 100000 | 100000 | 6.845 |
| tokioruntime | to | 100000 | 87131 | 52.015 |
| tokioruntime | tok | 87131 | 31746 | 16.282 |
| tokioruntime | toki | 31746 | 26119 | 13.470 |
| tokioruntime | tokio | 26119 | 22902 | 12.219 |
| tokioruntime | tokior | 22902 | 20756 | 13.398 |
| tokioruntime | tokioru | 20756 | 17317 | 6.243 |
| tokioruntime | tokiorun | 17317 | 17264 | 6.506 |
| tokioruntime | tokiorunt | 17264 | 17232 | 7.238 |
| tokioruntime | tokiorunti | 17232 | 17207 | 7.514 |
| tokioruntime | tokioruntim | 17207 | 17197 | 7.723 |
| tokioruntime | tokioruntime | 17197 | 17197 | 10.039 |

合成路径是确定性的 workspace/project/crate 路径，部分明确含 `src/main.rs` 和 `tokio-runtime/src/runtime/worker_*.rs`；其余保留真实路径后缀。每个 query/typing sequence 重复 7 次。计时排除文件遍历与预先 UTF32 转换，包含 pattern parse、逐候选打分、全部匹配项排序和取 top50。incremental typing 只重新打分上一键仍匹配的候选，ASCII 查询单调延长；每次 sequence 从全部路径重新开始。没有接入可选的 nucleo 后台调度器，也未测取消、debounce 或 UI 线程占用。

## 4. bench-text：headless shaping/layout

| shaping | 首次全量 ms | 热字体全量 ms | 全量 P95 ms | 单行重排 ms | 单行 P95 ms | 缓存 viewport ms | 进程峰值 MiB |
| --- | --- | --- | --- | --- | --- | --- | --- |
| advanced | 8.687 | 7.175 | 8.593 | 0.0462 | 0.0483 | 0.000625 | 18.0 |
| basic | 5.164 | 4.900 | 5.446 | 0.0369 | 0.0395 | 0.000625 | 20.0 |

| 模式 | 独立 raster cache keys | 首轮缓存填充+24k 查询 ms | 热缓存24k查询 ms | 24k次不缓存 raster ms |
| --- | --- | --- | --- | --- |
| advanced | 131 | 0.903 | 0.190 | 47.249 |
| basic | 131 | 0.669 | 0.194 | 46.770 |

字体为 `/System/Library/Fonts/Menlo.ttc`（Menlo Regular/Bold/Italic/BoldItalic），只加载这套字体，避免全系统字体扫描干扰。120 行，每行严格 200 个 ASCII code characters，14 px font / 20 px line height、Wrap::None；断言布局输出 120 runs、24,000 glyphs。完整布局每次创建新 Buffer 并 set_text，含 shape/layout 和 glyph 数消费，FontSystem 热缓存复用；单行只替换第 61 行末尾字符再 shape 可见范围。首轮 full 使用字体 cache 初始化，因此另列。advanced/basic 同一进程依次跑，RSS 是累计高水位。没有测试 CJK、emoji fallback、bidi、混合字体或 ligature 丰富的文本。

全视口重建超出 1–2 ms 目标，但单行更新足够小。Swash 的 headless 对照显示逐 glyph 重复 rasterization 明显昂贵，必须缓存 glyph image；GPU glyph atlas 是合理的实现方向。这里没有测 winit/wgpu/glyphon 窗口、GPU upload、draw calls、滚动或端到端 frame time，不能把热 CPU cache 查询时间当作 GPU 帧耗时。可选窗口演示已跳过。

## 结论/风险

1. **Rust 的 buffer / search building blocks 足以支撑继续做原型。** crop 的加载和多光标插入在本输入上更快，ropey 提供直接的 Unicode scalar 坐标；两者 cheap snapshots 都远小于 String 整份复制。100k cursors 的逐点插入已超过 16.7 ms 一帧，需要批量事务、游标位置更新策略与 undo/COW 一起优化。

2. **当前 stable syntect 不是 ST4 default Packages 的直接替代引擎。** 124/202 文件加载失败；成功子集仍有 19 条 unresolved references。extends 使用 126 个文件，branch_point 使用 20 个文件。Rust/JSON/YAML 的官方语法行为接近，但并非全过；C++/Markdown 差距大；Python 未跑完整；JS/HTML 载入受阻。把 bundled defaults 当当前 Packages 会得到显著更低通过率。缺口不能靠“换成 onig”整体修复。

3. **若 ST4 syntax compatibility 是产品要求，应预留实现/维护引擎的工作量。** 至少包括 inheritance 与相对/跨包引用解析、meta_prepend/meta_append、branch_point/branch/fail 的跨行回溯、version 2 的 scope/pop 语义、完整 embedded dependencies、scope 深度，以及官方测试 harness 的 symbol/reference 扩展。可选择深度改造 syntect 或另写解析器；需要持续跑官方 syntax tests 验收，而不是只看 YAML 能不能 load。onig 的语法支持/执行路径在本次更稳，但动态 backreference、未链接上下文和 fancy non-match 的静默错误仍须处理。

4. **高亮需要延迟和增量架构。** binary dump 可大幅缩短 SyntaxSet 构造；regex/context 首次编译、文件正文解析仍不能放进启动关键路径。大文件应先显示可见内容，后台 parse，保存行状态/checkpoint，只使受影响后缀失效。tree-sitter 提供结构化和 incremental 能力，但其 grammar/queries 不能替代 Sublime YAML 语义，且本次 Rust 单点编辑的重分析仍高于一帧。

5. **布局应缓存未变行并配合 glyph atlas。** 每帧重新 shape 24k glyphs 不满足 1–2 ms，但单行重排已很小。完整 editor 仍需自行实现 selections、IME/复杂文字、undo transaction、文件编码/换行、viewport/scroll、渲染调度、插件 API 和 UI；本 spike 不涵盖这些端到端成本。fuzzy 可以从 nucleo-matcher 起步，但大候选集仍应后台重算、取消过期 query，并避免每键全排序。

## 复现

在仓库根目录执行：

```sh
./spikes/run_all.sh
```

脚本只在 `spikes/` 下写入：隔离 `CARGO_HOME=data/cargo-home`、数据/克隆/日志在 `data/`、release 构建在 `target/`。`data/` 和 `target/` 已 gitignore。第一次运行联网下载；之后 Cargo.lock 与保留的数据、commit、source manifest/hash 保持输入可追溯。若从空 data/ 重跑，会浅克隆当时 Packages HEAD，并依赖当时 registry 的可用源文件；**要严格复现本表，请保留 data/ 或将 Packages checkout 到本报告 commit 并使用 source manifest 所记录的同版本输入**，不能声称任意未来 HEAD 都是同一数据。

每个基准是独立 binary crate；从 `spikes/` 执行示例：

```sh
export CARGO_HOME="$PWD/data/cargo-home"
cargo run --release --locked -p bench-buffer -- --engine crop --file data/text-1gb.txt --mode ops
cargo run --release --locked -p bench-syntax -- compat
cargo run --release --locked -p bench-syntax -- tests
cargo run --release --locked -p bench-syntax --no-default-features --features onig -- tests
cargo run --release --locked -p bench-syntax -- throughput --language Rust --mode highlight
cargo run --release --locked -p bench-syntax -- tree --language JavaScript
cargo run --release --locked -p bench-search -- regex
cargo run --release --locked -p bench-search -- files --threads 8
cargo run --release --locked -p bench-search -- fuzzy --paths 100000
cargo run --release --locked -p bench-text -- --font /System/Library/Fonts/Menlo.ttc
```

`run_all.sh` 在计时前完成两个后端构建并复制独立可执行文件，随后串行直接运行 release binaries，保证计时不含 cargo 构建开销。可按组重跑：`./spikes/run_all.sh --only syntax-tests`；或已构建时用 `python3 spikes/scripts/run_benchmarks.py --only throughput --force`。最后 `python3 spikes/scripts/report.py` 从原始 JSON 自动重建此文件。源码更新/日志重跑会先备份原内容到被忽略的 `data/backups/`，再原子替换。未执行 git commit。

### 运行完整性

记录了 42 个独立命令。所有最终记录的命令均正常退出；syntect 已捕获的加载/运行问题仍保留在语法结果中，并未改写为通过。
