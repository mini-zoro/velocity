#!/usr/bin/env python3
"""Render the checked-in Chinese report from raw measurements; no invented numbers."""
import collections,datetime,json,pathlib,subprocess,tomllib
ROOT=pathlib.Path(__file__).resolve().parents[1];DATA=ROOT/'data';RAW=DATA/'raw'
def load(name):
 p=RAW/(name+'.jsonl')
 return [json.loads(l) for l in p.read_text().splitlines() if l.startswith('{')] if p.exists() else []
def one(name):
 rows=load(name)
 return rows[0] if rows else {}
def med(d,key):return d[key]['median_ms']
def f(x,d=2):return f'{x:,.{d}f}'
def mib(n):return f(n/2**20,1)
def table(headers,rows):return '\n'.join(['| '+' | '.join(map(str,headers))+' |','| '+' | '.join(['---']*len(headers))+' |']+['| '+' | '.join(str(x).replace('|','\\|').replace('\n',' ') for x in r)+' |' for r in rows])
def aggregate(d,language,source):
 c=collections.Counter()
 for r in d.get('tests',[]):
  if r['language']!=language or r['source']!=source:continue
  c['files']+=1;c['planned']+=r['planned_cases'];c['planned_cells']+=r['planned_cells']
  if r['status']=='blocked_syntax_not_loaded':c['file_blocked']+=1;continue
  v=r['result']
  for key in ['checked_cases','passed_cases','failed_cases','checked_cells','passed_cells','failed_cells']:c[key]+=v[key]
  if v['error']:c['file_error']+=1
  elif v['failed_cases']:c['file_fail']+=1
  else:c['file_pass']+=1
 c['untested']=c['planned']-c['checked_cases']
 return c
