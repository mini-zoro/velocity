#!/usr/bin/env python3
"""Summarize process-isolated perf JSONL; write the Chinese acceptance report."""
import collections
import json
import math
from pathlib import Path
import statistics
import sys


def grouped(path):
    result = collections.defaultdict(list)
    for line in path.read_text(encoding="utf-8").splitlines():
        item = json.loads(line)
        key = (item["implementation"], item["corpus"], item["operation"])
        result[key].append(item)
    return result


def summary(items):
    values = sorted(item["total_ns"] / item["count"] for item in items)
    if len(values) < 5:
        raise ValueError("at least five measurements are required")
    return {
        "runs": len(values),
        "median_ns": statistics.median(values),
        "p95_ns": values[math.ceil(0.95 * len(values)) - 1],
        "peak_rss_bytes": max(item["peak_rss_bytes"] for item in items),
        "median_cpu_ns": statistics.median(item.get("cpu_ns", 0) / item["count"] for item in items),
        "content_bytes": items[0]["content_bytes"],
        "estimated_storage_bytes": max((item.get("estimated_storage_bytes") or 0 for item in items), default=0),
        "resident_rss_bytes": max((item.get("resident_rss_bytes") or 0 for item in items), default=0),
    }


raw_path = Path(sys.argv[1]).resolve()
root = Path(__file__).resolve().parent.parent
groups = grouped(raw_path)
stats = {key: summary(value) for key, value in groups.items()}
for implementation in ("vel-text", "crop", "ropey"):
    for corpus in ("ascii-100mb", "ascii-1gb", "unicode-100mb"):
        assert (implementation, corpus, "load") in stats, "full comparison pass is required"
for items in groups.values():
    assert {item["run"] for item in items} >= {1, 2, 3, 4, 5}


def get(op, corpus="ascii-1gb", implementation="vel-text"):
    return stats[(implementation, corpus, op)]


def ns(op, corpus="ascii-1gb", implementation="vel-text"):
    return get(op, corpus, implementation)["median_ns"]


def pair(op, unit="ms", corpus="ascii-1gb", implementation="vel-text"):
    data = get(op, corpus, implementation)
    scale = {"ns": 1, "µs": 1000, "ms": 1_000_000}[unit]
    return f'{data["median_ns"]/scale:,.3f} / {data["p95_ns"]/scale:,.3f} {unit}'


def rate(op, implementation="vel-text"):
    data = get(op, implementation=implementation)
    return data["content_bytes"] / data["median_ns"]


conversions = ["byte_char", "char_byte", "byte_utf16", "utf16_byte", "byte_line", "line_byte",
               "byte_point", "byte_point_utf16", "point_byte", "point_utf16_byte"]
slowest = max(conversions, key=ns)
load = get("load")
peak_ratio = load["peak_rss_bytes"] / load["content_bytes"]
storage_ratio = load["estimated_storage_bytes"] / load["content_bytes"]
rest_ratio = (load["resident_rss_bytes"] or load["peak_rss_bytes"]) / load["content_bytes"]
budget = [
    ("from_reader，1 GB ASCII", "≤ 300 ms", pair("load"), ns("load") <= 300e6),
    ("加载峰值 RSS，1 GB", "≤ 1.15 × 文件", f'{load["peak_rss_bytes"]/1e6:,.2f} MB，{peak_ratio:.4f} ×', peak_ratio <= 1.15),
    ("from_reader，100 MB Unicode", "≤ 80 ms", pair("load", corpus="unicode-100mb"), ns("load", "unicode-100mb") <= 80e6),
    ("clone", "≤ 50 ns", pair("clone", "ns"), ns("clone") <= 50),
    (f"随机转换，最慢项 {slowest}", "每项 ≤ 600 ns", pair(slowest, "ns"), ns(slowest) <= 600),
    ("随机单字符 insert", "≤ 2 µs", pair("insert", "µs"), ns("insert") <= 2000),
    ("随机单字符 delete", "≤ 2 µs", pair("delete", "µs"), ns("delete") <= 2000),
    ("apply_edits，前 10k 行", "≤ 1 ms", pair("batch_lines_10000"), ns("batch_lines_10000") <= 1e6),
    ("apply_edits，前 100k 行", "≤ 8 ms", pair("batch_lines_100000"), ns("batch_lines_100000") <= 8e6),
    ("apply_edits，均匀分布 100k", "≤ 15 ms", pair("batch_uniform_100000"), ns("batch_uniform_100000") <= 15e6),
    ("chunks 全遍历，1 GB", "≥ 5 GB/s", f'{rate("chunks"):.2f} GB/s；{pair("chunks")}', rate("chunks") >= 5),
    ("lines 全遍历，1 GB", "≥ 1 GB/s", f'{rate("lines"):.2f} GB/s；{pair("lines")}', rate("lines") >= 1),
    ("新载入文本存储开销", "≤ 10%", f'分配估计 +{(storage_ratio - 1)*100:.2f}%；载入完成 RSS {rest_ratio:.4f} ×', storage_ratio <= 1.10 and rest_ratio <= 1.10),
]
failures = [row[0] for row in budget if not row[3]]
rows = []


