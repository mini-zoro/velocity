//! Prepared 100k-project-word queries, five runs; construction is outside the query timer.
use std::{
    sync::{Arc, RwLock},
    time::Instant,
};
use vel_buffer::ChangeId;
use vel_complete::{CompletionEngine, ViewSnapshot};
use vel_highlight::ScopeProvider;
use vel_index::{FileAnalysis, Index, WordStatistics};
use vel_scope::Scope;
use vel_text::Text;
struct Scopes(Vec<Scope>);
impl ScopeProvider for Scopes {
    fn version(&self) -> ChangeId {
        ChangeId(1)
    }
    fn scopes_at(&self, _: usize) -> Option<&[Scope]> {
        Some(&self.0)
    }
}
fn main() {
    let mut stats = WordStatistics::default();
    for n in 0..100_000 {
        let word = format!("word_candidate_{n:06}");
        stats.words.insert(word.clone(), 1 + n % 8);
        stats
            .sequences
            .insert(vec!["let".into(), "value".into(), word.clone()], 1 + n % 4);
        stats.trailing.insert((word, "(".into()), 1);
    }
    stats.words.insert("let".into(), 100_000);
    stats.words.insert("value".into(), 100_000);
    let mut index = Index::new();
    index.replace_file(
        "/fixture/words.rs",
        FileAnalysis {
            symbols: Vec::new(),
            words: stats,
        },
    );
    let mut engine = CompletionEngine::new();
    engine.set_index(Arc::new(RwLock::new(index)));
    let scopes = Scopes(vec![Scope::new("source.rust").unwrap()]);
    for run in 0..5 {
        for query in ["wc99", "candidate42", "word", "zzzz", ""] {
            let text = format!("let value {query}");
            let view = ViewSnapshot::new(1, ChangeId(1), Text::from(text.as_str()));
            let start = Instant::now();
            let result = engine.query(&view, &[text.len()], &scopes);
            println!(
                "{{\"case\":\"complete_100k\",\"run\":{run},\"query\":\"{query}\",\"ms\":{},\"hits\":{}}}",
                start.elapsed().as_secs_f64() * 1000.,
                result.items.len()
            );
            std::hint::black_box(result);
        }
    }
}
