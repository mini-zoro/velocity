# Velocity：用 Rust 复刻 Sublime Text 4 —— 可行性分析与实施方案

> 日期：2026-11-13 ｜ 参照对象：Sublime Text 4 Build 4213（2026-09-21，当前最新稳定版）
> 本机：Apple M1 Pro / 8 核 / 16 GB / macOS 26 ｜ rustc 1.99.1
> 配套实测（均由 codex 子代理 · gpt-6.1-sol · max 编写并运行）：`spikes/`（buffer/语法/搜索/排版基准，`spikes/RESULTS.md`）、`prototypes/sregex/`（多模式正则原型，`prototypes/sregex/RESULTS.md`）

---

## 0. 结论先行

| 问题 | 结论 |
|---|---|
| 技术上能不能做？ | **能。** Sublime 的每个核心子系统在 Rust 生态里都有成熟的基础件或可行的自研路线。Zed 已经证明 Rust + GPU 自绘可以做出一线编辑器。 |
| 性能能否接近 Sublime？ | **文件 IO、搜索、多光标编辑**：实测已经达到或超过 ST 的水平（1GB 文件加载 257ms、内存约 1 倍文件大小；1 万光标一次事务 1.9ms；字面量搜索 19GB/s；13k 个文件全文搜索 226ms）。**排版、模糊匹配**要靠缓存和后台化才能达标（属于工程问题）。**语法高亮**是唯一真正的难点：实测 syntect 只能加载 **78/202** 个官方 ST4 语法，速度只有 ST3 的约 1/3（30k 行/s 对 94k 行/s），必须自研语法引擎和多正则引擎。官方 13,813 条正则中约 96% 能走 DFA；sregex 原型实测比 Oniguruma 逐条匹配快 1.36 倍（几何平均 1.92 倍），并且结果逐字节一致。**方向已经验证，但要追平 ST 还需要一个深度优化的生产级引擎**（见 §3.5）。好在编辑时只重新高亮可视区，所以吞吐差距不影响打字手感。 |
| "功能基本一致"能否做到？ | 编辑器本体（编辑、多光标、语法、主题、项目、搜索、构建、索引、Git、Vim 模式等）：**可以做到高度一致**。**Python 插件 API 兼容**（约 58 个类、460+ 个方法、83 个模块函数，外加事件回调）是工作量最大、长尾最长的一块，建议放在后期单独作为一个阶段。 |
| 多久？ | 按已定范围（**只做 macOS、暂不做 Python 插件、codex 2 路并行**）估算：**约 8～10 周可以日常使用，约 6～8 个月发布功能基本一致的 macOS 1.0**。最后 10% 的手感打磨耗时通常和前 90% 相当。 |
| 最大风险 | ① 语法引擎的速度和保真度；② 插件 API 的行为一致性；③ macOS 中文输入法（IME）和原生手感；④ 字体渲染观感；⑤ 法律边界（名称、主题、默认包都不能照搬）。 |

**建议：做。** 采用"内容格式 100% 兼容 + 自研核心引擎 + 测试驱动的 AI 并行开发"路线。基准测试和 sregex 原型已经完成（见 §3），P0 剩下三件事：在本机实测 ST4 基线、验证 wgpu 首帧启动时间、验证 macOS 中文 IME。拿到数据后再决定是否全面投入。

---

## 1. 调研：Sublime Text 4 能力全景

### 1.1 功能清单（按子系统归类，来源：官方文档 + 各版本更新日志）

**编辑核心**
- 多选区/多光标：Ctrl/Cmd+D 逐个加选、Split into Lines、列选择（Alt/Option 拖拽）、全部选中同名词（Find All）、Expand Selection（到词/行/括号/缩进/作用域/标签）
- 完整的文本命令集：行操作（复制、移动、交换、合并、删除）、大小写转换（含 Convert Case）、缩进/反缩进/重新缩进、行注释/块注释、Wrap Paragraph、Sort Lines、Permute、Transpose、Jump Back/Forward、Soft Undo/Redo
- 撤销历史随会话持久化（4213 起：文件在外部被修改时不再恢复撤销历史）
- 自动缩进与缩进检测、自动配对括号/引号、Tab/空格转换、末尾空白清理（`trim_only_modified_white_space`）
- 代码折叠：按缩进 + 按语法（4142 起）
- 宏录制/回放（含 Find 命令）、片段（`.sublime-snippet`：字段、镜像、正则变换）
- 自动补全：buffer 内词 + 全项目索引（上下文感知）、`.sublime-completions`、插件补全（带 kind/annotation/details）
- 拼写检查（Hunspell 词典）、空白字符绘制、相对行号、标尺、自动换行（`wrap_width_style`）、连字（ligatures）

**导航与搜索**
- Goto Anything（Cmd+P）：模糊匹配文件，支持 `@` 符号、`:` 行号、`#` 词，可组合使用；Ctrl/Cmd 打开并排视图
- 命令面板（`.sublime-commands`）、Goto Symbol / Goto Symbol in Project / Goto Definition（F12）/ Goto Reference（Shift+F12），悬停弹出定义
- 查找/替换：正则、大小写、全词、选区内、保留大小写、增量查找、Find All、高亮全部匹配
- 在文件中查找（Find in Files）：文件模式过滤、结果缓冲区可点击跳转、在打开的文件中替换

**界面与布局**
- 标签页（多选标签 Tab Multi-Select，并排预览）、分屏布局（列/行/网格）、多窗口、侧边栏（文件夹树、打开的文件、Git 状态徽章）
- Minimap、无干扰模式（Distraction Free）、全屏、状态栏、输出面板、控制台（Python REPL）
- 主题引擎（`.sublime-theme`）：基于规则匹配控件 class/attributes/settings/parents/platforms，每个控件最多 4 层纹理/填充，支持动画、变量、继承、文件图标主题（4213 新增）
- 配色方案（`.sublime-color-scheme` JSON + 旧格式 `.tmTheme`）：变量、颜色函数、`hashed_foreground`、渐变背景、亮/暗模式自动切换
- minihtml：自研的 HTML/CSS 子集引擎，用于弹窗（popup）、行内块（phantom）、行尾注解（annotation）、HTML 标签页
- GPU 渲染（OpenGL 4.1，macOS 默认开启）、图片预览（PNG/JPG/GIF/TGA/PSD）

**语言支持**
- `.sublime-syntax`（v1/v2）：上下文栈、`push/pop/set/embed/escape`、`branch_point/fail`（非确定性回溯）、`extends` 多继承、`with_prototype`、`meta_*`、`clear_scopes`、懒加载 embed，4213 新增选择器 `>` 子作用域运算符
- `.tmLanguage` 兼容；`.tmPreferences`（缩进规则、注释符号、符号列表/索引规则）
- 官方默认语法包 sublimehq/Packages：**202 个语法、242 个语法测试文件**（许可证允许复制、修改、商用）
- 索引引擎：后台多进程扫描项目，记录定义与引用，提供 Goto Definition 和上下文补全

