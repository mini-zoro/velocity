# vel-buffer 性能验收记录

2026-11-14；M1 Pro / arm64，Rust 1.99.1，workspace release 默认配置。
时间为 median / P95，本次每项 7 个独立进程；P95 使用 nearest rank。MB 为十进制。

| 场景 | 原目标 | 最终实测 median / P95 | 验收 |
|---|---:|---:|---|
| 100 MB，10,000 caret 输入一字符并合并组 | ≤ 3 ms | 0.870 / 0.912 ms | 通过 |
| 100 MB，100,000 caret 输入一字符并合并组 | ≤ 25 ms | 9.704 / 10.279 ms | 通过 |
| 添加第 10,000 个字面匹配 | ≤ 2 ms | 0.028 / 0.049 ms | 通过 |
| 撤销 10,000 caret 的三字符打字组 | ≤ 5 ms | 1.103 / 1.143 ms | 通过 |
| 50 MB 子文档全选删除 | ≤ 5 ms | 0.686 / 0.810 ms | 通过 |
| 撤销 50 MB 全选删除 | ≤ 5 ms | 0.001 / 0.001 ms | 通过 |
| 10,000 编辑事务并映射 100,000 named regions | ≤ 10 ms | 3.917 / 4.160 ms | 通过 |
| 10,000 caret 单字符插入的撤销项 | ≤ 64 B/caret | 48.049 / 48.049 B/caret | 通过 |

## 测量范围

输入来自 `spikes/data/text-100mb.txt`，每个 worker 单独从 File 流式载入，不预建正文 String。
除指定场景的操作外，装载、初始选区、夹具生成和最终断言均不计时；输入字符的编辑数组生成、
候选根、事务验证、正文提交、所有 region 映射、选区安装、历史记账与分组合并都在计时内。
typing 先输入 x，计时第二个字符 y，确认 undo 一次会撤销两个字符。
undo 计时整个三事务组的逆向操作。named-region 项包含正文编辑与全部 100,000 个标记更新，
标记为排序的空 caret（每行一个），事务为前 10,000 行各插入 x。

查找以语料中首个 `use` 为非空字面 seed，先执行 9,998 次相同公开 Cmd+D 操作，
计时第 10,000 个选区的添加；保持增量缓存和真实的 256 事件软选区历史。
这是连续字面匹配的摊销路径，不代表任意 regex 或全文件 find-all。

删除夹具从同一 100 MB rope 分享前 50 MB，构造在计时外，然后全选删除/撤销；
没有 flatten 或复制被删正文。删除计时包含历史独占分配记账的树遍历；撤销切换共享根。

内存项统计撤销项的结构、区间/prefix Vec 容量、两个紧凑选区数组及共享插入字符串等元数据；
普通同文本插入不保留前后正文根，因此该场景没有撤销专属的正文快照。当前正文树、
进程/集合基础容量、malloc 记账及 size-class 舍入不计入单个撤销项。该数不是进程 RSS。
其他删除/替换事务的前后 rope 快照按相对独占可达分配计入 `history_retained_bytes()`，
共享未改动节点不重复计费。两个结果不可混称。

## 环境与限制

共 56 个观测；sysctl 不可用的观测为 56，原始 loadavg 字段记录 unavailable 与错误。
不推测实际系统负载。其他代理可能同时编译；本 lane 最终 pass 没有同时启动编译或测试。
基准用 getrusage 记录 CPU、缺页与 RSS，macOS ru_maxrss 是字节；其 FFI 仅存在于基准，带 SAFETY 注释。
ops worker 的 RSS 包含装载、选区及准备阶段，不能用于估计单个撤销项内存。

初轮实现已通过各时间预算；随后把纯插入从持有全文根改为逆删除批次 + 一个 Arc<str>，
避免将正文的 COW 保留误算成零成本元数据。删除保持共享快照，并补充独占分配记账。
本报告只使用最终 pass；探索记录在 gitignored `.perf-local/`。

未达标项：无。

## 复现

```sh
cargo bench -p vel-buffer --bench perf -- --runs 5 > crates/vel-buffer/perf-final.jsonl
python3 crates/vel-buffer/benches/report.py crates/vel-buffer/perf-final.jsonl
```

`--data FILE` 可改语料，`--only SCENARIO` 可单独测某项；正式验收必须完整跑且每项至少五次。
原始数据：[perf-final.jsonl](perf-final.jsonl)；统计：[perf-summary.json](perf-summary.json)。
验证：cargo fmt --all；cargo clippy --workspace --all-targets -- -D warnings；
cargo test --workspace --release；RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps；
另以 PROPTEST_CASES=1024 验证 buffer 模型，以及 vel-text 的 debug / 无默认 feature 路径。
