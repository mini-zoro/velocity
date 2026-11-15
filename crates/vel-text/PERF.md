# vel-text 性能验收记录

2026-11-14；32 路 B-tree、4 KiB 叶、紧凑 LF 索引；载入使用最多四个 scoped workers，每 worker 2 MiB 在途块，总暂存 ≤8 MiB；前 4 MiB 串行，Arc 节点按序在读取线程分配；提供 from_reader_serial 无线程入口。
本次所有预算的中位数均通过。

## 最终预算

时间列为 **median / P95**；本次每组 7 个独立进程。吞吐按十进制 GB/s；MB = 10⁶ 字节，RSS 列取全部观测的最大高水位。预算没有放宽。

| 操作 | 原目标 | 本次实测 | 验收 |
|---|---|---|---|
| from_reader，1 GB ASCII | ≤ 300 ms | 259.451 / 503.523 ms | 通过 |
| 加载峰值 RSS，1 GB | ≤ 1.15 × 文件 | 1,099.63 MB，1.0996 × | 通过 |
| from_reader，100 MB Unicode | ≤ 80 ms | 32.187 / 33.154 ms | 通过 |
| clone | ≤ 50 ns | 3.435 / 4.189 ns | 通过 |
| 随机转换，最慢项 byte_point_utf16 | 每项 ≤ 600 ns | 383.802 / 404.588 ns | 通过 |
| 随机单字符 insert | ≤ 2 µs | 1.248 / 1.586 µs | 通过 |
| 随机单字符 delete | ≤ 2 µs | 1.145 / 1.651 µs | 通过 |
| apply_edits，前 10k 行 | ≤ 1 ms | 0.434 / 0.449 ms | 通过 |
| apply_edits，前 100k 行 | ≤ 8 ms | 4.500 / 6.138 ms | 通过 |
| apply_edits，均匀分布 100k | ≤ 15 ms | 7.438 / 10.822 ms | 通过 |
| chunks 全遍历，1 GB | ≥ 5 GB/s | 417.16 GB/s；2.397 / 2.599 ms | 通过 |
| lines 全遍历，1 GB | ≥ 1 GB/s | 1.34 GB/s；744.078 / 886.692 ms | 通过 |
| 新载入文本存储开销 | ≤ 10% | 分配估计 +7.52%；载入完成 RSS 1.0996 × | 通过 |

载入峰值由 getrusage 独立测量；macOS 常驻量另用 proc_pid_rusage 在载入结束时读取，失败时回退到峰值上界。二者均包含进程基线。另在计时和 RSS 采集后估计可达分配，统计 String/Vec 容量、LF 表、Arc 头和保留的原叶，按地址去重；不含 malloc 记账或 size class 舍入。此项需分配估计与载入结束的 RSS 都在 10% 内才标通过。压缩内存、调度和页故障均受当前系统状态影响。ops 进程的高水位包含之前的操作，不能当作单份新载入文本的内存。

## 每项坐标转换

| 随机操作，1 GB ASCII | median ns/次 | P95 ns/次 |
|---|---:|---:|
| byte_char | 2.76 | 3.29 |
| char_byte | 0.94 | 1.00 |
| byte_utf16 | 2.42 | 2.50 |
| utf16_byte | 1.01 | 1.19 |
| byte_line | 324.79 | 343.02 |
| line_byte | 300.40 | 326.47 |
| byte_point | 349.44 | 369.03 |
| byte_point_utf16 | 383.80 | 404.59 |
| point_byte | 303.06 | 360.14 |
| point_utf16_byte | 332.43 | 362.69 |

ASCII 的 byte/scalar/UTF-16 恒等转换由总 summary 证明，不读正文；此表不声称 Unicode 随机转换也有相同常数。Unicode 的正确性另由 String 模型验证。

## 同一最终 pass 的 crop / ropey 对照

