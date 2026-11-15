# vel-text

Velocity 的持久化 UTF-8 文本存储。自行实现 B-tree；不包装其他 rope。
许可证随 workspace：MIT OR Apache-2.0。性能结果及未达标项见 [PERF.md](PERF.md)。

```rust
use vel_text::{Edit, PointUtf16, Text};

let mut text = Text::from("hello\n世界🦀");
let snapshot = text.clone();
text.apply_edits(&[
    Edit { range: 0..5, text: "hi" },
    Edit { range: 6..6, text: "你好 " },
]);
assert_eq!(snapshot.len_utf16(), 10);
assert_eq!(text.point_utf16_to_byte(PointUtf16 { row: 1, column: 0 }), 3);
```

## 坐标与边界

- 主坐标是 `usize` 字节偏移，范围为 `0..=len_bytes()`；`char_at`、`byte_at` 不接受 EOF。
- 字符坐标计 Unicode scalar，不是 grapheme。UTF-16 坐标计 code unit，严格转换拒绝 surrogate pair 中间的位置。
- 只有 LF 分行，空文本有一行，末尾 LF 产生空的最后一行。CR 是普通字符；本层不转换行尾、编码或 BOM。
- `Point` 的 column 是字节，`PointUtf16` 的 column 是 UTF-16；列不能超过本行正文长度，LF 不属于正文。
- `line_to_byte(len_lines())` 非法。所有严格坐标接口在越界或 scalar 边界错误时 panic，并附带原因。
- `clip_byte/char/utf16/line/point/point_utf16` 返回同单位的合法坐标。字节和 UTF-16 向前舍入；点的行先夹到最后一行，列再夹到该行正文。
- `floor_char_boundary`、`ceil_char_boundary` 都将越界输入夹到 EOF。
- `chunk_at_byte` 允许 scalar 内部的字节。EOF 返回最后一个 chunk；空视图返回 `("", 0)`。`chunks()` 对空视图不产生元素。
- `TextSlice` 是借用视图，不分配正文。长度、转换、子切片和迭代都相对于切片；切片开头和结尾分别作为自己的文本起点和终点。
- grapheme 使用 `unicode-segmentation::GraphemeCursor`，按请求跨 chunk 提供前文，不 flatten；切片使用自身的 grapheme 上下文。

## 树与内存布局

`Text` 持有 `Arc<Node>` 根，clone 只增加引用计数。`Node` 缓存总 summary、树高以及
`Leaf` / `Branch(Vec<Child>)`。普通叶保存 String 和紧凑 LF 位置表；小替换叶
保存原始普通叶的 Arc、替换区间和最多 32 字节的内联 UTF-8。每条 Child 边保存 `Arc<Node>` 和到该子树末尾的
**累计** summary。四个 `usize` 分别表示 bytes、scalars、UTF-16 units、LF 数量。
节点只含本层数据，没有父指针或整份文本索引；`Text` 和视图均为 Send + Sync。
`estimated_storage_bytes()` 统计可达分配，包含共享原叶、容量和索引；不含 malloc
额外记账及 size class 舍入，仅用于诊断。基准同时记录该估计和独立 load 的 RSS。

生产配置：内部节点最多 32 个孩子，非根至少 16 个；叶子最多 6144 字节，通常至少
3072 字节。装载目标为 4096 字节，给小插入保留大小上的余地；String 只分配实际
装载容量，不为整个 6144 字节上限预留。UTF-8 分界允许最小占用少至三个字节。
单孩子根会折叠，空树规范化为一个空叶子。单元测试使用 128 字节叶上限，在小输入
上反复覆盖多层分裂、合并；集成测试额外覆盖真实生产叶大小。

查询用累计 summary 二分下行，再在有界叶中完成局部转换，复杂度 O(log n)。ASCII
视图的 byte/scalar/UTF-16 可以直接换算。普通叶的 LF 表可二分或直接索引，避免随机
行查询扫描整个 4 KiB 正文；LF ≤255 个时每个位置只占低八位，并为每 256 字节块保存
累计数量；密集 LF 输入回退为 u16 位置。它不是文档全局的行表。LF 扫描使用
memchr；非 ASCII 的 UTF-16 总量等于 scalar 数量加上 UTF-8 四字节首字节数量。

`from_reader` 在调用线程读取并按文档顺序分配 Arc 节点；前 4 MiB 串行构建以保留开头行的索引局部性，
随后最多四个 scoped workers 并行校验 UTF-8、分配正文与 LF 索引。
每个 worker 最多一个 2 MiB 块在途；按原顺序回收叶子与复用输入块，暂存上限 8 MiB，
不保留第二份完整输入。跨块最多保留三个 UTF-8 字节，完整处理短读、Interrupted、
非法编码、截断与中途 I/O 错误；错误路径关闭队列并等待所有 worker 退出。
reader 不要求 Send 或 static；单核自动走串行，`from_reader_serial` 可显式使用无线程的原 1 MiB 路径。
树的分块、汇总和 LF 查询格式保持相同，自底向上组树；暂存、线程与指针数组计入加载峰值 RSS。

## 编辑与遍历

单叶编辑走专用路径：只复制共享路径与该叶，按删除/插入的 summary 差更新祖先；
独占节点可原地复用。至多 32 字节的单个小替换保留原叶的 Arc，并内联新内容，
零拷贝共享旧正文，最多提供三段 chunk。再次编辑该叶会压实，不形成编辑历史链。
Arc 引用计数保留在小型节点分配内，与正文页分开。跨叶替换和批量编辑走统一的有序遍历。

