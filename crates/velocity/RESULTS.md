# Round 4 results: settings, commands and real editing

Status: headless checks complete; native GUI/Metal measurements remain pending. No GUI,
Metal device, display wake or screen-unlock operation was attempted in the sandbox.
No changes to vel-buffer/vel-text APIs were needed. Defaults were authored from scratch.

Delivered: vel-config, vel-commands, vel-fileio and native integration. Files open/save in
tabs with encoding/BOM/line-ending retention, multi-caret edits/history, native file dialogs,
contextual keymaps and pairing, save prompts, settings reload and external-change handling.
Plain text is used until the syntax lane is integrated.

Headless validation: 61 Rust tests across the six lane crates (commands 5, config 6, fileio 7,
platform 6, render 7, application 30), including Unicode properties and the skipped-first-
drawable state machine. cargo fmt, clippy with -D warnings, rustdoc with -D warnings,
Python parser/dry-run checks, shell syntax and Swift helper compilation pass. Native
notifications did not arrive in the initial sandbox watcher test; the watcher now also
checks watched-file metadata, and atomic-replacement coverage passes with that fallback.

Release CPU evidence (one process per workload, not a GUI budget result):

| Workload | Observed |
| --- | --- |
| 1,000 indexed key bindings, 1,000,000 events | mean 0.1905 µs/lookup |
| 100,000,000-byte generated UTF-8 load | worker queue 0.0668 ms; complete 157.437 ms |
| Existing spikes/data/text-100mb.txt through vel-fileio | worker queue 0.0393 ms; complete 166.238 ms |
| keymap → command → buffer → CPU scene, 1 caret, 50 samples | p50 0.3094 ms; p95 0.6330 ms |
| same path, 1,000 carets, 50 samples | p50 2.2718 ms; p95 4.9673 ms |

Raw summary: [round4-cpu-20261113T182158526934Z.jsonl](output/round4-cpu-20261113T182158526934Z.jsonl).
Application logs: [1 caret](output/headless-1794594120757759-58865.jsonl),
[1,000 carets](output/headless-1794594120787205-58866.jsonl).
These samples use the GUI injector's Unicode/newline/ASCII mixture, including a two-scalar
combining sequence. CPU timing excludes GPU/native input. It differs from the previous
round's four-character smoke workload, so it establishes a new baseline rather than a
like-for-like regression comparison. No GUI latency or startup budget is claimed.

Startup fix: retained offscreen texture, cheap repeated copy/present, explicit skipped
callbacks, first callback receipt and first_real_scanout. Milestones flush as they occur.
CoreGraphics session/display checks run before launch; hosting-window occlusion is checked
while waiting with a one-second orderFront grace. measure.py also preflights before each
launch. The new Swift floor has the same retry/diagnostic behavior. Hardware must confirm
both successful scanout and approximately two-second failure on blocked displays.

Reviewer commands and the 100 MB asynchronous-open measurement are in RUNBOOK.md. Complete
IME_TEST.md's new editing smoke checklist, including encodings, tabs, dirty close prompts,
multicursor undo and external reload. Record actual native key equivalents, shifted keys,
keypad/modifier taps, candidate windows and the requested display configurations.

Known scope limits: Rust regex context syntax (no lookaround/backreferences); simple
plain-text scope selectors; no palette UI/snippet field navigation/wrap_lines. Text macro
state is per view. Native appearance, input geometry and keyboard layouts require GUI QA.

The earlier round's evidence/template follows for historical reference; its demo limitations
and smoke workload no longer describe the current implementation.

---

# Lane A/P1：原生 AppKit / wgpu 评审记录

状态：**硬件待测**。沙箱没有 window server 和 Metal，未运行 GUI，不填写虚构的
启动、打字、滚动、GPU 或 GUI 内存成绩。完整复现命令见 RUNBOOK.md。

本轮交付 vel-platform、vel-render、velocity；仅更新这三个 crate 与 Cargo.lock。
采用 objc2 AppKit + layer-hosting NSView + CAMetalLayer，渲染保留 wgpu 30.0.2 Metal。
首屏先在隐藏阶段完成文字布局/字形上传/离屏 GPU 提交，orderFront 后在同一个
applicationDidFinishLaunching 回调内直接 acquire/copy/present；不等待重绘事件或显示链 tick。
MTLDrawable presented handler 记录真实 presentedTime，并单列回调到达时间。

## 沙箱可验证项

- [x] cargo fmt --all
- [x] cargo clippy -p vel-platform -p vel-render -p velocity --all-targets -- -D warnings
- [x] cargo test -p vel-platform -p vel-render -p velocity
- [x] RUSTDOCFLAGS='-D warnings' cargo doc -p vel-platform -p vel-render -p velocity --no-deps
- [x] Python 测量解析/完整性自测、shell 语法检查
- [x] Swift 原生下限 helper 编译（不启动）
- [x] --headless-smoke：50 次 Unicode 编辑 + 可视区 scene CPU 时间（仅 smoke，无 GUI/GPU）

