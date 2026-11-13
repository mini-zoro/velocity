# macOS shell 原型：首轮评审实测与选型建议

已核对评审批次 **`20261113T150046.972369Z-74020`**；自动套件退出 0，`suite_complete.complete=true`，`failures=[]`。
原始数据来自本目录 `output/`，合并记录见 [stdout](output/claude-suite-stdout.jsonl) 和
[stderr](output/claude-suite-stderr.log)。本次修改没有在沙箱中重新运行 GUI。

**结论：优先验证自建 AppKit 薄层，保留 wgpu；暂用 swash 加 CoreText 回退。**

> **更正（2026-11-14，评审者）**：`native-floor/floor.swift` 记录的 "presented(scanout)" 是 presented 回调到达的时刻，
> 没有检查 `MTLDrawable.presentedTime`。复查发现紧跟 `orderFront` 提交的第一个 drawable 会被跳过（`presentedTime = 0`），
> 因此下文的原生下限 150–175 ms 不是真正上屏时间。复查时屏幕已锁定、显示器休眠，所有帧都被跳过，
> 真正的首帧上屏下限需要在屏幕点亮时重测（`crates/velocity/scripts/native-floor.swift` 已正确区分）。
暖启动仍明显超过 100 ms；当前实际约 4K、60 Hz 负载的 Fifo GPU render pass 达到 3 ms 目标。
IME 人工测试、字体视觉比较、120 Hz，以及新增加的 `phys_footprint` 都仍待评审者运行。

## 环境与复核方法

| 项目 | 本批记录 |
| --- | --- |
| 机器 | MacBook Pro，Apple M1 Pro，16 GB（型号/容量由评审者提供；CPU 已与日志核对） |
| 系统 / 编译器 | macOS 26.1（25B78）；rustc 1.99.1；arm64 |
| 电源 / 并发负载 | AC；评审者报告 loadavg 约 5.7–8，其他 agent 同时编译；绝对启动时间按偏悲观、受干扰样本看待 |
| 显示器 | 外接 4096×2304，外观 2048×1152，scale=2，日志刷新率 60000 mHz；未测 ProMotion/120 Hz |
| 默认画布 | 1200×800 逻辑点 = 2400×1600 物理像素 |
| 大画布 | 请求 3000×1800，系统限制为 2048×1089 逻辑点 = **4096×2178** 物理像素，接近 4K，非 6000×3600 |
| 输入 | `rust-10mb.rs`；默认前 5,000 行、151,781 字节；整份文件 10,000,000 字节、297,030 编辑器行 |
| 输入种子 / 日志 | seed=8531344294788297849；自动输入，`log_input=false`，外部输入干扰计数为 0 |
| present 模式 | surface 可用 Fifo / Immediate；Mailbox 不支持，未回退为 Fifo |

从 10 份 wgpu 暖启动、10 份 raw-Metal 暖启动原始文件重算每阶段统计，并逐项比对两份启动汇总；
从每份滚动的 600 帧、每份输入的 500 条样本重算统计，按 1e-9 ms 浮点允差核对汇总。
同时核对合并 stdout 中的原始记录、完成标记、窗口尺寸、输入干扰与遍历覆盖。
表中分位数均按 `(N−1)×p` 线性插值。每个滚动/输入配置只有一次运行；不要将同一运行内的帧分位数当成跨运行置信界。

环境来源：[environment](output/environment-20261113T150046.972369Z-74020.jsonl)；套件状态：[suite](output/suite-20261113T150046.972369Z-74020.jsonl)。

## 依赖版本

本轮没有升级依赖；实现与测量版本锁定于 `Cargo.lock`。

```text
winit                  0.30.13
wgpu                   30.0.2
swash                  0.2.10
objc2                  0.6.5
objc2-app-kit          0.3.2
objc2-foundation       0.3.2
objc2-core-foundation  0.3.2
objc2-core-text        0.3.2
objc2-core-graphics    0.3.2
objc2-metal            0.3.2
objc2-quartz-core      0.3.2
bytemuck               1.25.2
libc                   0.2.191
memmap2                0.9.12
pollster               1.0.1
serde                  1.0.229
serde_json             1.0.154
unicode-segmentation   1.13.3
unicode-width          0.2.3
naga (仅测试)          30.0.2
```

