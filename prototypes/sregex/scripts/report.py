#!/usr/bin/env python3
"""Render Chinese report from retained, machine-readable measurements."""
import json
import math
import shutil
import statistics
import time
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
OUT=ROOT/'output'
def read(name): return json.loads((OUT/name).read_text())
def langmap(rows): return {s['language']:s for s in rows}
def cell(v): return str(v).replace('|',r'\|').replace('\n',' ')
def table(head,rows):
    return '\n'.join(['| '+' | '.join(map(cell,head))+' |','|'+'|'.join(['---']*len(head))+'|']+
                     ['| '+' | '.join(map(cell,r))+' |' for r in rows])
def fmt(n): return f'{n:,}'
def median(v): return statistics.median(v)

env=read('environment.json');bench=langmap(read('bench.json'));diff=langmap(read('diff.json'))
recheck=read('recheck.json')
assert recheck['unaccounted_mismatches']==0
assert recheck['accounted_mismatches']==sum(c['diff']['mismatches_b'] for s in diff.values() for c in s['contexts'] if c['diff'])==546
comp=[langmap(read(f'compile-{i}.json')) for i in range(3)]
mem={(r['language'],r['engine']):r for r in read('memory.json')}
order=list(bench)
chunks=['# sregex-like 多模式 matcher 原型实测\n']
chunks+=['本报告由 `scripts/report.py` 从 `output/` 原始 JSON 生成；可复跑。所有吞吐是固定 context 的逐行 microbenchmark，尚未运行完整 Sublime/syntect parser，也未测 Sublime 本体。\n']

aggregate=[];aggregate_detail=[];all_speed_onig=[];all_speed_fancy=[];total={k:0. for k in ['onig','fancy','multi']}
for lang in order:
    s=bench[lang];cs=[c for c in s['contexts'] if c['selected']]
    valid=[c for c in cs if all('median_seconds' in c['timings'].get(k,{}) for k in ['onig','fancy','multi'])]
    ts={k:sum(c['timings'][k]['median_seconds'] for c in valid) for k in total}
    for k in total:total[k]+=ts[k]
    bs=s['scanned_bytes']*len(valid)
    for k in total:
        q=sum(c['timings'][k]['queries'] for c in valid)
        aggregate_detail.append([lang,k,fmt(q),f'{ts[k]:.6f}',f'{q/ts[k]:,.0f}',f'{bs/1e6/ts[k]:.2f}'])
    for c in valid:
        t=c['timings'];all_speed_onig.append(t['onig']['median_seconds']/t['multi']['median_seconds']);all_speed_fancy.append(t['fancy']['median_seconds']/t['multi']['median_seconds'])
    aggregate.append([lang,len(valid),f'{bs/1e6:.2f}',*[f'{bs/1e6/ts[k]:.2f}' for k in total],f'{ts["onig"]/ts["multi"]:.2f}×',f'{ts["fancy"]/ts["multi"]:.2f}×'])
chunks+=['## 主要结果\n',f'32 个所选 contexts 的合计时间比：B/A-Onig 加速 **{total["onig"]/total["multi"]:.2f}×**，B/A-fancy 加速 **{total["fancy"]/total["multi"]:.2f}×**。逐 context 的 B/A-Onig 范围 {min(all_speed_onig):.2f}–{max(all_speed_onig):.2f}×，几何平均 {math.exp(statistics.mean(map(math.log,all_speed_onig))):.2f}×；{sum(v>1 for v in all_speed_onig)}/{len(all_speed_onig)} 个 contexts 比缓存后的 Onig 快。这里的合计时间是每个 context 三次完整扫描时间的中位数之和。\n',table(['语言','contexts','累计扫描 MB','A-Onig MB/s','A-fancy MB/s','B MB/s','B/A-Onig','B/A-fancy'],aggregate)+'\n']
chunks.insert(4,'正确性检查覆盖 740 个 contexts，去重实际执行 222,857,936 个 query，对应完整语料×contexts 的 1,970,092,128 个参考 query。546 处差异全部由六种字符的 Unicode word-boundary 语义造成，独立重放已逐 context 全部归因；除此之外成功查询的 spans、pattern priority 和 captures 相同。Markdown 另有七个起点使 A/B 的 Onig 都触发 retry limit，未能验证后续该行查询，不能声称所有输入严格等价。\n')
equivalent={k:0. for k in total};equivalent_count=0
for s in bench.values():
    for c in s['contexts']:
        if not c['selected']:continue
        t=c['timings']
        if all('median_seconds' in t.get(k,{}) for k in total) and len({(t[k]['queries'],t[k]['matches'],t[k]['checksum']) for k in total})==1:
            equivalent_count+=1
            for k in total:equivalent[k]+=t[k]['median_seconds']
