use crate::{frontend::{self, Class, Translation}, loader::Context};
use anyhow::{Result, Context as _};
use onig::{Regex as Onig, Region, SearchOptions};
use regex_automata::{meta::{Regex, Cache}, util::syntax, nfa::thompson::WhichCaptures, Anchored, Input, MatchKind};
use serde::Serialize;
use std::{collections::BTreeMap, time::Instant};

type Span = (usize, usize);
const OPTIONS: SearchOptions = SearchOptions::SEARCH_OPTION_NONE;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hit {
    pub start: usize,
    pub end: usize,
    pub pattern: usize,
    pub captures: Vec<Option<Span>>,
}

#[derive(Serialize, Default)]
pub struct CompileStats {
    pub onig_ms: f64,
    pub frontend_ms: f64,
    pub fancy_ms: f64,
    pub onig_errors: BTreeMap<usize, String>,
    pub fancy_errors: BTreeMap<usize, String>,
    pub classes: BTreeMap<String, usize>,
    pub reasons: BTreeMap<String, usize>,
}

pub struct Pool {
    pub onig: Vec<Option<Onig>>,
    pub fancy: Vec<Option<fancy_regex::Regex>>,
    pub translated: Vec<Translation>,
    pub stats: CompileStats,
}

impl Pool {
    pub fn compile(patterns: &[String], with_fancy: bool) -> Self {
        let mut stats = CompileStats::default();
        let t = Instant::now();
        let onig = patterns.iter().enumerate().map(|(i,p)| match Onig::new(p) {
            Ok(r) => Some(r), Err(e) => {stats.onig_errors.insert(i,e.to_string());None}
        }).collect();
        stats.onig_ms = t.elapsed().as_secs_f64()*1000.;
        let t=Instant::now();
        let translated: Vec<_> = patterns.iter().map(|p|frontend::translate(p)).collect();
        stats.frontend_ms=t.elapsed().as_secs_f64()*1000.;
        for p in &translated {
            *stats.classes.entry(format!("{:?}",p.class)).or_default()+=1;
            if !p.reason.is_empty() {*stats.reasons.entry(p.reason.clone()).or_default()+=1;}
        }
        let t=Instant::now();
        let fancy=translated.iter().enumerate().map(|(i,p)| {
            if !with_fancy {return None;}
            match fancy_regex::RegexBuilder::new(&frontend::for_fancy(&p.normalized)).multi_line(true).build() {
                Ok(r)=>Some(r),Err(e)=>{stats.fancy_errors.insert(i,e.to_string());None}
            }
        }).collect();
        stats.fancy_ms=t.elapsed().as_secs_f64()*1000.;
        Pool {onig,fancy,translated,stats}
    }
}

#[derive(Clone, Copy, Default)]
struct Cached { known: bool, span: Option<Span> }
impl Cached {
    fn usable(self,pos:usize,dependent:bool)->bool { !dependent && self.known && self.span.is_none_or(|s|pos<=s.0) }
}

fn onig_find(re:&Onig,line:&str,pos:usize,region:&mut Region)->Result<Option<Span>> {
    let found = re.search_with_param(line,pos,line.len(),OPTIONS,Some(region),onig::MatchParam::default())?;
    Ok(found.and_then(|_|region.pos(0)))
}

fn onig_anchor(re:&Onig,line:&str,pos:usize,region:&mut Region)->Result<Option<Span>> {
    let found = re.match_with_param(line,pos,OPTIONS,Some(region),onig::MatchParam::default())?;
    Ok(found.and_then(|_|region.pos(0)))
}

fn captures_onig(pool:&Pool,gid:usize,line:&str,query:usize,span:Span,region:&mut Region)->Result<Vec<Option<Span>>> {
    let Some(re)=&pool.onig[gid] else {return Ok(vec![]);};
    let ok=if pool.translated[gid].position_dependent || pool.translated[gid].normalized.contains("\\K") {
        onig_find(re,line,query,region)?
    } else {onig_anchor(re,line,span.0,region)?};
    anyhow::ensure!(ok.is_some(),"winner did not reproduce for captures, gid={gid} span={span:?}");
    Ok((0..region.len()).map(|i|region.pos(i)).collect())
}