## 启动：wgpu 文字首屏

单位 ms。`main` 是 Rust 主函数入口；进程起点来自 `proc_pidinfo(PROC_PIDTBSDINFO)`，由主函数墙钟采样锚定单调时钟。
本批 `process_birth_error=null`。冷样本是新链接后的首次启动，未清空系统缓存；暖样本 N=10。
终点是 **`Queue::present()` 返回**，不包含合成、扫描输出或发光时刻。

| 阶段 | cold-ish main 累计 | cold-ish 进程累计 | 暖 main P50 / P95 | 暖进程 P50 / P95 | 阶段成本 P50 / P95 |
| --- | --- | --- | --- | --- | --- |
| main_entered | 0.000 | 1720.510 | 0.000 / 0.000 | 6.537 / 36.241 | 0.000 / 0.000 |
| measurement_ready | 0.450 | 1720.960 | 0.228 / 0.283 | 6.800 / 36.513 | 0.228 / 0.283 |
| event_loop_ready | 273.217 | 1993.727 | 117.825 / 145.102 | 123.921 / 152.716 | 117.584 / 144.912 |
| window_created | 1382.003 | 3102.513 | 253.620 / 296.365 | 259.670 / 304.417 | 144.181 / 154.330 |
| wgpu_instance | 1382.121 | 3102.631 | 253.640 / 296.384 | 259.689 / 304.437 | 0.018 / 0.019 |
| surface_created | 1384.806 | 3105.316 | 254.643 / 297.298 | 260.693 / 305.351 | 0.921 / 1.031 |
| adapter | 1386.412 | 3106.922 | 254.777 / 297.422 | 260.827 / 305.475 | 0.128 / 0.138 |
| device_queue | 2872.715 | 4593.225 | 257.681 / 300.018 | 263.730 / 308.071 | 2.716 / 3.086 |
| surface_configured | 2874.287 | 4594.797 | 257.774 / 300.094 | 263.823 / 308.147 | 0.093 / 0.144 |
| renderer_ready | 3073.596 | 4794.106 | 258.617 / 300.864 | 264.666 / 308.917 | 0.782 / 1.006 |
| file_loaded | 3075.514 | 4796.024 | 259.258 / 301.293 | 265.307 / 309.346 | 0.465 / 1.931 |
| font_loaded | 3076.591 | 4797.101 | 259.352 / 301.367 | 265.402 / 309.420 | 0.090 / 0.097 |
| atlas_warmed | 3155.237 | 4875.747 | 288.613 / 320.392 | 294.662 / 328.445 | 24.377 / 36.387 |
| first_frame_encoded | 3459.572 | 5180.082 | 390.358 / 410.447 | 396.394 / 418.061 | 86.789 / 112.815 |
| first_present_returned | 3461.328 | 5181.838 | 390.762 / 410.742 | 396.799 / 418.356 | 0.354 / 0.854 |

暖启动总计 **390.762 / 410.742 ms（main P50 / P95）**；进程口径 **396.799 / 418.356 ms**，本批没有达到 ≤100 ms。
冷样本分别为 **3461.328 / 5181.838 ms（main / 进程）**，其中 pre-main=1720.510 ms；
`device_queue` 单阶段 1486.303 ms，`renderer_ready` 单阶段 199.309 ms。
这些阶段包含驱动初始化、编译/缓存与调度等待，日志不能将冷样本全部归因于 shader cache。

将每次运行内的 `window_created→surface_configured` 先求差再统计，wgpu 初始化仅 **3.954 / 4.313 ms**；
包含 pipeline/atlas 对象建立的 `window_created→renderer_ready` 为 **4.758 / 5.142 ms**。
`metal-decision` 的 2.862 ms 是 instance、adapter、device 三项成本中位数之和，未包括 surface，不能代替这个约 4 ms 的范围。

