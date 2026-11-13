use anyhow::{Result,Context as _};
use serde::{Serialize,Deserialize};
use sregex_prototype::{loader::{self,Syntax,Context},matcher::{Pool,Multi,Baseline,BaselineKind,Hit,next_boundary}};
use std::{collections::{BTreeMap,HashMap,HashSet},path::{Path,PathBuf},time::Instant,hint::black_box};

const LANGUAGES:[(&str,&str);8]=[
    ("rust","Rust/Rust.sublime-syntax"),("javascript","JavaScript/JavaScript.sublime-syntax"),
    ("python","Python/Python.sublime-syntax"),("cpp","C++/C++.sublime-syntax"),
    ("json","JSON/JSON.sublime-syntax"),("markdown","Markdown/Markdown.sublime-syntax"),
    ("yaml","YAML/YAML.sublime-syntax"),("html","HTML/HTML.sublime-syntax")];

#[derive(Deserialize,Serialize)]
struct Corpus {path:String,bytes:u64,sha256:String,source:String}

#[derive(Default,Serialize,Clone)]
struct Diff {
    fancy_checked:bool,
    unique_lines:u64,represented_lines:u64,unique_queries:u64,represented_queries:u64,
    mismatches_b:u64,represented_mismatches_b:u64,mismatches_fancy:u64,represented_mismatches_fancy:u64,
    oracle_errors:u64,b_errors:u64,fancy_errors:u64,
    categories_b:BTreeMap<String,u64>,categories_fancy:BTreeMap<String,u64>,samples:Vec<serde_json::Value>,
    rejected_candidates:u64,repair_checks:u64,shorter_core_rescues:u64,elapsed_ms:f64,
}

fn category(a:&Option<Hit>,b:&Option<Hit>)->String {
    match (a,b) {
        (Some(a),Some(b)) if a.pattern!=b.pattern=>"pattern_priority".into(),
        (Some(a),Some(b)) if (a.start,a.end)!=(b.start,b.end)=>"match_span".into(),
        (Some(_),Some(_))=>"capture_spans".into(),
        _=>"missing_or_extra_match".into(),
    }
}

fn differential(pool:&Pool,syntax:&Syntax,ctx:&Context,multi:&Multi,lines:&[(&str,u64)],with_fancy:bool)->Diff {
    let t=Instant::now();let mut d=Diff{fancy_checked:with_fancy,..Diff::default()};
    let mut a=Baseline::new(pool,ctx,BaselineKind::Onig);
    let mut f=Baseline::new(pool,ctx,BaselineKind::Fancy);
    let mut b=multi.runner(pool,ctx);
    for &(line,count) in lines {
        d.unique_lines+=1;d.represented_lines+=count;
        a.reset_line();f.reset_line();b.reset_line();
        let mut pos=0;
        while pos<=line.len(){
            d.unique_queries+=1;d.represented_queries+=count;
            let ah=match a.find(line,pos){Ok(v)=>v,Err(e)=>{d.oracle_errors+=1;
                if d.samples.len()<12{d.samples.push(serde_json::json!({"error":"onig","detail":format!("{e:#}"),"line":line,"pos":pos}));}break;}};
            let bh=b.find(line,pos);
            if let Err(e)=&bh {d.b_errors+=1;
                if d.samples.len()<12{d.samples.push(serde_json::json!({"error":"B","detail":format!("{e:#}"),"line":line,"pos":pos}));}}
            if let Ok(bh)=bh && ah!=bh {
                d.mismatches_b+=1;d.represented_mismatches_b+=count;
                *d.categories_b.entry(category(&ah,&bh)).or_default()+=1;
                if d.samples.len()<12 {d.samples.push(serde_json::json!({"engine":"B","line":line,"pos":pos,"a":ah,"b":bh,
                    "a_regex":ah.as_ref().map(|h|&syntax.patterns[ctx.patterns[h.pattern]]),"b_regex":bh.as_ref().map(|h|&syntax.patterns[ctx.patterns[h.pattern]])}));}
            }
            if with_fancy {
                let fh=f.find(line,pos);
                if let Err(e)=&fh {d.fancy_errors+=1;
                    if d.samples.len()<12{d.samples.push(serde_json::json!({"error":"fancy","detail":format!("{e:#}"),"line":line,"pos":pos}));}}
                if let Ok(fh)=fh && ah!=fh {d.mismatches_fancy+=1;d.represented_mismatches_fancy+=count;
                    *d.categories_fancy.entry(category(&ah,&fh)).or_default()+=1;
                    if d.samples.len()<12{d.samples.push(serde_json::json!({"engine":"fancy","line":line,"pos":pos,"a":ah,"b":fh,
                        "a_regex":ah.as_ref().map(|h|&syntax.patterns[ctx.patterns[h.pattern]]),"b_regex":fh.as_ref().map(|h|&syntax.patterns[ctx.patterns[h.pattern]])}));}}
            }
            let Some(h)=ah else{break;};pos=next_boundary(line,h.end.max(pos+1));
        }
    }
    d.rejected_candidates=b.rejected_candidates;d.repair_checks=b.repair_checks;d.shorter_core_rescues=b.shorter_core_rescues;
    d.elapsed_ms=t.elapsed().as_secs_f64()*1000.;d
}