**项目与工具**
- 项目文件 `.sublime-project` / `.sublime-workspace`、Quick Switch Project、会话恢复和 Hot Exit（未保存内容也能恢复）
- 构建系统 `.sublime-build`：`cmd/shell_cmd`、`file_regex/line_regex` 错误跳转、变体（variants）、`selector` 自动选择、环境变量、用行内注解显示错误
- Git 集成：侧边栏状态徽章、状态栏分支信息、Diff 标记（对比磁盘或 HEAD）、增量 Diff（Show/Revert Diff Hunk）、与 Sublime Merge 联动
- 命令行 `subl`（单实例、`--wait`、读 stdin、`--safe-mode`），Vintage（Vi）模式，安全模式

**可定制性与扩展**
- 设置分层：Default → 平台 → User → 语法专属 → 项目 → 视图；JSON 允许注释和尾逗号，程序修改设置时保留注释与布局
- 键位 `.sublime-keymap`：组合/序列键（Cmd+K, Cmd+…）、修饰键双击（`["ctrl","ctrl"]`）、上下文条件（`context`：选择器/设置/正则等），鼠标映射 `.sublime-mousemap`，菜单 `.sublime-menu`
- 包系统：目录包 / `.sublime-package`（zip），资源覆盖机制（Packages/、Installed Packages/、User 覆盖）
- Python 插件 API：独立 `plugin_host` 进程，内嵌 Python 3.14（4213 起替代 3.8）+ 遗留 3.3（默认禁用）；`sublime` / `sublime_plugin` 两个模块，**约 58 个类、462 个方法、83 个模块函数**（统计自官方 API 参考），外加约 40 个事件回调（`on_modified_async`、`on_query_context`、`on_query_completions`……）
- 生态核心：Package Control、LSP（sublimelsp）、SublimeLinter、Terminus、GitGutter、A File Icon 等

### 1.2 内部架构（公开信息整理）

| 模块（内部名） | 作用 | 来源 |
|---|---|---|
| **px** | 自研平台层：窗口、事件、文件、系统集成（Win/Linux/macOS） | Tristan Hume 逆向符号表 |
| **skyline** | 自研控件框架，核心是 `skyline_text_control` | 同上 |
| **render context** | 渲染抽象层：ST3 用 Skia（CPU），ST4 用 OpenGL 4.1（GPU）。字形批处理让单帧耗时从 52ms 降到 3ms，OpenGL 后端约 9000 行 | 官方博客 *Faster Rendering Using Hardware Acceleration* |
| **sregex** | 自研高速正则引擎，推测能同时匹配多条正则；不支持的特性回落到 Oniguruma | Hume 逆向 |
| **TokenStorage** | 存储并渲染高亮 token | Hume 逆向 |
| 索引 | 后台低优先级**多进程**，leveldb 存符号 | 官方 Indexing 文档 + 逆向 |
| plugin_host | 独立进程运行全部插件（崩溃隔离），API 调用是同步 RPC | 官方 API Environments 文档 |
| 其他依赖 | Google densehash、Boost、breakpad、Hunspell、yaml-cpp、snappy、lzma；自研 git 库 | 逆向 + 官方博客 |

要点：Sublime 的快来自"全栈自研 + 简单 2D 绘制 + 批处理 + 专用正则引擎 + 后台进程"。**它没有任何不可复制的技术壁垒**，难点在于十几年积累下来的工程打磨。

### 1.3 公开的性能基线

| 指标 | Sublime Text 4 | 来源/说明 |
|---|---|---|
| 冷启动 | 原版约 50–80ms；也有报告 0.4s 到 1.4s 的（差异主要来自环境） | Sublime 论坛用户实测；MonoVM 2026 |
| 空载内存 | 约 55–80MB | 同上 |
| 按键到屏幕的延迟（sqlite3.c，15 万行） | 中位数 18.1ms / P95 30.0ms（60Hz 屏，Zed 为 28.6/38.2） | Peeknext 基准 2026-08，M2 Pro。注：发布方是竞品，可能有偏向 |
| 按键延迟（gcc.c，75 万行） | 20.2ms / P95 30.4ms | 同上 |
| 大文件 | 100 万行占 260MB、500 万行占 1.1GB；1000 万行以上没通过可用性测试。整个文件读入内存，不做 mmap 或分页 | 同上 + Sublime 论坛 |
| 长行 | 单行 1 万字符以上会明显卡顿 | 论坛 |
| 高亮速度（ST3 时期） | jQuery 247KB / 9200 行：**98ms**（约 9.4 万行/s）；syntect 同样测试 600ms（约慢 6 倍） | syntect README（较旧，以 §3 实测为准） |
| GPU 渲染 | 4K/8K 下 3ms/帧（独显）；Intel HD 6000 上 4K 为 9ms | 官方博客 |

**目标定义**：在"同一台机器、同一测试文件"条件下，下面每项指标做到 ST4 的 **≤1.2 倍**；大文件和长行这两项要**超过** ST4（它的弱项，也是我们的差异化机会）。P0 阶段会在本机装 ST4 实测，建立真实基线。

---

## 2. Rust 生态现状与前车之鉴

| 项目 | 现状（2026-11） | 对我们的启示 |
|---|---|---|
| **Zed / GPUI** | Zed 很成熟。GPUI 官方 crate 停在 0.2.2（2025-10），2026-02 官方宣布暂停对社区的 GPUI 支持；社区有 gpui-ce 0.3、gpui-unofficial、Longbridge 的 gpui-fast 等 fork | 证明了 Rust GPU 自绘编辑器可行。但 GPUI 依赖不稳定，样式模型也和 `.sublime-theme` 规则引擎不匹配。**可以借鉴设计（SumTree、平台层、文本系统），不建议直接依赖。** 另外，Zed 在大文件和按键延迟上不如 ST，说明"用 Rust"不等于"快"，架构才是关键 |
| **Lapce / floem** | 0.4.6（2026-01），开发缓慢，有稳定性问题；floem 的 wgpu 版本落后 | wgpu 自绘可行，但通用 GUI 框架会拖慢编辑器的关键路径 |
| **xi-editor** | 已归档。作者 Raph Levien 的复盘：rope 设计很成功；**前后端异步分离、CRDT、异步插件是错误**，复杂度成倍增加 | 核心用**同步、单线程拥有 buffer** 的模型；插件同步 RPC（和 ST 一样）；后台工作只读快照 |
| **Helix** | 终端编辑器，ropey + tree-sitter；贡献了 `regex-cursor`（能在 rope 上做正则搜索）和 `spellbook`（纯 Rust Hunspell） | 两个库都可以直接用 |
| **syntect 5.3** | 能读 sublime-syntax。主线对 `branch_point`、v2 语义的支持不完整；社区 PR #705（2026-08，**未合并**）声称修复后官方语法测试 0 失败 | 可以作为语义参考和对照基准，但性能架构（逐条正则 + Oniguruma）注定追不上 ST，**需要自研引擎** |
| 其他基础件 | wgpu 30、winit 0.31-beta、cosmic-text 0.19、swash、harfrust 0.16、parley 0.13、ropey 2.0-beta、crop 0.4、regex-automata 0.4、fancy-regex 0.20、onig 6.5、nucleo 0.5、ignore/grep-searcher、gix 0.90、pyo3 0.30（完整支持 Python 3.14）、tree-sitter 0.27、notify、encoding_rs、muda、arboard、rfd、accesskit | 基础件都很齐全。需要自研的是"编辑器专用"的部分 |

