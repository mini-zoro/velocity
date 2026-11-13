# sregex-like context matcher spike

独立 workspace；所有源码、生成物和 benchmark 输出都在此目录。正式结果见 [RESULTS.md](RESULTS.md)。不声称复现 Sublime 私有引擎，也不把 context 固定扫描的吞吐当成完整高亮器吞吐。

```sh
cd /Users/zoro/dev/velocity/prototypes/sregex
python3 scripts/prepare.py
cargo test --release --locked
cargo build --release --locked
target/release/sregex-prototype inventory
target/release/sregex-prototype run --diff-only --diff-fancy-selected --output output/diff.json
target/release/sregex-prototype run --bench-only --repeat 3 --output output/bench.json
python3 scripts/measure_startup.py
cargo run --release --example semantics > output/semantics.json
cargo run --release --example recheck > output/recheck.json
python3 scripts/report.py
```

可用 `--languages rust,javascript,python,cpp,json,markdown,yaml,html`、`--max-bytes 32768`、`--contexts statements,global` 做定向实验。`--no-fancy` 省略 fancy 编译/执行；`--strict-empty-cores` 关闭纯正向 lookahead 的候选定位优化，直接测试空 core 的朴素算法。`compile` 子命令只收集编译，`memory --languages rust --engine multi` 用新进程测 eager 编译的常驻内存。原始数据不由报告脚本修改。

差分默认以四个 worker 并行不同 contexts（`--diff-jobs 1` 可串行）。吞吐始终单线程且在差分 worker 全部结束后运行。`--diff-fancy-selected` 让 fancy 只对比 benchmark 所选的四个 contexts，B 仍检查所有 ≥5 patterns 的 contexts。`cargo run --release --example semantics > output/semantics.json` 记录 Unicode word boundary/full case folding 的独立探针。

- `src/loader.rs`：YAML 1.2、递归变量、同目录继承、`meta_prepend`/`meta_append`、按优先级 include 展开、匿名 contexts、环检测。
- `src/frontend.rs`：保守的 Onig 方言翻译、regex-syntax HIR 可编译性检查和边界 lookahead 拆分。无法证明安全拆分的模式保留 Onig。
- `src/matcher.rs`：A-Onig、A-fancy、每 context 单个多模式 meta regex、边界验证、同位置优先级修复、Onig fallback、赢家 captures、逐行缓存。
- `src/main.rs`：固定 context 逐行 driver、全字段差分、三次 release 基准、启动/内存记录。

差分按完整行内容去重执行，再按频次还原查询数：所有引擎的逐行缓存都会 reset，且 driver 没有跨行状态，所以相同行的全部 query 序列、结果和 captures 完全相同。这是全语料覆盖的确定性压缩，不是抽样。相同 ordered pattern-ID 列表的 contexts 也复用差分结果。性能基准始终逐行读取并处理完整语料，不做这种去重。

缓存保留每个 pattern 的下一次命中或 no-match，只有 `pos` 超过已知 start 才重新搜索；换行清空。包含 `\G` 或 `\K` 的模式每次重新搜索，避免把依赖 query 起点的结果错误复用。保留完整行和 byte offset 给 regex 引擎；空匹配后若 `pos+1` 落在 UTF-8 字符中间，向上取整到下一个字符边界。

纯正向 lookahead 的 core 可能为零宽，直接放入 union 会遮住所有后续规则。因此默认将其某个正向 guard 的 body 用作同一个 DFA 的候选 start 定位式，再把 end 还原为零宽 core 的 end，仍 anchored 验证全部 guard。其他边界 lookahead 放入去除 guard 的 core。验证失败时按 context 顺序在同一 start 做原始 Onig anchored 检查，处理低优先级规则、core 缩短和 alternative 回溯；都失败才推进一个字符。完整 `LeftmostFirst` 的 pattern ID 映射回原始 context 顺序后，与 Onig fallbacks 合并。

没有语法 context switching、prototype 注入、跨文件 includes、embed escape、branch/fail、动态回溯引用替换或 `with_prototype` 传播。同目录 `extends` 用来完整加载 JavaScript/HTML 的 Plain 基类，不属于跨文件 include。指向其他语法的 includes 被跳过并记录；继承路径中的 scope 和跨目录路径会明确报错。动态 `\1`/`\2` 在脱离 runtime context 后可能无法独立编译，原始错误逐条保留，所有 matcher 一致排除这些规则。

> Note: the large raw outputs (`output/bench.json`, `output/diff.json`, `output/compile-*.json`, smoke runs,
> inventory) are not committed to keep the repository small; rerun the scripts above to regenerate them.
> All numbers quoted in `RESULTS.md` come from those runs.