`apply_edits` 要求原文坐标、start 升序、区间不重叠；允许相接及同位置多次插入，
同位置按输入顺序拼接。先验证全部区间和 UTF-8 边界，错误时不改变正文；Unicode
边界检查也是一次有序树遍历。正文修改再遍历一次，只进入受影响子树，在每个受影响
叶中处理所有编辑：单个小替换用内联表示，其余一次拼接。
批量路径缓存同一插入字符串的 scalar/UTF-16/LF 汇总，纯插入不扫描空删除片段。跨叶删除跳过完整子树，未触及节点保持共享。树形不变时复用
孩子数组；仅在分裂/删除/占用不足时重组并修复相邻节点。复杂度取决于编辑数、触及
的树路径与被复制的叶内容；不会为每个光标分别搜索和修改整条路径。

`Chunks` 持有前后两个 zipper，前后交替迭代不会重复；跨叶移动摊销 O(1)。
`Lines` 直接消费 chunks、扫描 LF 并维护累计 summary，不对每行重新查树。
`Display`、`chars`、`bytes` 不 flatten。相同根和对齐的共享子树可快速判断相等，
不同分块也按内容比较。Hash 以固定的逐字节调用序列计算，分块方式不影响结果。
Debug 最多输出 64 个 scalar 的预览。

## 搜索

默认启用 `regex-cursor`。`ChunkCursor::from(&text)` 或
`ChunkCursor::new(slice)` 实现 `regex_cursor::Cursor`；偏移相对于该视图，前进/后退
失败不改变位置，UTF-8 scalar 不会被拆开。可用 `--no-default-features` 去掉该依赖。

## 验证与复现

```sh
cargo fmt --all
cargo clippy -p vel-text --all-targets -- -D warnings
cargo test -p vel-text
cargo test -p vel-text --release
cargo test -p vel-text --no-default-features
RUSTDOCFLAGS='-D warnings' cargo doc -p vel-text --no-deps
cargo bench -p vel-text --bench perf -- --runs 5 > crates/vel-text/perf-final.jsonl
python3 crates/vel-text/benches/report.py crates/vel-text/perf-final.jsonl
```

基准默认读取 `spikes/data/text-{100mb,1gb}.txt`；可传 `--data DIR`。
自动在系统临时目录生成 100,000,000 字节混合 Unicode 文件。`--impl vel-text|crop|ropey`
筛实现，`--only load|ops` 筛阶段。每一独立 worker 都先装载原始文本；load 的峰值
不会混入编辑、输入 String 或模型。ops 从保留的快照复制后编辑，统计首次 COW，
不把结果析构、编辑数组生成、随机坐标生成算入操作时间。clone 批量持有至计时结束。
转换每轮随机查询一百万次；单点编辑一万次。crop/ropey 的 batch 是逆序逐点插入。
line-start 预计算对所有实现均在计时外。另测 vel-text 自身的逆序逐点插入，验证
batch 相对同一实现的收益。

每个观测输出一行 JSON：run、wall time、CPU time、minor/major faults、getrusage
峰值 RSS、macOS 载入完成的常驻 RSS（proc_pid_rusage）和该次调用 `sysctl -n vm.loadavg` 的结果。沙箱拒绝 sysctl 时原样记录
unavailable，不伪造负载数字。macOS 的 ru_maxrss 单位是字节；Linux 换算自 KiB。
RSS 是整个 worker 的高水位，ops 后面的记录可能包含先前操作的峰值；内存预算
只使用隔离的 load worker。预算取五次 wall time 的中位数，P95 用 nearest rank。

可运行 `--impl vel-text --profile --runs 5` 分解加载：scan 只读、校验 UTF-8、数 LF，
strings 额外持有分块 String，arc_strings 再给每块加 Arc；每项独立进程，观察分配
和建树之外的最低成本。它们是诊断项，不替代正式 from_reader 数字。

## 来源与依赖

本实现全部从零编写，未复制 Sublime 二进制、随应用分发的文件，或其他 rope 的源码。
设计背景是 spec、spike 及公开的 B-tree、copy-on-write、多度量 rope 思路（crop、
ropey、xi-rope、Zed rope/sum_tree）；没有改编代码需要逐段归属。运行时依赖
memchr、simdutf8、unicode-segmentation，以及可选的 regex-cursor；crop、ropey、proptest、
libc 仅作测试/性能对照。库内不使用 unsafe；基准中的 getrusage / proc_pid_rusage FFI 有 SAFETY 注释。

探索阶段数据已移至 gitignored 的 `.perf-local/perf-data/`；发布结果只保留 `perf-final.jsonl` 与 `perf-summary.json`。
本轮新增并行/串行、非 Send reader、2 MiB 分界及多个在途任务后的错误传播测试。

## Round 11: progressive engine integration

`Text::from_reader_snapshots(reader, publish_bytes, interval, callback)` publishes the first
completed 2 MiB and subsequent persistent prefixes while one bounded worker pipeline continues.
It preserves the original loader's UTF-8 seams, short-read/error handling and at most 8 MiB of
in-flight input. Callback errors stop the pipeline and join its workers. Every prefix is provisional
until the terminal Result succeeds. The existing from_reader/from_reader_serial paths are unchanged.
Prefix assembly shares complete subtrees and repairs the joining boundary without flattening.
Tree invariants and preserved prefix snapshots are tested throughout construction. The progressive
pipeline is isolated in its own module; the ordinary rope API and node layout are unchanged.

Lane A should normally use [vel-largefile](../vel-largefile/README.md), which adds cancellation,
a coalescing worker mailbox, strict BOM/binary/fallback routing, progress and prefix measurements.
Its LongLineIndex supplies optional grapheme/cell/word metrics without enlarging every rope node.
The paired text/buffer regression gate is recorded in [../vel-largefile/PERF.md](../vel-largefile/PERF.md).
