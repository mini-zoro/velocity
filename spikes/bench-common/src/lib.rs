use serde_json::{json, Value};
use std::{hint::black_box, time::Instant};

pub fn emit(mut value: Value) {
    value["peak_rss_bytes"] = json!(peak_rss());
    println!("{}", value);
}
pub fn peak_rss() -> u64 {
    let mut r = std::mem::MaybeUninit::<libc::rusage>::uninit();
    unsafe {
        if libc::getrusage(libc::RUSAGE_SELF, r.as_mut_ptr()) != 0 { return 0; }
        let n = r.assume_init().ru_maxrss as u64;
        if cfg!(target_os = "macos") { n } else { n * 1024 }
    }
}
pub fn stats(samples: &[f64]) -> Value {
    let mut v = samples.to_vec(); v.sort_by(f64::total_cmp);
    assert!(!v.is_empty());
    let n = v.len();
    let median = if n % 2 == 0 { (v[n/2-1]+v[n/2])/2.0 } else { v[n/2] };
    json!({"n":n,"median_ms":median,"min_ms":v[0],"p95_ms":v[((n as f64*0.95).ceil() as usize).saturating_sub(1).min(n-1)],"samples_ms":samples})
}
pub fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let now = Instant::now(); let result = black_box(f());
    (result, now.elapsed().as_secs_f64()*1000.0)
}
pub fn repeats(n: usize, mut f: impl FnMut()) -> Value {
    stats(&(0..n).map(|_| timed(|| f()).1).collect::<Vec<_>>())
}
pub struct Rng(pub u64);
impl Rng { pub fn next(&mut self) -> usize { let mut x=self.0; x^=x<<13; x^=x>>7; x^=x<<17; self.0=x; x as usize } }
pub fn arg(args: &[String], key: &str, default: &str) -> String {
    args.windows(2).find(|w| w[0]==key).map(|w|w[1].clone()).unwrap_or_else(||default.to_owned())
}
pub fn data_dir(args: &[String]) -> std::path::PathBuf { std::path::PathBuf::from(arg(args,"--data","data")) }