---

## 3. 技术验证（Spike）实测结果

> 本节数据来自 `spikes/`，由 codex 子代理（gpt-6.1-sol · max）编写并运行；原始数据见 `spikes/RESULTS.md`。

测量注意：测试时本机同时有其他会话的 codex 进程在跑（load average 约 10–12，机器是 8 核），**绝对数值偏保守**；同机串行对比得出的相对结论有效。没有测 Sublime 本体（P0 补测）。

### 3.1 文本存储（bench-buffer，crop 0.4.3 vs ropey 1.6.1）

| 指标 | crop | ropey | 结论 |
|---|---:|---:|---|
| 加载 100MB（中位数） | **24.7ms** | 88.2ms | crop 快约 3.5 倍 |
| 加载 1GB（中位数 / 峰值内存） | **257ms / 996MB** | 742ms / 1090MB | 内存约等于 1 倍文件大小，比 ST 的大文件表现更好 |
| 1 万个光标同时插入（一次事务，1GB 文件） | **1.86ms** | 2.53ms | 远低于一帧（16.7ms） |
| 10 万个光标同时插入 | 22.3ms | 28.2ms | 超过一帧 → 需要批量拼接 API（当前是逐点插入） |
| 快照克隆（用于撤销/后台线程） | 约 2ns | 约 2ns | O(1) 结构共享，后台只读快照的模型成立 |
| 随机插入 1 万次（1GB 文件） | 18.5ms | 17.9ms | 约 1.8µs/次 |

**选型**：底座选 crop（加载和多光标更快），在上面补 char/UTF-16 度量索引（crop 原生只有字节和行的索引）；或者参考 crop/Zed SumTree 自研多度量 rope。

### 3.2 语法（bench-syntax，syntect 5.3.0 对官方 Packages @ `3f8b2c5`）—— 关键结论

**兼容性：syntect 稳定版无法承载 ST4 的默认语法集**

| 项目 | 结果 |
|---|---|
| 202 个官方语法逐个加载 | **78 个成功 / 124 个失败**（onig 和 fancy 两个后端结果一样）；成功的子集里还有 19 处跨语法引用没有链接上 |
| 失败原因 | 缺少 `match`（继承语法的 `meta_prepend/append` 等）102 个；缺少 `main`/`contexts`（只声明了 `extends` 的文件）21 个；作用域深度超过 8 层 1 个 |
| 用到 `extends` 的 126 个文件 | **123 个加载失败** |
| 运行期错误 | fancy-regex 编译动态正则时 panic；Python 在第 208 行遇到未解析的上下文引用后中止 |

**官方语法测试的断言通过率（onig，只算成功加载的子集）**

| 语言 | 通过/失败/未执行 | 通过率 |
|---|---|---:|
| Rust | 2402 / 11 / 0 | 99.5% |
| YAML | 603 / 4 / 0 | 99.3% |
| JSON | 133 / 2 / 0 | 98.5% |
| Python | 1311 / 399 / **4602 没执行** | 76.7%（覆盖率只有 27%） |
| C++ | 1079 / 676 / 0 | 61.5% |
| Markdown | 668 / 4209 / 0 | **13.7%** |
| JavaScript / HTML | 语法加载失败 | — |

**吞吐（只解析不着色 / 解析加着色，10MB 输入）**

| 后端 | Rust | JavaScript（syntect 自带的旧版语法） | JSON |
|---|---|---|---|
| fancy-regex | 0.33 MB/s · 9.8k 行/s | 0.62 MB/s · 17k 行/s | 2.2 MB/s |
| **oniguruma** | **1.02 MB/s · 30k 行/s**（加着色：26k 行/s） | 1.57 MB/s · 44k 行/s（加着色：41k 行/s） | 2.2 MB/s |
| *参照：ST3* | — | *jQuery 9200 行用时 98ms ≈ 94k 行/s，约 2.5MB/s，**这个数字包含着色*** | — |

> Markdown 在 onig 下能跑到 16.9 MB/s，但断言通过率只有 13.7%。快只是因为大量规则没有生效，**这个速度不能算数**。

**结论**：syntect 在 ST4 语义上（继承、branch/fail、v2）缺口很大，速度比 ST3 慢约 2.3–3.6 倍（按行/s，包含着色；ST4 官方说比 ST3 还快）。**必须自研语法引擎 + 多正则引擎**，syntect 只作为语义参考和下限基准。另外，SyntaxSet 用二进制 dump 加载只要 **0.8ms**（从 YAML 构建要 621ms），说明"预编译缓存"这条启动路径是对的。

**tree-sitter 对照**：10MB 文件全量解析，Rust 730ms、JS 545ms；在 Rust 文件中部改一个字符后增量重解析要 41ms。它不兼容 sublime-syntax，只作为未来可选的"结构化能力"（比如折叠和选择扩展）候选。

### 3.3 搜索与导航（bench-search）

| 项目 | 结果 | 结论 |
|---|---|---|
| 1GB 内存文本，字面量正则 `pub fn` | **19.4 GB/s** | 没问题 |
| 1GB 内存文本，复杂正则 | 423 MB/s | 没问题 |
| 在文件中查找（13,306 个文件 / 173MB，8 线程） | 首次 438ms，缓存热时 **226ms** | ripgrep 级别 |
| Goto Anything（10 万条路径，nucleo-matcher） | 全量打分 + top50：12.7–26ms；**逐字符增量输入每键 6–73ms** | ⚠️ 超出 10ms 目标 → 需要后台线程 + 取消过期查询 + 只做 top-K 部分排序 + 前缀结果复用（nucleo 的并发调度器）+ 预先计算路径分段 |

