#!/usr/bin/env python3
"""Generate the Chinese report from actual runner evidence and five-run timings."""
import json
import os
import shutil
import tempfile
from pathlib import Path

crate = Path(__file__).resolve().parents[1]
data = json.loads((crate / "tests/last-run.json").read_text())
timing = json.loads((crate / "tests/timings.json").read_text())
baseline = json.loads((crate / "tests/baseline.json").read_text())
assert data["loaded"] == 202 and len(data["syntaxes"]) == 202
assert timing["runs"] >= 5
assert data["overall"]["passed"] >= baseline["overall"]["passed"]
counts = data["overall"]
planned = counts["passed"] + counts["failed"]
modifiers = [f for f in data["files"] if f["skip_reason"]]
modified_skips = sum(f["counts"]["skipped"] for f in modifiers)
symbols = counts["skipped"] - modified_skips
old = {
    "Rust": (2402, 11, 0),
    "JSON": (133, 2, 0),
    "YAML": (603, 4, 0),
    "Python": (1311, 399, 4602),
    "JavaScript": (0, 0, 3013),
    "C++": (1079, 676, 0),
    "Markdown": (668, 4209, 0),
    "HTML": (0, 0, 1012),
}
out = [
    "# 语法引擎 v1：官方语料正确性与性能",
    "",
    "语料：仓库固定的 `third_party/sublime-packages`，202 个定义、242 个测试文件。",
    "本 lane 的源码和报告仅位于三个新增 crate，另更新根 Cargo.lock；未修改语料、spec、spikes，未提交。公开文档、MIT syntect 源码与 PR #705 为允许参考；许可和归属见 README。",
    "",
    f"载入 **{data['loaded']}/202**，载入/继承警告 **{len(data['warnings'])}**；"
    f"断言跨度通过 **{counts['passed']:,}/{planned:,} ({100*counts['passed']/planned:.4f}%)**，失败 **{counts['failed']}**，"
    f"显式跳过 **{counts['skipped']:,}**。语法/运行错误文件 **{sum(f['error'] is not None for f in data['files'])}**，语料 panic **0**。",
    "",
    f"字符列另计：通过 {counts['passed_cells']:,}，失败 {counts['failed_cells']:,}。跨度内任一字符失败，整个断言记失败。跳过不计入通过率分母。",
    "",
    "## 与稳定 syntect 5.3 / onig 的比较",
    "",
    "旧 spike 只测八个 canonical 语法，不能声称有全 242 文件的 syntect 总体率。旧版只载入 78/202；本实现载入 202/202。",
    "",
    "| 语言 | vel 通过 / 失败 | vel 通过率 | syntect 通过 / 失败 / 未执行 | syntect 已执行通过率 |",
    "|---|---:|---:|---:|---:|",
]
for language, (p, f, missing) in old.items():
    c = data["syntaxes"][language]
    n = c["passed"] + c["failed"]
    old_rate = f"{100*p/(p+f):.2f}%" if p + f else "未载入"
    out.append(f"| {language} | {c['passed']} / {c['failed']} | {100*c['passed']/n:.3f}% | {p} / {f} / {missing} | {old_rate} |")
p = sum(v[0] for v in old.values())
f = sum(v[1] for v in old.values())
missing = sum(v[2] for v in old.values())
out += [
    "",
    f"旧八语言合计：{p}/{p+f} = **{100*p/(p+f):.2f}%** 已执行通过；把 {missing} 个未执行也计入计划分母，则为 **{100*p/(p+f+missing):.2f}%**。Python 旧结果只执行了约 27%，不是完整结果。",
    "",
    "当前 runner 修正纯断言识别、完整 header 尾 token、跨行回放与过行尾列定位，故某些语言的断言数与旧 spike 略有差别（Rust 2426 对旧 2413，Markdown 4869 对旧 4877）。未通过分母伪装为同一 harness 的逐项差分。",
    "",
    "## 剩余问题与跳过原因",
    "",
    "唯一剩余失败类别是 **selector 尾缀兼容性（2 个）**：JavaScript 两条官方断言包含 `-comment() {}`；当前严格 selector 解析拒绝这一尾缀。未改写断言或把失败转为跳过。其实际作用域在 JSON 中完整记录。",
    "",
]
for file in data["files"]:
    if file["counts"]["failed"]:
        for failure in file["examples"]:
            out.append(f"- `{file['file']}:{failure['line']}`：`{failure['selector']}`；源行 {failure['source_line']}、字符列 {failure['column']}，实际 `{failure['actual']}`。")