fn parallel_diffs(pool:&Pool,syntax:&Syntax,lines:&[(&str,u64)],fancy:bool,selected_fancy:bool,picked:&HashSet<String>,jobs:usize,triggers:bool)->HashMap<Vec<usize>,Diff> {
    use std::sync::{mpsc,atomic::{AtomicUsize,Ordering}};
    let selected_vectors:HashSet<_>=syntax.contexts.iter().filter(|c|picked.contains(&c.name)).map(|c|c.patterns.clone()).collect();
    let mut seen=HashSet::new();
    let tasks:Vec<_>=syntax.contexts.iter().filter(|c|c.patterns.len()>=5 && seen.insert(c.patterns.clone())).collect();
    let next=AtomicUsize::new(0);let (tx,rx)=mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..jobs.max(1).min(tasks.len().max(1)) {
            let tx=tx.clone();let tasks=&tasks;let next=&next;let selected_vectors=&selected_vectors;
            scope.spawn(move ||loop {
                let i=next.fetch_add(1,Ordering::Relaxed);if i>=tasks.len(){break;}
                let ctx=tasks[i];let multi=Multi::compile_with_triggers(pool,ctx,triggers);
                let d=differential(pool,syntax,ctx,&multi,lines,fancy && (!selected_fancy || selected_vectors.contains(&ctx.patterns)));
                eprintln!("  diff {} {}/{} {}: {} queries, mismatch B={} F={}, errors A={} B={}",syntax.name,i+1,tasks.len(),ctx.name,d.unique_queries,d.mismatches_b,d.mismatches_fancy,d.oracle_errors,d.b_errors);
                if tx.send((ctx.patterns.clone(),d)).is_err(){break;}
            });
        }
        drop(tx);rx.into_iter().collect()
    })
}

#[derive(Serialize)]
struct Timing {seconds:Vec<f64>,median_seconds:f64,queries:u64,matches:u64,checksum:u64,queries_per_second:f64,mb_per_second:f64,runtime_fallbacks:u64}

enum Engine<'a>{A(Baseline<'a>),B(sregex_prototype::matcher::MultiRunner<'a>)}
impl Engine<'_>{
    fn reset(&mut self){match self{Self::A(a)=>a.reset_line(),Self::B(b)=>b.reset_line()}}
    fn find(&mut self,l:&str,p:usize)->Result<Option<Hit>>{match self{Self::A(a)=>a.find(l,p),Self::B(b)=>b.find(l,p)}}
    fn fallbacks(&self)->u64{match self{Self::A(a)=>a.runtime_fallbacks,_=>0}}
}

fn scan(engine:&mut Engine<'_>,text:&str)->Result<(u64,u64,u64)> {
    let mut queries=0;let mut matches=0;let mut sum=0u64;
    for line in text.split_inclusive('\n') {
        engine.reset();let mut pos=0;
        while pos<=line.len(){
            queries+=1;
            let Some(h)=engine.find(black_box(line),black_box(pos))? else{break;};
            matches+=1;
            sum=sum.wrapping_mul(0x100000001b3).wrapping_add((h.start as u64)^((h.end as u64)<<16)^((h.pattern as u64)<<32));
            for c in &h.captures{sum=sum.wrapping_add(c.map_or(0,|(s,e)|(s as u64).wrapping_mul(31)+e as u64));}
            pos=next_boundary(line,h.end.max(pos+1));
        }
    }
    black_box(sum);Ok((queries,matches,sum))
}