def add(text=""):
    rows.append(text)


add("# vel-text 性能验收记录")
add()
add("2026-11-14；32 路 B-tree、4 KiB 叶、紧凑 LF 索引；载入使用最多四个 scoped workers，"
    "每 worker 2 MiB 在途块，总暂存 ≤8 MiB；前 4 MiB 串行，Arc 节点按序在读取线程分配；提供 from_reader_serial 无线程入口。")
add("本次未全部达标：" + "、".join(failures) + "。" if failures else "本次所有预算的中位数均通过。")
add()
add("## 最终预算")
add()
add(f"时间列为 **median / P95**；本次每组 {min(item['runs'] for item in stats.values())} 个独立进程。吞吐按十进制 GB/s；"
    "MB = 10⁶ 字节，RSS 列取全部观测的最大高水位。预算没有放宽。")
add()
add("| 操作 | 原目标 | 本次实测 | 验收 |")
add("|---|---|---|---|")
for name, target, measured, passed in budget:
    add(f'| {name} | {target} | {measured} | {"通过" if passed else "**未达标**"} |')
add()
add("载入峰值由 getrusage 独立测量；macOS 常驻量另用 proc_pid_rusage 在载入结束时读取，"
    "失败时回退到峰值上界。二者均包含进程基线。另在计时和 RSS 采集后估计可达分配，统计 String/Vec 容量、LF 表、"
    "Arc 头和保留的原叶，按地址去重；不含 malloc 记账或 size class 舍入。此项需分配"
    "估计与载入结束的 RSS 都在 10% 内才标通过。压缩内存、调度和页故障均受当前系统状态影响。"
    "ops 进程的高水位包含之前的操作，不能当作单份新载入文本的内存。")
add()
add("## 每项坐标转换")
add()
add("| 随机操作，1 GB ASCII | median ns/次 | P95 ns/次 |")
add("|---|---:|---:|")
for op in conversions:
    item = get(op)
    add(f'| {op} | {item["median_ns"]:.2f} | {item["p95_ns"]:.2f} |')
add()
add("ASCII 的 byte/scalar/UTF-16 恒等转换由总 summary 证明，不读正文；"
    "此表不声称 Unicode 随机转换也有相同常数。Unicode 的正确性另由 String 模型验证。")
add()
add("## 同一最终 pass 的 crop / ropey 对照")
add()
add("每格时间均为 median / P95。crop 不提供 scalar 或 UTF-16 度量；"
    "ropey 的 UTF-16 与 byte 通过 char 座标组合。crop/ropey 批量项为逆序逐点插入。")
add()
add("| 操作 | vel-text | crop 0.4.3 | ropey 1.6.1 |")
add("|---|---:|---:|---:|")
comparison = [("加载 100 MB ASCII", "load", "ms", "ascii-100mb"),
              ("加载 1 GB ASCII", "load", "ms", "ascii-1gb"),
              ("加载 100 MB Unicode", "load", "ms", "unicode-100mb")]
comparison += [(op, op, unit, "ascii-1gb") for op, unit in
               [("clone", "ns"), ("byte_line", "ns"), ("line_byte", "ns"),
                ("insert", "µs"), ("delete", "µs"), ("batch_lines_10000", "ms"),
                ("batch_lines_100000", "ms"), ("batch_uniform_100000", "ms"),
                ("chunks", "ms"), ("lines", "ms")]]
for label, op, unit, corpus in comparison:
    cells = [pair(op, unit, corpus, implementation) for implementation in ("vel-text", "crop", "ropey")]
    add(f'| {label} | ' + " | ".join(cells) + " |")
add("| 隔离加载 1 GB 的最大 RSS | " + " | ".join(
    f'{get("load", implementation=i)["peak_rss_bytes"]/1e6:,.2f} MB' for i in ("vel-text", "crop", "ropey")) + " |")
add()
add("## batch 与 vel-text 自身逐点编辑")
add()
add("两者使用相同的原文坐标与快照。逐点路径逆序执行，时间不含坐标构造或析构。")
add()
add("| 场景 | apply_edits median / P95 | 逐点 median / P95 | median 加速比 |")
add("|---|---:|---:|---:|")
for label, batch, sequential in [("前 10k 行", "batch_lines_10000", "sequential_lines_10000"),
                                 ("前 100k 行", "batch_lines_100000", "sequential_lines_100000"),
                                 ("均匀 100k", "batch_uniform_100000", "sequential_uniform_100000")]:
    add(f'| {label} | {pair(batch)} | {pair(sequential)} | {ns(sequential)/ns(batch):.2f} × |')
