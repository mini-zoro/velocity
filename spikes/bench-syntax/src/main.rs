mod syntest;
use anyhow::{anyhow, bail, Result};
use bench_common::{arg, data_dir, emit, stats, timed};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, hint::black_box, path::{Path,PathBuf}};
use syntect::{dumps, easy::HighlightLines, highlighting::ThemeSet, parsing::{ParseState,SyntaxDefinition,SyntaxSet,SyntaxSetBuilder}, util::LinesWithEndings};
use walkdir::WalkDir;
#[cfg(all(feature="fancy", feature="onig"))]
compile_error!("Select one backend: --no-default-features --features fancy OR onig");
pub fn panic_message(p:Box<dyn std::any::Any+Send>)->String {p.downcast_ref::<String>().cloned().or_else(||p.downcast_ref::<&str>().map(|s|s.to_string())).unwrap_or_else(||"non-string panic".into())}
fn backend()-> &'static str {if cfg!(feature="onig"){"onig"}else{"fancy"}}
fn syntax_files(root:&Path)->Vec<PathBuf>{let mut v=WalkDir::new(root).into_iter().filter_map(|e|e.ok()).filter(|e|e.file_type().is_file()&&e.path().extension().is_some_and(|x|x=="sublime-syntax")).map(|e|e.into_path()).collect::<Vec<_>>();v.sort();v}
fn successful_set(data:&Path)->Result<SyntaxSet>{Ok(dumps::from_uncompressed_dump_file(data.join(format!("syntax-{}.packdump",backend())))?)}
fn feature_flags(s:&str)->Value {
 let mut flags=json!({});
 for (key,re) in [("extends",r"(?m)^extends\s*:"),("version2",r"(?m)^version\s*:\s*2\s*(?:#.*)?$"),("branch_point",r"(?m)^\s*(?:-\s*)?branch_point\s*:"),("fail",r"(?m)^\s*(?:-\s*)?fail\s*:"),("embed",r"(?m)^\s*(?:-\s*)?embed\s*:"),("escape",r"(?m)^\s*(?:-\s*)?escape\s*:")] {flags[key]=json!(regex::Regex::new(re).unwrap().find_iter(s).count());}
 flags
}
fn category(e:&syntect::parsing::ParseSyntaxError)->&'static str {
 use syntect::parsing::ParseSyntaxError::*;
 match e {MainMissing=>"MainMissing (often inheritance)",MissingMandatoryKey("match")=>"MissingMandatoryKey(match)",MissingMandatoryKey("contexts")=>"MissingMandatoryKey(contexts)",MissingMandatoryKey(_)=>"MissingMandatoryKey(other)",RegexCompileError(_,_)=>"RegexCompileError",TypeMismatch=>"TypeMismatch",InvalidScope(_)=>"InvalidScope (scope depth > 8 in ActionScript)",_=>"Other"}
}
fn compat(data:&Path)->Result<()> {
 let root=data.join("Packages");let files=syntax_files(&root);
 let mut folder_times=Vec::new();let mut folder_error=String::new();let mut folder_loaded=0;
 for _ in 0..5 {let mut b=SyntaxSetBuilder::new();let (r,ms)=timed(||b.add_from_folder(&root,true));folder_times.push(ms);folder_loaded=b.syntaxes().len();folder_error=r.err().map(|e|e.to_string()).unwrap_or_default();}
 let mut b=SyntaxSetBuilder::new();b.add_plain_text_syntax();let mut rows=Vec::new();let mut counts=BTreeMap::<String,usize>::new();let mut features=BTreeMap::<String,usize>::new();
 let now=std::time::Instant::now();
 for path in files {
  let s=fs::read_to_string(&path)?;let flags=feature_flags(&s);
  for (k,v) in flags.as_object().unwrap(){if v.as_u64().unwrap()>0{*features.entry(k.clone()).or_default()+=1;}}
  let mut row=json!({"file":path.strip_prefix(&root)?.to_string_lossy(),"features":flags});
  match SyntaxDefinition::load_from_str(&s,true,path.file_stem().and_then(|s|s.to_str())) {
   Ok(d)=>{row["ok"]=json!(true);row["name"]=json!(d.name);b.add(d);},
   Err(e)=>{let c=category(&e);*counts.entry(c.to_owned()).or_default()+=1;row["ok"]=json!(false);row["category"]=json!(c);row["error"]=json!(e.to_string());row["debug_error"]=json!(format!("{e:?}"));}
  }rows.push(row);
 }
 let ss=b.build();let total_ms=now.elapsed().as_secs_f64()*1000.0;
 let unlinked=ss.find_unlinked_contexts().into_iter().collect::<Vec<_>>();
 dumps::dump_to_uncompressed_file(&ss,data.join(format!("syntax-{}.packdump",backend())))?;
 let output=json!({"bench":"syntax_compat","backend":backend(),"total":rows.len(),"ok":rows.iter().filter(|r|r["ok"]==true).count(),"failed":rows.iter().filter(|r|r["ok"]==false).count(),"folder_loaded_before_error":folder_loaded,"folder_error":folder_error,"folder_attempt":stats(&folder_times),"isolated_load_and_build_ms":total_ms,"failure_categories":counts,"feature_file_counts":features,"unlinked_count":unlinked.len(),"unlinked_contexts":unlinked,"files":rows});
 fs::write(data.join(format!("syntax-{}-compat.json",backend())),serde_json::to_string_pretty(&output)?)?;emit(output);Ok(())
}
fn startup(data:&Path)->Result<()> {
 let rows:Value=serde_json::from_str(&fs::read_to_string(data.join(format!("syntax-{}-compat.json",backend())))?)?;
 let paths=rows["files"].as_array().unwrap().iter().filter(|r|r["ok"]==true).map(|r|data.join("Packages").join(r["file"].as_str().unwrap())).collect::<Vec<_>>();
 let mut yaml=Vec::new();let mut binary=Vec::new();let mut bundled=Vec::new();let mut count=0;
 for _ in 0..5 {
  let (ss,ms)=timed(||->Result<SyntaxSet>{let mut b=SyntaxSetBuilder::new();b.add_plain_text_syntax();for p in &paths {b.add(SyntaxDefinition::load_from_str(&fs::read_to_string(p)?,true,p.file_stem().and_then(|s|s.to_str()))?);}Ok(b.build())});count=ss?.syntaxes().len();yaml.push(ms);
  let (ss,ms)=timed(||successful_set(data));black_box(ss?);binary.push(ms);
  let (ss,ms)=timed(SyntaxSet::load_defaults_newlines);black_box(ss);bundled.push(ms);
 }
 let ss=successful_set(data)?;let (_,dump_ms)=timed(||dumps::dump_to_uncompressed_file(&ss,data.join(format!("syntax-{}.packdump",backend()))));
 emit(json!({"bench":"syntax_startup","backend":backend(),"syntax_count_including_plain":count,"yaml_same_successful_subset":stats(&yaml),"dump_same_successful_subset":stats(&binary),"bundled_defaults_dump":stats(&bundled),"dump_creation_ms":dump_ms,"dump_bytes":fs::metadata(data.join(format!("syntax-{}.packdump",backend())))?.len(),"note":"YAML includes IO+regex validation+linking; dumps are lazy on context/regex first use; metadata not in custom dump"}));Ok(())
}
fn throughput(data:&Path,args:&[String])->Result<()> {
 let language=arg(args,"--language","Rust");let mode=arg(args,"--mode","parse");
 let file=match language.as_str(){"Rust"=>"rust-10mb.rs","JavaScript"=>"javascript-10mb.js","JSON"=>"json-20mb.json","Markdown"=>"markdown-5mb.md",_=>bail!("unsupported language")};
 let text=fs::read_to_string(data.join(file))?;let lines=LinesWithEndings::from(&text).count();
 let official=successful_set(data)?;let default=SyntaxSet::load_defaults_newlines();let source=arg(args,"--source","auto");
 let (ss,source)=if source!="bundled"&&official.find_syntax_by_name(&language).is_some(){(&official,"official successfully loaded subset")}else{(&default,"syntect bundled defaults")};
 let syntax=ss.find_syntax_by_name(&language).ok_or_else(||anyhow!("no syntax {language}"))?;
 let themes=ThemeSet::load_defaults();let theme=&themes.themes["base16-ocean.dark"];let mut times=Vec::new();let mut checksum=0usize;
 for _ in 0..5 {
  let (r,ms)=timed(||std::panic::catch_unwind(std::panic::AssertUnwindSafe(||->Result<usize>{
   let mut n=0usize;
   if mode=="parse"{let mut p=ParseState::new(syntax);for l in LinesWithEndings::from(&text){let ops=p.parse_line(l,ss)?;n=n.wrapping_add(ops.len());}}
   else{let mut h=HighlightLines::new(syntax,theme);for l in LinesWithEndings::from(&text){let spans=h.highlight_line(l,ss)?;for (s,t) in spans{n=n.wrapping_add(t.len()+s.foreground.r as usize);}}}
   Ok(black_box(n))
  })));
  let r=match r {Ok(r)=>r,Err(p)=>Err(anyhow!("runtime panic: {}",panic_message(p)))};
  match r {Ok(n)=>checksum=n,Err(e)=>{emit(json!({"bench":"syntax_throughput","backend":backend(),"language":language,"mode":mode,"source":source,"status":"error","error":e.to_string(),"elapsed_ms":ms}));return Ok(());}}
  times.push(ms);
 }
 let s=stats(&times);let sec=s["median_ms"].as_f64().unwrap()/1000.0;
 emit(json!({"bench":"syntax_throughput","backend":backend(),"language":language,"mode":mode,"source":source,"syntax_name":syntax.name,"status":"ok","bytes":text.len(),"lines":lines,"time":s,"mb_s":text.len() as f64/1e6/sec,"lines_s":lines as f64/sec,"checksum":checksum}));Ok(())
}
fn tree(data:&Path,args:&[String])->Result<()> {
 use tree_sitter::{InputEdit,Parser,Point};
 let language=arg(args,"--language","Rust");
 let (name,grammar)=match language.as_str(){"Rust"=>("rust-10mb.rs",tree_sitter_rust::LANGUAGE.into()),"JavaScript"=>("javascript-10mb.js",tree_sitter_javascript::LANGUAGE.into()),_=>bail!("unsupported tree grammar")};
 let text=fs::read_to_string(data.join(name))?;let mut parser=Parser::new();parser.set_language(&grammar)?;let mut full=Vec::new();let mut inc=Vec::new();let mut last=None;
 for _ in 0..5 {let (tree,ms)=timed(||parser.parse(&text,None));let tree=tree.ok_or_else(||anyhow!("tree parse canceled"))?;full.push(ms);last=Some(tree);}
 let original=last.unwrap();let mut offset=text.len()/2;while !text.is_char_boundary(offset){offset+=1;}
 let c=text[offset..].chars().next().unwrap();let mut edited=text.clone();edited.insert(offset,'x');
 let row=text[..offset].bytes().filter(|b|*b==b'\n').count();let col=text[..offset].rfind('\n').map(|p|offset-p-1).unwrap_or(offset);let start=Point::new(row,col);
 let edit=InputEdit{start_byte:offset,old_end_byte:offset,new_end_byte:offset+1,start_position:start,old_end_position:start,new_end_position:Point::new(row,col+1)};
 let mut changed_ranges=0;let mut has_error=false;
 for _ in 0..5 {let mut old=original.clone();old.edit(&edit);let (t,ms)=timed(||parser.parse(&edited,Some(&old)));let t=t.ok_or_else(||anyhow!("incremental parse canceled"))?;inc.push(ms);changed_ranges=old.changed_ranges(&t).count();has_error=t.root_node().has_error();black_box(t);}
 emit(json!({"bench":"tree_sitter","language":language,"bytes":text.len(),"full":stats(&full),"incremental_1char":stats(&inc),"edit_offset":offset,"following_char":c.to_string(),"original_has_error":original.root_node().has_error(),"edited_has_error":has_error,"changed_ranges":changed_ranges,"note":"concatenated source; full parse may contain ERROR nodes; edit application and string clone excluded; native Parser::parse(String)"}));Ok(())
}
fn main()->Result<()> {
 let args=std::env::args().collect::<Vec<_>>();let data=data_dir(&args);let command=args.get(1).map(|s|s.as_str()).unwrap_or("compat");
 match command {"compat"=>compat(&data),"tests"=>syntest::run(&data,backend()),"throughput"=>throughput(&data,&args),"startup"=>startup(&data),"tree"=>tree(&data,&args),_=>bail!("commands: compat/tests/throughput/startup/tree")}
}