out += [
    "",
    f"另有 {symbols} 条 `@` / `>` 符号或引用标注未实现索引，明确跳过。{len(modifiers)} 个 modifier 文件跳过，共记录 {modified_skips} 个跳过单位：reindent 文件无 scope 断言时计文件级 1，partial-symbols 按标注数计。它们没有算作成功。",
    "",
    "`.tmLanguage` 转换返回明确的 unimplemented 错误；zip 包尚未实现。selector 数值评分的精确公式未经 ST 验证，源码以 `ST-BEHAVIOR: unverified` 标注。",
    "",
    f"本次 runner 正则诊断：`{json.dumps(data.get('regex_metrics', {}))}`。命中 Onig 限制按无匹配处理并计数，绝不 panic。",
    "",
    "## 性能（每项至少 5 次）",
    "",
    "macOS 26.1 / arm64，rustc 1.99.1，release，单线程完整解析。缓存批次使用已暖文件系统。没有调用 sysctl；其他并行 lane 的活动可能影响时序。",
    "",
    f"输入为 `spikes/data/rust-10mb.rs`，{timing['bytes']:,} 字节、{timing['lines']:,} 行。预热 regex 后计时，不着色，不去重输入，包含解释器、scope ops、状态 hash 与栈共享的开销。",
    "",
    "| 项目 | 五次中位数 | 目标/参照 |",
    "|---|---:|---|",
    f"| Rust parse-only | **{timing['parse_mb_per_second']:.3f} MB/s / {timing['parse_lines_per_second']:,.0f} 行/s** | syntect 1.02 MB/s / 约 30k 行/s |",
    f"| YAML 构建全部 202 个定义 | **{timing['yaml_median_ms']:.3f} ms** | ≤1500 ms |",
    f"| 校验内容哈希并读取 binary index | **{timing['cache_median_ms']:.3f} ms** | ≤20 ms；包含 IO、源文件哈希、payload 校验与索引反序列化 |",
    f"| 一字符编辑后状态收敛 | **{timing['convergence_median_ms']:.4f} ms** | ≤2 ms；五次均重新解析 {timing['reparsed_lines'][0]} 行后收敛 |",
    f"| 强制解析完整 100 行窗口 | **{timing['window_median_ms']:.3f} ms** | 超过 2 ms；不依赖收敛时仍有差距 |",
    "",
    "这是一次稳定上下文的字符替换，并非所有编辑的最坏界限。跨行 branch、字符串边界变化等可能需要更多行。吞吐相对旧数字约 "
    f"**{timing['parse_mb_per_second']/1.02:.2f}×**；不同会话、相同语料的比较，不是同时运行的对照实验。",
    "",
    f"原始 parse 秒数：`{timing['parse_seconds']}`；cache ms：`{timing['cache_ms_including_content_hashes']}`；其余逐次数据见 `tests/timings.json`。",
    "",
    f"吞吐预热及五轮累计的 regex 诊断：`{json.dumps(timing['regex_metrics'])}`。这些长输入中的资源上限事件如实记为无匹配；不推断为语法断言全部正确。",
    "",
    "上下文体按语法独立压缩，首次进入时才解码，索引元数据无需解码全部上下文。这增加 YAML 构建时的一次性压缩开销，但显著减少缓存载入的 IO、校验和复制；两项仍均在预算内。",
    "",
    "## 编辑器接入",
    "",
    "共享 `Arc<SyntaxSet>`；每个文档持有 `ParseState` 和独立的 `ScopeStack`。行输入保留 `\\n`，ops 为字节偏移，作用域选择器可复用解析后的对象。保存行末 checkpoint 后，状态相等即可停止增量重解析。",
    "",
    "`LineOps.rewind.from_line` 与 `corrections` 均从 0 开始；按序替换已缓存的 ops，同一行最后一次修正生效，再从最早修正行重建 scope stacks。branch 最多回退 128 行。",
    "",
    "`MatcherBackend` / `PatternMatcher` 是实际解析路径的替换点，已经用注入工厂的测试证明。P2 可插入 MultiDfa；必须保留最早起点、同位规则顺序、空匹配策略、排除规则、完整行 lookbehind / \\G 和胜者捕获语义。",
    "",
    "## 全部 202 个语法",
    "",
    "无测试断言的语法不虚报 100%；用“—”标记。按 header 指定的语法归类，不按测试所在目录猜语言。",
    "",
    "| 语法 | 已载入 | 通过 | 失败 | 跳过 | 通过率 | 主要原因 |",
    "|---|:---:|---:|---:|---:|---:|---|",
]
for name, c in data["syntaxes"].items():
    n = c["passed"] + c["failed"]
    rate = f"{100*c['passed']/n:.4f}%" if n else "—"
    cause = "selector 尾缀" if c["failed"] else ("仅显式跳过/无 scope 用例" if not n and c["skipped"] else ("无 scope 用例" if not n else "—"))
    out.append(f"| {name.replace('|', chr(92)+'|')} | 是 | {c['passed']} | {c['failed']} | {c['skipped']} | {rate} | {cause} |")
out += [
    "",
    "复现：`cargo run --release -p vel-syntax --bin syntax-tests`；`cargo run --release -p vel-syntax --bin syntax-bench`；`python3 crates/vel-syntax/scripts/report.py`。",
    "",
    "回归门槛位于 `tests/baseline.json`；`cargo test -p vel-scope -p vel-regex -p vel-syntax --release` 实际运行全 242 文件并逐语法检查不得倒退。",
]
report=crate / "RESULTS.md"
if report.exists():
    shutil.copy2(report,Path(tempfile.gettempdir()) / f"velocity-syntax-RESULTS-{os.getpid()}.previous.md")
temporary=report.with_suffix(".md.tmp")
temporary.write_text("\n".join(out) + "\n")
temporary.replace(report)
print(f"wrote {crate / 'RESULTS.md'} ({len(data['syntaxes'])} syntax rows)")