fn benchmark(engine:&mut Engine<'_>,text:&str,repeats:usize)->Result<Timing> {
    // Warm up on a prefix ending at a whole line, preserving the same engine's
    // lazy-DFA cache for the timed, full-corpus passes.
    let end=text.char_indices().map(|(i,_)|i).find(|&i|i>=text.len().min(64*1024)).unwrap_or(text.len());
    let end=text[..end].rfind('\n').map_or(end,|i|i+1);
    scan(engine,&text[..end])?;
    let before=engine.fallbacks();let mut times=vec![];let mut result=(0,0,0);
    for _ in 0..repeats{let t=Instant::now();let r=scan(engine,text)?;times.push(t.elapsed().as_secs_f64());
        if !times.is_empty() && result.0!=0 {anyhow::ensure!(result==r,"non-deterministic benchmark");}result=r;}
    let mut sorted=times.clone();sorted.sort_by(f64::total_cmp);let median=sorted[sorted.len()/2];
    Ok(Timing{seconds:times,median_seconds:median,queries:result.0,matches:result.1,checksum:result.2,
        queries_per_second:result.0 as f64/median,mb_per_second:text.len() as f64/1e6/median,runtime_fallbacks:engine.fallbacks()-before})
}

fn selected(s:&Syntax)->HashSet<String> {
    let preferences:&[&str]=match s.name.as_str(){
        "rust"=>&["statements","declarations","keywords","numbers"],
        "javascript"=>&["statement","expression-begin","expression-end","literal-variable"],
        "python"=>&["statements","function-call-argument-list-body","maybe-set-body","class-definition-base-list-body"],
        "cpp"=>&["global","statements","expressions","block"],
        "json"=>&["main","array-body","expect-object-value","object-value-end"],
        "markdown"=>&["paragraph","list-paragraph","fenced-code-block-body","strikethrough-content"],
        "yaml"=>&["main","block-node-12","flow-mapping-body","flow-sequence-body"],
        "html"=>&["main","html","tag-html","tag-attributes"],_=>&[]};
    let mut contexts:Vec<_>=s.contexts.iter().filter(|c|c.patterns.len()>=5).collect();
    contexts.sort_by_key(|c|std::cmp::Reverse(c.patterns.len()));
    let mut seen=HashSet::new();let mut names=HashSet::new();
    for name in preferences {
        if let Some(c)=contexts.iter().find(|c|c.name==*name){if seen.insert(c.patterns.clone()){names.insert(c.name.clone());}}
        if names.len()==4{return names;}
    }
    for c in contexts{if seen.insert(c.patterns.clone()){names.insert(c.name.clone());}if names.len()==4{break;}}
    names
}

fn unique_lines(text:&str)->Vec<(&str,u64)> {
    // Deterministic first-occurrence order. Per-line engine state resets, so
    // equal full lines have identical query trajectories and captures.
    let mut map=HashMap::new();let mut lines=vec![];
    for line in text.split_inclusive('\n'){
        let next=lines.len();let id=*map.entry(line).or_insert_with(||{lines.push((line,0));next});lines[id].1+=1;
    }lines
}

fn save(path:&Path,value:&impl Serialize)->Result<()> {
    // All output writes are atomic. Back up an existing result before replace.
    if path.exists(){let backup=Path::new("backups");std::fs::create_dir_all(backup)?;
        let name=path.file_name().unwrap().to_string_lossy();
        std::fs::copy(path,backup.join(format!("{}-{}",std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos(),name)))?;}
    let tmp=path.with_extension("tmp");std::fs::write(&tmp,serde_json::to_vec_pretty(value)?)?;std::fs::rename(tmp,path)?;Ok(())
}