chunks+= [f'Rust statements 的完整 trajectory 存在上述已知 Unicode 差异，以上 32-context 全量比值包含它。仅取 query/match/checksum 全部相同的 {equivalent_count} 个 contexts，B/A-Onig 为 **{equivalent["onig"]/equivalent["multi"]:.2f}×**、B/A-fancy 为 **{equivalent["fancy"]/equivalent["multi"]:.2f}×**；这只是等价工作量子集，不能替代全量结果或真实 parser 的 context 频率分布。\n']

chunks+=['## 环境与版本\n',f'{env["hardware"][0]["chip_type"]}，{env["hardware"][0]["physical_memory"]}，{env["hardware"][0]["number_processors"]}；macOS 26.1 / {env["machine"]}。`{env["rustc"]}`；`{env["cargo"]}`。吞吐单线程，release，默认优化，无 `target-cpu=native`，`debug=1` 仅保留符号。差分的 contexts 用四个 worker 并行，吞吐测量时无差分任务运行。\n',
table(['crate','Cargo.lock 版本'],[[n,env['dependencies'][n]] for n in ['regex-automata','regex-syntax','fancy-regex','onig','onig_sys','yaml-rust2','memchr','aho-corasick']])+'\n',
f'语法来源：本机官方 Packages，commit `{env["packages_commit"]}`。直接依赖由本次 `cargo search` 核对 stable，确切依赖锁定在 `Cargo.lock`；环境记录在 `output/environment.json`。\n']

chunks+=['## 方法与覆盖边界\n',
'Loader 展开递归变量、同文件 includes，并保持 ordered priority；环只截断当前递归栈上的边。支持匿名 push/set/branch contexts；支持同目录继承和 `meta_prepend`/`meta_append`，因此加载的是完整 JavaScript/HTML Plain 基类加覆盖。跳过 prototype 自动注入、显式 prototype include、跨文件/`scope:` include、embed 的语法切换及 escape、with_prototype 传播、branch/fail 状态。保留 embed 规则自己的 match。C++ 通过 scope:source.c 引入的 C 规则因此缺席，不能将这里的 C++ 数值当作完整 C++ 高亮器表现。\n',
'A-Onig 为原始 pattern 的 `Syntax::default()`（Ruby）逐条搜索；A-fancy 使用相同 Onig 语法归一化后的 fancy-regex。二者保留每条规则在当前行的下一次 match/no-match，`pos <= cached_start` 时复用；发现当前 pos 的 winner 后跳过低优先级规则。B 使用同样的 fallback cache，另有已验证 union winner 的缓存。`\\G`/`\\K` 规则不复用 search cache。所有引擎都只为最终赢家提取 captures，B 用原始 Onig anchored captures，以完整保留 lookahead 内的 captures。\n',
'Onig 搜索需要 Region 来获取 group 0 的 end，内部仍会维护 captures；这里“只为赢家提取”指不为败者复制/返回全部 capture spans。B 的 union NFA 禁用了 explicit captures，fallback 的 Region 用法与 A 一致。\n',
'B 每 context 一个 `regex_automata::meta::Regex` 多模式 union，`MatchKind::LeftmostFirst`，只保留 implicit group 0，32 MiB NFA limit，16 MiB hybrid cache 上限。meta 是 DFA/NFA/PikeVM 的自适应组合，不能把每次查询都称为纯 DFA；Unicode、容量限制或短文本都可能触发其他执行路径。额外 guard matcher 只做 anchored 验证。\n',
'边界 lookahead 从 core 剥离，分别在 match.start / match.end 验证。失败时按 context priority 在同一 start 做原始 Onig anchored 检查，再决定推进，处理 lower-priority tie、alternative 和更短 core。纯正向 lookahead 的零宽 core 默认用一个 positive guard body 定位候选 start，再还原零宽 end；`--strict-empty-cores` 可关闭此优化。它避免空 core 每字符遮蔽其他规则。\n',
'Driver 为 `pos=0; find(line,pos); pos=max(end,pos+1)`，无匹配换行；保留换行符并传完整行与 byte offset。空匹配后若 pos 落在 UTF-8 字符中，推进到下一个字符边界。benchmark 对每条实际 corpus 行执行，先用最多 64 KiB 预热同一 engine cache，再对完整语料扫描三次取中位数；contexts 间轮换引擎顺序。计时包含缓存 reset、赢家 captures、driver 和 checksum，排除 YAML/I/O/编译。\n',
'全语料差分按完整行去重，按频次恢复查询总数；缓存逐行清空、driver 无跨行状态，所以相同行具有完全相同的所有 query 轨迹。相同 ordered pattern-ID 列表的 contexts 复用同一差分结果。这是确定性压缩覆盖，不是抽样；性能计时不使用去重。差分在 A-Onig 的每个 query 起点对比 `(start,end,pattern_index,全部 capture spans)`，包括 no-match。异常独立统计，不伪装成 no-match 或语义差异。\n']

