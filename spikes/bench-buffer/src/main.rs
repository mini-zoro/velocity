use anyhow::{bail, Result};
use bench_common::{arg, emit, repeats, stats, timed, Rng};
use serde_json::json;
use std::{fs::File, hint::black_box, io::{BufReader, Read}, path::Path};

trait Text: Clone {
    fn load(path: &Path) -> Result<Self>;
    fn len(&self) -> usize;
    fn lines(&self) -> usize;
    fn to_line(&self, pos: usize) -> usize;
    fn from_line(&self, line: usize) -> usize;
    fn insert(&mut self, pos: usize, s: &str);
    fn delete(&mut self, pos: usize);
}
impl Text for ropey::Rope {
    fn load(p: &Path) -> Result<Self> { Ok(Self::from_reader(BufReader::new(File::open(p)?))?) }
    fn len(&self) -> usize { self.len_chars() }
    fn lines(&self) -> usize { self.len_lines() }
    fn to_line(&self, p: usize) -> usize { self.char_to_line(p) }
    fn from_line(&self, l: usize) -> usize { self.line_to_char(l) }
    fn insert(&mut self, p: usize, s: &str) { self.insert(p,s) }
    fn delete(&mut self, p: usize) { self.remove(p..p+1) }
}
impl Text for crop::Rope {
    fn load(p: &Path) -> Result<Self> {
        // ASCII corpus makes each 64 KiB boundary a valid UTF-8 boundary.
        // Reject other inputs instead of silently benchmarking wrong character indexes.
        let mut f=BufReader::new(File::open(p)?); let mut b=[0u8;65536];
        let mut builder=crop::RopeBuilder::new();
        loop { let n=f.read(&mut b)?; if n==0 { break; } if !b[..n].is_ascii() { bail!("crop comparison requires the generated ASCII corpus"); } builder.append(std::str::from_utf8(&b[..n])?); }
        Ok(builder.build())
    }
    fn len(&self) -> usize { self.byte_len() }
    fn lines(&self) -> usize { self.line_len() }
    fn to_line(&self, p: usize) -> usize { self.line_of_byte(p) }
    fn from_line(&self, l: usize) -> usize { self.byte_of_line(l) }
    fn insert(&mut self, p: usize, s: &str) { self.insert(p,s) }
    fn delete(&mut self, p: usize) { self.delete(p..p+1) }
}
fn run<T: Text>(path: &Path, engine: &str, mode: &str) -> Result<()> {
    let bytes=std::fs::metadata(path)?.len();
    if mode=="load" {
        let mut times=Vec::new(); let mut lines=0;
        for _ in 0..5 { let (r,ms)=timed(||T::load(path)); let r=r?;lines=r.lines();black_box(&r);times.push(ms);drop(r); }
        emit(json!({"bench":"buffer","engine":engine,"mode":mode,"bytes":bytes,"lines":lines,"load":stats(&times)}));return Ok(());
    }
    let base=T::load(path)?;let mut result=json!({"bench":"buffer","engine":engine,"mode":mode,"bytes":bytes,"lines":base.lines(),"index_unit":"Unicode scalar (ASCII == byte)","seed":123456789});
    let mut rng=Rng(123456789);
    let positions=(0..1_000_000).map(|_|rng.next()%base.len()).collect::<Vec<_>>();
    let lines=(0..1_000_000).map(|_|rng.next()%base.lines()).collect::<Vec<_>>();
    result["line_count_1m"]=repeats(5,||{for _ in 0..1_000_000 {black_box(black_box(&base).lines());}});
    result["char_to_line_1m"]=repeats(5,||{let mut sum=0usize;for &p in &positions {sum=sum.wrapping_add(base.to_line(p));}black_box(sum);});
    result["line_to_char_1m"]=repeats(5,||{let mut sum=0usize;for &l in &lines {sum=sum.wrapping_add(base.from_line(l));}black_box(sum);});
    for operation in ["insert_10k","delete_10k"] {
        let mut times=Vec::new();
        for _ in 0..5 {
            let mut r=base.clone();let mut rng=Rng(987654321);
            let (_,ms)=timed(||for _ in 0..10_000 {let p=rng.next()%r.len();if operation=="insert_10k"{r.insert(p,"x");}else{r.delete(p);}});
            assert_eq!(r.len(),base.len()+if operation=="insert_10k" {10_000}else{0}-if operation=="delete_10k"{10_000}else{0});times.push(ms);black_box(&r);
        }
        result[operation]=stats(&times);
    }
    // Hold 1000 snapshots until after timing, to exclude drop from clone cost.
    let mut times=Vec::new();
    for _ in 0..7 {let (clones,ms)=timed(||(0..1000).map(|_|black_box(&base).clone()).collect::<Vec<_>>());times.push(ms/1000.0);black_box(&clones);drop(clones);}
    result["clone_one"]=stats(&times);
    for count in [10_000,100_000] {
        assert!(base.lines()>=count);let mut r=base.clone();let mut times=Vec::new();
        for _ in 0..100 {
            let (_,ms)=timed(||{
                let offsets=(0..count).map(|l|r.from_line(l)).collect::<Vec<_>>();
                for p in offsets.into_iter().rev() {r.insert(p,"foo");}
            });times.push(ms);
        }
        assert_eq!(r.len(),base.len()+count*3*100);
        result[format!("multicursor_{count}_100tx")]=stats(&times);
    }
    emit(result);Ok(())
}
fn string_baseline(path: &Path) -> Result<()> {
    let mut times=Vec::new();
    for _ in 0..5 {let (s,ms)=timed(||std::fs::read_to_string(path));let s=s?;black_box(&s);times.push(ms);}
    let base=std::fs::read_to_string(path)?;
    let mut copies=Vec::new();for _ in 0..5 {let (s,ms)=timed(||black_box(&base).clone());black_box(&s);copies.push(ms);}
    emit(json!({"bench":"buffer","engine":"String","mode":"load","bytes":base.len(),"load":stats(&times),"clone_one":stats(&copies),"edits":"not measured: optional baseline only load/clone; naive random edits move O(N) bytes"}));Ok(())
}
fn main() -> Result<()> {
    let args=std::env::args().collect::<Vec<_>>();let path=arg(&args,"--file","data/text-100mb.txt");
    let engine=arg(&args,"--engine","ropey");let mode=arg(&args,"--mode","ops");
    match engine.as_str() {"ropey"=>run::<ropey::Rope>(Path::new(&path),&engine,&mode),"crop"=>run::<crop::Rope>(Path::new(&path),&engine,&mode),"string"=>string_baseline(Path::new(&path)),_=>bail!("engine must be ropey/crop/string")}
}
