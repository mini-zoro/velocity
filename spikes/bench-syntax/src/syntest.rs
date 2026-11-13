//! Scope-test logic adapted from syntect v5.3.0 examples/syntest.rs (MIT, (c) Tristan Hume).
//! Extensions: modern header modifiers; explicit skips for symbol/ref annotations; no unwrap on
//! parser/selector errors; both assertion-span and asserted-character counts; byte -> char columns.
use anyhow::Result;
use bench_common::emit;
use regex::Regex;
use serde_json::{json,Value};
use std::{fs,path::Path,str::FromStr,sync::LazyLock};
use syntect::{easy::ScopeRegionIterator,highlighting::ScopeSelectors,parsing::{ParseState,Scope,ScopeStack,SyntaxReference,SyntaxSet}};
use walkdir::WalkDir;
static HEADER:LazyLock<Regex>=LazyLock::new(||Regex::new(r#"^\s*(?P<start>\S+?)\s*SYNTAX\s+TEST\s+(?:(?P<modifier>[^\s"]+)\s+)?"(?P<path>[^"]+)"(?:\s+(?P<end>[^\s\w]+))?(?:\s.*)?$"#).unwrap());
static MARK:LazyLock<Regex>=LazyLock::new(||Regex::new(r"^\s*(?P<mark><-|\^+|@+|>)(?P<selector>.*)$").unwrap());
struct Annotation<'a>{begin:usize,end:usize,selector:&'a str,reference:bool}
fn annotation<'a>(line:&'a str,start:&str,end:Option<&str>)->Option<Annotation<'a>>{
 let line=line.trim_end_matches(['\r','\n']);
 let at=line.find(start)?;let tail=&line[at+start.len()..];let c=MARK.captures(tail)?;let m=c.name("mark")?;
 let reference=m.as_str().starts_with('@')||m.as_str()==">";
 if reference {let after=&tail[m.end()..];if !after.is_empty()&&!after.starts_with(char::is_whitespace){return None;}}
 let s=c.name("selector")?.as_str();let selector=if let Some(end)=end{s.split(end).next().unwrap()}else{s};
 let (begin,end)=if m.as_str()=="<-"||m.as_str()==">"{let b=line[..at].chars().count();(b,b+1)}else{let b=line[..at+start.len()+m.start()].chars().count();(b,b+m.as_str().chars().count())};
 Some(Annotation{begin,end,selector,reference})
}
#[derive(Default)]struct Counts{cases:usize,passed:usize,failed:usize,cells:usize,passed_cells:usize,failed_cells:usize}
fn evaluate(text:&str,start:&str,end:Option<&str>,ss:&SyntaxSet,syntax:&SyntaxReference)->Value{
 let mut state=ParseState::new(syntax);let mut stack=ScopeStack::new();let mut scopes=Vec::<(usize,usize,Vec<Scope>)>::new();let mut count=Counts::default();let mut examples=Vec::new();let mut error=None;let mut references=0;
 for (n,line) in text.split_inclusive('\n').enumerate(){
  let a=annotation(line,start,end);
  if let Some(a)=&a {
   if a.reference{references+=1;}else{
    let selector=match ScopeSelectors::from_str(&format!(" {}",a.selector.trim())){Ok(s)=>s,Err(e)=>{error=Some(format!("selector at line {}: {e}",n+1));break;}};
    count.cases+=1;count.cells+=a.end-a.begin;let mut failed=0;
    for col in a.begin..a.end{
     let region=scopes.iter().find(|(b,e,_)|*b<=col&&col<*e).or(scopes.last());
     let pass=region.is_some_and(|(_,_,s)|selector.does_match(s).is_some());
     if pass{count.passed_cells+=1;}else{count.failed_cells+=1;failed+=1;}
    }
    if failed==0{count.passed+=1;}else{count.failed+=1;if examples.len()<8{examples.push(json!({"assertion_line":n+1,"selector":a.selector.trim(),"begin_char":a.begin,"end_char":a.end,"actual_scopes":scopes.iter().find(|(b,e,_)|*b<=a.begin&&a.begin<*e).or(scopes.last()).map(|(_,_,s)|format!("{s:?}"))}));}}
   }
  }
  // Match the released syntect syntest default: parse_test_lines=true. Assertions target
  // the last non-annotation source line; annotation lines still affect parser state.
  let parsed=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||state.parse_line(line,ss)));
  let ops=match parsed{Ok(Ok(v))=>v,Ok(Err(e))=>{error=Some(format!("parse at line {}: {e}",n+1));break;},Err(p)=>{error=Some(format!("runtime panic at line {}: {}",n+1,super::panic_message(p)));break;}};
  if a.is_none(){scopes.clear();}
  let mut col=0;
  for (s,op) in ScopeRegionIterator::new(&ops,line){
   if let Err(e)=stack.apply(op){error=Some(format!("scope stack at line {}: {e}",n+1));break;}
   let len=s.chars().count();if len>0&&a.is_none(){scopes.push((col,col+len,stack.as_slice().to_vec()));col+=len;}
  }
  if error.is_some(){break;}
 }
 json!({"checked_cases":count.cases,"passed_cases":count.passed,"failed_cases":count.failed,"checked_cells":count.cells,"passed_cells":count.passed_cells,"failed_cells":count.failed_cells,"error":error,"examples":examples,"reference_symbol_markers_skipped":references})
}
fn self_check()->Result<()> {
 use syntect::parsing::{SyntaxDefinition,SyntaxSetBuilder};
 let def=SyntaxDefinition::load_from_str("name: Fixture\nscope: source.fixture\ncontexts:\n  main:\n    - match: '[a-z]+'\n      scope: keyword.fixture\n",true,None)?;
 let mut b=SyntaxSetBuilder::new();b.add(def);let ss=b.build();
 let text="   abc 中\n// ^^^ keyword.fixture\n// <- source.fixture\n//     ^ -comment\n";
 let r=evaluate(text,"//",None,&ss,&ss.syntaxes()[0]);assert_eq!(r["checked_cases"],3);assert_eq!(r["failed_cases"],0);assert!(r["error"].is_null());Ok(())
}
pub fn run(data:&Path,backend:&str)->Result<()> {
 self_check()?;
 let root=data.join("Packages");let official=super::successful_set(data)?;let defaults=SyntaxSet::load_defaults_newlines();
 let compat:Value=serde_json::from_str(&fs::read_to_string(data.join(format!("syntax-{backend}-compat.json")))?)?;
 let specs=[("Rust","Rust/Rust.sublime-syntax"),("Python","Python/Python.sublime-syntax"),("JavaScript","JavaScript/JavaScript.sublime-syntax"),("C++","C++/C++.sublime-syntax"),("JSON","JSON/JSON.sublime-syntax"),("Markdown","Markdown/Markdown.sublime-syntax"),("HTML","HTML/HTML.sublime-syntax"),("YAML","YAML/YAML.sublime-syntax")];
 let mut tests=Vec::new();let mut excluded=Vec::new();
 for entry in WalkDir::new(&root).into_iter().filter_map(|e|e.ok()).filter(|e|e.file_type().is_file()&&e.file_name().to_string_lossy().starts_with("syntax_test_")){
  let path=entry.path();let text=fs::read_to_string(path)?.replace('\r',"");let first=text.lines().next().unwrap_or("");
  let Some(header)=HEADER.captures(first)else{excluded.push(json!({"file":path.strip_prefix(&root)?.to_string_lossy(),"reason":"header malformed/unrecognized"}));continue;};
  let declared=header.name("path").unwrap().as_str().strip_prefix("Packages/").unwrap_or(header.name("path").unwrap().as_str());
  let Some(&(language,_))=specs.iter().find(|(_,p)|*p==declared)else{excluded.push(json!({"file":path.strip_prefix(&root)?.to_string_lossy(),"reason":"outside the eight canonical target syntax paths","declared":declared}));continue;};
  if let Some(m)=header.name("modifier"){excluded.push(json!({"file":path.strip_prefix(&root)?.to_string_lossy(),"reason":format!("unsupported non-scope test modifier: {}",m.as_str()),"language":language}));continue;}
  let start=header.name("start").unwrap().as_str();let end=header.name("end").map(|m|m.as_str());
  let (mut cases,mut cells,mut refs)=(0,0,0);
  for line in text.split_inclusive('\n'){if let Some(a)=annotation(line,start,end){if a.reference{refs+=1;}else{cases+=1;cells+=a.end-a.begin;}}}
  if cases==0 {excluded.push(json!({"file":path.strip_prefix(&root)?.to_string_lossy(),"language":language,"reason":"no scope assertions (symbol-only/fixture)","reference_symbol_markers":refs}));continue;}
  let row=compat["files"].as_array().unwrap().iter().find(|r|r["file"]==declared).unwrap();
  for (source,ss) in [("official",&official),("bundled",&defaults)]{
   let syntax=if source=="official"&&row["ok"]!=true {None}else{ss.find_syntax_by_name(language)};
   let mut r=json!({"file":path.strip_prefix(&root)?.to_string_lossy(),"language":language,"source":source,"planned_cases":cases,"planned_cells":cells,"symbol_reference_skips":refs});
   if let Some(syntax)=syntax {let result=evaluate(&text,start,end,ss,syntax);r["result"]=result;r["status"]=json!(if r["result"]["error"].is_null(){"completed"}else{"parser_error"});}
   else{r["status"]=json!("blocked_syntax_not_loaded");r["load_error"]=row["error"].clone();}
   tests.push(r);
  }
 }
 tests.sort_by_key(|r|format!("{} {} {}",r["language"],r["source"],r["file"]));
 let output=json!({"bench":"syntax_tests","backend":backend,"harness":"adapted syntect v5.3.0 syntest; parse_test_lines=true; ^ and <- scope assertions; header modifiers excluded; @/> explicitly skipped","harness_self_check":"passed (positive, negative, <- and Unicode line)","tests":tests,"excluded":excluded});
 fs::write(data.join(format!("syntax-{backend}-tests.json")),serde_json::to_string_pretty(&output)?)?;emit(output);Ok(())
}