#[derive(Clone, Copy)]
pub enum BaselineKind {Onig, Fancy}

pub struct Baseline<'a> {
    pool:&'a Pool, context:&'a Context, kind:BaselineKind,
    cache:Vec<Cached>, region:Region,
    pub runtime_fallbacks:u64,
}

impl<'a> Baseline<'a> {
    pub fn new(pool:&'a Pool,context:&'a Context,kind:BaselineKind)->Self {
        Self{pool,context,kind,cache:vec![Cached::default();context.patterns.len()],region:Region::new(),runtime_fallbacks:0}
    }
    pub fn reset_line(&mut self) {self.cache.fill(Cached::default());}
    pub fn find(&mut self,line:&str,pos:usize)->Result<Option<Hit>> {
        let mut best:Option<(usize,usize,usize)>=None;
        for (index,&gid) in self.context.patterns.iter().enumerate() {
            let Some(onig)=&self.pool.onig[gid] else {continue;};
            if !self.cache[index].usable(pos,self.pool.translated[gid].position_dependent) {
                let span=match (self.kind,&self.pool.fancy[gid]) {
                    (BaselineKind::Fancy,Some(re))=>match re.find_from_pos(line,pos) {
                        Ok(m)=>m.map(|m|(m.start(),m.end())),
                        Err(_)=>{self.runtime_fallbacks+=1;onig_find(onig,line,pos,&mut self.region)?}
                    },
                    _=>onig_find(onig,line,pos,&mut self.region).with_context(||format!("baseline rule {index}, gid {gid}"))?,
                };
                self.cache[index]=Cached{known:true,span};
            }
            if let Some((start,end))=self.cache[index].span {
                if best.is_none_or(|b|start<b.0) {best=Some((start,end,index));}
                // Exact start cannot be beaten by a later rule.
                if start==pos {break;}
            }
        }
        let Some((start,end,index))=best else {return Ok(None);};
        let gid=self.context.patterns[index];
        let captures=match (self.kind,&self.pool.fancy[gid]) {
            (BaselineKind::Fancy,Some(re))=>{
                let from=if self.pool.translated[gid].position_dependent {pos} else {start};
                match re.captures_from_pos(line,from) {
                    Ok(Some(c))=>(0..c.len()).map(|i|c.get(i).map(|m|(m.start(),m.end()))).collect(),
                    _=>{self.runtime_fallbacks+=1;captures_onig(self.pool,gid,line,pos,(start,end),&mut self.region)?}
                }
            },
            _=>captures_onig(self.pool,gid,line,pos,(start,end),&mut self.region)?,
        };
        Ok(Some(Hit{start,end,pattern:index,captures}))
    }
}

struct BoundaryGuard {positive:bool,leading:bool,re:Regex}
struct Fast {index:usize,gid:usize,guards:Vec<BoundaryGuard>,zero_width_trigger:bool,has_line_anchor:bool}

pub struct Multi {
    re:Option<Regex>,fast:Vec<Fast>,fallback:Vec<usize>,
    pub compile_ms:f64,
    pub compile_errors:Vec<String>,
    pub memory_bytes:usize,
    pub fast_patterns:usize,
    pub boundary_patterns:usize,
    pub trigger_patterns:usize,
}

fn compile_many(patterns:&[String])->Result<Regex> {
    Regex::builder()
        .configure(Regex::config().match_kind(MatchKind::LeftmostFirst).which_captures(WhichCaptures::Implicit).nfa_size_limit(Some(32*1024*1024)).hybrid_cache_capacity(16*1024*1024))
        .syntax(syntax::Config::new().multi_line(true))
        .build_many(patterns).context("multi meta compilation")
}

