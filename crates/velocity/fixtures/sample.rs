// Velocity · native AppKit / wgpu demo (MIT OR Apache-2.0).
use std::sync::Arc;

pub struct Document {
    pub title: String,
    pub lines: usize,
}

impl Document {
    pub fn greeting(&self) -> String {
        let language = "你好 · 日本語 · café · 🦀 👩‍💻 ❤️ 🇨🇳";
        format!("{}: {} lines — {}", self.title, self.lines, language)
    }
}

fn main() {
    let document = Arc::new(Document { title: "Velocity".into(), lines: 5000 });
    println!("{}", document.greeting());
}