### 3.4 文字排版（bench-text，cosmic-text 0.19 + Menlo）

| 项目 | 结果 | 结论 |
|---|---|---|
| 满屏 120×200 字符全量排版（热缓存） | 4.9–7.2ms | ⚠️ 超出 1–2ms 的预算 → 不能每帧全量排版 |
| 单行重排 | **0.037–0.046ms** | ✅ 行级缓存 + 只重排脏行完全够用 |
| 2.4 万个字形查询（热缓存） | 0.2ms；不缓存直接光栅化要 48ms | 必须用字形图集（glyph atlas） |

**结论**：排版走"行级布局缓存 + 等宽快速路径（ASCII 等宽字体可以跳过完整 shaping）+ GPU 字形图集"，和 ST 的做法一致。

### 3.5 正则特性统计与 sregex 原型（自研引擎的依据）

对 sublimehq/Packages 全部 202 个 `.sublime-syntax`、**13,813 条 `match` 正则**（已展开 `{{variables}}`）的统计，脚本见 `spikes/scripts/regex_features.py` 和 `lookahead_pos.py`：

| 特性 | 条数 | 占比 | DFA 能否处理 |
|---|---:|---:|---|
| 不含任何回溯特性 | 9,306 | 67.4% | ✅ 直接进多模式 DFA |
| 含前瞻 `(?=` `(?!` | 4,390 | 31.8% | 见下 |
| ├ 整条就是一个前瞻 | 2,019 | 14.6% | ✅ 改成"零宽锚定匹配"，DFA 处理 |
| ├ 前瞻只在开头/结尾 | 1,911 | 13.8% | ✅ DFA 匹配主体后，在端点做一次锚定 DFA 验证 |
| └ 前瞻嵌套在中间 | 460 | 3.3% | ❌ 需回溯引擎 |
| 后顾 `(?<=` `(?<!` | 19 | 0.1% | ❌（短的定长后顾可以反向 DFA） |
| 反向引用 `\1` `\k<>` | 96 | 0.7% | ❌ |
| 占有量词 `*+` 等 | 50 | 0.4% | ⚠️ 多数可以等价改写 |

**合计约 96% 的正则能走 DFA 快路径，回溯引擎只需兜底约 4%。** 这正是 Sublime 用 sregex（快）+ Oniguruma（兜底）的设计。在 Rust 里可以用 `regex-automata` 的多模式 lazy DFA（`LeftmostFirst` 语义恰好等价于 ST 的"最早位置优先，同位置按规则顺序"）实现，兜底用 fancy-regex 或 onig。

**sregex 原型实测**（`prototypes/sregex/`，codex · gpt-6.1-sol · max；完整报告见 `prototypes/sregex/RESULTS.md`）

原型实现了 Oniguruma 语法前端（递归变量、include 展开和环检测、`\h`/POSIX/`(?x)` 等方言翻译），每个上下文合成一个多模式 hybrid DFA（B），首尾前瞻做锚定验证，其余规则回落到 Oniguruma，只给胜出的规则求捕获组。对照组是 syntect 式逐条匹配（A，带行内缓存）。

| 维度 | 结果 |
|---|---|
| **正确性**（740 个上下文，2.2 亿次去重查询） | 除一类已知差异外完全一致：546 处差异全部来自 `²³¹¼½¾` 这 6 个字符的 Unicode `\b` 语义（Onig 和 Rust 的词字符定义不同，可以在前端修正）。Markdown 有 7 处是 Oniguruma 自己触发回溯上限。10 项 release 测试全部通过 |
| **速度**（8 种语言 × 4 个热点上下文，完整语料，3 轮） | 合计比 Oniguruma **快 1.36 倍**（工作量完全一致的 31 个上下文：1.69 倍），几何平均 **1.92 倍**，比 fancy-regex 快 2.44 倍；**28/32 个上下文更快** |
| 分语言 | Python 3.90× · Markdown 2.57× · JS 1.65× · HTML 1.61× · JSON 1.45× · C++ 1.35× · YAML 1.18× · **Rust 0.54×** |
| 最好与最差 | Rust `numbers` **10.3×**、`keywords` 8.8×；Rust `statements`（128 条规则）**0.46×**，C++ `global` 0.91× |
| **编译成本**（一次性编译全部上下文） | C++ **2.96 秒 / 370MB**，Rust 0.58 秒 / 163MB（A-Onig 只要 15ms / 12MB） |

**这组数据说明了什么**

1. **方向成立**：多模式 DFA 在大多数上下文上更快，而且可以做到逐字节语义一致，这是自研引擎的前提。
2. **"96% 能进 DFA"不等于"高亮快 6 倍"**：规则多、零宽规则密集、守卫（guard）复杂的上下文（Rust `statements`）里，候选验证会吃掉收益。生产引擎必须做到：把常见的有限前瞻编译进自动机本身，而不是事后验证；按规则优先级输出候选集合，避免逐条锚定修复；字面量预过滤；按 NFA 大小配置 DFA 缓存。
3. **必须懒编译 + 磁盘缓存**：一次性全量编译在启动和内存上都不可接受（C++ 要 3 秒）。只编译活跃的上下文，相同的规则集合共享编译结果，编译产物缓存到磁盘（syntect 的二进制 dump 加载只要 0.8ms，可以佐证）。
4. **正则只是一部分**：Hume 说 syntect 有约 50% 的时间花在正则上，所以按 Amdahl 定律，光换正则引擎整体只能提速约 1.3 倍，**上下文栈、作用域操作、内存分配也要一起优化**。
5. **对用户体感的影响可控**：编辑时只需要重新高亮可视区和受影响的行（约 100 行）。即使只有 syntect 级别的 3 万行/s，满屏重新高亮也只要约 3ms。**吞吐差距主要影响"打开大文件时的全文高亮、索引、minimap"，不影响打字手感。** 所以语法引擎可以分两步走：先保证正确性（100% 通过语法测试），再逐步追平 ST 的吞吐。

### 3.6 P0 两路并行的首轮结果（2026-11-13）

**A 路：macOS 外壳原型**（`prototypes/macos-shell/`，winit 0.30 + wgpu 30 + swash，CoreText 兜底光栅化；GUI 部分由 Claude 在沙箱外实测，外接 4K 60Hz 显示器，系统负载约 6–8）