#[cfg(target_os="macos")]
#[allow(deprecated)]
fn rss()->u64 {
    unsafe {let mut info:libc::mach_task_basic_info=std::mem::zeroed();let mut n=libc::MACH_TASK_BASIC_INFO_COUNT;
        let rc=libc::task_info(libc::mach_task_self(),libc::MACH_TASK_BASIC_INFO,(&mut info as *mut libc::mach_task_basic_info).cast(),&mut n);
        if rc==0{info.resident_size}else{0}}
}
#[cfg(not(target_os="macos"))]
fn rss()->u64 {0}
fn peak_rss()->u64 {
    unsafe {let mut info:libc::rusage=std::mem::zeroed();libc::getrusage(libc::RUSAGE_SELF,&mut info);
        if cfg!(target_os="macos"){info.ru_maxrss as u64}else{info.ru_maxrss as u64*1024}}
}

fn memory(syntax:&Syntax,kind:&str)->Result<serde_json::Value> {
    let before=rss();let peak_before=peak_rss();
    let pool=Pool::compile(&syntax.patterns,kind=="fancy");
    let multis:Vec<_>=if kind=="multi"{syntax.contexts.iter().map(|c|Multi::compile(&pool,c)).collect()}else{vec![]};
    let runners:Vec<_>=multis.iter().zip(&syntax.contexts).map(|(m,c)|m.runner(&pool,c)).collect();
    let after=rss();let peak=peak_rss();
    let meta:usize=multis.iter().map(|m|m.memory_bytes).sum();let caches:usize=runners.iter().map(|r|r.cache_bytes()).sum();
    black_box((&pool,&multis,&runners));
    Ok(serde_json::json!({"language":syntax.name,"engine":kind,"before_rss_bytes":before,"loaded_rss_bytes":after,"delta_rss_bytes":after.saturating_sub(before),
        "peak_before_bytes":peak_before,"peak_rss_bytes":peak,"meta_memory_bytes":meta,"initial_meta_caches_bytes":caches,
        "contexts":syntax.contexts.len(),"patterns":syntax.patterns.len(),"note":"fresh subprocess; eager all contexts; no corpus; cold per-context caches; pool retains Onig capture/safety copies also for fancy"}))
}

