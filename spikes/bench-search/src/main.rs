use anyhow::{bail,Result};
use bench_common::{arg,data_dir,emit,stats,timed};
use grep_regex::RegexMatcher;
use grep_searcher::{BinaryDetection,Searcher,SearcherBuilder,Sink,SinkFinish,SinkMatch};
use ignore::{WalkBuilder,WalkState};
use nucleo_matcher::{Config,Matcher,Utf32String,pattern::{CaseMatching,Normalization,Pattern}};
use serde_json::json;
use std::{fs,hint::black_box,path::PathBuf,sync::atomic::{AtomicU64,Ordering}};
use walkdir::WalkDir;
fn regex_bench(args:&[String])->Result<()> {
 let path=arg(args,"--file","data/text-1gb.txt");let (bytes,load)=timed(||fs::read(path));let bytes=bytes?;
 for (kind,pattern) in [("literal","pub fn"),("complex",r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+[A-Za-z_]\w*\s*(?:<[^>\n]{0,160}>)?\s*\(")] {
  let re=regex::bytes::Regex::new(pattern)?;let mut samples=Vec::new();let mut count=0;
  for _ in 0..5 {let (n,ms)=timed(||re.find_iter(black_box(&bytes)).count());count=black_box(n);samples.push(ms);}
  let s=stats(&samples);let sec=s["median_ms"].as_f64().unwrap()/1000.0;
  emit(json!({"bench":"search_regex","kind":kind,"pattern":pattern,"bytes":bytes.len(),"input_read_ms_excluded":load,"matches":count,"time":s,"mb_s":bytes.len() as f64/1e6/sec,"note":"in-memory regex::bytes, all matches counted, IO excluded"}));
 }Ok(())
}
#[derive(Default)]struct FileSink{lines:u64,bytes:u64,binary:bool}
impl Sink for FileSink {
 type Error=std::io::Error;
 fn matched(&mut self,_:&Searcher,_:&SinkMatch<'_>)->std::io::Result<bool>{self.lines+=1;Ok(true)}
 fn finish(&mut self,_:&Searcher,f:&SinkFinish)->std::io::Result<()>{self.bytes=f.byte_count();self.binary=f.binary_byte_offset().is_some();Ok(())}
}
fn roots(args:&[String])->Vec<PathBuf>{
 let home=std::env::var("HOME").unwrap();let data=data_dir(args);
 [PathBuf::from(arg(args,"--registry",&format!("{home}/.cargo/registry/src"))),data.join("cargo-home/registry/src"),data.join("Packages")].into_iter().filter(|p|p.exists()).collect()
}
fn files_bench(args:&[String])->Result<()> {
 let roots=roots(args);let threads:usize=arg(args,"--threads","8").parse()?;
 let pattern=r"pub\s+(?:async\s+)?fn\s+[A-Za-z_][A-Za-z0-9_]*";let matcher=RegexMatcher::new(pattern)?;
 let mut samples=Vec::new();let mut first=json!({});let mut warm=json!({});
 for round in 0..6 {
  let (files,selected_bytes,searched_bytes,matches,errors,binaries)=(AtomicU64::new(0),AtomicU64::new(0),AtomicU64::new(0),AtomicU64::new(0),AtomicU64::new(0),AtomicU64::new(0));
  let (_,ms)=timed(||{
   let mut builder=WalkBuilder::new(&roots[0]);for r in &roots[1..]{builder.add(r);}builder.threads(threads);
   builder.build_parallel().run(||{
    let matcher=&matcher;let (files,selected_bytes,searched_bytes,matches,errors,binaries)=(&files,&selected_bytes,&searched_bytes,&matches,&errors,&binaries);
    let mut searcher=SearcherBuilder::new().binary_detection(BinaryDetection::quit(b'\0')).build();
    Box::new(move|entry|{
     let entry=match entry{Ok(e)=>e,Err(_)=>{errors.fetch_add(1,Ordering::Relaxed);return WalkState::Continue;}};
     if entry.file_type().is_none_or(|t|!t.is_file()){return WalkState::Continue;}
     files.fetch_add(1,Ordering::Relaxed);
     if let Ok(m)=entry.metadata(){selected_bytes.fetch_add(m.len(),Ordering::Relaxed);}
     let mut sink=FileSink::default();
     if searcher.search_path(matcher,entry.path(),&mut sink).is_err(){errors.fetch_add(1,Ordering::Relaxed);}
     searched_bytes.fetch_add(sink.bytes,Ordering::Relaxed);matches.fetch_add(sink.lines,Ordering::Relaxed);binaries.fetch_add(sink.binary as u64,Ordering::Relaxed);WalkState::Continue
    })
   });
  });
  let row=json!({"elapsed_ms":ms,"files":files.load(Ordering::Relaxed),"selected_bytes":selected_bytes.load(Ordering::Relaxed),"searched_bytes":searched_bytes.load(Ordering::Relaxed),"matched_lines":matches.load(Ordering::Relaxed),"errors":errors.load(Ordering::Relaxed),"binary_files":binaries.load(Ordering::Relaxed)});
  if round==0{first=row;}else{samples.push(ms);warm=row;}
 }
 emit(json!({"bench":"search_files","threads":threads,"roots":roots,"pattern":pattern,"first_coldish":first,"warm":stats(&samples),"warm_counts":warm,"note":"first traversal only, no OS cache eviction; some files previously read for corpora/build; ignore default filters; quit binary at NUL; bytes from SinkFinish"}));Ok(())
}
fn fuzzy_bench(args:&[String])->Result<()> {
 let min_paths:usize=arg(args,"--paths","100000").parse()?;let mut paths=Vec::<String>::new();
 for root in roots(args){for e in WalkDir::new(root).into_iter().filter_map(|e|e.ok()).filter(|e|e.file_type().is_file()){
  paths.push(e.path().to_string_lossy().into_owned());
 }}
 paths.sort();paths.dedup();let real_count=paths.len();
 // Preserve actual suffix distribution, add deterministic workspace/crate prefixes.
 let real=paths.clone();let mut i=0;
 while paths.len()<min_paths{
  let s=if i%5==0{format!("workspaces/project-{}/crates/tokio-runtime/src/runtime/worker_{}.rs",i/100,i)}else if i%7==0{format!("workspaces/project-{}/crates/app-{}/src/main.rs",i/100,i%101)}else{format!("workspaces/project-{}/{}",i/real.len(),real[i%real.len()].trim_start_matches('/'))};paths.push(s);i+=1;
 }
 let (haystacks,index_ms)=timed(||paths.iter().map(|s|Utf32String::from(s.as_str())).collect::<Vec<_>>());
 let mut matcher=Matcher::new(Config::DEFAULT.match_paths());let all=(0..paths.len()).collect::<Vec<_>>();
 for query in ["srcmainrs","tokioruntime"]{
  let mut samples=Vec::new();let mut matched=0;
  for _ in 0..7 {
   let (n,ms)=timed(||{
    let p=Pattern::parse(query,CaseMatching::Ignore,Normalization::Smart);let mut scores=Vec::new();
    for (i,h) in haystacks.iter().enumerate(){if let Some(score)=p.score(h.slice(..),&mut matcher){scores.push((score,i));}}
    matched=scores.len();scores.sort_unstable_by(|a,b|b.cmp(a));black_box(&scores[..scores.len().min(50)]);scores.len()
   });black_box(n);samples.push(ms);
  }
  let mut typing=Vec::new();
  for _ in 0..7 {
   let mut candidates=all.clone();let mut chars=Vec::new();
   for n in 1..=query.len(){let prefix=&query[..n];let before=candidates.len();let (_,ms)=timed(||{let p=Pattern::parse(prefix,CaseMatching::Ignore,Normalization::Smart);let mut scores=Vec::new();candidates.retain(|&i|{if let Some(score)=p.score(haystacks[i].slice(..),&mut matcher){scores.push((score,i));true}else{false}});scores.sort_unstable_by(|a,b|b.cmp(a));black_box(&scores[..scores.len().min(50)]);});chars.push((ms,before,candidates.len()));}
   typing.push(chars);
  }
  let incremental=(0..query.len()).map(|c|json!({"prefix":&query[..c+1],"time":stats(&typing.iter().map(|v|v[c].0).collect::<Vec<_>>()),"candidates_before":typing[0][c].1,"matches":typing[0][c].2})).collect::<Vec<_>>();
  emit(json!({"bench":"search_fuzzy","query":query,"paths":paths.len(),"real_paths":real_count,"synthetic_paths":paths.len()-real_count,"index_conversion_ms":index_ms,"matches":matched,"full_rescore_and_top50":stats(&samples),"incremental_typing":incremental,"incremental_total":stats(&typing.iter().map(|v|v.iter().map(|x|x.0).sum::<f64>()).collect::<Vec<_>>()),"note":"single-threaded nucleo-matcher; UTF32 conversion excluded; score + descending sort/top50 included; incremental filters previous matches only (monotonic ASCII extension), no nucleo background scheduler"}));
 }Ok(())
}
fn main()->Result<()> {
 let args=std::env::args().collect::<Vec<_>>();match args.get(1).map(|s|s.as_str()).unwrap_or("regex"){"regex"=>regex_bench(&args),"files"=>files_bench(&args),"fuzzy"=>fuzzy_bench(&args),_=>bail!("commands: regex/files/fuzzy")}
}