每格时间均为 median / P95。crop 不提供 scalar 或 UTF-16 度量；ropey 的 UTF-16 与 byte 通过 char 座标组合。crop/ropey 批量项为逆序逐点插入。

| 操作 | vel-text | crop 0.4.3 | ropey 1.6.1 |
|---|---:|---:|---:|
| 加载 100 MB ASCII | 38.028 / 47.866 ms | 25.430 / 31.476 ms | 68.815 / 72.988 ms |
| 加载 1 GB ASCII | 259.451 / 503.523 ms | 311.611 / 469.149 ms | 728.723 / 798.172 ms |
| 加载 100 MB Unicode | 32.187 / 33.154 ms | 62.723 / 69.272 ms | 102.303 / 109.133 ms |
| clone | 3.435 / 4.189 ns | 3.513 / 4.692 ns | 2.837 / 3.229 ns |
| byte_line | 324.792 / 343.017 ns | 505.064 / 939.689 ns | 704.861 / 1,067.350 ns |
| line_byte | 300.397 / 326.471 ns | 748.822 / 828.225 ns | 748.604 / 1,018.283 ns |
| insert | 1.248 / 1.586 µs | 2.351 / 2.583 µs | 5.440 / 6.044 µs |
| delete | 1.145 / 1.651 µs | 1.355 / 1.915 µs | 1.528 / 1.948 µs |
| batch_lines_10000 | 0.434 / 0.449 ms | 0.936 / 1.064 ms | 1.357 / 1.458 ms |
| batch_lines_100000 | 4.500 / 6.138 ms | 8.976 / 10.884 ms | 13.793 / 13.999 ms |
| batch_uniform_100000 | 7.438 / 10.822 ms | 382.468 / 581.691 ms | 156.719 / 186.709 ms |
| chunks | 2.397 / 2.599 ms | 3.071 / 4.803 ms | 20.531 / 25.081 ms |
| lines | 744.078 / 886.692 ms | 1,218.852 / 1,486.854 ms | 1,685.826 / 1,757.871 ms |
| 隔离加载 1 GB 的最大 RSS | 1,099.63 MB | 1,043.56 MB | 1,141.56 MB |

## batch 与 vel-text 自身逐点编辑

两者使用相同的原文坐标与快照。逐点路径逆序执行，时间不含坐标构造或析构。

| 场景 | apply_edits median / P95 | 逐点 median / P95 | median 加速比 |
|---|---:|---:|---:|
| 前 10k 行 | 0.434 / 0.449 ms | 9.593 / 10.056 ms | 22.10 × |
| 前 100k 行 | 4.500 / 6.138 ms | 93.395 / 103.792 ms | 20.75 × |
| 均匀 100k | 7.438 / 10.822 ms | 20.133 / 21.103 ms | 2.71 × |

## 方法与机器状态

仓库登记主机为 Apple M1 Pro、8 核、16 GB。本轮确认 arm64、macOS 26.1 (25B78)，rustc 1.99.1 (58cf8f9c3)，Cargo 1.99.1；使用 rust-toolchain.toml 固定的 1.99。release 默认优化，保留 line tables，未改 allocator、LTO 或 target-cpu。

每个观测前执行 `sysctl -n vm.loadavg`，共 378 次；378 次被沙箱拒绝，原始输出逐条保存在 JSONL 的 loadavg 字段。实际负载不可读，未推测或伪造数字。其他代理可能同时编译；最终 pass 没有并行运行本 lane 的测试或编译任务。

ASCII 输入来自 spikes/data/text-100mb.txt 和 text-1gb.txt，分别为 100,000,000、1,000,000,000 字节。Unicode 由基准流式生成 100,000,000 字节，含 CJK、astral emoji 和组合音标，非 ASCII 字节占 30.16%。所有输入均有效 UTF-8。