add()
add("## 方法与机器状态")
add()
add("仓库登记主机为 Apple M1 Pro、8 核、16 GB。本轮确认 arm64、macOS 26.1 (25B78)，"
    "rustc 1.99.1 (58cf8f9c3)，Cargo 1.99.1；使用 rust-toolchain.toml 固定的 1.99。"
    "release 默认优化，保留 line tables，未改 allocator、LTO 或 target-cpu。")
add()
all_items = [item for items in groups.values() for item in items]
unavailable = sum(item["loadavg"].startswith("unavailable:") for item in all_items)
add(f"每个观测前执行 `sysctl -n vm.loadavg`，共 {len(all_items)} 次；{unavailable} 次被沙箱拒绝，"
    "原始输出逐条保存在 JSONL 的 loadavg 字段。实际负载不可读，未推测或伪造数字。"
    "其他代理可能同时编译；最终 pass 没有并行运行本 lane 的测试或编译任务。")
add()
add("ASCII 输入来自 spikes/data/text-100mb.txt 和 text-1gb.txt，分别为 100,000,000、"
    "1,000,000,000 字节。Unicode 由基准流式生成 100,000,000 字节，含 CJK、"
    "astral emoji 和组合音标，非 ASCII 字节占 30.16%。所有输入均有效 UTF-8。")
add()
add("每个 load 样本独立进程，只从 File 流式载入，不预建整份输入 String。ops 每轮也"
    "独立载入；单点/批量编辑从保留的原始快照开始，包含 COW，计时不含析构。随机转换"
    "每轮一百万次，单点编辑一万次；clone 批量保留十万份到计时结束。batch 的坐标和"
    "编辑数组均预计算，所有实现相同；每次插入 foo。P95 采用 nearest rank，五轮时就是最大值。")
add()
add("RSS 用 getrusage(RUSAGE_SELF)，macOS ru_maxrss 直接为字节，未使用 /usr/bin/time。"
    "同时记录 CPU ns、minor/major faults，wall time 包括被调度出去的时间与缺页等待。"
    "chunks 只取每块长度，其 GB/s 按覆盖字节量换算，不表示逐字节读取正文的带宽。"
    "lines 扫描 LF；vel-text 视图不含 LF，crop/ropey 使用各自原生 line 迭代器。")
add()
add("## 实际迭代与剩余成本")
add()
add("| 阶段（不同时间段，非严格 A/B） | 均匀 100k batch median ms |")
add("|---|---:|")
for filename, label in [("perf-initial.jsonl", "2 KiB 满叶，16 路；共享叶复制后再拼接"),
                        ("perf-fifth.jsonl", "消除重复叶复制，保留孩子数组，32 路"),
                        ("perf-eighth.jsonl", "共享原叶，至多 32 字节小替换独立保存"),
                        ("perf-ninth.jsonl", "4 KiB 装载，小替换直接内联节点"),
                        ("perf-tenth.jsonl", "有界叶 LF 表，避免扫描 4 KiB 正文")]:
    path = root / ".perf-local" / "perf-data" / filename
    if path.exists():
        items = grouped(path).get(("vel-text", "ascii-1gb", "batch_uniform_100000"))
        if items:
            add(f'| {label} | {summary(items)["median_ns"]/1e6:.3f} |')
add(f'| 最终：ASCII 已证明边界与 LF 表验证 | {ns("batch_uniform_100000")/1e6:.3f} |')
add()
add("还尝试过 Arc<str> 持有正文：2 KiB 正文加引用头跨过 malloc size class，导致额外"
    "空间；将引用头移出正文页后，分散编辑减少了正文页触碰。最终保留普通叶 String，"
    "小替换引用原叶 Arc，不复制正文，不产生历史链。UTF-16 汇总改为 scalar 数加四字节"
    "首字节数；流式 UTF-8 校验改用 simdutf8。以上没有在库内引入 unsafe。")
add()
profile = root / ".perf-local" / "perf-data" / "perf-profile.jsonl"
if profile.exists():
    add("早期隔离诊断（2 KiB 分块，尚未采用最终 LF 索引）如下；用于定位读取和分配成本，"
        "不替代最终 4 KiB 实现的验收数据。")
    add()
    add("| 诊断 | wall median ms | CPU median ms |")
    add("|---|---:|---:|")
    for (_, _, op), items in grouped(profile).items():
        item = summary(items)
        add(f'| {op} | {item["median_ns"]/1e6:.3f} | {item["median_cpu_ns"]/1e6:.3f} |')
    add()
