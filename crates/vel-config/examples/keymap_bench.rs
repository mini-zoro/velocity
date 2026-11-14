//! CPU-only lookup microbenchmark; no display or native input.
use vel_config::{
    json,
    keymap::{Key, Keymap, SettingsContext},
    settings::Settings,
};
fn main() {
    let bindings:Vec<_>=(0..1000).map(|i|json!({"keys":[format!("ctrl+key_{i}")],"command":"insert","args":{"characters":"x"}})).collect();
    let mut keymap = Keymap::default();
    keymap.append(&json!(bindings).to_string()).unwrap();
    let settings = Settings::default();
    let context = SettingsContext(&settings);
    let keys: Vec<_> = (0..1000)
        .map(|i| Key::parse(&format!("ctrl+key_{i}")).unwrap())
        .collect();
    let n = 1_000_000;
    let now = std::time::Instant::now();
    for i in 0..n {
        std::hint::black_box(keymap.resolve(&keys[i % 1000], &context));
    }
    println!(
        "{{\"kind\":\"keymap_lookup\",\"bindings\":1000,\"events\":{n},\"mean_us\":{}}}",
        now.elapsed().as_secs_f64() * 1e6 / n as f64
    );
}