较大的暖阶段是事件循环 117.584 ms、窗口创建 144.181 ms、首屏布局/图集准备 24.377 ms、
图集预热后至编码结束 86.789 ms（均为各阶段成本 P50）。这些中位数不能直接相加或用累计中位数相减。
`font_loaded→atlas_warmed` 还包含首次 `RedrawRequested` 派发等待，不能视为纯 CPU 光栅化时间。
最后一段在 `RedrawRequested` 处理函数内，包含获取 surface texture 和编码；原型没有更细的首次 acquire/事件派发标记，
不能把整段都说成“等待首个 RedrawRequested”。

来源：[startup-summary](output/startup-summary-20261113T150046.972369Z-74020.jsonl)；[startup-coldish](output/startup-coldish-20261113T150046.972369Z-74020.jsonl)；
`startup-warm-01` 至 `startup-warm-10`；[metal-decision](output/metal-decision-20261113T150046.972369Z-74020.jsonl)。

## winit + raw Metal 与原生 AppKit 下界

以下 raw-Metal 仍使用 **winit 窗口和事件循环**，只清屏，不加载文字/字体。暖样本 N=10，单位 ms。
它的 `first_present_returned` 是安排 drawable present 后 `commandBuffer.commit()` 返回，仍不是显示完成。

| 阶段 | 暖 main P50 / P95 | 暖进程 P50 / P95 | 阶段成本 P50 / P95 |
| --- | --- | --- | --- |
| main_entered | 0.000 / 0.000 | 7.232 / 18.201 | 0.000 / 0.000 |
| measurement_ready | 0.199 / 0.511 | 7.438 / 18.687 | 0.199 / 0.511 |
| event_loop_ready | 104.084 / 155.958 | 112.364 / 173.823 | 103.881 / 155.448 |
| window_created | 255.772 / 346.422 | 263.429 / 364.287 | 149.213 / 190.463 |
| metal_device | 256.670 / 346.576 | 264.327 / 364.441 | 0.098 / 1.026 |
| metal_queue | 257.400 / 347.679 | 265.057 / 365.544 | 0.729 / 1.182 |
| metal_layer_configured | 258.194 / 348.673 | 265.851 / 366.538 | 0.828 / 1.089 |
| metal_drawable_acquired | 376.457 / 472.370 | 384.000 / 490.235 | 114.889 / 139.698 |
| first_frame_encoded | 377.612 / 473.621 | 385.156 / 491.486 | 1.220 / 3.469 |
| metal_present_scheduled | 377.622 / 473.631 | 385.166 / 491.496 | 0.010 / 0.013 |
| first_present_returned | 377.700 / 473.715 | 385.243 / 491.580 | 0.076 / 0.121 |

| 路径 | 起点 / 工作负载 | 暖结果 | 终点与证据 |
| --- | --- | --- | --- |
| winit + wgpu | Rust main，文字首屏 | 390.762 / 410.742 ms，P50 / P95 | present 返回；本批 10 份原始记录 |
| winit + raw Metal | Rust main，仅 clear | 377.700 / 473.715 ms，P50 / P95 | commit 返回；[metal-summary](output/metal-summary-20261113T150046.972369Z-74020.jsonl) |
| Swift AppKit + CAMetalLayer | 首条 Swift 语句，仅 clear | 约 150–175 ms，评审者报告范围 | `addPresentedHandler` 后主队列回调；源码在 [floor.swift](native-floor/floor.swift)，逐次原始输出未随本批保存 |

原生 floor 的阶段范围由评审者补充：NSApplication ready 约 55–65 ms，didFinishLaunching 约 75–100 ms，
窗口/layer 再约 45 ms，orderFront 再约 10 ms，nextDrawable 约 1 ms，显示回调再约 15 ms。
这些是阶段/增量的近似范围，不能拼成精确分位数；没有原始逐次输出，因此本轮不编造其 N、P50/P95 或进程口径。
评审者另报告首次 LaunchServices 注册后，裸二进制与 `.app` 没有明显差别；这仍是补充观察。