chunks+=['### 语料\n',table(['语言','MB','来源','SHA-256（前 16 位）'],[[l,f'{bench[l]["scanned_bytes"]/1e6:.3f}',bench[l]['corpus']['source'],bench[l]['corpus']['sha256'][:16]] for l in order])+'\n',
'Rust/JavaScript/JSON/Markdown 使用现有生成语料；其余来自官方 Packages 的 syntax_test 源代码，去除 scope assertion 行，再按完整 seed 重复到至少 5 MB。保留注释和测试中的不完整/错误代码，适合语法高亮场景，但不是自然项目频率分布。完整 hash、seed 和文件清单见 `output/corpora.json`；生成文件在 gitignored `data/`。\n']

classification=[]
for l in order:
    s=diff[l];classes=s['compile']['classes']
    classification.append([l,len(s['contexts']),sum(c['patterns']>=5 for c in s['contexts']),len(s['patterns']),classes.get('Dfa',0),classes.get('Boundary',0),classes.get('Backtracking',0),len(s['compile']['onig_errors'])])
chunks+=['## 分类\n',table(['语言','全部 contexts','≥5 patterns','唯一展开 regex','(a) DFA','(b) 边界 lookahead','(c) fallback','Onig 编译失败'],classification)+'\n',
'计数是去重后的展开 pattern 字符串，包含无法独立编译的动态 backref 模式；并非原始 match 规则次数，也不是全仓库 13,813 条规则的重新统计。各 context 的展开规则数与实际 fast/boundary 数在原始 JSON 中。前端保守地将内部 lookaround、lookbehind、backrefs、subroutine、原子组、possessive quantifier、\\G/\\K、不支持的 POSIX 类/flags 留给 Onig。\n']

correct=[];mismatches=[];errors=[]
for l in order:
    s=diff[l];cs=[c for c in s['contexts'] if c['diff'] is not None];seen=set();actual=0
    for c in cs:
        key=tuple(c['pattern_ids'])
        if key not in seen:seen.add(key);actual+=c['diff']['unique_queries']
    summed=lambda k:sum(c['diff'][k] for c in cs)
    correct.append([l,len(cs),fmt(actual),fmt(summed('represented_queries')),fmt(summed('mismatches_b')),f"{summed('mismatches_fancy')} / {sum(c['diff']['fancy_checked'] for c in cs)} contexts",fmt(summed('oracle_errors')),fmt(summed('b_errors'))])
    for c in cs:
        d=c['diff']
        if d['mismatches_b'] or d['mismatches_fancy']:
            mismatches.append([l,c['name'],d['mismatches_b'],d['mismatches_fancy'],d['categories_b'],d['categories_fancy']])
        if d['oracle_errors'] or d['b_errors'] or d['fancy_errors']:
            errors.append([l,c['name'],d['oracle_errors'],d['b_errors'],d['fancy_errors'] if d['fancy_checked'] else '未测'])