impl Multi {
    pub fn compile(pool:&Pool,context:&Context)->Self {
        Self::compile_with_triggers(pool,context,true)
    }
    pub fn compile_with_triggers(pool:&Pool,context:&Context,triggers:bool)->Self {
        let t=Instant::now();let mut fast=vec![];let mut fallback=vec![];let mut cores=vec![];let mut errors=vec![];let mut boundaries=0;
        let mut trigger_patterns=0;
        for (index,&gid) in context.patterns.iter().enumerate() {
            if pool.onig[gid].is_none(){continue;}
            let p=&pool.translated[gid];
            if p.class==Class::Backtracking {fallback.push(index);continue;}
            let mut guards=vec![];let mut failed=false;
            for g in &p.guards {
                match compile_many(std::slice::from_ref(&g.expr)) {
                    Ok(re)=>guards.push(BoundaryGuard{positive:g.positive,leading:g.leading,re}),
                    Err(e)=>{errors.push(format!("pattern {index}, guard: {e:#}"));failed=true;break;}
                }
            }
            if failed{fallback.push(index);continue;}
            if !guards.is_empty(){boundaries+=1;}
            let core=p.core.as_ref().unwrap();
            let zero=triggers && regex_syntax::ParserBuilder::new().multi_line(true).build().parse(core).is_ok_and(|h|h.properties().maximum_len()==Some(0));
            let trigger=p.guards.iter().find(|g|g.positive).filter(|_|zero);
            // An assertion-only pattern otherwise adds epsilon to the union,
            // masking every later rule at every character. A positive guard
            // provides an exact candidate-start locator; its consumed end is
            // discarded and the original zero-width core is restored.
            if let Some(g)=trigger {cores.push(format!("(?:{core})(?:{})",g.expr));trigger_patterns+=1;}
            else {cores.push(core.clone());}
            fast.push(Fast{index,gid,guards,zero_width_trigger:trigger.is_some(),has_line_anchor:p.normalized.contains('^')});
        }
        let re=if cores.is_empty(){None}else{match compile_many(&cores){
            Ok(r)=>Some(r),Err(e)=>{errors.push(format!("context {}: {e:#}",context.name));fallback.extend(fast.iter().map(|f|f.index));fast.clear();boundaries=0;None}
        }};
        fallback.sort_unstable();
        let memory_bytes=re.as_ref().map_or(0,Regex::memory_usage)+fast.iter().flat_map(|f|&f.guards).map(|g|g.re.memory_usage()).sum::<usize>();
        Self{fast_patterns:fast.len(),boundary_patterns:boundaries,trigger_patterns,re,fast,fallback,compile_ms:t.elapsed().as_secs_f64()*1000.,compile_errors:errors,memory_bytes}
    }
    pub fn runner<'a>(&'a self,pool:&'a Pool,context:&'a Context)->MultiRunner<'a> {
        MultiRunner{multi:self,pool,context,cache:self.re.as_ref().map(Regex::create_cache),
            fallback_cache:vec![Cached::default();self.fallback.len()],fast_cache:Cached::default(),region:Region::new(),
            fast_pattern_index:0,rejected_candidates:0,repair_checks:0,shorter_core_rescues:0}
    }
}

pub struct MultiRunner<'a> {
    multi:&'a Multi,pool:&'a Pool,context:&'a Context,cache:Option<Cache>,
    fallback_cache:Vec<Cached>,fast_cache:Cached,fast_pattern_index:usize,region:Region,
    pub rejected_candidates:u64,pub repair_checks:u64,pub shorter_core_rescues:u64,
}