fn main()->Result<()> {
    let args:Vec<_>=std::env::args().skip(1).collect();
    let mode=args.first().map(String::as_str).unwrap_or("run");
    let val=|flag:&str|args.iter().position(|s|s==flag).and_then(|i|args.get(i+1)).cloned();
    let root=PathBuf::from(env!("CARGO_MANIFEST_DIR"));std::env::set_current_dir(&root)?;
    let packages=root.join("../../spikes/data/Packages");
    let filter=val("--languages");let repeats=val("--repeat").unwrap_or("3".into()).parse::<usize>()?.max(1);
    let max_bytes=val("--max-bytes").map(|s|s.parse::<usize>()).transpose()?;
    let with_fancy=!args.iter().any(|s|s=="--no-fancy");
    let do_diff=mode=="run" && !args.iter().any(|s|s=="--bench-only");let do_bench=mode=="run" && !args.iter().any(|s|s=="--diff-only");
    std::fs::create_dir_all("output")?;
    let manifest:BTreeMap<String,Corpus>=if mode=="inventory" || mode=="memory"{BTreeMap::new()}else{serde_json::from_slice(&std::fs::read("output/corpora.json").context("run scripts/prepare.py first")?)?};
    let mut output=vec![];
    for (lang,file) in LANGUAGES {
        if filter.as_ref().is_some_and(|f|!f.split(',').any(|l|l==lang)){continue;}
        let start=Instant::now();let syntax=loader::load(lang,&packages.join(file))?;let load_ms=start.elapsed().as_secs_f64()*1000.;
        if mode=="memory" {println!("{}",memory(&syntax,&val("--engine").unwrap_or("multi".into()))?);continue;}
        let picked=val("--contexts").map(|s|s.split(',').map(str::to_string).collect()).unwrap_or_else(||selected(&syntax));
        if mode=="inventory"{output.push(serde_json::json!({"syntax":syntax,"selected":picked}));continue;}
        let pool=Pool::compile(&syntax.patterns,with_fancy);
        let mut text=std::fs::read_to_string(&manifest[lang].path)?;
        if let Some(n)=max_bytes{let end=next_boundary(&text,n.min(text.len()));text.truncate(end);}
        let lines=unique_lines(&text);
        eprintln!("{lang}: {} contexts, {} unique patterns, {} bytes, {} unique / {} lines; classes {:?}; errors onig={} fancy={}",
            syntax.contexts.len(),syntax.patterns.len(),text.len(),lines.len(),lines.iter().map(|l|l.1).sum::<u64>(),pool.stats.classes,pool.stats.onig_errors.len(),pool.stats.fancy_errors.len());
        let mut rows=vec![];let mut compile_total=0.;let mut memory_total=0usize;let mut compile_errors=vec![];
        let diff_cache=if do_diff {parallel_diffs(&pool,&syntax,&lines,with_fancy,args.iter().any(|a|a=="--diff-fancy-selected"),&picked,
            val("--diff-jobs").unwrap_or("4".into()).parse()?,!args.iter().any(|a|a=="--strict-empty-cores"))}else{HashMap::new()};
        for (i,ctx) in syntax.contexts.iter().enumerate(){
            let multi=Multi::compile_with_triggers(&pool,ctx,!args.iter().any(|a|a=="--strict-empty-cores"));compile_total+=multi.compile_ms;memory_total+=multi.memory_bytes;
            compile_errors.extend(multi.compile_errors.clone());
            let diff=if do_diff && ctx.patterns.len()>=5 {
                diff_cache.get(&ctx.patterns).cloned()
            }else{None};
            let mut timings=BTreeMap::new();
            if do_bench && picked.contains(&ctx.name){
                // Rotate engine order deterministically between contexts to
                // reduce a systematic first/last engine temperature bias.
                let order=if i%2==0{vec!["onig","fancy","multi"]}else{vec!["multi","fancy","onig"]};
                for kind in order {
                    if kind=="fancy" && !with_fancy{continue;}
                    let mut e=match kind {"multi"=>Engine::B(multi.runner(&pool,ctx)),"fancy"=>Engine::A(Baseline::new(&pool,ctx,BaselineKind::Fancy)),_=>Engine::A(Baseline::new(&pool,ctx,BaselineKind::Onig))};
                    match benchmark(&mut e,&text,repeats){Ok(t)=>{eprintln!("  {} {kind}: {:.3}s {:.2} MB/s {} queries",ctx.name,t.median_seconds,t.mb_per_second,t.queries);timings.insert(kind,serde_json::to_value(t)?);},
                        Err(e)=>{eprintln!("  {} {kind}: ERROR {e:#}",ctx.name);timings.insert(kind,serde_json::json!({"error":format!("{e:#}")}));}}
                }
            }
            if i%50==0{eprintln!("  {lang} context {}/{} {}",i+1,syntax.contexts.len(),ctx.name);}
            rows.push(serde_json::json!({"name":ctx.name,"patterns":ctx.patterns.len(),"pattern_ids":ctx.patterns,"selected":picked.contains(&ctx.name),
                "fast_patterns":multi.fast_patterns,"boundary_patterns":multi.boundary_patterns,"compile_multi_ms":multi.compile_ms,"meta_memory_bytes":multi.memory_bytes,
                "trigger_patterns":multi.trigger_patterns,
                "compile_errors":multi.compile_errors,"diff":diff,"timings":timings,
                "fancy_fallback_rules":ctx.patterns.iter().filter(|&&gid|pool.onig[gid].is_some() && pool.fancy[gid].is_none()).count(),
                "onig_invalid_rules":ctx.patterns.iter().filter(|&&gid|pool.onig[gid].is_none()).count()}));
        }
        output.push(serde_json::json!({"language":lang,"corpus":manifest[lang],"scanned_bytes":text.len(),"load_ms":load_ms,
            "compile":pool.stats,"multi_compile_ms":compile_total,"b_total_compile_ms":pool.stats.onig_ms+pool.stats.frontend_ms+compile_total,
            "meta_memory_bytes_sum":memory_total,"multi_compile_errors":compile_errors,
            "patterns":syntax.patterns,"translations":pool.translated,"files":syntax.files,"skips":syntax.skips,"contexts":rows}));
        // Checkpoint after each language, so a long run remains inspectable.
        let path=val("--output").unwrap_or("output/run.json".into());save(Path::new(&path),&output)?;
    }
    if mode=="memory"{return Ok(());}
    let path=val("--output").unwrap_or(if mode=="inventory"{"output/inventory.json"}else{"output/run.json"}.into());
    save(Path::new(&path),&output)?;Ok(())
}