chunks+=['## 差分正确性\n',table(['语言','检查 contexts','实际去重 query','全语料×contexts query','B 不同 query','fancy 差异 / 检查范围','Onig 异常','B 异常（A 成功时）'],correct)+'\n',
'不同 query/异常列按 context×唯一行计，重复语料还原后的差异计数另存 `represented_mismatches_*`。实际去重 query 还消除了具有相同 ordered pattern 列表的 contexts；其余列保留每个 context 的独立覆盖含义。异常后的剩余该行查询无法完整验证，不能据此声称所有输入严格等价。\n']
chunks+=['B 共检查 740 个 ≥5 patterns contexts。fancy 仅检查 benchmark 所选 contexts 及 ordered patterns 完全相同的 contexts，因此 JSON/HTML 的检查数可超过四个；未检查的 fancy context 不计作零差异证明。\n',
'Rust 的全部 546 个 B 差异来自六条含 `²`、`³`、`¹`、`¼`、`½`、`¾` 的源代码行，均为 Unicode word-boundary 导致的 pattern priority 变化。例：`    163, // \'²\'` 的 byte pos=13，A-Onig 的 `\\b(?:|Box|Option|...)\\b` 命中空串，而 Rust 的 `\\b` 不把该字符视作 word，B 找到后面的引号。它们属于已明确识别的 Onig-vs-Rust 语义差异，没有用“Unicode”泛指其他未知错误。`statements` 的 fancy 对照也出现相同六处差异。\n',
'六条源代码行的独立完整 driver 重放，逐 context 的 mismatch 计数与全语料计数完全相等，复现 546 / 未归因 0，记录在 `output/recheck.json`。因此这些语义差异以外，所有成功比较的 query 的 spans、priority 和 captures 均相同。\n',
'Markdown 的七个异常都来自 `markdown` context 的 rule 39 / 全局 pattern gid 258（展开的表格行 lookahead，含多层 inline-code/emphasis 分支），原始 Onig 报 `retry-limit-in-match over`。例如 `let hir = parse("a|b").unwrap();`，pos=0。差分 driver 在 A 报错后结束该行，所以表中 B 异常只统计 A 成功的查询；随后独立复测这七个起点，B 的 Onig fallback 也全部触发相同 retry-limit，详见 `output/recheck.json`。没有改大 retry limit，也未把异常算成 mismatch；这些行后续 query 未能验证。十条动态 context backref 无法独立编译，A/B 一致排除，列在上一节和 JSON 中。\n']
chunks += [table(['语言','context','B mismatch','fancy mismatch','B 原因类别','fancy 原因类别'],mismatches)+'\n' if mismatches else '所有成功比较的 query 都一致；没有 match/span/priority/capture 差异。\n']
if errors: chunks += [table(['语言','context','A-Onig 异常','B 异常','fancy 异常'],errors)+'\n']

chunks+=['### 语法与语义修正\n',
'已处理 `\\h/\\H` 为十六进制字符、Unicode POSIX classes（Rust 原生 POSIX classes 是 ASCII）、Onig `\\w`、`(?x)` 的注释/空格、Onig `(?m)` → Rust `(?s)`、`\\Z`、`{,n}` → `{0,n}`、Ruby `\\</\\>` 为字面尖括号。Onig 的 `^` 不匹配终端换行后的虚拟空行：B 仅对这类边界 candidate 复核 Onig，fancy 用等价的零宽 assertion 改写。Rust 将 `a++` 当作重复量词组合，不能视作 Onig possessive 的等价翻译，明确 fallback。\n',
'仍需统一 Unicode tables、`\\b` 的 word 集合及多字符 case folding：Rust 的 word 集合包括 Join_Control，Onig Ruby 不包括；Onig 的 ignore-case 可有多字符折叠，Rust 通常为 simple folding。仅编译通过并不证明等价。命中这些特征时生产实现应统一字符属性/边界实现或保守 fallback；具体实测差异以本节上表和原始 samples 为准，不能把任意 mismatch 自动归入 Unicode。\n']
chunks+=['独立探针 `output/semantics.json` 还验证了 `\\b` 在 `a<U+200C>b` 上的差异，以及 `(?i:ss)` 在 `ß` 上 A-Onig 成功而 B/fancy 不匹配。这两个探针没有加入吞吐 corpus；本次 corpus 内差异仅为上一段六种字符。\n']