本轮最终 headless 验证共 **36 项 Rust 单元测试通过**（platform 5、render 7、app 24），
fmt / clippy / rustdoc 均通过。未遇到并行 vel-text/vel-buffer 工作导致的依赖构建失败。

CPU smoke 原始文件：[headless-1794586442049930-19843.jsonl](output/headless-1794586442049930-19843.jsonl)。
release、5000 行、50 次 Unicode 编辑 + 可视区 scene：p50 **0.2125 ms**、p95 **0.3746 ms**，
mean 0.2221 ms、max 0.4486 ms。仅一个 smoke 进程，包含冷的首批 CPU 布局；未建立跨版本回归基线。
`gui_started=false`、`gpu_started=false`；不可与 GUI 输入/滚动/GPU 延迟预算混用。
首屏 GUI footprint 及所有硬件表格仍待填。

## 硬件环境与输入

| 项目 | 实测 |
| --- | --- |
| 时间、macOS、机型、内存 | 待填 |
| 电源、load average、并发任务 | 待填 |
| 显示器、逻辑/物理分辨率、scale、refresh | 待填 |
| 输入法、键盘布局 | 待填 |
| release 构建 / 冷暖条件 / run 目录 | 待填 |
| first-frame surface 重试次数 | 待填 |

## 同一会话下限与启动

| 指标（ms） | N | p50 | p95 | 说明 |
| --- | --- | --- | --- | --- |
| Swift floor main → presentedTime | 待填 | 待填 | 待填 | 新 helper 是真实 scanout，旧 P0 是回调到达 |
| Velocity warm main → scanout | 待填 | 待填 | 待填 | 1200×800 points，5000 行 |
| Velocity warm process → scanout | 待填 | 待填 | 待填 | 包含内核进程出生到 main |
| warm main → present-return | 待填 | 待填 | 待填 | 不等同 scanout |
| scanout → callback 延迟 | 待填 | 待填 | 待填 | 诊断回调迟到 |
| Velocity cold-ish main → scanout | 待填 | 待填 | 待填 | 新构建首次，不清系统/驱动缓存 |

目标：warm ≤ 同会话 floor + 30 ms。差值：待填。原始 JSONL：待填。
阶段：didFinishLaunching、window/layer、主字体、adapter/device、shader、pipeline、scene、
first_offscreen_submitted、orderFront、first_submit、present-return、scanout。逐项填入汇总。

wgpu 30.0.2 的 PipelineCache 当前只实现 Vulkan；Metal adapter 支持位与
实际 device/shader/pipeline 创建时间仍由环境记录确认。未支持时不会写假 binary cache。

## 输入、滚动与内存

| 工作负载 / 指标 | N | p50 | p95 | 目标 / 结果 |
| --- | --- | --- | --- | --- |
| typing 1200×800 receipt → present-return | 500 | 待填 | 待填 | ≤2 / ≤4 ms |
| typing 实际 4K receipt → present-return | 500 | 待填 | 待填 | ≤2 / ≤4 ms |
| 4K scroll CPU event → submit | 600 | 待填 | 待填 | p95 ≤1.5 ms |
| 4K GPU render pass | 600 / 不支持 | 待填 | 待填 | p95 ≤3 ms |
| after_first_frame phys_footprint | 待填 | 待填 | 待填 | ≤80 MB |
| after_first_frame resident_size | 待填 | 待填 | 待填 | 单列 RSS |
| after_full_sweep footprint / RSS | 待填 | 待填 | 待填 | 检查有界缓存 |

实际分辨率、timestamp 支持、surface 重试、外部输入污染、watchdog/错误记录：待填。
不将 GPU pass 时间当端到端延迟；不将 RSS 当 Activity Monitor 内存。

## 原生行为人工验收

IME_TEST.md 完成情况：待填。重点记录 UTF-16 replacementRange、日文再转换、表情选择器、
长按重音、组合选择区、候选窗在 Retina/多显示器的位置，以及 focus/resize/scroll。
字体：Menlo、PingFang、Apple Color Emoji；CoreText fallback 次数与颜色/基线观感：待填。
菜单/剪贴板/撤销、全屏、深浅色切换、idle CPU / 120 Hz：待填。

当前限制：单选区 demo；无保存/项目/完整语法/段落 bidi/自动换行。fallback/shaping 是
代码编辑器网格方案，复杂脚本与 ZWJ/旗帜仍须视觉核对。首屏离屏纹理会短暂增加
GPU 内存；Metal 驱动、AppKit 生命周期和 callback 行为只能在真机验证。

结论与后续修复：待评审数据后填写。