impl MultiRunner<'_> {
    pub fn reset_line(&mut self){self.fallback_cache.fill(Cached::default());self.fast_cache=Cached::default();}
    pub fn cache_bytes(&self)->usize {self.cache.as_ref().map_or(0,Cache::memory_usage)}
    fn fast_find(&mut self,line:&str,pos:usize)->Result<Option<(usize,usize,usize)>> {
        let Some(re)=&self.multi.re else{return Ok(None);};
        let mut at=pos;
        while at<=line.len(){
            let Some(m)=re.search_with(self.cache.as_mut().unwrap(),&Input::new(line).range(at..)) else {return Ok(None);};
            let f=&self.multi.fast[m.pattern().as_usize()];
            let end=if f.zero_width_trigger{m.start()}else{m.end()};
            let guards_ok=f.guards.iter().all(|g|g.re.is_match(Input::new(line).range(if g.leading{m.start()}else{end}..).anchored(Anchored::Yes))==g.positive);
            let terminal_anchor=f.has_line_anchor && m.start()==line.len() && line.ends_with('\n');
            if guards_ok && (!terminal_anchor || onig_anchor(self.pool.onig[f.gid].as_ref().unwrap(),line,m.start(),&mut self.region)?.is_some()) {
                return Ok(Some((m.start(),end,f.index)));
            }
            // A failed guard can require a shorter core, another alternative,
            // or a lower-priority rule AT THE SAME START. Enumerating only
            // non-overlapping DFA matches loses all three cases.
            self.rejected_candidates+=1;
            for alternative in &self.multi.fast {
                self.repair_checks+=1;
                if let Some((s,e))=onig_anchor(self.pool.onig[alternative.gid].as_ref().unwrap(),line,m.start(),&mut self.region)? {
                    if alternative.index==f.index && e!=m.end(){self.shorter_core_rescues+=1;}
                    return Ok(Some((s,e,alternative.index)));
                }
            }
            if m.start()==line.len(){return Ok(None);}
            at=next_boundary(line,m.start()+1);
        }
        Ok(None)
    }
    pub fn find(&mut self,line:&str,pos:usize)->Result<Option<Hit>> {
        // Store the pattern index separately: a single union cache represents
        // the next *verified* winner, not an unverified core candidate.
        let fast=if self.fast_cache.usable(pos,false) {
            self.fast_cache.span.map(|(s,e)|(s,e,self.cached_fast_index()))
        } else {
            let m=self.fast_find(line,pos)?;
            self.fast_cache=Cached{known:true,span:m.map(|(s,e,_)|(s,e))};
            if let Some((_,_,i))=m{self.fast_index_set(i);}
            m
        };
        let mut best=fast;
        for (j,&index) in self.multi.fallback.iter().enumerate(){
            // Like the baseline, never evaluate a lower-priority rule after
            // an exact-position winner. This also avoids irrelevant Onig
            // retry-limit errors from patterns the reference never executes.
            if best.is_some_and(|b|b.0==pos && index>=b.2){break;}
            let gid=self.context.patterns[index];
            if !self.fallback_cache[j].usable(pos,self.pool.translated[gid].position_dependent){
                let span=onig_find(self.pool.onig[gid].as_ref().unwrap(),line,pos,&mut self.region)
                    .with_context(||format!("B fallback rule {index}, gid {gid}"))?;
                self.fallback_cache[j]=Cached{known:true,span};
            }
            if let Some((s,e))=self.fallback_cache[j].span {
                if best.is_none_or(|b|(s,index)<(b.0,b.2)){best=Some((s,e,index));}
                if s==pos{break;}
            }
        }
        let Some((start,end,index))=best else{return Ok(None);};
        let captures=captures_onig(self.pool,self.context.patterns[index],line,pos,(start,end),&mut self.region)?;
        Ok(Some(Hit{start,end,pattern:index,captures}))
    }
    // Separate storage avoids tying a verified span to regex-automata's ID,
    // which need not equal context priority when some rules use Onig.
    fn cached_fast_index(&self)->usize { self.fast_pattern_index }
    fn fast_index_set(&mut self,i:usize) {self.fast_pattern_index=i;}
}

/// Driver positions are byte offsets. After an empty UTF-8 match, advance one
/// scalar boundary instead of passing an invalid UTF-8 offset to a regex API.
pub fn next_boundary(line:&str,mut pos:usize)->usize {
    while pos<line.len() && !line.is_char_boundary(pos){pos+=1;} pos
}