speed=[];detail=[];checksum=[]
for l in order:
    for c in bench[l]['contexts']:
        if not c['selected']:continue
        t=c['timings'];ok=all('median_seconds' in t.get(k,{}) for k in total)
        if not ok:speed.append([l,c['name'],c['patterns'],'ERROR','ERROR','ERROR','—','—']);continue
        speed.append([l,c['name'],c['patterns'],*[f'{t[k]["mb_per_second"]:.2f}' for k in total],f'{t["onig"]["median_seconds"]/t["multi"]["median_seconds"]:.2f}×',f'{t["fancy"]["median_seconds"]/t["multi"]["median_seconds"]:.2f}×'])
        for k in total:
            v=t[k];detail.append([l,c['name'],k,fmt(v['queries']),f'{v["median_seconds"]:.6f}',f'{v["queries_per_second"]:,.0f}',f'{v["mb_per_second"]:.2f}',v['runtime_fallbacks']])
        if len({(t[k]['queries'],t[k]['matches'],t[k]['checksum']) for k in total})!=1:checksum.append([l,c['name'],*[t[k]['checksum'] for k in total]])
chunks+=['## 逐 context 性能\n',table(['语言','context','patterns','A-Onig MB/s','A-fancy MB/s','B MB/s','B/A-Onig','B/A-fancy'],speed)+'\n',
'contexts 选择优先指定常见入口/表达式，再补最大且 ordered pattern 列表不同的 contexts，最终每语言四个；不是根据实测速度挑选。原始三次时间保存在 `output/bench.json`。速度比 <1 表示 B 更慢。\n']
chunks+=['### 各语言完整指标\n',table(['语言','engine','queries 合计','时间中位数之和 s','queries/s','MB/s'],aggregate_detail)+'\n',
'每语言指标是四个固定 contexts 扫描同一 corpus 四次的合计；总时间为四个 context 的单次完整扫描时间中位数之和，queries 为对应四个计数之和。不是某个真实 parser 在该语言上的动态 context 混合吞吐。\n']
if checksum:chunks+=['以下 benchmark trajectories/checksum 有差异，不能把速度比当作等价工作量的结论：\n',table(['语言','context','onig checksum','fancy checksum','B checksum'],checksum)+'\n']
else:chunks+=['全部所选 context 的三引擎 query 数、match 数和含 capture spans 的 checksum 一致。checksum 是吞吐运行的额外完整性检查，不能代替逐 query 差分。\n']

startup=[];memory=[]
for l in order:
    get=lambda f:median([f(c[l]) for c in comp])
    startup.append([l,f'{get(lambda s:s["load_ms"]):.2f}',f'{get(lambda s:s["compile"]["onig_ms"]):.2f}',f'{get(lambda s:s["compile"]["frontend_ms"]+s["compile"]["fancy_ms"]):.2f}',f'{get(lambda s:s["multi_compile_ms"]):.2f}',f'{get(lambda s:s["b_total_compile_ms"]):.2f}'])
    memory.append([l,*[f'{mem[l,k]["loaded_rss_bytes"]/2**20:.1f}' for k in total],f'{mem[l,"multi"]["delta_rss_bytes"]/2**20:.1f}',f'{mem[l,"multi"]["meta_memory_bytes"]/2**20:.1f}',f'{mem[l,"multi"]["initial_meta_caches_bytes"]/2**20:.1f}'])