| 项目 | 结果 | 结论 |
|---|---|---|
| 热启动到首帧 `present()` 返回 | p50 **391ms**：winit 事件循环 118 + 窗口 136 + **wgpu 全部初始化只要约 4ms** + 图集 29 + 首个 drawable 约 100 | ❌ 超出 100ms 目标；瓶颈在 winit/AppKit，不在 wgpu |
| 原生下限（Swift + AppKit + CAMetalLayer，`native-floor/`） | 第一次 drawable 回调约 **150–175ms**。⚠️ **更正（2026-11-14）**：这个程序没有检查 `presentedTime`，后来复查发现紧跟 `orderFront` 提交的第一个 drawable 会被跳过（`presentedTime = 0`），所以这个数字是"回调时刻"，**不是真正上屏的时间**。真正首帧上屏的下限需要在屏幕点亮时重测（复查时屏幕已锁定、显示器在休眠，所有帧都被跳过） | 自研 AppKit 层能省多少，等重测确认；winit 窗口创建多花约 90ms 这一点仍然成立 |
| 滚动单帧，2400×1600 | CPU 编码提交 p50 0.46ms，GPU p50 0.32 / p95 2.1ms | ✅ |
| 滚动单帧，**4K**（4096×2178） | CPU p50 0.44ms，GPU p50 **1.57** / p95 2.1ms | ✅ 和 ST 官方公布的 4K 约 3ms 同一水平 |
| 打字：注入到 `present()` 返回（Fifo） | p50 **1.75ms** / p95 3.2ms（4K：0.87 / 2.9ms） | ✅ 远低于一帧 |
| Mailbox 呈现模式 | macOS Metal 表面不支持 | Immediate 模式出现 28–31 秒的首帧延迟，**正式方案只用 Fifo** |
| IME | winit 在 macOS 上会忽略替换范围（replacement range），`insertText` 的提交时机也受限 | 中文输入法的人工检查清单 `IME_TEST.md` 待你实测 |

**决定**：`vel-platform` **自研 AppKit 层**（objc2，实现 `NSTextInputClient`，参考 Zed 的做法），不用 winit。理由主要是 IME（winit 忽略替换范围）和对窗口生命周期的完全控制，启动收益待重测量化。**渲染继续用 wgpu**（初始化只占约 4ms，4K 单帧 GPU 约 1.6ms）。启动优化方向：在 `orderFront` 之前同步画好第一帧，省掉一轮重绘事件的往返；缓存渲染管线；字体兜底改成懒加载。**所有启动测量都必须以第一个 `presentedTime > 0` 的帧为准，并统计被跳过的帧数**。

**B 路：`vel-text` 正式实现**（`crates/vel-text/`：自研持久化 32 叉 B 树，4KiB 分块，四种度量的汇总，叶内紧凑换行索引，零 `unsafe`）

| 预算项 | 目标 | 实测 | crop / ropey |
|---|---:|---:|---:|
| 1 万光标批量编辑 | ≤ 1ms | **0.49ms** | 0.95 / 1.43ms |
| 10 万光标批量编辑 | ≤ 8ms | **5.06ms** | 9.10 / 13.4ms |
| 10 万个分散编辑 | ≤ 15ms | **8.52ms** | **118.8 / 185.9ms** |
| 随机坐标转换（最慢的一项） | ≤ 600ns | 404ns | byte_line：crop 566 / ropey 688ns |
| 单字符插入 / 删除 | ≤ 2µs | 1.51 / 1.16µs | crop 2.26 / 1.16µs |
| 快照克隆 | ≤ 50ns | 3.8ns | — |
| 100MB Unicode 加载 | ≤ 80ms | 66ms | crop 64 / ropey 109ms |
| 静态内存开销 | ≤ 10% | +7.5% | — |
| **1GB ASCII 加载** | ≤ 300ms | ❌ **654ms**（Claude 在沙箱外复测 735ms） | crop 259 / ropey 735ms |

13 项预算达标 12 项。1GB 加载的瓶颈是纯 CPU（建换行索引 + 汇总），下一轮改成并行建叶节点。

## 4. 分模块可行性评估

难度：1（现成库）～5（大规模自研，行为细节多）

| 子系统 | 难度 | 方案 | 可复用库 | 主要风险 |
|---|:-:|---|---|---|
| 文本存储（rope）+ 撤销 | 2 | B-tree rope，维护字节/字符/UTF-16/行数等摘要；批量编辑一次遍历完成 | **crop**（实测加载比 ropey 快 3 倍，多光标快约 25%），需要补充 char/UTF-16 度量 | API 用 code point 偏移，LSP 用 UTF-16，要同时维护多种度量；10 万光标要用批量拼接 |
| 多光标与 Region 追踪 | 3 | 选区和 `add_regions` 存成有序数组，按编辑做变换；1 万个光标一次事务完成 | 自研 | 各命令在多光标下的边界行为（要逐条对照 ST） |
| **语法引擎（sublime-syntax）** | **5** | 自研编译器 + 解析器：上下文栈、branch/fail 回溯、extends、v1/v2 语义、增量重解析（状态收敛就停）、后台线程优先处理可视区 | syntect 作语义参考；**242 个官方语法测试作验收** | 正确性靠测试兜底；性能靠下一行 |
| **正则引擎（sregex 等价物）** | **5** | Oniguruma 语法解析 → 按特性分类 → 每个上下文合成一个多模式 lazy DFA；前瞻验证；回溯兜底；只给胜出的那条规则求捕获组 | regex-syntax、regex-automata、fancy-regex/onig、memchr/aho-corasick | 语义要和 Oniguruma 逐字节一致（`\b`、Unicode 类、`(?x)` 等） |
| 配色/作用域选择器 | 3 | 作用域原子化 + 选择器评分（照 ST 的计分规则）+ 按作用域栈 ID 缓存样式 | 自研（参考 syntect） | 计分细节、`>` 运算符、颜色函数 |
| GPU 渲染器 | 3 | wgpu（Metal/DX12/Vulkan）实例化批量绘制矩形/字形/九宫格纹理/阴影/波浪线；字形图集 | wgpu、swash/CoreText、harfrust | wgpu 初始化耗时会影响启动（P0 测）；必要时 macOS 直接用 Metal |
| 文字排版 | 3 | 等宽快速路径 + 比例字体通用路径、连字、自动换行、折叠、phantom 行内块；行级布局缓存 | cosmic-text / parley / swash | macOS 上观感要接近 CoreText（必要时直接用 CoreText 光栅化） |
| 控件框架 + 主题引擎 | 4 | 自研轻量保留模式控件树（标签栏、侧边栏、快速面板、输入面板、状态栏、minimap、弹窗……），`.sublime-theme` 规则引擎负责样式和动画 | taffy（可选） | 工作量大，需要逐像素对照 |
| minihtml | 3 | 自研 HTML/CSS 子集解析和块/行内布局（规范是公开、有限的） | 自研 | LSP 插件重度使用，CSS 细节要对 |
| 平台层 | 4 | winit 起步，macOS 深度集成（NSTextInputClient、原生菜单、Dock、窗口标签）走 objc2 | winit、objc2、muda、arboard、rfd、accesskit | **中文 IME**（候选窗位置、预编辑）是 P0 级风险 |
| 查找/替换/在文件中查找 | 2 | 在 rope 上用 `regex-cursor` 搜索；文件搜索用 ripgrep 的组件 | regex-cursor、ignore、grep-searcher | 很少 |
| Goto Anything / 命令面板 | 3 | 模糊匹配 + 复刻 ST 的排序偏好（4213 微调过）；后台线程、取消过期查询、top-K 部分排序、前缀结果复用 | nucleo / nucleo-matcher | 实测：10 万条路径时每键 6–73ms，要优化到 10ms 以内；排序"手感" |
| 索引（定义/引用/补全） | 3 | 后台线程池/子进程用语法引擎提取符号，嵌入式数据库存储，文件监听做增量更新 | ignore、notify、redb | 大仓库的资源控制 |
| Git 集成 + 增量 Diff | 2 | gix 读状态和 HEAD；每次编辑增量更新行级 diff | gix、imara-diff | 少 |
| 构建系统 | 2 | 子进程 + 输出面板 + `file_regex` 结果跳转 + 行内注解 | std::process | 少 |
| 片段/补全/宏/会话/Hot Exit/编码 | 2 | 按文档实现 | encoding_rs、chardetng、quick-xml、plist | 细节多 |
| 设置/键位/菜单/资源包 | 3 | 宽松 JSON（注释、尾逗号、修改时保留格式）；键位上下文求值；zip 包和覆盖机制 | zip、自研 JSONC | 键位 context 语义 |
| Vintage（Vi）模式 | 3 | 在命令层实现（和 ST 一样，本质是一组命令 + 键位上下文） | 自研 | 行为细节 |
| **Python 插件 API** | **5** | 独立 `plugin_host` 进程，PyO3 内嵌 CPython 3.14；自己实现 `sublime` / `sublime_plugin` 模块；同步 RPC（共享内存或 UDS，目标单次往返 10µs 以内）；同步事件超时保护 | pyo3 | 460+ 个方法的行为一致性；Python 运行时体积约 20–30MB |
| 拼写检查 | 1 | Hunspell 兼容 | spellbook + LibreOffice 词典 | 少 |

