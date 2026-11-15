# vel-buffer

Velocity 的共享文档与视图编辑模型，依赖自研 `vel-text`，不包含 UI、语法或像素布局。
许可证：MIT OR Apache-2.0。仅参考本轮 spec 与 [公开 API 文档](https://www.sublimetext.com/docs/api_reference.html)，
未读取随编辑器分发的代码或二进制。未核实的语义集中列于 [BEHAVIOR.md](BEHAVIOR.md)。

```rust
use vel_buffer::{Buffer, Region, Selection, ops};

let mut buffer = Buffer::new("hello\nworld");
let view = buffer.new_view();
buffer.set_selection(view, Selection::from_regions([
    Region::caret(5), Region::caret(11),
]));
let old = buffer.change_id();
ops::insert(&mut buffer, view, "!");
assert_eq!(buffer.text().to_string(), "hello!\nworld!");
assert!(buffer.is_dirty());
assert!(buffer.undo(view));
assert_eq!(buffer.text().to_string(), "hello\nworld");
assert!(!buffer.is_dirty());
assert!(buffer.change_id() > old);
```

## 模型与坐标

`Buffer` 持有正文、共享撤销栈、变更日志和多个 `ViewState`；`BufferId` / `ViewId` 为进程内单调分配的身份。
`view(id).sel()` 返回只读选区，交互式更改走 `set_selection`，从而不会绕过软撤销和打字分组边界。
所有坐标为 UTF-8 字节；编辑与选区要求合法 scalar 边界，插件适配层以后负责字符坐标转换。
LF 是唯一行分隔符，CR 保留为正文字符。普通化选区至少含一个 caret；`new` / `clear` 允许临时空集合。

`Region` 保留 anchor/caret 方向和 `Option<f64>` 的纵向目标。`Selection` 内部是共享 Arc 数组：
caret 列表每点只保存一个 `usize`，非空选区另配 anchor 数组，纵向目标仅在需要时分配。
批量构造先排序再线性归并，`add` 二分定位并局部合并；primary 与文档排序独立。
一百万个区域不按对象分别堆分配。公开 API 的 `iter()` 返回复制的 `Region` 值。

## 事务与位置映射

`transact(view, kind, edits, selection)` 的 edits 全部使用事务开始前的坐标，升序、无重叠；
相接与同位置插入按 `vel-text` 的契约处理。先在共享候选根上调用一次 `Text::apply_edits`，
验证提交后的选区，再发布结果；非法输入不会更改正文、选区、撤销栈或 change ID。
编辑重置目标列，外部视图的选区与所有 named regions 同步映射。

`ChangeSet` 保存 `(old_range, new_len)` 和累计的新起点，单点映射 O(log edits)。
10k 编辑映射 100k named regions 不逐次修改正文；注释集合允许重叠，按 begin 稳定排序。
选择点在插入位置向右黏附；删除内部点坍缩到替换起点。注释的 begin 向右、end 向左，
空注释按 caret 处理，`drop_empty` 集合会丢弃坍缩项。
`transform_*_from` 对未来 ID、超出旧正文长度或已过保留窗口的 ID 返回 `None`。
历史映射不重新验证旧文本的 UTF-8 边界，调用方应只传入合法历史坐标。

每次事务（含 undo/redo 的每个步骤）增加 `ChangeId`。日志默认保留 10,000 个事务图，
同时受 `HistoryLimits::bytes` 的独立字节上限约束；过旧 ID 不能用于异步结果合并。

## 撤销、保存与内存

一次同文本的纯插入只保存紧凑的逆区间、一个共享 `Arc<str>` 和前后选区；
不会为每个 caret 保留 String 或旧正文根。undo 构造一个逆向删除批次，redo 重用原插入批次。
其他事务用同一紧凑描述符和前后 `Text` 快照保存逆编辑的来源；被删正文保持在共享树中，
undo/redo 通过替换根恢复，50 MB 删除没有正文复制或逐块重插。分组保存步骤，普通 undo
一次逆序执行整组，soft undo 一次处理一个事务或一个选区事件。

`can_group` 是唯一打字策略入口：同一视图、连续相同 caret、同字符类可以合并；
word 后空白属于前一组，下一 word 开新组；尚未出现 word 的前导空白可跟第一个 word 合并。
标点和 LF 类切换、显式选区改变、其他编辑、`break_undo_group` / `mark_saved` 都断组。
新事务清空全局 redo，并移除所有视图中指向被丢弃分支的软历史引用。
保存点使用稳定的内容修订身份，undo/redo 回到该身份时 `is_dirty()` 为 false。

`HistoryLimits` 默认 10,000 个事务 / 256 MiB；超限丢弃最旧的完整组，redo 也计数。
字节记账包含选区数组、变更图及快照相对当前正文独占的可达分配；比较前后两根取较大值，
沿历史累加作为保守界。共享根立即返回；其他快照比较为 O(nodes)，不在普通打字路径使用。
此值不是 allocator/RSS 的精确测量，
共享未修改正文不重复计入。变更日志使用独立的同额上限。每视图软选区历史默认 256 个事件，
可配置更高；文本事件只引用同一个撤销步骤，不能延长被淘汰步骤的生命周期。
`last_undo_metadata_bytes()` 是基准用的元数据诊断，包含两个选区数组和区间索引；
计时、内存公式、共享内存边界见 [PERF.md](PERF.md)。

## 编辑操作与布局

`ops` 实现本轮全部非语法操作：insert / paste、grapheme 与 word 删除、字符/单词/子词/自定义 stops
移动、绝对移动、选区展开、Cmd+D 与全选匹配、单选区、拆行、列添加、拼行/交换行/复制行、
缩进/取消缩进、四种大小写、transpose、排序/逆序和行尾空白清理。语法相关的 snippet、括号、scope、
comment 留到相应 crate。每个编辑操作生成一个原坐标批次，同一行只处理一次。

横向 `move_by(..., None)`；`Lines` / `Pages` 及 `select_lines` 必须提供 `VerticalLayout`。
`MonospaceLayout` 用于测试和无界面调用：tab 按 stop 推进，宽 CJK/emoji 使用 `unicode-width` 的
簇宽度，组合字符不拆开。连续纵向移动保留 `xpos`，水平操作和编辑重置目标。

字词、删除和查找直接访问 rope 的 chunk / grapheme 边界，不 flatten 正文。
字面搜索从上次匹配末尾继续，只为跨 chunk 的接缝保留 needle 长度的尾部；
Cmd+D 的第 10,000 次调用不重新扫描前 9,999 个匹配。
行级改写仅物化相关行，大小写与 transpose 仅物化相关选区。它们不是全文大文件热路径。

## 验证与性能复现

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
RUSTDOCFLAGS=-D\ warnings cargo doc --workspace --no-deps
PROPTEST_CASES=1024 cargo test -p vel-buffer --release --test model
cargo bench -p vel-buffer --bench perf -- --runs 5 > crates/vel-buffer/perf-final.jsonl
python3 crates/vel-buffer/benches/report.py crates/vel-buffer/perf-final.jsonl
```

模型测试覆盖 String 顺序编辑、全历史正文与选区往返、交错的软撤销、随机位置映射、
选区并集和 named-region 偏置；操作测试覆盖多 caret、CJK、ZWJ emoji、组合音标、CR、
空缓冲区和缺少尾 LF。库用 `#![forbid(unsafe_code)]`；只有基准的 OS 记账 FFI 使用有 SAFETY 注释的 unsafe。
中间测量置于 `.perf-local/`，不随 crate 交付。最终结果与局限见 [PERF.md](PERF.md)。

## Round seven: native Hot Exit history

`Buffer::export_history() -> HistorySnapshot` captures owned groups, shared steps, both undo
cursors, saved-state identity, per-view soft selection/edit events, limits, owner IDs, typing
continuation, named regions, settings and retained ChangeSets. Capture clones references and
bounded metadata; it does not copy document text or extract edit strings. Serialize with serde
on the existing session writer. The v1 payload stores forward/inverse UTF-8 edit deltas relative
to the separately supplied current text, delta-coded coordinates/IDs, varints, zlib and base64.
It stores no full historical document copies. BLAKE3 plus current byte length bind it to text.
Serde formats can persist the opaque payload; do not inspect its private packed representation.

```rust
use std::collections::BTreeMap;
use vel_buffer::{Buffer, HistorySnapshot, ViewId, ops};
let mut buffer = Buffer::new("hello");
let old_view = buffer.new_view();
ops::insert(&mut buffer, old_view, "!");
let current_text = buffer.text().clone();
let owned = buffer.export_history(); // application capture; writer may serialize later
let json = serde_json::to_vec(&owned)?;
let decoded: HistorySnapshot = serde_json::from_slice(&json)?;
let remap: BTreeMap<_, _> = decoded.view_ids().into_iter()
    .map(|old| (old, ViewId::fresh())).collect();
let mut restored = Buffer::restore_history(current_text, decoded, &remap)?;
assert!(restored.undo(remap[&old_view]));
assert_eq!(restored.text().to_string(), "hello");
# Ok::<(), Box<dyn std::error::Error>>(())
```

The import map must cover exactly the attached old views with unique nonzero fresh IDs. Import
attaches those IDs; it does not require placeholder Buffer views. Closed historical owners get
fresh hidden IDs, so their text steps remain undoable without reviving closed sheets. Import
checks format/version, text digest/length, edit ordering/content/scalar boundaries, selection
shapes/primary/goals, revision chains, cursor, references, view ownership and log geometry.
Malformed imports return `HistoryError` without touching an existing live buffer. Decoding is
bounded to 512 MiB; restore/serialization are background operations. Small histories have fixed
metadata overhead: the 2× edit-byte size target is measured on the specified 10,000-step fixture,
not a promise for an empty history or arbitrary incompressible selection/annotation metadata.

`ChangeId`s are rebased while preserving stored relative ages. Old asynchronous results must
be discarded across a restart. `changes_since(from)` returns retained `(ChangeId, ChangeSet)`
pairs, including each step of a grouped undo/redo. `None` means the mapping window expired.
This is a descriptor API, not a stream of intermediate text roots: if an incremental consumer
missed multiple steps and lacks the matching intermediate snapshots, reset it from current
text. The session crate can replace its command-replay adapter with this native payload next
round; no session/application sources were changed in this lane.

Round-seven budget, validation and raw timing evidence: [engine report](../vel-git/ROUND7.md).