chunks+=['## 启动编译与内存\n',table(['语言','YAML/load ms','A-Onig ms','A-fancy+翻译 ms','B union/guards ms','B 总 ms'],startup)+'\n',
'启动列是三次独立进程的中位数：每语法唯一展开 regex 共享编译，includes 重复不重复编译 A；B eager 编译所有 contexts 和 guards，B 总计包括 Onig capture/fallback pool、前端、所有 union/guards。A-fancy 在有效规则上若有编译失败会明确 fallback 到 Onig；逐 context 的 `fancy_fallback_rules` 与 runtime fallback 计数保留在 JSON。这里 A-fancy+翻译列未计共同保留的 Onig safety pool，需结合 fallback 计数解释。\n',
table(['语言','A-Onig RSS MiB','A-fancy RSS MiB','B RSS MiB','B 增量 RSS MiB','B ΣAPI memory_usage MiB','B 初始 cache MiB'],memory)+'\n',
'内存分别用新子进程测量，每次只加载一个语法和一个 engine，不载入 corpus；B 保留该语法所有 compiled contexts 和初始 per-context union caches。RSS 含共享库、allocator、YAML/前端；增量是加载 engine 后减去 loader 后的 RSS。API 列是每个 meta/guard 对象 memory_usage 的和，可能重复计算共享结构，并包含未完全驻留的容量，不能视作独占物理 heap；因此 C++ 的 API 之和可以大于 RSS。cold cache 会在扫描时增长，16 MiB 是配置的每个 hybrid DFA cache 上限，forward/reverse caches 可分别占用。fancy 进程保留 Onig 安全副本，故其 RSS 不是纯 fancy 最小值。peak RSS 另存原始 memory.json。\n']

chunks+=['## 对真实引擎的结论与风险\n',
'多模式搜索的收益由 context 和候选形态决定。宽关键字集合通常能分享扫描与 literal prefilter；规则少、零宽规则密集、长 Unicode identifiers、复杂 guards 或 Onig fallback 较多时，候选验证、PikeVM/反向定位和 captures 会吞掉收益。不能从总体正则覆盖率推出“高亮一定快六倍”。\n',
'本次 Rust statements 只有 0.46×，拖累 Rust 四个 contexts 合计到 0.54×；C++ global / preprocessor-if-branch-global 为 0.91× / 0.87×；Markdown strikethrough-content 约 1.00×，可视为持平。JSON 的 B 虽胜 Onig，合计与 fancy 基本持平（0.99×）。相对而言 Python 合计 3.90×、Markdown 2.57×、Rust numbers 10.27×，显示收益高度依赖工作负载。C++ eager 编译 2.96 秒、RSS 370.1 MiB，相比 A-Onig 14.86 ms / 11.7 MiB，启动和内存成本不能忽略。\n',
'应 lazy 编译活跃 context，共享相同 ordered pattern 集合及 guard matcher；按 NFA 大小配置 cache，并对 eager 内存设限。首次命中成本、cache thrashing、非 ASCII 触发的慢路径需独立观测。当前为每 context 重复编译大量相似 guards，适合验证方法，不适合直接放入编辑器启动路径。\n',
'生产实现需要完整 Onig front end/HIR 语义验证，按 rule priority 输出可验证的候选集合，避免失败 guard 触发 O(patterns) anchored 修复。需要处理可变长度 core 的回溯、零宽循环、同起点 tie、lookahead captures、动态 delimiter backrefs、\\G/\\K 与 search-start、Unicode boundaries/full folding、CRLF/多行输入和 retry-limit。可将常见有限 lookahead 编入 automaton/候选 predicate，剩余规则用 Onig。\n',
'真正 parser 还要处理 context stack、prototype/include/embed、branch/fail 与 syntax inheritance 的全部规则；本次没有接入 syntect ParseState interception。还应在真实项目文件上按活跃 context 分布测端到端编辑、增量重高亮和 p95/p99 延迟，才能决定自研还是先复用 syntect。\n',
'验证：`cargo test --release --locked` 通过 10 项测试，包括短字符串穷举、cache、同位置 priority、core 回缩、lookahead captures、terminal newline 与方言翻译。当前 toolchain 未安装 rustfmt/clippy，未完成这两项可选检查。\n',
'## 完整计时指标\n',table(['语言','context','engine','queries','总时间中位 s','queries/s','MB/s','runtime fallback'],detail)+'\n',
'复跑命令与实现细节见 [README.md](README.md)。原始数据：`output/diff.json`、`output/bench.json`、`output/compile-{0,1,2}.json`、`output/memory.json`；所有编译失败原因、skips、翻译后 core/guard 和 mismatch samples 均保留。无 git commit。\n']
path=ROOT/'RESULTS.md'
if path.exists():
    (ROOT/'backups').mkdir(exist_ok=True)
    shutil.copy2(path,ROOT/'backups'/f'{time.time_ns()}-RESULTS.md')
tmp=path.with_suffix('.tmp');tmp.write_text('\n'.join(chunks));tmp.replace(path)
print(path)