---

## 5. 架构方案

### 5.1 总体结构

```
┌──────────────────────────── velocity（主进程）────────────────────────────┐
│ 主线程（唯一可写 buffer 的线程，同步模型，吸取 xi 的教训）                    │
│   平台事件 → 键位解析（context 求值）→ 命令分发 → Buffer 编辑事务             │
│   → 标记脏区 → 布局（只算可视区）→ 生成绘制列表 → GPU 提交                   │
│                                                                           │
│ 后台线程（只读 rope 快照，结果用版本号合并回主线程）                         │
│   · 语法解析（可视区优先、增量、状态收敛就停）  · 查找/在文件中查找            │
│   · 索引工作池  · Git 状态/Diff  · 文件监听/加载（大文件分块）  · 拼写检查     │
└───────────────┬───────────────────────────────────────────────────────────┘
                │ 同步 RPC（共享内存环形缓冲 / UDS + postcard）
┌───────────────▼───────────────┐   ┌──────────────────────────────┐
│ plugin_host（CPython 3.14）     │   │ vel CLI（subl 等价，单实例 IPC） │
│ sublime / sublime_plugin 模块  │   └──────────────────────────────┘
└───────────────────────────────┘
```

### 5.2 Cargo workspace 划分

| crate | 职责 |
|---|---|
| `vel-text` | rope、多度量（byte/char/utf16/line）、快照、批量编辑 |
| `vel-buffer` | Buffer/View 模型、选区、Region 集合、撤销/软撤销、变更追踪（`TextChange`/`HistoricPosition`） |
| `vel-regex` | Oniguruma 语法兼容的正则前端 + 多模式 DFA + 回溯兜底（sregex 等价物） |
| `vel-syntax` | sublime-syntax/tmLanguage 加载、继承解析、编译、解析器、增量高亮、作用域与选择器、tmPreferences |
| `vel-theme` | 配色方案（.sublime-color-scheme/.tmTheme）+ UI 主题规则引擎（.sublime-theme） |
| `vel-render` | wgpu 渲染器、字形图集、批处理、纹理九宫格 |
| `vel-layout` | 文本排版（等宽快速路径、换行、折叠、phantom/annotation 行内块） |
| `vel-ui` | 控件树：编辑区、标签、侧边栏、快速面板、面板、状态栏、minimap、弹窗 |
| `vel-minihtml` | HTML/CSS 子集 |
| `vel-platform` | 窗口、IME、菜单、剪贴板、拖放、对话框、DPI（winit + objc2/windows-rs） |
| `vel-config` | 宽松 JSON（保留注释）、设置分层、键位/鼠标映射/菜单/命令面板、资源包加载 |
| `vel-commands` | 内置命令集，命令名和参数与 ST 一致，这样 `.sublime-keymap` 可以原样使用 |
| `vel-search` / `vel-fuzzy` / `vel-index` / `vel-git` / `vel-build` / `vel-spell` / `vel-session` | 各功能模块 |
| `vel-plugin-host` | Python 插件宿主进程 + RPC 协议 |
| `velocity` / `vel-cli` | 主程序、命令行工具 |

### 5.3 关键设计决策

1. **核心同步、后台只读**：只有主线程能修改 buffer。后台任务拿 O(1) 克隆的 rope 快照，结果带版本号，合并时按编辑做位置变换。不用 CRDT，也不让前后端异步分离（xi 的教训）。
2. **命令层与 ST 同名同参**：`move`、`move_to`、`insert`、`left_delete`、`find_under_expand`、`split_selection_into_lines`……这样键位、菜单、宏、命令面板文件能直接复用，Python 插件里的 `view.run_command(...)` 也天然兼容。
3. **语法引擎 = 编译期分析 + 运行期 DFA**：每个"上下文 + prototype + with_prototype 组合"**懒编译**成一个多模式匹配器，相同的规则集合共享编译结果，编译产物缓存到磁盘（原型实测：一次性全量编译 C++ 语法要 3 秒、370MB，不可接受）；常见的有限前瞻编进自动机，而不是事后验证；每行解析状态做 hash-consing（结构共享），增量重解析时状态一致就停；token 存在单独的行对齐结构里（对应 TokenStorage）。
4. **渲染只做简单 2D**：矩形、字形、纹理、阴影、波浪线，全部实例化批处理；只重绘脏区；字形光栅化在 macOS 上可选 CoreText，保证观感一致。
5. **自研控件框架**：`.sublime-theme` 规则引擎要求我们掌控控件树和属性状态（hover/pressed/dirty/selected……），这是不用 GPUI/floem/iced 的根本原因。
6. **大文件模式（超越 ST）**：mmap + 后台分块建 rope、显示进度、可以先浏览；超过阈值自动降级（关闭全文高亮、关闭 minimap、按需高亮可视区）；超长行切片排版。
7. **插件同步 RPC，但要设防**：同步事件（`on_query_context`、`on_text_command`、`on_query_completions`）有超时；`*_async` 事件走队列；插件崩溃后可以重启，编辑器不受影响。