add("最终未达标项为：" + ("、".join(failures) if failures else "无") + "。"
    "1 GB 载入仍包含文件读取、正文分配、LF 索引分配与组树；本轮将 UTF-8 校验和叶构建并行化，按序回收块，提供 from_reader_serial 无线程入口。均匀编辑还需复制受影响的"
    "内部孩子数组、分配新的叶节点并维护 Arc 引用计数。CPU/缺页计数见原始数据。"
    "8 worker / 4 MiB 块的初轮测得 202.45 ms，但载入结束 RSS 约 1.124×，超出原有 +10% 存储预算；8 worker / 1 MiB 块为 437.95 ms；4 worker / 2 MiB 块探索值为 263.51 ms、RSS 1.098×。这些是不同时间段的诊断，最终验收只看上表。随后交错 A/B 发现工作线程分配 Arc 节点时前 100k 行 batch 有回退，因此把 Arc 分配移回读取线程，并串行构建前 4 MiB 保留开头行的索引局部性；普通叶的 chunk 遍历跳过小补丁分片检查。最后缓存重复插入字符串的汇总，并跳过空删除片段，消除批量路径的重复扫描。"
    "未通过的预算不作为已完成验收。")
add()
add("## 相对上一轮的 5% 回退门禁")
add()
add("参考值来自上一轮已交付 perf-summary.json；不是同一时间段的 A/B。所有 >5% 项仍须按工程规则复测，不能用绝对预算通过替代回退门禁。")
add()
REFERENCE_NS = {'byte_char': 2.960959, 'char_byte': 0.943708, 'byte_utf16': 2.497459, 'utf16_byte': 0.989875, 'byte_line': 354.340333, 'line_byte': 336.076125, 'byte_point': 404.121417, 'byte_point_utf16': 373.224291, 'point_byte': 328.315833, 'point_utf16_byte': 348.518584, 'clone': 3.75041, 'insert': 1514.4417, 'delete': 1164.4625, 'batch_lines_10000': 487375.0, 'sequential_lines_10000': 9701584.0, 'batch_lines_100000': 5063209.0, 'sequential_lines_100000': 91921041.0, 'batch_uniform_100000': 8515250.0, 'sequential_uniform_100000': 19477333.0, 'chunks': 3869958.0, 'lines': 808569834.0}
regressions=[]
add("| 操作 | 上轮 median | 本轮 median | 变化 |")
add("|---|---:|---:|---:|")
for operation, previous in REFERENCE_NS.items():
    current=ns(operation)
    delta=(current/previous-1)*100
    if delta>5:
        regressions.append(operation)
    add(f"| {operation} | {previous:,.3f} ns | {current:,.3f} ns | {delta:+.2f}% |")
add()
add("超过 5% 的项目：" + ("、".join(regressions) if regressions else "无") + "。")
add("并行装载的早期版本曾在交错 A/B 中使前 100k 行 batch 从 5.065 ms 变成 6.311 ms；最终改为读取线程按序分配 Arc 节点，前 4 MiB 串行保留索引局部性。早期 A/B 仅用于诊断，不代替本次最终 pass；负载不可读，不能把剩余差异直接归因于其他代理。")
add()
add("## 复现与产物")
add()
add("```sh\ncargo bench -p vel-text --bench perf -- --runs 5 > crates/vel-text/perf-final.jsonl\n"
    "python3 crates/vel-text/benches/report.py crates/vel-text/perf-final.jsonl\n```")
add()
add(f"最终原始记录：[perf-final.jsonl]({raw_path.name})；完整统计：[perf-summary.json](perf-summary.json)。"
    "探索阶段记录在 gitignored 的 .perf-local/perf-data/，本报告只用最终 pass 做预算验收。")
add()
add("验证：cargo fmt --all；cargo clippy -p vel-text --all-targets -- -D warnings；"
    "cargo test -p vel-text（debug 与 release）；--no-default-features；"
    "RUSTDOCFLAGS=-D warnings cargo doc -p vel-text --no-deps。本轮增加了非 Send reader、多个在途块后的 I/O/UTF-8 错误、2 MiB Unicode 分界与串行路径一致性测试。另以 PROPTEST_CASES=1024"
    "运行 release 模型测试，包含生产叶大小集成测试。")
serialized = [{"implementation": key[0], "corpus": key[1], "operation": key[2], **value}
              for key, value in stats.items()]
for name, data in [("PERF.md", "\n".join(rows) + "\n"),
                   ("perf-summary.json", json.dumps(serialized, indent=2, ensure_ascii=False) + "\n")]:
    path = root / name
    if path.exists():
        backup = root / ".perf-local" / "backups" / name
        backup.parent.mkdir(parents=True, exist_ok=True)
        backup.write_bytes(path.read_bytes())
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(data, encoding="utf-8")
    temporary.replace(path)
print("未达标：" + ("、".join(failures) if failures else "无"))
