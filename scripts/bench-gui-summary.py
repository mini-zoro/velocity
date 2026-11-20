"""Summarize successful native benchmark records; missing support is not zero."""
import json
import math
from pathlib import Path
import statistics
import sys

SOURCE = "[Peeknext](https://peeknext.com/benchmarks/) (M2 Pro)"
GPU = "[Sublime HQ](https://www.sublimetext.com/blog/articles/hardware-accelerated-rendering) (RX560 / 2080 Ti)"


def quantile(values, p):
    values = sorted(values)
    rank = (len(values) - 1) * p
    lo = math.floor(rank)
    return values[lo] + (values[math.ceil(rank)] - values[lo]) * (rank - lo)


def records(root, name, kind, phase=None):
    for n in range(1, 6):
        path = root / f"{n}-{name}.jsonl"
        if not path.is_file():
            continue
        try:
            rows = [json.loads(line) for line in path.read_text().splitlines()]
        except ValueError:
            continue
        if not any(row.get("complete") is True for row in rows):
            continue
        matches = [row for row in rows if row.get("kind") == kind
                   and (phase is None or row.get("phase") == phase)]
        if len(matches) == 1:
            yield matches[0], path.with_suffix(".uptime").read_text().strip()


def row(root, label, name, kind, extract, public="not published",
        comparable="not comparable", unit="ms", phase=None, budget=None):
    values = [(extract(entry), load) for entry, load in records(root, name, kind, phase)]
    values = [(pair, load) for pair, load in values if pair and None not in pair]
    n = len(values)
    if n:
        p50, p95 = zip(*(pair for pair, _ in values))
        # For a repeated distribution (typing/scroll), use median of per-process
        # quantiles. For a single observation per process, report across processes.
        p50 = statistics.median(p50)
        p95 = quantile(p95, 0.95) if kind in {"startup", "memory"} else statistics.median(p95)
        load = "; ".join(x.split("load averages: ")[-1] for _, x in values)
        measured = f"{p50:.3f} / {p95:.3f} {unit} ({n}/5; LA: {load})"
        verdict = "Sublime parity unverified"
        if budget is not None and n == 5:
            ok = all(x is None or a <= x for a, x in zip((p50, p95), budget))
            verdict = ("Velocity budget met" if ok else "Velocity budget missed") + "; parity unverified"
    else:
        measured, verdict = "unavailable (0/5)", "not verified"
    if 0 < n < 5:
        verdict = "incomplete; not verified"
    print(f"| {label} | {public} | {measured} | n/a | {comparable} | {verdict} |")


def pair(entry, field, scale=1):
    value = entry.get(field)
    return None if value is None else (value / scale,) * 2


def dist(entry, field):
    value = entry.get(field)
    return None if value is None else (value["p50"], value["p95"])


def main(root):
    print("| Metric | Sublime public value (source, hardware) | Velocity p50 / p95 | Ratio | Comparability | PLAN verdict |")
    print("|---|---|---|---|---|---|")
    for field, label in [("scanout_main_ms", "Main to first scanout"),
                         ("scanout_process_ms", "Process birth to first scanout")]:
        row(root, label, "startup", "startup", lambda r: pair(r, field),
            "~400 ms cold-start mean (MonoVM, i5-12400)", "similar, not cold or same endpoint")
    for name, label, public in [
        ("typing-sqlite-1", "sqlite3.c, 1 caret", f"18.1 / 30.0 ms pixel change, {SOURCE}"),
        ("typing-sqlite-1000", "sqlite3.c, 1,000 carets", "not published"),
        ("typing-gcc-1", "gcc.c, 1 caret", f"20.2 / 30.4 ms pixel change, {SOURCE}"),
    ]:
        row(root, f"{label}: injection to present-return", name, "typing_summary",
            lambda r: dist(r, "inject_to_present_return"), public, "not comparable", budget=(2, 4))
    sizes = sorted({tuple(r["actual_physical_size"])
                    for r, _ in records(root, "scroll-4k", "startup")})
    label = ", ".join(f"{w}x{h}" for w, h in sizes) or "size unreported"
    exact_4k = sizes == [(3840, 2160)]
    row(root, f"Scroll CPU to submit ({label})", "scroll-4k", "scroll_summary",
        lambda r: dist(r, "cpu_event_to_submit"), budget=(None, 1.5) if exact_4k else None)
    row(root, f"GPU render pass ({label})", "scroll-4k", "scroll_summary",
        lambda r: dist(r, "gpu_render_pass"), f"3 ms full render at 4K, {GPU}",
        "similar; GPU pass only", budget=(None, 3) if exact_4k else None)
    for field, name in [("phys_footprint_bytes", "footprint"), ("rss_bytes", "RSS")]:
        for phase, label in [("after_first_frame", "First frame"), ("after_full_sweep", "Sweep")]:
            row(root, f"{label} {name} (5,000 lines)", "memory-5k", "memory",
                lambda r: pair(r, field, 1e6), unit="MB", phase=phase,
                budget=(None, 80) if name == "footprint" and phase == "after_first_frame" else None)
    for file in ["generated-1m.c", "generated-5m.c", "log-250mb.txt",
                 *(f"line-{n}.txt" for n in (10000, 100000, 1000000, 50000000))]:
        name = f"large-{file}"
        row(root, f"{file}: native large-file exercise to submit", name,
            "large_file_native", lambda r: dist(r, "event_to_submit_ms"))
        for field, kind in [("phys_footprint_bytes", "footprint"), ("rss_bytes", "RSS")]:
            public = {"generated-1m.c": f"259.6 MB Activity Monitor, {SOURCE}",
                      "generated-5m.c": f"1.10 GB Activity Monitor, {SOURCE}"}.get(file)
            row(root, f"{file}: after-benchmark {kind}", name, "memory",
                lambda r: pair(r, field, 1e6), public or "not published",
                "similar; generated bytes and feature policy differ" if public else "not comparable",
                unit="MB", phase="after_benchmark")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
