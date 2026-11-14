#!/usr/bin/env python3
"""Generate the Chinese performance report from unmodified five-run observations."""
import hashlib
import json
import math
import shutil
import statistics
import tempfile
from pathlib import Path

CRATE = Path(__file__).resolve().parents[1]
ROOT = CRATE.parents[1]


def read(path):
    return json.loads(path.read_text())


def median(values):
    assert len(values) >= 5
    return statistics.median(values)


def p95(values):
    return sorted(values)[math.ceil(len(values) * .95) - 1]


def main():
    throughput = read(CRATE / "throughput.json")
    profile = read(CRATE / "contexts.json")
    focused = read(CRATE / "focused-contexts.json")["timings"]
    cold = read(CRATE / "compile.json")["runs"]
    differential = read(CRATE / "differential.json")
    latest_large = read(CRATE / "large-differential.json")
    corpus = read(ROOT / "crates/vel-syntax/tests/baseline.json")
    highlight = read(ROOT / "crates/vel-highlight/timings.json")
    assert not throughput["quick"] and not throughput["instrumented"]
    assert profile["quick"] and profile["instrumented"]
    assert len(cold) >= 5
    official, large = differential["official"], latest_large["large_diff"]
    assert len(official) == 242 and len(large) == 8
    assert all(row["divergences"] == 0 for row in official + large)
    compile_runs = [run["multi"]["compile_ms"] for run in cold]
    compile_times = [value for run in compile_runs for value in run]
    comparisons = []
    for name, reference in profile["contexts"]["onig"].items():
        candidate = profile["contexts"]["multi"].get(name)
        if not candidate or not all(reference["queries"]) or reference["queries"] != candidate["queries"]:
            continue
        if not all(candidate["nanos"]):
            continue
        ratio = median(reference["nanos"]) / median(candidate["nanos"])
        comparisons.append((ratio, name, reference, candidate))
    comparisons.sort()
    worst = comparisons[0]
    stable = [row for row in comparisons if min(row[2]["queries"]) >= 100]
    background = [row["seconds"] for row in highlight if row["case"] == "full_10mb_rust_background"]
    core = [row["seconds"] for row in highlight if row["case"] == "full_10mb_rust"]
    edits = [sample for row in highlight if row["case"] == "worker_edit_10k_visible_100" for sample in row["samples"]]
    full_mb = 10 / median(background)
    ratios = [median(row["onig_seconds"]) / median(row["multi_seconds"]) for row in throughput["timings"]]
    rows = [
        "# Lane B，第五轮：生产多模式匹配与 parser 热路径",
        "",
        "本报告由 scripts/report.py 从原始 JSON 生成。未提交 git commit；未调用 sysctl。",
        "",
        "## 预算结果",
        "",
        "| 指标 | 本轮实测 | 目标 |",
        "| --- | ---: | --- |",
        f"| Rust 10 MB，实际后台 parse+style | {full_mb:.3f} MB/s；中位 {median(background):.6f} s / P95 {p95(background):.6f} s | ≥2.5 MB/s |",
        f"| 八语言独立 backend 加速 | 最低 {min(ratios):.3f}×；{sum(ratio >= 2 for ratio in ratios)}/8 达到 2× | 每种 ≥2× |",
        f"| 最差已测 context | {worst[1]}：{worst[0]:.3f}× | ≥1× |",
        f"| 冷进程首次 context 编译 | pooled P50 {statistics.median(compile_times):.4f} ms / P95 {p95(compile_times):.4f} ms | P95 ≤2 ms |",
        f"| 八语料完成后的 DFA/NFA/cache 实际驻留 | {throughput['multi']['resident_bytes']/2**20:.3f} MiB；峰值 {throughput['multi']['peak_bytes']/2**20:.3f} MiB | ≤32 MiB |",
        f"| edit → exact visible batch | 500 samples P95 {p95(edits):.5f} ms | ≤0.2 ms |",
        "",
        "预算按实测逐项判断；表中未达标项仍是未完成的性能目标，不能用总体平均值替代。",
        "",
        "## 八语言吞吐：同一 parser，关闭双方的稳定行 memoization",
        "",
        "release，Darwin arm64 / macOS 26.1，rustc 1.99.1；单线程；每引擎先完整预热一次，随后交替先后顺序执行五次。",
        "计时包含真实 context 切换、prototype/embed、branch replay、regex 搜索、赢家 captures 和 LineOps；排除文件 I/O、YAML 加载和预热编译。",
        "双方都使用 parse_line_into 重用输出；双方都关闭稳定行 memoization，以隔离 matcher 差异。后台服务则开启默认 memoization，不能混称同一指标。",
        "P95 为 nearest rank；五次样本的 P95 等于其中最大值。其他 lane 可能活动，未删除噪声或异常值。",
        "",
        "| 语言 | MB | 新 backend MB/s | Onig MB/s | 加速 | 新 median / P95 s | Onig median / P95 s |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in throughput["timings"]:
        a, b, mb = row["onig_seconds"], row["multi_seconds"], row["bytes"] / 1e6
        rows.append(f"| {row['language']} | {mb:.3f} | {mb/median(b):.3f} | {mb/median(a):.3f} | {median(a)/median(b):.3f}× | {median(b):.6f} / {p95(b):.6f} | {median(a):.6f} / {p95(a):.6f} |")
    rows += [
        "",
        "## Context 剖析与原型慢路径",
        "",
        f"独立的 instrumented run 使用每语言最前约 200 KB，五次完整 parser 流程，检查 {len(comparisons)} 个具有五次相同 query 数的 context。",
        "每次 context 调用用 Instant 单独累计；不把这组带计时开销的数据当成完整文件吞吐。低频 context 的固定计时/锁开销占比可能很高，仍保留其最差值。",
        f"每次至少 100 queries 的 {len(stable)} 个 context 中，最差为 {stable[0][1]}，{stable[0][0]:.3f}×。",
        "",
        "| context | 每次 queries | 新 median ms | Onig median ms | 加速 |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    selected = comparisons[:10]
    for item in comparisons:
        if item[1] in ("Rust::statements", "C++::global") and item not in selected:
            selected.append(item)
    for ratio, name, a, b in selected:
        rows.append(f"| {name} | {a['queries'][0]} | {median(b['nanos'])/1e6:.6f} | {median(a['nanos'])/1e6:.6f} | {ratio:.3f}× |")
    rows += ["", "另对原型点名的两个 context 做独立固定扫描：每种前 200 KB、完整 C/C++ includes、五次、先逐 query 核对 captures；不执行 parser context 切换或自动 prototype 注入。这组数据不冒充整文件 parser 吞吐。", "",
        "| 固定 context | 每次 queries | 新 median / P95 ms | Onig median / P95 ms | 加速 |",
        "| --- | ---: | ---: | ---: | ---: |"]
    for row in focused:
        a, b = row["onig_seconds"], row["multi_seconds"]
        rows.append(f"| {row['language']}::{row['context']} | {row['queries'][0]} | {median(b)*1000:.4f} / {p95(b)*1000:.4f} | {median(a)*1000:.4f} / {p95(a)*1000:.4f} | {median(a)/median(b):.3f}× |")
    rows += [
        "",
        "原型在 Rust statements 的主要问题是 Unicode 边界/PikeVM、重复 guard matcher 和失败后的逐规则 anchored 修复。",
        "本轮使用 ASCII hybrid DFA 和 exact Onig Unicode fallback；边界小谓词直接判断，独立 lookahead 绝不顺序消费；纯正向、无 captures 的 lookahead 在自动机定位后直接还原零宽 span；没有有效定位式的 epsilon 候选留给 Onig。拒绝候选后仅按原始顺序修复该 query。",
        "C++ 的全局候选包含不规则 lookaround 时，先生成保守候选，再用原式核实；固定捕获布局可直接生成 inline spans。",
        "DFA 与 fallback 先比较 span/priority，再为最终赢家物化 captures；query 起点的赢家不再搜索不可能抢先的低优先级 fallback。",
        "少于四条规则的上下文直接建立原 Onig matcher，避免 DFA 与额外 dispatch 开销。其他 context 仍可能不及 Onig；上表完整披露。可选 adaptive 会比较 64 个 warm queries，但短前缀会误判后续流量，默认关闭；未将其不同工作量混入主表。",
        "共享机器争用时注册/搜索使用 try_lock 并退回 Onig，避免一个后台文档阻塞另一个文档的前台高亮；有专门的并发回归测试。",
        "",
        "## Lazy 编译与内存",
        "",
        "五次独立进程各从新 SyntaxSet 开始，依次进入八语言的前 64 KiB 完整行。计时覆盖前端、NFA、forward/reverse DFA 与 literal set；只编译实际进入的 context。",
        "Onig 赢家/不支持规则的单模式编译仍按需发生，其成本不冒充 DFA 编译。编译数组含未能建立 DFA 而选择 fallback 的尝试。",
        "",
        "| 独立进程 | contexts 尝试 | compile P50 / P95 ms | 首次编译合计 ms | peak RSS MiB |",
        "| --- | ---: | ---: | ---: | ---: |",
    ]
    for index, run in enumerate(cold):
        times = run["multi"]["compile_ms"]
        rss = f"{run['rss_bytes']/2**20:.3f}" if run["rss_bytes"] else "未测"
        rows.append(f"| {index+1} | {len(times)} | {statistics.median(times):.4f} / {p95(times):.4f} | {sum(times):.3f} | {rss} |")
    rows += [
        "",
        f"主吞吐进程：{throughput['multi']['pattern_sets']} 个 ordered lists，{throughput['multi']['compilations']} 次 lazy 尝试，{throughput['multi']['evictions']} 次 LRU 驱逐。",
        "32 MiB 限额覆盖整个 backend 的所有语言，包含 forward/reverse NFA、DFA transition caches 和 literal automata；共享列表不重复持有机器。",
        "SearchCache/checkpoint 不持有编译机器，因而不会阻止驱逐。小内存配置、LRU 重编译、共享与并发均有测试。",
        "API memory 不包含源模式/HIR、Onig、allocator、语法索引、parser/checkpoints 或短暂编译工作区。RSS 由 wait4/rusage 单独读取每个子进程，包含进程与冷前缀解析，不能和纯 DFA 预算混为一谈；没有调用被 sandbox 禁止的 sysctl。",
        "未增加磁盘 DFA cache；编译延迟和内存证据保留在 compile.json / throughput.json。",
        "",
        "## Parser 热路径与服务",
        "",
        "parse_line_into 复用 operations/corrections；Region 与常见 capture ops 使用 inline storage；删除了每 token 未使用的 MatchOperation clone。",
        "稳定、非 speculative 的 (完整输入行，语义起始状态) → (结束状态，interned operations) 按文档缓存，最多 4096 条及 4 MiB 源文本；hash 命中仍核对完整文本与状态。",
        "cache 不参与 checkpoint 的语义相等性；命中不会丢失绝对行计数，下一次真实解释前恢复行号；rewind/corrections/warnings 不缓存。",
        "分支 replay、动态捕获、Unicode/换行编辑、缓存 suffix 收敛、scheme 切换及跨行修正均保留测试。",
        "一个含捕获的稳定流式 fixture，在关闭 memoization 和启用 memoization 两种模式下，各 10,000 行均为 0 次 Rust allocation；这不测 Onig 的 C allocator，也不声称冷 miss、分支快照、styled spans 和返回 batch 永不分配。",
        "",
        f"Rust 10 MB core parse+style/checkpoints：中位 {median(core):.6f} s / P95 {p95(core):.6f} s。",
        f"实际 worker：中位 {median(background):.6f} s / P95 {p95(background):.6f} s；对比上轮 7.444 s 的中位数为 {7.444/median(background):.3f}×。",
        f"稳定编辑服务 500 samples P95 {p95(edits):.5f} ms，最坏 {max(edits):.5f} ms。计时包括 ChangeSet 提交与获得 exact 100-line viewport；文本替换在计时外。",
        "该编辑场景是稳定 token 编辑，不代表任意多行 delimiter 修改；单个病态 Onig 调用仍不可抢占，UI 请求沿用非阻塞降级协议。",
        "",
        "## 差分与单调 baseline",
        "",
        f"官方：{len(official)} 文件、202 语法，{sum(row['lines'] for row in official):,} 行 / {sum(row['bytes'] for row in official):,} bytes，{sum(row['rewinds'] for row in official):,} 次 rewind。",
        f"大文件：{sum(row['lines'] for row in large):,} 行 / {sum(row['bytes'] for row in large):,} bytes，{sum(row['rewinds'] for row in large):,} 次 rewind。均为 0 divergences，无抽样、无文件跳过。",
        "诊断模式逐 query 核对 pattern index、span、所有 capture slots；逐行核对完整 LineOps、corrections、rewind 和 warnings。",
        "额外 raw-Onig 探针修复了原前缀过滤器的 x-mode class 空白差异和 Rust-only class 操作符差异；精确 DFA 赢家先验证原 Onig 源确实可编译。终端换行后的 ^（包括内部 ^）保留 Onig 语义。HIR 可消去 {0} 分支，输出仍按 Native 元数据保留全部未参与 capture slots。",
        "官方文本使用与 assertion harness 相同的 CR 归一化；大文件原样解析。单元测试另覆盖 CRLF、空串、终端换行、exclusions、nonempty、独立 lookahead、短串穷举、\u00b2\u00b3\u00b9\u00bc\u00bd\u00be 与 join controls、full folding、search-start 和自定义 regex 限制。",
        "整个官方 release differential 还把启用行 memoization 的结果与 uncached Onig 比较，避免缓存掩盖底层差异。",
        "",
        f"基线：scopes **{corpus['scope_counts']['passed']:,}/{corpus['scope_counts']['failed']}**，symbols **{corpus['symbol_counts']['passed']}/{corpus['symbol_counts']['failed']}**，reindent **{corpus['reindent_counts']['passed']}/{corpus['reindent_counts']['failed']}**，零 skips。",
        "partial-symbols 文件照常评估 scopes；selector 仍使用最长可解析前缀通则，没有特定 suffix 或文件名例外。",
        "本轮修复两种 PHP 变体、Bash case 与 Bash heredoc。一般规则：完整同一行 body 不再延续缩进；preserveIndent 选取前导实际代码而非后方 heredoc；metadata increase 不再错误套用外层括号的基准；clause terminator 使用退出前的作用域，case 结束恢复对应层级。",
        "",
        "| 剩余文件 | variant | 首处不同的行 |",
        "| --- | --- | ---: |",
    ]
    for file in corpus["files"]:
        for failure in file["examples"]:
            if failure["selector"].startswith("reindent"):
                rows.append(f"| {file['file']} | {failure['selector']} | {failure['line']} |")
    rows += [
        "",
        "详细差异保留在 vel-syntax/tests/baseline.json 和 RESULTS.md；所有已通过文件均维持单调。",
        "",
        "## 复跑与来源",
        "",
        "```sh",
        "cargo fmt -p vel-regex -p vel-syntax -p vel-highlight --check",
        "cargo clippy -p vel-regex -p vel-syntax -p vel-highlight --all-targets -- -D warnings",
        "cargo test --release -p vel-regex -p vel-syntax -p vel-highlight",
        "RUSTDOCFLAGS='-D warnings' cargo doc -p vel-regex -p vel-syntax -p vel-highlight --no-deps",
        "python3 crates/vel-regex/scripts/measure.py",
        "python3 crates/vel-regex/scripts/report.py",
        "```",
        "",
        "官方 Packages commit：3f8b2c544e8f74a76e7212da449dcfe3f01b2245。仅使用公开 permissive 语法与本仓库原型；未读取应用私有实现。",
        "原始 evidence：differential.json、large-differential.json、throughput.json、contexts.json、focused-contexts.json、compile.json，及 vel-highlight/timings.json。",
        "",
        "| 输入 | bytes | SHA-256 |",
        "| --- | ---: | --- |",
    ]
    for row in large:
        path = ROOT / row["file"]
        rows.append(f"| {row['file']} | {path.stat().st_size} | {hashlib.sha256(path.read_bytes()).hexdigest()} |")
    target = CRATE / "PERF.md"
    if target.exists():
        shutil.copy2(target, Path(tempfile.gettempdir()) / "velocity-regex-PERF.previous.md")
    temporary = target.with_suffix(".md.tmp")
    temporary.write_text("\n".join(rows) + "\n")
    temporary.replace(target)


if __name__ == "__main__":
    main()