def main():
 runs=json.loads((RAW/'runs.json').read_text());corpus=json.loads((DATA/'corpora.json').read_text());versions={}
 for manifest in [ROOT/'Cargo.toml',*(ROOT/n/'Cargo.toml' for n in ['bench-common','bench-buffer','bench-syntax','bench-search','bench-text'])]:
  config=tomllib.loads(manifest.read_text());deps=config.get('workspace',{}).get('dependencies',{}) | config.get('dependencies',{})
  for name,spec in deps.items():
   version=spec if isinstance(spec,str) else spec.get('version')
   if version:versions[name]={'version':version.lstrip('=')}
 machine=json.loads((DATA/'machine.json').read_text());parts=[]
 def add(s):parts.append(s)
 add('# Rust 编辑器 building blocks：可行性 spike 与实测\n')
 add(f'测量日期：2026-11-13（Asia/Shanghai）；报告生成时间：{datetime.datetime.now().astimezone().isoformat(timespec="seconds")}。全部是本机实际运行得到的结果。没有测量 Sublime Text 本体，因此不能把下列数字解释为“达到 Sublime 的某个百分比”。\n')
 add('## 机器、版本与测量方法\n')
 add('Apple MacBook Pro MacBookPro18,3，M1 Pro，8 核（6P + 2E），16 GB，macOS 26.1 (25B78)，aarch64-apple-darwin。接 AC，100% 电量。rustc 1.99.1 (`58cf8f9c3`, 2026-10-13)，cargo 1.99.1 (`5f94df478`, 2026-08-27)，LLVM 23.1.1。Release profile，默认优化等级 3，debug=1；未开启额外 LTO、target-cpu=native 或自定义 allocator。\n')
 add('版本来自当日 crates.io sparse index，选择未 yanked、无 prerelease 后缀的最新 stable，并在 manifests 中精确固定。`Cargo.lock` 固定全部传递依赖；下表是直接使用的 crate。syntect 的 regex 后端遵循其发布版本的依赖约束，未私自替换成不兼容的新版依赖。\n')
 groups={'bench-buffer':['ropey','crop'],'bench-syntax':['syntect','tree-sitter','tree-sitter-rust','tree-sitter-javascript'],'bench-search':['regex','ignore','grep-searcher','grep-regex','nucleo-matcher'],'bench-text':['cosmic-text'],'公共/辅助':['anyhow','serde_json','walkdir','libc']}
 add(table(['程序','crate = exact version'],[(g,'、'.join(f'`{n} = {versions[n]["version"]}`' for n in names)) for g,names in groups.items()])+'\n')
 lock=(ROOT/'Cargo.lock').read_text();import re
 entries=re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"',lock)
 extra=[(n,v) for n,v in entries if n in ['fancy-regex','onig','onig_sys','swash','skrifa','harfrust','fontdb']]
 add('关键传递依赖：'+ '、'.join(f'`{n} {v}`' for n,v in extra)+'。完整依赖树可由 `cargo tree --locked` 重建。\n')
 add('计时使用 `std::time::Instant`。独立程序串行运行，排除编译时间；通常 5 次取 median，fuzzy 7 次，完整 viewport 9 次热字体样本，单行/缓存 100 次，多光标 100 个连续事务。P95 为 nearest-rank；5 次样本的 P95 就是最大值。RSS 是整个进程生命周期的高水位，单位 MiB。`/usr/bin/time -l` 在此沙箱因 `sysctl kern.clockrate: Operation not permitted` 失败，已在 `data/raw/rss-method.json` 保存探测结果；改用 Rust 进程内 `libc::getrusage(RUSAGE_SELF).ru_maxrss`（macOS 返回 bytes）。不将测量工具失败误记为被测程序失败。\n')
 add('文件系统缓存没有清除。“cold-ish”只表示该搜索进程第一次遍历，之前生成数据/编译已读取部分文件；不是真正冷磁盘。其他读文件结果以五次混合样本中位数为主；所有逐次样本、错误和执行命令在 `data/raw/*.jsonl`、`*.time.txt`、`runs.json`。固定随机种子；`black_box` 消费输出，编辑后检查长度，排版断言 120 行与 24,000 glyphs。\n')
 add('## 输入数据\n')
 add(f'官方 Packages 浅克隆 commit：`{corpus["packages_commit"]}`。Rust 来自原有 `~/.cargo/registry/src` 的 {corpus["rust_source_files"]} 个真实 `.rs` 文件；记录源文件路径和 SHA256 于 `data/source_manifest.json`。buffer 语料将非 ASCII 字符替成 `?`，使两种索引单位可比较；原始 UTF-8 Rust 文件另存供语法基准。JS 为 lodash 4.17.21 的 unminified + minified 文件重复拼接；Markdown 来自 crate README；JSON 为合法的结构化数组。拼接源码并非可编译的单一程序，末尾也可能截断，因此 tree-sitter 的 ERROR 节点是已观察到的输入特征。\n')
 rows=[]
 for name in ['text-100mb.txt','text-1gb.txt','rust-10mb.rs','javascript-10mb.js','json-20mb.json','markdown-5mb.md']:
  d=corpus[name];rows.append([name,f(d['bytes']/1e6,6),d['sha256']])
 add(table(['文件（data/ 下）','MB（10⁶ bytes）','SHA256'],rows)+'\n')
 add('buffer 两个文件的行数分别为 '+f(one('buffer-ropey-100mb-load').get('lines',0),0)+'、'+f(one('buffer-ropey-1gb-load').get('lines',0),0)+'，行长混合；两者末尾都不是换行，因此 ropey/crop 的最终空行语义差异不影响这次行数。重复拼接带来很强的内容重复与缓存局部性；不能外推到随机二进制、超长单行或所有 Unicode 文本。\n')
 add('## 1. bench-buffer\n')
 add('加载计时包含文件读取、UTF-8 校验和建树。ropey 使用 `Rope::from_reader`；crop 使用 64 KiB 流式 `RopeBuilder`，避免先保留完整 String 使 RSS 翻倍。String 是可选 baseline，仅测加载和整份 clone；没有实现 gap buffer，也没有进行其 O(N) 随机编辑。String 的进程 RSS 包含 clone 时同时存在的两份正文，不能当成单份加载后的常驻量。\n')
 rows=[]
 for size in ['100mb','1gb']:
  for e in ['ropey','crop','string']:
   n=f'buffer-{e}-{size}-load';d=one(n)
   if not d:continue
   rows.append([e,size,f(med(d,'load')),f(d['load']['p95_ms']),mib(d['peak_rss_bytes']),f(med(d,'clone_one')) if e=='string' else '见下一表'])
 add(table(['实现','输入','加载 median ms','P95 ms','进程峰值 RSS MiB','String clone ms'],rows)+'\n')
 rows=[]
 for size in ['100mb','1gb']:
  for e in ['ropey','crop']:
   d=one(f'buffer-{e}-{size}-ops')
   if not d:continue
   rows.append([e,size,f(med(d,'line_count_1m')),f(med(d,'char_to_line_1m')),f(med(d,'line_to_char_1m')),f(med(d,'insert_10k')),f(med(d,'delete_10k')),f(med(d,'clone_one')*1e6,2),mib(d['peak_rss_bytes'])])
 add(table(['实现','输入','行数读取×1M ms','char→line×1M ms','line→char×1M ms','随机插入×10k ms','随机删除×10k ms','clone ns/次','ops 峰值 MiB'],rows)+'\n')
 add('**crop 不提供与 ropey 同义的 native Unicode-scalar `char_to_line/line_to_char` API**：此处调用 `line_of_byte/byte_of_line`，依赖 ASCII 输入的 byte==char。它不代表 crop 对 Unicode scalar 索引有同样的性能；编辑器需统一 byte/UTF-16/grapheme 坐标或补充索引。snapshot 按 1,000 份 clone 一批、持有至计时结束，7 批取每次摊销 median，排除 drop；纳秒值包含摊销的 Vec 分配，并非完整 undo 事务耗时。随机编辑各 5 组，每组从原始共享 snapshot clone，计入首次 COW 的成本。\n')
 rows=[]
 for size in ['100mb','1gb']:
  for e in ['ropey','crop']:
   d=one(f'buffer-{e}-{size}-ops')
   for count in [10000,100000]:
    v=d.get(f'multicursor_{count}_100tx')
    if v:rows.append([e,size,f(count,0),f(v['median_ms'],3),f(v['p95_ms'],3),v['n']])
 add(table(['实现','输入','光标数','每事务 median ms','P95 ms','事务数'],rows)+'\n')
 add('每个事务重新计算前 N 行的行首 offset，再逆序逐个插入 `foo`，计时包含 offset 查找、Vec 分配和全部插入；这是一段连续逻辑事务，没有调用 rope 的 bulk-edit API。连续 100 次不重置正文（分别增加 3 MB / 30 MB）；一直保留最初 snapshot，但没有为每个事务保留一份新的 undo snapshot。当前结果不能代替完整 selections/undo 合并成本。\n')
 add('## 2. bench-syntax：ST4 兼容性（重点）\n')
 compat={b:one(f'syntax-{b}-compat') for b in ['fancy','onig']}
 add('### 2.1 官方语法载入\n')
 rows=[]
 for b,d in compat.items():rows.append([b,d.get('total'),d.get('ok'),d.get('failed'),d.get('folder_loaded_before_error'),d.get('unlinked_count'),f(d.get('isolated_load_and_build_ms',0)),mib(d.get('peak_rss_bytes',0))])
 add(table(['后端','文件数','逐文件 load OK','load 失败','add_from_folder 失败前已加','去重未链接引用数','逐文件尝试+build ms','峰值 MiB'],rows)+'\n')
 add('`add_from_folder` 两个后端都在 `ASP/Embeddings/CSS (for ASP).sublime-syntax` 停止，错误为 `Missing mandatory key in YAML file: match`。随后独立尝试全部文件，成功的才加入同一 SyntaxSet。**load OK 只说明 YAML/regex 的表面结构被接受，不代表 ST4 语义兼容**，例如 `extends` 和 `branch_point/fail` 可能被忽略。成功子集的跨语言引用也不完整；`find_unlinked_contexts` 返回的是去重描述集合，19 不是所有引用出现位置的计数。\n')
 rows=[]
 for cat in sorted({k for d in compat.values() for k in d.get('failure_categories',{})}):
  sample=next((r for r in compat['onig'].get('files',[]) if r.get('category')==cat),{})
  rows.append([cat,compat['fancy'].get('failure_categories',{}).get(cat,0),compat['onig'].get('failure_categories',{}).get(cat,0),sample.get('file','')])
 add(table(['实际错误分类','fancy','onig','样本文件'],rows)+'\n')
 add('`MissingMandatoryKey(match)` 经常来自继承语法中的 `meta_prepend/meta_append` 等规则，不把 102 个错误全部归因于 branch。`MainMissing` / 缺少 contexts 常见于仅声明继承的文件。ActionScript 的 `InvalidScope` 错误文字写 “Max 2^16-2 atoms”，但本例实际有超过 8 段的 scope（如 `keyword.operator.assignment.augmented.bitwise.shift.left.unsigned.actionscript`）；syntect 的 scope 深度上限也是 8。没有观测到顶层 `RegexCompileError`，不代表运行中不会遇到 regex 编译错误。\n')
 rows=[]
 for key,label in [('extends','extends'),('version2','version: 2'),('branch_point','branch_point'),('fail','fail'),('embed','embed'),('escape','escape')]:
  files=[r for r in compat['onig'].get('files',[]) if r['features'][key]>0]
  rows.append([label,len(files),sum(r['ok'] for r in files),sum(not r['ok'] for r in files),sum(r['features'][key] for r in files)])
 add(table(['源码关键字（锚定 YAML key）','使用文件数','其中 load OK','其中 load 失败','key 出现次数'],rows)+'\n')
 add('这些特征互相重叠，不能相加当成失败总数。`version: 2` 本身不会让加载器拒绝文件；embed/escape 在 syntect 中有实现，但依赖语法缺失仍会造成错误。稳定版 5.3.0 的 YAML loader 没有实现 extends/inheritance 与 branch/fail 的回溯语义，必须靠下面的行为测试判断接受后的实际效果。\n')
 add('### 2.2 官方 syntax_test_* 行为测试\n')
 tests={b:json.loads((DATA/f'syntax-{b}-tests.json').read_text()) if (DATA/f'syntax-{b}-tests.json').exists() else {} for b in ['fancy','onig']}
 add('适配 released syntect v5.3.0 的 `examples/syntest.rs`：按最近一个不含测试标记的源代码行的 scopes 检查 `^` 范围和 `<-` 行首；与该例程默认一致，`parse_test_lines=true`。每个断言范围全部字符匹配才记作一个 case 通过，同时保留按字符计数。现代 header 支持无空格 `T:SYNTAX TEST`；reindent 等 modifier 不执行；symbol/reference 的 `@` / `>` 标记明确跳过并记录。用人工 grammar 验证正向、负向、`<-` 与包含 Unicode 的行；两个后端自检均通过。运行时错误保留发生前的计数，后续记“未执行”。这测的是官方测试预期 scope，未启动 Sublime 本体。\n')
 language_order=['Rust','Python','JavaScript','C++','JSON','Markdown','HTML','YAML']
 rows=[]
 for b in ['fancy','onig']:
  for lang in language_order:
   c=aggregate(tests[b],lang,'official');rate=100*c['passed_cases']/c['checked_cases'] if c['checked_cases'] else None;cov=100*c['checked_cases']/c['planned'] if c['planned'] else 0
   rows.append([b,lang,f"{c['file_pass']}/{c['file_fail']}/{c['file_error']}/{c['file_blocked']}",f"{c['passed_cases']}/{c['failed_cases']}/{c['untested']}",'—' if rate is None else f(rate,2)+'%',f(cov,2)+'%'])
 add('官方成功载入子集（没有混入 bundled 语法）：\n')
 add(table(['后端','语言','文件 P/F/E/B','断言 P/F/未执行','已执行断言通过率','计划断言覆盖率'],rows)+'\n')
 add('文件列：P=全部 scope cases 通过，F=完成但存在失败，E=解析错误或 panic，B=语法未加载。通过率只针对已执行 cases；覆盖率给出已执行/计划总数，避免 Python 的部分执行被误解为整体通过。JavaScript、HTML 的 B 是载入阻塞，不能伪装成 0 条失败。\n')
 rows=[]
 for lang in language_order:
  a=aggregate(tests['fancy'],lang,'bundled');b=aggregate(tests['onig'],lang,'bundled')
  def case(c):return f"{c['passed_cases']}/{c['failed_cases']}/{c['untested']} ({100*c['passed_cases']/c['checked_cases']:.2f}%)" if c['checked_cases'] else '未执行'
  rows.append([lang,a['files'],case(a),case(b),f"{a['file_pass']}/{a['file_fail']}/{a['file_error']} ; {b['file_pass']}/{b['file_fail']}/{b['file_error']}"])
 add('同一批官方测试对 syntect bundled defaults 的补充对照（较旧语法快照；不等于成功加载当前 Packages）：\n')
 add(table(['语言','测试文件数','fancy 断言 P/F/未执行 (通过率)','onig 断言 P/F/未执行 (通过率)','文件 P/F/E（fancy ; onig）'],rows)+'\n')
 ex=collections.Counter(r['reason'] for r in tests['onig'].get('excluded',[]));unique_files=len({r['file'] for r in tests['onig'].get('tests',[])});add(f'全仓共发现 {sum(ex.values())+unique_files} 个测试文件，其中 {unique_files} 个纳入八语言 scope tests。排除项：'+ '；'.join(f'{reason} = {count}' for reason,count in ex.items())+'。只纳入上面八个 canonical syntax 路径；按 header 声明归类，而非所在目录或扩展名；例如 Rust 目录中声明 Markdown 的 frontmatter 测试计入 Markdown。TSX、MultiMarkdown、C 等派生/其他语法不混入分母。逐测试文件、具体失败 selector、actual scopes 和错误位置见 `data/syntax-{fancy,onig}-tests.json`。\n')
 errrows=[]
 for b,d in tests.items():
  seen=set()
  for r in d.get('tests',[]):
   error=r.get('result',{}).get('error')
   if error:
    key=(r['language'],r['source'],error.split(':',1)[-1][:80])
    if key not in seen:errrows.append([b,r['source'],r['file'],error]);seen.add(key)
 add(table(['后端','语法来源','运行错误样本','错误'],errrows[:12])+'\n')
 add('### 2.3 parse 与 parse+style 吞吐\n')
 add('预读整个输入 String，计时排除 IO 与 SyntaxSet/ThemeSet 加载；每次新建 ParseState 或 HighlightLines，从头扫描所有行。parse-only 消费 scope ops，parse+style 用 `base16-ocean.dark` 消费 style spans；这两种模式是分别独立扫描，并不是相减得到 style 成本。每行保留换行。每个成功项 5 次取 median，包含该进程第一次触发的 lazy regex compile，但中位数通常由热缓存主导。\n')
 rows=[]
 for b in ['fancy','onig']:
  for lang in ['Rust','JavaScript','JSON','Markdown']:
   row=[b,lang];sources=[];peaks=[]
   for mode in ['parse','highlight']:
    name=f'throughput-{b}-{lang}-{mode}';d=one(name);fallback=one(name+'-bundled')
    if d.get('status')!='ok' and fallback and fallback.get('status')=='ok':d=fallback
    sources.append(d.get('source',''));peaks.append(d.get('peak_rss_bytes',0))
    if d.get('status')=='ok':row.extend([f(d['mb_s'],3),f(d['lines_s'],0)])
    else:row.extend(['失败/未完成','—'])
   row.extend([' / '.join(dict.fromkeys(sources)),mib(max(peaks))]);rows.append(row)
 add(table(['后端','语言','parse MB/s','parse lines/s','parse+style MB/s','parse+style lines/s','来源（fallback 明示）','进程峰值 MiB'],rows)+'\n')
 controls=[]
 for b in ['fancy','onig']:
  a=one(f'throughput-{b}-Markdown-parse-bundled');h=one(f'throughput-{b}-Markdown-highlight-bundled')
  if a.get('status')=='ok' and h.get('status')=='ok':controls.append([b,f(a['mb_s'],3),f(a['lines_s'],0),f(h['mb_s'],3),f(h['lines_s'],0)])
 if controls:add('Markdown 的相同 bundled grammar 补充对照（fancy 是必需 fallback，onig 是控制组）：\n\n'+table(['后端','parse MB/s','parse lines/s','parse+style MB/s','parse+style lines/s'],controls)+'\n')
 throughput_errors=[]
 for name,run in runs.items():
  if name.startswith('throughput-'):
   d=one(name)
   if d and d.get('status')!='ok':throughput_errors.append([name,d.get('source'),d.get('error')])
   elif not d:throughput_errors.append([name,'—',run['status']])
 if throughput_errors:add(table(['未完成的原始尝试','来源','错误/状态'],throughput_errors)+'\n')
 add('表中的 MB 是十进制（10⁶ bytes）。成功载入但高亮不能完成时会记录原错误；若有 bundled 重试，速度表明确使用重试来源。不同来源的 grammar 完整度不同，不能把较少 scope ops 的高吞吐当成等价语义下的引擎胜负。fancy 后端内部也会将某些 regex search 的错误当作 non-match，现有 API 无法完整统计此类丢失匹配。\n')
 add('### 2.4 SyntaxSet 启动\n')
 rows=[]
 for b in ['fancy','onig']:
  d=one(f'syntax-{b}-startup')
  if not d:continue
  rows.append([b,d['syntax_count_including_plain'],f(med(d,'yaml_same_successful_subset')),f(med(d,'dump_same_successful_subset'),3),f(med(d,'bundled_defaults_dump'),3),f(d['dump_creation_ms'],3),f(d['dump_bytes']/1024,1)])
 add(table(['后端','相同子集含 Plain 数量','YAML→build ms','同子集 dump→load ms','bundled dump load ms','生成自制 dump ms','dump KiB'],rows)+'\n')
 add('YAML 与自制 binary dump 使用同一批成功语法，均 5 次。YAML 包含 IO、regex 验证与 linking；dump 加载没有预热全部 contexts/regex，也没有加载 custom metadata，启动后的第一次实际解析仍需额外工作。bundled defaults 是 syntect 自带另一套预制语法，只作为常见部署路径的启动参考。不能用失败即停止的 `add_from_folder` 时间冒充全套加载时间。\n')
 add('### 2.5 tree-sitter 对照\n')
 rows=[]
 for lang in ['Rust','JavaScript']:
  d=one('tree-'+lang)
  if not d:continue
  rows.append([lang,f(d['bytes']/1e6,1),f(med(d,'full')),f(d['bytes']/1e3/med(d,'full'),2),f(med(d,'incremental_1char'),3),d['original_has_error'],d['edited_has_error'],d['changed_ranges'],mib(d['peak_rss_bytes'])])
 add(table(['grammar','MB','全量 median ms','全量 MB/s','中部插入 1 char 后 reparse ms','原文 ERROR','编辑后 ERROR','changed ranges','进程峰值 MiB'],rows)+'\n')
 add('相同的 10 MB 拼接输入，每项 5 次；在 byte 5,000,000 左右的 UTF-8 边界插入 `x`，正确调用 Tree::edit 和 InputEdit，再以旧树进行 incremental parse。编辑位置构造、String 拷贝与 Tree::edit 不计入 reparse；只测 parser.parse。这些输入含 ERROR 节点，Rust 的一次编辑造成较大重分析，不能据此断言所有正常 Rust 编辑都需要同样耗时。changed_ranges 是结构变化范围；0 不等于跳过解析或文本未变。RSS 包含相邻 full samples 暂时同时持有的旧树/新树以及正文副本，不是单棵 AST 的大小。该对照没有主题着色，也不提供 Sublime syntax 兼容性。\n')
 add('## 3. bench-search\n')
 rows=[]
 for d in load('search-regex'):rows.append([d['kind'],f(d['bytes']/1e6,0),f(med(d,'time'),3),f(d['mb_s'],1),f(d['matches'],0),mib(d['peak_rss_bytes'])])
 add(table(['regex::bytes 搜索','输入 MB','median ms','MB/s','match 数','进程峰值 MiB'],rows)+'\n')
 add(r'literal pattern 为 `pub fn`；complex pattern 为 `(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+[A-Za-z_]\w*\s*(?:<[^>\n]{0,160}>)?\s*\(`。已预读 1 GB 输入，计时只扫描和计数，不含磁盘 IO、结果排序或 UI 展示。'+'\n')
 d=one('search-files');a=d.get('first_coldish',{});w=d.get('warm_counts',{});warm=d.get('warm',{})
 if a:
  add(table(['find in files：8 workers','文件数','选中 bytes','实际扫描 bytes','匹配行','耗时 ms','扫描 MB/s','错误 / binary'],[
   ['first cold-ish',a['files'],a['selected_bytes'],a['searched_bytes'],a['matched_lines'],f(a['elapsed_ms']),f(a['searched_bytes']/1e3/a['elapsed_ms']),f"{a['errors']} / {a['binary_files']}"],
   ['warm（5 次 median）',w['files'],w['selected_bytes'],w['searched_bytes'],w['matched_lines'],f(warm['median_ms']),f(w['searched_bytes']/1e3/warm['median_ms']),f"{w['errors']} / {w['binary_files']}"]])+'\n')
 add('范围为原有 `~/.cargo/registry/src`、本次隔离的 `data/cargo-home/registry/src` 和官方 Packages；部分 crate 在两份 registry 中重复，但路径不同，按两个文件搜索。ignore 默认过滤，隐藏文件/忽略规则生效；grep-regex + grep-searcher 流式读文件，NUL 判 binary 后退出。吞吐以 SinkFinish 实际扫描 bytes 计算，未用包含 binary 未读尾部的 selected bytes 虚增吞吐。fuzzy 的路径采集用普通 WalkDir，包含隐藏路径，因此其真实路径数与此表不同。\n')
 rows=[]
 for d in load('search-fuzzy'):
  steps=d['incremental_typing'];rows.append([d['query'],d['paths'],d['real_paths'],d['synthetic_paths'],f(med(d,'full_rescore_and_top50')),f(med(d,'incremental_total')),f(steps[-1]['time']['median_ms']),f(d['index_conversion_ms']),mib(d['peak_rss_bytes'])])
 add(table(['Goto Anything query','总路径','真实','合成','全量 score+top50 ms','逐字符总 ms','最后一键 ms','UTF32 索引构建 ms','峰值 MiB'],rows)+'\n')
 rows=[]
 for d in load('search-fuzzy'):
  for s in d['incremental_typing']:rows.append([d['query'],s['prefix'],s['candidates_before'],s['matches'],f(s['time']['median_ms'],3)])
 add(table(['query','当前 prefix','重算候选数','剩余候选','每键 median ms'],rows)+'\n')
 add('合成路径是确定性的 workspace/project/crate 路径，部分明确含 `src/main.rs` 和 `tokio-runtime/src/runtime/worker_*.rs`；其余保留真实路径后缀。每个 query/typing sequence 重复 7 次。计时排除文件遍历与预先 UTF32 转换，包含 pattern parse、逐候选打分、全部匹配项排序和取 top50。incremental typing 只重新打分上一键仍匹配的候选，ASCII 查询单调延长；每次 sequence 从全部路径重新开始。没有接入可选的 nucleo 后台调度器，也未测取消、debounce 或 UI 线程占用。\n')
 add('## 4. bench-text：headless shaping/layout\n')
 rows=[]
 for d in load('text'):
  rows.append([d['mode'],f(d['first_full_layout_ms'],3),f(med(d,'full_layout'),3),f(d['full_layout']['p95_ms'],3),f(med(d,'one_line_relayout'),4),f(d['one_line_relayout']['p95_ms'],4),f(med(d,'unchanged_cached_viewport'),6),mib(d['peak_rss_bytes'])])
 add(table(['shaping','首次全量 ms','热字体全量 ms','全量 P95 ms','单行重排 ms','单行 P95 ms','缓存 viewport ms','进程峰值 MiB'],rows)+'\n')
 rows=[]
 for d in load('text'):rows.append([d['mode'],d['unique_raster_keys'],f(d['raster_first_cached_pass_ms'],3),f(med(d,'raster_warm_cached_pass'),3),f(med(d,'raster_without_cache'),3)])
 add(table(['模式','独立 raster cache keys','首轮缓存填充+24k 查询 ms','热缓存24k查询 ms','24k次不缓存 raster ms'],rows)+'\n')
 add('字体为 `/System/Library/Fonts/Menlo.ttc`（Menlo Regular/Bold/Italic/BoldItalic），只加载这套字体，避免全系统字体扫描干扰。120 行，每行严格 200 个 ASCII code characters，14 px font / 20 px line height、Wrap::None；断言布局输出 120 runs、24,000 glyphs。完整布局每次创建新 Buffer 并 set_text，含 shape/layout 和 glyph 数消费，FontSystem 热缓存复用；单行只替换第 61 行末尾字符再 shape 可见范围。首轮 full 使用字体 cache 初始化，因此另列。advanced/basic 同一进程依次跑，RSS 是累计高水位。没有测试 CJK、emoji fallback、bidi、混合字体或 ligature 丰富的文本。\n')
 add('全视口重建超出 1–2 ms 目标，但单行更新足够小。Swash 的 headless 对照显示逐 glyph 重复 rasterization 明显昂贵，必须缓存 glyph image；GPU glyph atlas 是合理的实现方向。这里没有测 winit/wgpu/glyphon 窗口、GPU upload、draw calls、滚动或端到端 frame time，不能把热 CPU cache 查询时间当作 GPU 帧耗时。可选窗口演示已跳过。\n')
 add('## 结论/风险\n')
 add('1. **Rust 的 buffer / search building blocks 足以支撑继续做原型。** crop 的加载和多光标插入在本输入上更快，ropey 提供直接的 Unicode scalar 坐标；两者 cheap snapshots 都远小于 String 整份复制。100k cursors 的逐点插入已超过 16.7 ms 一帧，需要批量事务、游标位置更新策略与 undo/COW 一起优化。\n')
 add(f'2. **当前 stable syntect 不是 ST4 default Packages 的直接替代引擎。** {compat["onig"].get("failed")}/{compat["onig"].get("total")} 文件加载失败；成功子集仍有 {compat["onig"].get("unlinked_count")} 条 unresolved references。extends 使用 {compat["onig"].get("feature_file_counts",{}).get("extends")} 个文件，branch_point 使用 {compat["onig"].get("feature_file_counts",{}).get("branch_point")} 个文件。Rust/JSON/YAML 的官方语法行为接近，但并非全过；C++/Markdown 差距大；Python 未跑完整；JS/HTML 载入受阻。把 bundled defaults 当当前 Packages 会得到显著更低通过率。缺口不能靠“换成 onig”整体修复。\n')
 add('3. **若 ST4 syntax compatibility 是产品要求，应预留实现/维护引擎的工作量。** 至少包括 inheritance 与相对/跨包引用解析、meta_prepend/meta_append、branch_point/branch/fail 的跨行回溯、version 2 的 scope/pop 语义、完整 embedded dependencies、scope 深度，以及官方测试 harness 的 symbol/reference 扩展。可选择深度改造 syntect 或另写解析器；需要持续跑官方 syntax tests 验收，而不是只看 YAML 能不能 load。onig 的语法支持/执行路径在本次更稳，但动态 backreference、未链接上下文和 fancy non-match 的静默错误仍须处理。\n')
 add('4. **高亮需要延迟和增量架构。** binary dump 可大幅缩短 SyntaxSet 构造；regex/context 首次编译、文件正文解析仍不能放进启动关键路径。大文件应先显示可见内容，后台 parse，保存行状态/checkpoint，只使受影响后缀失效。tree-sitter 提供结构化和 incremental 能力，但其 grammar/queries 不能替代 Sublime YAML 语义，且本次 Rust 单点编辑的重分析仍高于一帧。\n')
 add('5. **布局应缓存未变行并配合 glyph atlas。** 每帧重新 shape 24k glyphs 不满足 1–2 ms，但单行重排已很小。完整 editor 仍需自行实现 selections、IME/复杂文字、undo transaction、文件编码/换行、viewport/scroll、渲染调度、插件 API 和 UI；本 spike 不涵盖这些端到端成本。fuzzy 可以从 nucleo-matcher 起步，但大候选集仍应后台重算、取消过期 query，并避免每键全排序。\n')
 add('## 复现\n')
 add('在仓库根目录执行：\n\n```sh\n./spikes/run_all.sh\n```\n')
 add('脚本只在 `spikes/` 下写入：隔离 `CARGO_HOME=data/cargo-home`、数据/克隆/日志在 `data/`、release 构建在 `target/`。`data/` 和 `target/` 已 gitignore。第一次运行联网下载；之后 Cargo.lock 与保留的数据、commit、source manifest/hash 保持输入可追溯。若从空 data/ 重跑，会浅克隆当时 Packages HEAD，并依赖当时 registry 的可用源文件；**要严格复现本表，请保留 data/ 或将 Packages checkout 到本报告 commit 并使用 source manifest 所记录的同版本输入**，不能声称任意未来 HEAD 都是同一数据。\n')
 add('每个基准是独立 binary crate；从 `spikes/` 执行示例：\n\n```sh\nexport CARGO_HOME="$PWD/data/cargo-home"\ncargo run --release --locked -p bench-buffer -- --engine crop --file data/text-1gb.txt --mode ops\ncargo run --release --locked -p bench-syntax -- compat\ncargo run --release --locked -p bench-syntax -- tests\ncargo run --release --locked -p bench-syntax --no-default-features --features onig -- tests\ncargo run --release --locked -p bench-syntax -- throughput --language Rust --mode highlight\ncargo run --release --locked -p bench-syntax -- tree --language JavaScript\ncargo run --release --locked -p bench-search -- regex\ncargo run --release --locked -p bench-search -- files --threads 8\ncargo run --release --locked -p bench-search -- fuzzy --paths 100000\ncargo run --release --locked -p bench-text -- --font /System/Library/Fonts/Menlo.ttc\n```\n')
 add('`run_all.sh` 在计时前完成两个后端构建并复制独立可执行文件，随后串行直接运行 release binaries，保证计时不含 cargo 构建开销。可按组重跑：`./spikes/run_all.sh --only syntax-tests`；或已构建时用 `python3 spikes/scripts/run_benchmarks.py --only throughput --force`。最后 `python3 spikes/scripts/report.py` 从原始 JSON 自动重建此文件。源码更新/日志重跑会先备份原内容到被忽略的 `data/backups/`，再原子替换。未执行 git commit。\n')
 failures=[(n,r['status'],r.get('exit_code')) for n,r in runs.items() if r['status']!='ok']
 add('### 运行完整性\n')
 add(f'记录了 {len(runs)} 个独立命令。'+('进程级失败/超时如下（语法内容错误已在前面按语言记录，不混入进程退出码）：\n\n'+table(['命令','状态','退出码'],failures) if failures else '所有最终记录的命令均正常退出；syntect 已捕获的加载/运行问题仍保留在语法结果中，并未改写为通过。')+'\n')
 text='\n'.join(parts)
 subprocess.run(['python3',str(ROOT/'scripts/write_file.py'),'RESULTS.md'],input=text,text=True,check=True)
 print('Wrote',ROOT/'RESULTS.md')
if __name__=='__main__':main()