每个 load 样本独立进程，只从 File 流式载入，不预建整份输入 String。ops 每轮也独立载入；单点/批量编辑从保留的原始快照开始，包含 COW，计时不含析构。随机转换每轮一百万次，单点编辑一万次；clone 批量保留十万份到计时结束。batch 的坐标和编辑数组均预计算，所有实现相同；每次插入 foo。P95 采用 nearest rank，五轮时就是最大值。

RSS 用 getrusage(RUSAGE_SELF)，macOS ru_maxrss 直接为字节，未使用 /usr/bin/time。同时记录 CPU ns、minor/major faults，wall time 包括被调度出去的时间与缺页等待。chunks 只取每块长度，其 GB/s 按覆盖字节量换算，不表示逐字节读取正文的带宽。lines 扫描 LF；vel-text 视图不含 LF，crop/ropey 使用各自原生 line 迭代器。

## 实际迭代与剩余成本

| 阶段（不同时间段，非严格 A/B） | 均匀 100k batch median ms |
|---|---:|
| 2 KiB 满叶，16 路；共享叶复制后再拼接 | 522.447 |
| 消除重复叶复制，保留孩子数组，32 路 | 60.415 |
| 共享原叶，至多 32 字节小替换独立保存 | 25.915 |
| 4 KiB 装载，小替换直接内联节点 | 21.597 |
| 有界叶 LF 表，避免扫描 4 KiB 正文 | 24.961 |
| 最终：ASCII 已证明边界与 LF 表验证 | 7.438 |

还尝试过 Arc<str> 持有正文：2 KiB 正文加引用头跨过 malloc size class，导致额外空间；将引用头移出正文页后，分散编辑减少了正文页触碰。最终保留普通叶 String，小替换引用原叶 Arc，不复制正文，不产生历史链。UTF-16 汇总改为 scalar 数加四字节首字节数；流式 UTF-8 校验改用 simdutf8。以上没有在库内引入 unsafe。

早期隔离诊断（2 KiB 分块，尚未采用最终 LF 索引）如下；用于定位读取和分配成本，不替代最终 4 KiB 实现的验收数据。

| 诊断 | wall median ms | CPU median ms |
|---|---:|---:|
| scan | 232.227 | 202.818 |
| strings | 448.248 | 329.769 |
| arc_strings | 550.463 | 355.762 |

最终未达标项为：无。1 GB 载入仍包含文件读取、正文分配、LF 索引分配与组树；本轮将 UTF-8 校验和叶构建并行化，按序回收块，提供 from_reader_serial 无线程入口。均匀编辑还需复制受影响的内部孩子数组、分配新的叶节点并维护 Arc 引用计数。CPU/缺页计数见原始数据。8 worker / 4 MiB 块的初轮测得 202.45 ms，但载入结束 RSS 约 1.124×，超出原有 +10% 存储预算；8 worker / 1 MiB 块为 437.95 ms；4 worker / 2 MiB 块探索值为 263.51 ms、RSS 1.098×。这些是不同时间段的诊断，最终验收只看上表。随后交错 A/B 发现工作线程分配 Arc 节点时前 100k 行 batch 有回退，因此把 Arc 分配移回读取线程，并串行构建前 4 MiB 保留开头行的索引局部性；普通叶的 chunk 遍历跳过小补丁分片检查。最后缓存重复插入字符串的汇总，并跳过空删除片段，消除批量路径的重复扫描。未通过的预算不作为已完成验收。

## 相对上一轮的 5% 回退门禁

参考值来自上一轮已交付 perf-summary.json；不是同一时间段的 A/B。所有 >5% 项仍须按工程规则复测，不能用绝对预算通过替代回退门禁。