Swift 的回调对应显示呈现，口径强于 Rust 的 present-return；但代码在 `DispatchQueue.main.async` 中打点，
没有读取 `drawable.presentedTime`，因此还含回调派发延迟，不能当作高精度像素发光测量。
API 定义见 [Apple MTLDrawable](https://developer.apple.com/documentation/metal/mtldrawable)。

两个 clear-only 路径的表观差约 200 ms，支持优先验证 AppKit 自有薄层；它不是同负载、同时间窗口的配对因果实验。
raw-Metal 的 `metal_layer_configured→metal_drawable_acquired` 成本 P50=114.889 ms，包含首次 redraw 等待与 nextDrawable，
目前不能拆开二者。应将“去掉约 90 ms 窗口开销与约 100 ms 首帧往返”作为待验证假设，而非已证明的逐项节省。
即使完全达到当前原生 floor 的 150–175 ms，仍超过 100 ms。

## 连续滚动

每配置 N=600 个成功采样帧，首帧不计入；步进 20 逻辑点，边界 clamp=0。
TIMESTAMP_QUERY 可用，GPU 查询在采样完成后统一读取。
CPU event→submit 是墙钟范围，含布局、上传和 acquire 等待；`encode_submit` 仅是 acquire 之后的编码/submit。
GPU timestamp 覆盖 clear + 矩形 + 字形 render pass，**不含上传、readback、合成和 scanout**。

| 配置 / 实际物理像素 | 指标 | N | P50 ms | P95 ms | P99 ms | max ms |
| --- | --- | --- | --- | --- | --- | --- |
| Fifo / 2400×1600 | CPU encode+submit | 600 | 0.463 | 0.687 | 0.840 | 1.045 |
| Fifo / 2400×1600 | GPU render pass | 600 | 0.323 | 2.082 | 2.216 | 2.656 |
| Fifo / 2400×1600 | acquire 等待 | 600 | 15.075 | 18.457 | 31.205 | 45.063 |
| Fifo / 2400×1600 | event→submit | 600 | 16.430 | 19.650 | 32.631 | 46.366 |
| Fifo / 2400×1600 | event→present 返回 | 600 | 16.468 | 19.689 | 32.650 | 46.395 |
| Fifo / 4096×2178 | CPU encode+submit | 600 | 0.436 | 0.846 | 1.089 | 5.450 |
| Fifo / 4096×2178 | GPU render pass | 600 | 1.566 | 2.109 | 2.135 | 2.333 |
| Fifo / 4096×2178 | acquire 等待 | 600 | 14.872 | 17.115 | 21.713 | 59.223 |
| Fifo / 4096×2178 | event→submit | 600 | 16.403 | 18.696 | 23.619 | 60.620 |
| Fifo / 4096×2178 | event→present 返回 | 600 | 16.446 | 18.749 | 23.648 | 60.647 |
| Immediate / 2400×1600 | CPU encode+submit | 600 | 0.194 | 0.551 | 1.052 | 2.184 |
| Immediate / 2400×1600 | GPU render pass | 600 | 0.324 | 0.335 | 1.284 | 3.482 |
| Immediate / 2400×1600 | acquire 等待 | 600 | 0.361 | 0.834 | 1.518 | 6.117 |
| Immediate / 2400×1600 | event→submit | 600 | 0.899 | 1.717 | 2.592 | 6.592 |
| Immediate / 2400×1600 | event→present 返回 | 600 | 0.927 | 1.785 | 2.669 | 6.617 |
| Immediate / 4096×2178 | 未运行 | — | — | — | — | — |

Fifo 的约 16.4 ms event→present 主要来自 acquire 的约 14.9–15.1 ms 等待，符合本批 60 Hz 连续滚动的节奏；
它不是 CPU 编码花了整整一帧。4K Fifo GPU max=2.333 ms、P95=2.109 ms，本次 600 帧没有超过 3 ms；
默认 Fifo 也没有超过。Immediate 的 GPU 有 2/600 帧超过 3 ms。

大画布的中位字形实例数仅 **1553**（默认约 1118.5），是此 Rust 片段的真实行长/密度；
这些数据不等同于 24,000 字符满列压力测试，也没有验证 120 Hz 端到端帧节奏。

来源：[scroll-default](output/scroll-default-20261113T150046.972369Z-74020.jsonl)、[scroll-large](output/scroll-large-20261113T150046.972369Z-74020.jsonl)、[scroll-immediate](output/scroll-immediate-20261113T150046.972369Z-74020.jsonl)。

## 自动输入与 Immediate 异常

每配置 500 次插入，同一 seed、30–90 ms 定时器；没有真实 HID/IME 路径，也没有外部按键干扰。
终点均为 submit 或 present-return；`timer_lateness` 是预定注入时刻到事件处理入口，另列，避免把调度延迟藏进“很低的输入延迟”。

| 配置 / 实际物理像素 | 指标 | N | P50 ms | P95 ms | P99 ms | max ms |
| --- | --- | --- | --- | --- | --- | --- |
| Fifo / 2400×1600 | inject→submit | 500 | 1.713 | 3.120 | 3.593 | 6.013 |
| Fifo / 2400×1600 | inject→present 返回 | 500 | 1.748 | 3.188 | 3.711 | 6.045 |
| Fifo / 2400×1600 | 计时器迟到 | 500 | 1.753 | 2.106 | 2.206 | 5.698 |
| Fifo / 4096×2178 | inject→submit | 500 | 0.852 | 2.883 | 3.587 | 4.068 |
| Fifo / 4096×2178 | inject→present 返回 | 500 | 0.871 | 2.935 | 3.710 | 4.152 |
| Fifo / 4096×2178 | 计时器迟到 | 500 | 1.517 | 2.091 | 2.235 | 5.013 |
| Immediate / 2400×1600 | inject→submit | 500 | 3.127 | 12.911 | 28.864 | 101.141 |
| Immediate / 2400×1600 | inject→present 返回 | 500 | 3.223 | 13.607 | 30.461 | 101.383 |
| Immediate / 2400×1600 | 计时器迟到 | 500 | 2.048 | 6.931 | 13.810 | 59.481 |
| Immediate / 4096×2178 | 未运行 | — | — | — | — | — |

Immediate 没有带来更好的稀疏输入尾延迟。最差输入 id=146、frame=147：inject→submit=101.141 ms，
inject→present=101.3835 ms，该次 submit 后到 present-return 仅约 0.243 ms，主要延迟在提交之前。
49/500 次超过 10 ms，23/500 次超过 16 ms；有两次插入共用同一帧，Fifo 两组均无合帧。
但这不是所有尾延迟都来自 submit 之前的证明：Immediate 的 submit 后差值 max 还有 16.428 ms。

| 配置 | 第一帧 present-return（main ms） | 全程 surface 重试 | 采样后结论 |
| --- | --- | --- | --- |
| 默认 Fifo 滚动 | 472.930 | 1 | 无严重首次 acquire 停滞 |
| 4K Fifo 滚动 | 456.288 | 1 | 无严重首次 acquire 停滞 |
| Immediate 滚动 | 31543.145 | 1710 | 首帧本身已停滞约 31.5 s |
| 默认 Fifo 输入 | 839.969 | 1 | 输入采样在首帧之后开始 |
| 4K Fifo 输入 | 755.815 | 1 | 输入采样在首帧之后开始 |
| Immediate 输入 | 28455.419 | 1529 | 首帧本身已停滞约 28.5 s |

代码在 acquire 失败后固定等待 16 ms，并在 acquire **之前**重建 scene、调用 `queue.write_buffer`；
这是 Immediate 异常和暂存分配值得检查的明确路径。重试计数跨首帧、采样和遍历，日志没有按阶段/原因拆分，
不能说全部 1529 次都发生在输入期间，更不能仅凭它证明 101 ms 尾延迟的原因。
结合并发编译负载，本轮保留 Fifo 为默认，Immediate 仅保留实验地位。
下一次应记录 Timeout/Occluded/Outdated/Lost 的分项、首帧与采样阶段计数、逐次 acquire/prepare/submit 时间，
并在相同低负载条件重复；当前没有证据支持把尾延迟简单归因于 GPU 绘制慢。

来源：[typing-default](output/typing-default-20261113T150046.972369Z-74020.jsonl)、[typing-large](output/typing-large-20261113T150046.972369Z-74020.jsonl)、[typing-immediate](output/typing-immediate-20261113T150046.972369Z-74020.jsonl)；首帧/重试来自各文件的 startup 与 complete。

## 内存：本批只有 resident_size，phys_footprint 待重跑

下表统一为 **MiB = 2²⁰ 字节**，避免把评审摘要中的十进制 MB 与 MiB 混用。
旧批次的 `rss_bytes` 是 `task_info(MACH_TASK_BASIC_INFO).resident_size`，包含共享/文件映射页；
不能直接与 Activity Monitor 或公开编辑器 footprint 数值比较，也不能据此判定 80 MB 私有内存目标是否达成。

本轮新增每条 memory 记录的 `phys_footprint_bytes`、独立错误和方法字段，使用
`task_info(TASK_VM_INFO, REV1).phys_footprint`；旧 `rss_bytes` 与方法字段保持不变。
ABI 与本机 SDK 的 revision-1 前缀核对：flavor=22、count=38 个 32 位字、footprint offset=144、前缀 152 字节。
内核错误或不足 revision-1 的返回必须记为 null+错误，禁止伪造为 0。
接口布局参考 [Apple XNU task_info.h](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/task_info.h)。
以下历史数据均不含这个字段，**无法事后补算**；必须重新运行硬件测量。

| 配置 | 加载内容 | 首帧 RSS MiB | 结束 RSS MiB | 全加载内容已遍历 | phys_footprint 首帧 / 结束 |
| --- | --- | --- | --- | --- | --- |
| raw Metal clear，暖启动中位数 | 不加载文件 | 87.312 | — | — | 待重跑 |
| wgpu 文字，暖启动中位数 | 151,781 B / 5,000 行 | 94.055 | — | — | 待重跑 |
| Fifo 滚动 / 默认 | 151,781 B / 5,000 行 | 92.891 | 83.172 | 是 | 待重跑 |
| Fifo 滚动 / 4K | 151,781 B / 5,000 行 | 93.953 | 85.172 | 是 | 待重跑 |
| Immediate 滚动 / 默认 | 151,781 B / 5,000 行 | 108.406 | 266.938 | 是 | 待重跑 |
| Fifo 输入 / 默认 | 151,781 B / 5,000 行 | 93.250 | 80.594 | 不适用（输入后） | 待重跑 |
| Fifo 输入 / 4K | 151,781 B / 5,000 行 | 94.078 | 80.719 | 不适用（输入后） | 待重跑 |
| Immediate 输入 / 默认 | 151,781 B / 5,000 行 | 89.047 | 76.766 | 不适用（输入后） | 待重跑 |
| Fifo 整份文件遍历 / 默认 | 10,000,000 B / 297,030 行 | 113.391 | 161.312 | 是 | 待重跑 |

**纠正归属：279,904,256 B（279.904 MB / 266.938 MiB）是默认尺寸 Immediate 滚动的结束值，
不是 4K Fifo。4K Fifo 结束只有 89,309,184 B（89.309 MB / 85.172 MiB）。**

所有这些运行的 atlas 都保持 2048² RGBA8、16,777,216 B（16 MiB），instance buffer=1 MiB；
三组短文件滚动最终均为 356 个 glyph cache keys，没有 atlas 扩容。
查询缓冲区合计约 0.156 MiB，也解释不了约 158.5 MiB 的 Immediate 首帧后 RSS 增长。
一个未计对齐的 BGRA drawable 默认约 14.65 MiB、4K 约 34.03 MiB；实际数量/驻留与驱动记账没有记录，
不能据此算出总 RSS。尤其在异常值属于默认尺寸之后，不能把它归咎于“4K atlas/drawables”。

可疑路径是大量 acquire 重试之前的重复上传/暂存分配，以及原生驱动/资源池的保留；
这只是代码和日志支持的排查方向，不是已经取得的内存分类证据。需要 footprint 重跑和 Metal/VM 分类采样才能归因。
Fifo 两组短文件运行的结束 RSS 反而低于首帧，进一步说明进程 RSS 会受共享页、缓存和回收变化影响。

整份 10 MB 文件在 7,617 个遍历帧后从 113.391 增到 161.312 MiB（约 +47.922 MiB）；
仍只使用 16 MiB atlas，glyph keys 从 210 增至 572。
最终触发了 PingFang SC 与 Apple Color Emoji 的只读映射（文件分别约 78.2 MB、188.6 MB），
映射文件大小不能当作实际 RSS 增量；行 token 缓存有 1,024 行上限。
这些记录说明增长不只来自文件字节，但不足以把 +47.922 MiB 精确分摊到 buffer、字体或驱动。

来源：[memory-entire-file](output/memory-entire-file-20261113T150046.972369Z-74020.jsonl) 以及上述六份滚动/输入文件的 memory、final_render_state。

## IME、字体与未覆盖事项

| 项目 | 本轮证据 / 状态 | 结论 |
| --- | --- | --- |
| Pinyin、Shift、preedit 内 Backspace、Esc、候选窗随滚动/缩放 | 人工 checklist 尚未运行 | 待人测，不能标通过 |
| Kana 转换、Ctrl+Cmd+Space、Option+E→E、全角标点 | 人工 checklist 尚未运行 | 待人测，尤其检查原生 insertText / replacementRange |
| CPU 状态/几何、Unicode 编辑、bitmap 方向、原生字体 glyph ID | headless 回归测试覆盖 | 只能验证代码与像素生成，不能代替真实输入法或视觉测试 |
| 默认与 4K 短文件 | 仅 Menlo；警告 0；CoreText raster 回退 0 | 此批没有比较两种 rasterizer 的画质或速度 |
| 整份 Rust 文件 | Menlo + 公共 `PingFangSC-Regular` + emoji；CoreText raster 回退 0 | 与沙箱可解析到 UI PingFang 的条件不同；不能据此删除回退 |
| 泛 Unicode fallback | 整份遍历出现 20 条 missing-glyph 警告，包括数学数字、Cherokee、部分古代字符、韩文与阿拉伯文 | 目前只选择 Menlo/PingFang/emoji 的 fallback 列表不够生产使用；需要更通用字体发现与复杂文字布局 |
| Mailbox | 滚动/输入均 `unsupported`，available `[Fifo, Immediate]` | 没有样本，不得补成零延迟或 Fifo 结果 |
| 4K Immediate / 120 Hz | 本批都未运行 | 需要单独补测 |

字形警告见 [memory-entire-file](output/memory-entire-file-20261113T150046.972369Z-74020.jsonl) 的 `final_render_state.fonts.warnings`。
Mailbox 来源：[scroll-mailbox-runner](output/scroll-mailbox-runner-20261113T150046.972369Z-74020.jsonl)、[typing-mailbox-runner](output/typing-mailbox-runner-20261113T150046.972369Z-74020.jsonl)。

## 选型建议

| 决策 | 建议 | 理由与边界 |
| --- | --- | --- |
| winit / AppKit | **下一阶段优先做自己的 AppKit 薄层与 NSTextInputClient**，沿用当前渲染器 | winit + raw Metal 也约 378 ms，原生 clear-only floor 报告约 150–175 ms，启动瓶颈主要在窗口/生命周期/首次 drawable 路径；winit 还缺 replacementRange，insertText 的 commit 受 marked text 条件限制。约 200 ms 是待验证的潜在收益，不是已隔离测得的纯 winit 开销 |
| wgpu / raw Metal | **保留 wgpu（Metal 后端）**，raw Metal 继续作诊断基线 | 暖初始化含 surface 约 3.954 ms，含 renderer 对象约 4.758 ms；本负载 4K GPU P50=1.566、P95=2.109 ms。大改 Metal 渲染器不能解释/消除数百 ms 的窗口路径成本；冷启动缓存仍需单独调查 |
| swash / CoreText | **暂用 swash 快路径 + 保留原始 CTFont 的 CoreText raster 回退** | ASCII 快路径/atlas 与 CPU 测试有效；本批公共 PingFang 路径可由 swash 光栅化，而沙箱 UI 字体可能需要 CoreText。没有人工视觉对比与同负载 rasterizer 基准，不选出画质/速度赢家 |
| present 策略 | **默认 Fifo，frame latency hint=1** | 连续滚动正常节奏、稀疏输入尾延迟优于 Immediate；Immediate 首帧停滞、输入尾延迟与 RSS 异常未解释；Mailbox 不支持 |
| 内存预算 | **等待 phys_footprint 后再与公开编辑器数字比较** | 当前都是 resident_size；不宣称 98 MB 是 Activity Monitor 内存，也不把 4K 误判为 280 MB |

## 接近 ≤100 ms 的具体启动计划

1. **先建立可比基线。** 停止其他编译后，以同窗口、相同负载、同缓存条件交替跑 winit/AppKit 两条路径，
   同时保留 kernel→main、main→present-return、drawable `presentedTime`；新增首个 redraw 入口、acquire 起止、PSO 建立、
   submit 的分段标记。Swift floor 也应保存逐次输出并区分 presentedTime 与主队列 callback 时间。
2. **去掉首帧的事件循环往返。** 用 Rust + objc2 的最小 NSApplication/NSWindow/NSView/CAMetalLayer 封装，
   layer 就绪后直接编码/提交第一帧，后续再进入按变化重绘。完整实现 NSTextInputClient 的 marked/selected/replacement range、
   insertText 与候选窗坐标，使用现有 IME_TEST 验收；保留 wgpu Surface 与 atlas renderer。
3. **把可提前的首屏工作放到 orderFront 前。** 提前确定物理尺寸、主字体、首屏布局、atlas 和 PSO，
   可先绘制到 offscreen texture；若隐藏 layer 的 drawable 可用则预提交，否则 orderFront 后立即拷贝/提交已准备内容，
   不假定隐藏窗口一定可取得 drawable，也不为了等 RedrawRequested 额外绕一轮。
   在 AppKit 允许的生命周期内尽早做窗口工作；可并行的 CPU 文件/字体准备与 Metal device 初始化移出串行关键链。
4. **分别处理冷、暖缓存。** 保持 shader、pipeline descriptor/key 稳定，利用实际 Metal/驱动已有缓存，
   调查可行的 binary archive/预编译接口及失效规则。当前锁定的 wgpu 30.0.2 将 `Features::PIPELINE_CACHE` 标为 Vulkan 支持、
   Metal 未实现，不能把它直接当作 macOS 的现成修复。冷样本的 device 1.486 s 与 renderer 199 ms 应用驱动/编译跟踪进一步拆分。
5. **保留窄字体快路径。** Menlo 直接读取、首屏所需 glyph 精确预热、避免系统全量字体扫描；
   CJK/emoji 按真正缺字需求逐面懒加载，保持 CPU/GPU cache；改善 scene/上传复用，避免 acquire 失败时继续堆积准备工作。
   全文件读取、高亮缓存、非首屏字体工作不进入首帧关键路径。
6. **以数据确认剩余预算。** 原生 floor 也还需再省约 50–75 ms 才能达到 main→显示 100 ms；
   进程口径还须包含 loader，60 Hz 呈现可再占一个约 16.7 ms 的显示周期。
   `.app` 包装、换 raw Metal 或一次 cache 优化都不等于已达标；需同时给出低负载冷/暖统计、footprint 和真实 IME 结果。

本轮代码仅新增 footprint 观测与校验，没有修改 GPU/重试策略，因此这些优化都是下一阶段计划，尚无优化后的性能承诺。
构建、headless syscall/ABI/JSON 回归与现有 CPU 测试通过；硬件 footprint、人工 IME 与视觉对比按 RUNBOOK 补测。
