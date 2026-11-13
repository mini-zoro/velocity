//! Explicitly record known Unicode differences; don't silently waive mismatches.
use sregex_prototype::{loader::Context,matcher::{Pool,Multi,Baseline,BaselineKind}};
fn main()->anyhow::Result<()> {
    let mut rows=vec![];
    for (pattern,line) in [(r"\b", "a\u{200c}b"),(r"(?i:ss)","ß"),(r"^", "x\n"),(r"\<", "<"),(r":/{,2}","://")] {
        let pool=Pool::compile(&[pattern.into()],true);let ctx=Context{name:"probe".into(),patterns:vec![0]};let multi=Multi::compile(&pool,&ctx);
        let mut a=Baseline::new(&pool,&ctx,BaselineKind::Onig);let mut f=Baseline::new(&pool,&ctx,BaselineKind::Fancy);let mut b=multi.runner(&pool,&ctx);
        for pos in (0..=line.len()).filter(|&p|line.is_char_boundary(p)) {
            rows.push(serde_json::json!({"pattern":pattern,"line":line,"pos":pos,"onig":a.find(line,pos)?,"fancy":f.find(line,pos)?,"B":b.find(line,pos)?}));
        }
    }
    println!("{}",serde_json::to_string_pretty(&rows)?);Ok(())
}