---

## 6. 兼容性与法律边界

**格式 100% 兼容（直接读用户已有的配置和包）**：`.sublime-syntax` `.tmLanguage` `.tmPreferences` `.sublime-color-scheme` `.tmTheme` `.sublime-theme` `.sublime-settings` `.sublime-keymap` `.sublime-mousemap` `.sublime-menu` `.sublime-commands` `.sublime-build` `.sublime-snippet` `.sublime-completions` `.sublime-macro` `.sublime-project` `.sublime-workspace` `.sublime-package`。

**能直接用的**：sublimehq/Packages 的 202 个语法、语法测试、tmPreferences、片段、构建文件（许可证："Permission to copy, use, modify, sell and distribute this software is granted"，个别文件另有声明的除外）。

**不能用、必须自己做的**：
- 名称、Logo、图标："Sublime Text" 是商标，项目叫 **Velocity**
- 默认 UI 主题（Default/Adaptive）、默认配色（Mariana、Monokai、Breakers、Celeste、Sixteen 的 ST 版本文件）、`Default` 包（默认键位、菜单、Python 命令实现）、`sublime_plugin.py`：这些都不在开源仓库里，要**按公开文档和可观察行为自己重写**（键位分配属于功能事实，可以对齐，但文件要自己写）
- **洁净室原则**：只参考官方文档、公开博客、黑盒行为观察，**不反编译、不读 ST 的二进制或内部代码**
- Package Control：packagecontrol.io 是社区服务，接入前要确认使用条款，或者搭建自己的包索引

---

## 7. 性能目标与度量体系

| 指标 | 目标（M1 Pro） | 怎么测 |
|---|---|---|
| 冷启动到首帧 | ≤ 100ms（ST 原版约 50–80ms） | `vel --startup-trace` + 外部计时 |
| 空载内存 | ≤ 80MB（不含 plugin_host） | `footprint` / task_info |
| 按键到屏幕延迟 | 中位数 ≤ 1 帧 + 合成延迟，P95 ≤ 2 帧（在 15 万行和 75 万行 C 文件上） | Keypress 式像素探测（同 Peeknext 方法） |
| 万光标同时输入 | 每次按键 ≤ 8ms | `bench-buffer` + UI 端到端 |
| 打开 100MB / 1GB 文件 | 100MB 文件 ≤ 1s 可交互；1GB 文件不卡死，内存 ≤ 1.3 倍文件大小 | 自动化脚本 |
| 高亮吞吐 | ≥ ST4 同文件实测（P0 测定，ST3 时期约 9.4 万行/s） | `bench-syntax` |
| 编辑后的重新高亮 | 可视区 ≤ 2ms | 增量基准 |
| Goto Anything（10 万文件） | 每次按键 ≤ 10ms | `bench-search` |
| 在文件中查找 | 和 ripgrep 同一量级 | `bench-search` |
| 单帧渲染（4K、满屏代码） | ≤ 3ms（GPU 时间） | wgpu 时间戳查询 |

**CI 性能门禁**：每个 PR 跑一遍基准，退步超过 5% 直接拒绝（数据放在 `perf/` 目录下长期跟踪）。

---

## 8. 路线图

人力假设：**你（产品负责人/最终决策）+ Claude（架构、规格、评审、集成）+ 2 路并行的 codex 子代理（gpt-6.1-sol · max）**。范围：**只做 macOS，暂不做 Python 插件**（见 §11）。

| 阶段 | 时长 | 交付物 | 验收标准 |
|---|---|---|---|
| **P0 验证** | 1–2 周 | ✅ 基准测试（`spikes/`）✅ sregex 原型（`prototypes/sregex/`）✅ 开源仓库骨架（workspace、双许可证、CI）✅ **A 路** macOS 外壳原型：渲染和打字延迟达标，启动超标，已决定改为自研 AppKit 层 ✅ **B 路** `vel-text`：13 项预算达标 12 项（见 §3.6）；待做：中文 IME 人工测试（`IME_TEST.md`）；可选：本机装 ST4 测基线 | 首帧和 IME 达标或有明确的优化路径 → 进入 P1。**结论：可以进入 P1**（启动的优化路径已经由原生下限实测验证） |
| **P1 能打字** | 4–6 周 | 窗口、渲染器、文字排版、rope + 撤销、多光标、基础命令集、打开/保存/编码、**语法引擎 v1：先保证正确性（官方语法测试通过率 ≥ 90%），懒编译 + 磁盘缓存**、配色方案、标签和分屏、Goto Anything（文件）、命令面板、查找替换 | 能用它来开发它自己（dogfooding）；按键延迟和启动时间达到 §7 目标；可视区重新高亮 ≤ 2ms |
| **P2 Sublime 手感** | 6–8 周 | 完整命令集 + 默认键位对齐、**语法测试 100% 通过**、**sregex v2 吞吐优化**（前瞻融合、优先级候选集、预过滤；目标 ≥ ST4 同文件实测吞吐的 0.8 倍）、侧边栏和项目、minimap、在文件中查找、构建系统、片段/补全、宏、折叠、会话和 Hot Exit、大文件模式、Vintage | 和 ST4 差分测试的黄金用例 ≥ 95% 一致 |
| **P3 UI 与智能** | 6–8 周 | `.sublime-theme` 引擎 + 自研默认主题/配色、minihtml（弹窗/phantom/注解）、索引（Goto Symbol/Definition/Reference）、Git 集成、增量 Diff、拼写检查、无干扰模式 | 能加载 3 个流行的第三方主题（如 Ayu、Material）并正确显示 |
| **P4 macOS 打磨与发布** | 4–6 周 | 原生手感细节（窗口标签、全屏、Dock、Services、拖放、触控板手势）、无障碍（VoiceOver）、自动更新、崩溃上报、签名公证、Homebrew Cask、性能加固 | §7 的全部性能目标在 macOS 上达标；发布 1.0 |
| *推后（未排期）* | — | Python 插件 API（plugin_host，约 8–12 周）、Windows/Linux 移植（约 6–8 周） | — |