| 操作 | 上轮 median | 本轮 median | 变化 |
|---|---:|---:|---:|
| byte_char | 2.961 ns | 2.761 ns | -6.76% |
| char_byte | 0.944 ns | 0.942 ns | -0.15% |
| byte_utf16 | 2.497 ns | 2.422 ns | -3.01% |
| utf16_byte | 0.990 ns | 1.012 ns | +2.29% |
| byte_line | 354.340 ns | 324.792 ns | -8.34% |
| line_byte | 336.076 ns | 300.397 ns | -10.62% |
| byte_point | 404.121 ns | 349.443 ns | -13.53% |
| byte_point_utf16 | 373.224 ns | 383.802 ns | +2.83% |
| point_byte | 328.316 ns | 303.057 ns | -7.69% |
| point_utf16_byte | 348.519 ns | 332.428 ns | -4.62% |
| clone | 3.750 ns | 3.435 ns | -8.40% |
| insert | 1,514.442 ns | 1,248.388 ns | -17.57% |
| delete | 1,164.463 ns | 1,144.800 ns | -1.69% |
| batch_lines_10000 | 487,375.000 ns | 434,125.000 ns | -10.93% |
| sequential_lines_10000 | 9,701,584.000 ns | 9,592,916.000 ns | -1.12% |
| batch_lines_100000 | 5,063,209.000 ns | 4,500,125.000 ns | -11.12% |
| sequential_lines_100000 | 91,921,041.000 ns | 93,395,250.000 ns | +1.60% |
| batch_uniform_100000 | 8,515,250.000 ns | 7,438,084.000 ns | -12.65% |
| sequential_uniform_100000 | 19,477,333.000 ns | 20,132,500.000 ns | +3.36% |
| chunks | 3,869,958.000 ns | 2,397,167.000 ns | -38.06% |
| lines | 808,569,834.000 ns | 744,077,541.000 ns | -7.98% |

超过 5% 的项目：无。
并行装载的早期版本曾在交错 A/B 中使前 100k 行 batch 从 5.065 ms 变成 6.311 ms；最终改为读取线程按序分配 Arc 节点，前 4 MiB 串行保留索引局部性。早期 A/B 仅用于诊断，不代替本次最终 pass；负载不可读，不能把剩余差异直接归因于其他代理。

## 复现与产物

```sh
cargo bench -p vel-text --bench perf -- --runs 5 > crates/vel-text/perf-final.jsonl
python3 crates/vel-text/benches/report.py crates/vel-text/perf-final.jsonl
```

最终原始记录：[perf-final.jsonl](perf-final.jsonl)；完整统计：[perf-summary.json](perf-summary.json)。探索阶段记录在 gitignored 的 .perf-local/perf-data/，本报告只用最终 pass 做预算验收。

验证：cargo fmt --all；cargo clippy -p vel-text --all-targets -- -D warnings；cargo test -p vel-text（debug 与 release）；--no-default-features；RUSTDOCFLAGS=-D warnings cargo doc -p vel-text --no-deps。本轮增加了非 Send reader、多个在途块后的 I/O/UTF-8 错误、2 MiB Unicode 分界与串行路径一致性测试。另以 PROPTEST_CASES=1024运行 release 模型测试，包含生产叶大小集成测试。

## Round 11: additive progressive loader

The original results above remain intact. The new `Text::from_reader_snapshots` is isolated in
`src/tree/progressive.rs`; existing loader/edit/query implementations and Node layout stay the
same. Prefixes share complete subtrees, with at most 8 MiB of source input in flight.

All existing text benchmarks and all eight buffer scenarios were rerun with alternating,
process-isolated A/B pairs against clean HEAD 760baf9: 25 pairs per metric, and 101 for the tiny
50 MB undo case. Both paired and marginal median regressions stay within 5%; the largest paired
increase is undo at +4.59%, CI [1.000, 1.09486]. This is a point-estimate gate, not a claim that
all confidence intervals exclude 5%. Initial estimates, an A/A control, and all old/new samples
are preserved. New feature budgets, tails, memory accounting, full CIs and reproduction are in
[../vel-largefile/PERF.md](../vel-largefile/PERF.md). Workspace fmt, clippy -D warnings, release
tests (421 passed) and rustdoc -D warnings passed. No commit was made.
