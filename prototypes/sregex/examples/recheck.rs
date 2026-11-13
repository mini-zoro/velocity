//! Account for every corpus mismatch with an exact six-line replay, and
//! independently exercise B at the query positions where the oracle errored.
use anyhow::{Result,ensure};
use serde_json::{Value,json};
use sregex_prototype::{loader,matcher::{Pool,Multi,Baseline,BaselineKind,next_boundary}};
use std::{collections::BTreeSet,path::Path};

fn main()->Result<()> {
    let root=Path::new(env!("CARGO_MANIFEST_DIR"));
    let diff:Vec<Value>=serde_json::from_slice(&std::fs::read(root.join("output/diff.json"))?)?;
    let rust=diff.iter().find(|s|s["language"]=="rust").unwrap();
    let lines:BTreeSet<String>=rust["contexts"].as_array().unwrap().iter()
        .flat_map(|c|c["diff"]["samples"].as_array().into_iter().flatten())
        .filter(|s|s["engine"]=="B").map(|s|s["line"].as_str().unwrap().to_string()).collect();
    ensure!(lines.len()==6,"expected six distinct Unicode source lines");
    let syntax=loader::load("rust",&root.join("../../spikes/data/Packages/Rust/Rust.sublime-syntax"))?;
    let pool=Pool::compile(&syntax.patterns,false);let mut total=0;let mut rows=vec![];
    for ctx in syntax.contexts.iter().filter(|c|c.patterns.len()>=5) {
        let expected=rust["contexts"].as_array().unwrap().iter().find(|c|c["name"]==ctx.name).unwrap()["diff"]["mismatches_b"].as_u64().unwrap();
        let m=Multi::compile(&pool,ctx);let mut b=m.runner(&pool,ctx);let mut a=Baseline::new(&pool,ctx,BaselineKind::Onig);let mut count=0;
        for line in &lines {
            a.reset_line();b.reset_line();let mut pos=0;
            while pos<=line.len(){let ah=a.find(line,pos)?;let bh=b.find(line,pos)?;
                count+=u64::from(ah!=bh);let Some(h)=ah else{break;};pos=next_boundary(line,h.end.max(pos+1));}
        }
        ensure!(count==expected,"unaccounted mismatch in {}: six-line replay {count}, full corpus {expected}",ctx.name);
        total+=count;if count>0{rows.push(json!({"context":ctx.name,"replayed_mismatches":count,"full_corpus_mismatches":expected}));}
    }
    let md=diff.iter().find(|s|s["language"]=="markdown").unwrap();
    let syntax=loader::load("markdown",&root.join("../../spikes/data/Packages/Markdown/Markdown.sublime-syntax"))?;
    let pool=Pool::compile(&syntax.patterns,false);let mut errors=vec![];
    for c in md["contexts"].as_array().unwrap() {
        let samples:Vec<_>=c["diff"]["samples"].as_array().into_iter().flatten().filter(|s|s["error"]=="onig").collect();
        if samples.is_empty(){continue;}
        let ctx=syntax.contexts.iter().find(|x|x.name==c["name"].as_str().unwrap()).unwrap();
        let m=Multi::compile(&pool,ctx);let mut b=m.runner(&pool,ctx);
        for s in samples{b.reset_line();let result=b.find(s["line"].as_str().unwrap(),s["pos"].as_u64().unwrap() as usize);
            errors.push(json!({"context":ctx.name,"line":s["line"],"pos":s["pos"],"B":match result{Ok(v)=>json!({"result":v}),Err(e)=>json!({"error":format!("{e:#}")})}}));}
    }
    println!("{}",serde_json::to_string_pretty(&json!({"unicode_lines":lines,"accounted_mismatches":total,"unaccounted_mismatches":0,"contexts":rows,"oracle_error_rechecks":errors}))?);
    Ok(())
}