**里程碑节奏**（2 路并行，只做 macOS，不含 Python 插件）：P1 结束（约第 8～10 周）就能日常用来写代码；P3 结束（约第 5～6 个月）功能覆盖约 85%；P4 结束（**约 6～8 个月**）发布 macOS 1.0。

**为以后补 Python 插件留好接口**：命令层（与 ST 同名同参）、事件钩子（`on_modified`/`on_query_context` 等）、Region/Settings 模型从第一天起按 ST 的 API 语义设计，这样以后的 plugin_host 只是一个 RPC 适配层，不需要重构核心。

**没有插件的代价**：用户会失去 LSP、SublimeLinter、GitGutter 等热门插件（GitGutter 的功能 ST 已内置）。其中 **LSP 影响最大**，可以考虑在 P3 之后把 LSP 客户端做成内置功能，作为唯一的范围例外，需要你届时决定。

---

## 9. 人机协作工作流（Claude + codex 子代理）

1. **规格先行**：每个 crate 开工前，Claude 写一份 `specs/<crate>.md`，包含接口、行为描述、验收测试清单、性能预算。
2. **测试先行，测试当裁判**：
   - 语法引擎：242 个官方语法测试文件（约 56 万个断言列）是现成的、可自动判定的标准答案，非常适合交给 AI 去冲
   - 编辑行为：在本机 ST4 上用它自己的 Python API 写一个录制脚本，批量执行"输入文本 + 命令序列"并记录结果，生成**黄金用例库**做差分测试（黑盒观察，不违反洁净室原则；需要你提供 ST4 许可证）
   - 不变量：rope、撤销、Region 变换用 `cargo-fuzz` 和 proptest 做模糊测试
   - 性能：基准门禁（见 §7）
3. **并行实现**：固定 **2 路** codex 并行（`codex exec -m gpt-6.1-sol -c model_reasoning_effort="max" -s workspace-write -C <dir>`）。每路只改自己负责的目录（有了首个提交后改用独立的 git worktree）；路线分工上，一路负责"平台/渲染/UI"，另一路负责"核心引擎"（text → regex → syntax → ……），两条线天然不互相依赖。
4. **评审合入**：Claude 做代码评审（正确性、架构一致性、性能），测试和基准都通过后再合入；接口变更由 Claude 统一协调。
5. **你只在关键节点决策**：每个阶段结束看演示 + 数据报告，决定继续、调整还是砍掉。

---

## 10. 风险登记

| # | 风险 | 可能性 | 影响 | 应对 |
|---|---|:-:|:-:|---|
| R1 | 自研语法引擎的吞吐追不上 ST | 中 | 中 | 实测 syntect 只能加载 78/202 个语法、速度约为 ST3 的 1/3，所以自研是必然。sregex 原型已证明可以逐字节一致、整体快 1.36–1.92 倍，但最热的上下文会变慢。应对：先做正确性，再按"上下文 profile → 定向优化"迭代（P2）；因为只重新高亮可视区，吞吐不足只影响大文件全文高亮和索引，不影响打字手感；最坏情况是大文件降级（只高亮可视区） |
| R9 | 大项目（10 万个文件以上）Goto Anything 每键超过 10ms | 中 | 中 | 实测每键 6–73ms → 后台线程、取消过期查询、top-K 部分排序、增量复用前缀结果 |
| R2 | 插件 API 行为细节太多 | 高 | 中 | **已决定推后**，不在 1.0 范围内；核心按 ST 的 API 语义设计，以后补做时只需写适配层 |
| R3 | macOS 中文输入法、原生手感 | 中 | 高 | P0 就验证 IME；不行就在 macOS 上换成自研 AppKit 平台层（参考 Zed 的做法） |
| R4 | 字体渲染观感和 ST 不一样 | 中 | 中 | macOS 用 CoreText 光栅化；伽马和亮度对齐要逐像素对照 |
| R5 | wgpu 初始化拖慢启动 | 中 | 中 | P0 测量；首帧可以先走 CPU 预渲染，或在 macOS 上直接用 Metal |
| R6 | AI 生成的代码架构漂移、质量不稳 | 中 | 高 | 规格 + 测试裁判 + 人工评审门禁；crate 边界清晰；Claude 统一维护架构文档 |
| R7 | 法律（商标、主题、默认包） | 低 | 高 | 洁净室；自研名称、主题、配色、默认包；只用 sublimehq/Packages 这类许可证明确的资源 |
| R8 | 范围膨胀 | 高 | 中 | 严格按阶段验收；"和 ST 一致"之外的新功能一律推迟 |

---

## 11. 决策记录

| # | 事项 | 决定（2026-11-13） |
|---|---|---|
| 1 | 平台 | **先只做 macOS**；Windows/Linux 推后 |
| 2 | Python 插件 | **暂不做**；1.0 只支持原生格式的包（语法、配色、主题、键位、菜单、片段、补全、构建、宏、项目） |
| 3 | 开源 | **开源**，许可证为 **MIT OR Apache-2.0** 双许可（Rust 社区惯例，和所有依赖兼容）。远程仓库 `github.com/mini-zoro/velocity` 已是公开仓库，首次推送前需要你确认 |
| 4 | 并行度 | **codex 2 路并行** |
| 5 | 本机装 ST4 作对照基准 | **待定**（不阻塞）。没装之前，性能对照用 §1.3 的公开数据，行为对照以官方文档为准 |

---

## 参考资料

- Sublime Text 4 发布说明：https://www.sublimetext.com/blog/articles/sublime-text-4
- Build 4213 更新日志：https://www.sublimetext.com/blog/
- GPU 渲染博客：https://www.sublimetext.com/blog/articles/hardware-accelerated-rendering
- 官方文档（语法、API、主题、索引、Git、增量 Diff 等）：https://www.sublimetext.com/docs/
- Tristan Hume《Disassembling Sublime Text》：https://thume.ca/2016/12/03/disassembling-sublime-text/
- syntect：https://github.com/trishume/syntect ；branch_point issue #271；PR #705
- xi-editor 复盘：https://raphlinus.github.io/xi/2020/06/27/xi-retrospective.html
- 编辑器延迟/大文件基准（Peeknext，竞品发布）：https://koieditor.com/benchmarks/
- Sublime 论坛：Zed vs ST4 讨论、超大文件性能讨论
- GPUI 现状：crates.io gpui / gpui-ce，github.com/iamnbutler/gpui-unofficial
- Lapce 现状：github.com/lapce
- PyO3 更新日志：https://pyo3.rs/main/changelog