#[cfg(test)]
mod tests {
    use super::*;
    fn compare(patterns:&[&str],lines:&[&str]) {
        let patterns:Vec<String>=patterns.iter().map(|p|p.to_string()).collect();
        let pool=Pool::compile(&patterns,true);
        assert!(pool.stats.onig_errors.is_empty(),"{:?}",pool.stats.onig_errors);
        let ctx=Context{name:"test".into(),patterns:(0..patterns.len()).collect()};
        let multi=Multi::compile(&pool,&ctx);
        assert!(multi.compile_errors.is_empty());
        let mut a=Baseline::new(&pool,&ctx,BaselineKind::Onig);let mut b=multi.runner(&pool,&ctx);
        for &line in lines {
            // Every starting position, in increasing order, also exercises
            // cache reuse across queries not present in a token-only driver.
            a.reset_line();b.reset_line();
            for pos in (0..=line.len()).filter(|&p|line.is_char_boundary(p)) {
                assert_eq!(a.find(line,pos).unwrap(),b.find(line,pos).unwrap(),"patterns={patterns:?},line={line:?},pos={pos}");
            }
        }
    }
    #[test] fn rejected_guard_preserves_same_position_priority() {
        compare(&["a(?!b)","(a)b","b","(?=a)","z"],&["ab","aa","zab","ab\n",""]);
        compare(&["(?=z)a","(a)","b","z","(?<=x)y"],&["a","ab","za","xya"]);
    }
    #[test] fn shorter_core_and_alternative_are_reconsidered() {
        compare(&["(a*)(?=a)","a+","b","z","x"],&["a","aaa","baaa","aaa\n"]);
        compare(&["(?:ab|a)(?=b)","a","b","z","x"],&["ab","abb","aab"]);
        compare(&["(?i:(a+)(?!b))","a+","b","z","x"],&["Aab","AAA","aaa"]);
    }
    #[test] fn anchoring_cache_flags_classes_and_captures() {
        compare(&[r"\G(a)","(b)","(?m:c.d)",r"\h+",r"[[:alpha:]]+"],&["aaba","baba","c\nd","0aF","é中文\n"]);
        compare(&[r"a\K(b)",r"a++a","ab",r"\Z",r"(?x: [ #] \h+ )"],&["ab","aab","aaa","#fa"," a1","ab\n",""]);
    }
    #[test] fn exhaustive_small_alphabet_queries() {
        let cases:Vec<Vec<&str>>=vec![
            vec!["a(?=b)","(a*)(?=a)","(?=b)","(b+)","(?!a)"],
            vec!["(?=ab)ab","a(?!b)","a","b","$"],
            vec!["(a)\\1","a(?=b)c","(?<=a)b","a+","b"],
            vec!["(?x) (a) #foo\n (?!b)","^a","b$","\\Z","."],
        ];
        let mut strings=vec![String::new()];
        for len in 1..=5 {for mut code in 0..3usize.pow(len){let mut s=String::new();for _ in 0..len{s.push(['a','b','\n'][code%3]);code/=3;}strings.push(s);}}
        let refs:Vec<_>=strings.iter().map(String::as_str).collect();
        for p in cases{compare(&p,&refs);}
    }
    #[test] fn ruby_escapes_abbreviated_repetition_and_terminal_newline() {
        compare(&[r"\<",r"\>",r":/{,2}",r"^\s*$\n?",r"^(?=\s*$)",r"^(?!>)"],&["<word>",":",":/","://",":///","\n","text\n",""," >\n"]);
    }
    #[test] fn multiple_guards_unicode_posix_and_literal_extended_space() {
        compare(&[r"(?!ab)(?=a)(a+)(?=b)(?!bc)",r"[[:punct:]]+",r"(?x) [ #] \h+",r"[[:word:]]+",r"\é"],&["aaab","aabc","€!","#fF"," a1","é中文","a\u{200c}b"]);
    }
}

