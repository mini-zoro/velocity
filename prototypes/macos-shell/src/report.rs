//! JSONL output, startup milestones, and a bounded-run watchdog.
use crate::{cli::Mode, native};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize)]
pub struct Milestone {
    pub phase: String,
    pub main_ms: f64,
    pub process_ms: Option<f64>,
    pub delta_ms: f64,
}
#[derive(Debug)]
pub struct Clock {
    pub entered: Instant,
    pub before_main_ms: Option<f64>,
    pub birth_error: Option<String>,
    pub milestones: Vec<Milestone>,
}
impl Clock {
    pub fn new(entered: Instant, wall_at_entry: SystemTime) -> Self {
        let (before_main_ms, birth_error) = match native::process_birth().and_then(|birth| {
            wall_at_entry
                .duration_since(birth)
                .map_err(|e| format!("kernel start time is later than main wall snapshot: {e}"))
        }) {
            Ok(d) => (Some(ms(d)), None),
            Err(e) => (None, Some(e)),
        };
        let first = Milestone {
            phase: "main_entered".into(),
            main_ms: 0.,
            process_ms: before_main_ms,
            delta_ms: 0.,
        };
        Self {
            entered,
            before_main_ms,
            birth_error,
            milestones: vec![first],
        }
    }
    pub fn mark(&mut self, phase: &str) {
        self.mark_at(phase, Instant::now());
    }
    pub fn mark_at(&mut self, phase: &str, at: Instant) {
        let elapsed = ms(at.duration_since(self.entered));
        let previous = self.milestones.last().map_or(0., |m| m.main_ms);
        self.milestones.push(Milestone {
            phase: phase.into(),
            main_ms: elapsed,
            process_ms: self.before_main_ms.map(|n| n + elapsed),
            delta_ms: elapsed - previous,
        });
    }
}
pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}

struct Sink {
    file: File,
}
#[derive(Clone)]
pub struct Reporter {
    sink: Arc<Mutex<Sink>>,
    pub path: PathBuf,
    run_id: String,
    mode: Mode,
    entered: Instant,
}
impl Reporter {
    pub fn new(mode: Mode, path: Option<&Path>, entered: Instant) -> Result<Self, String> {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_micros();
        let run_id = format!("{epoch}-{}", std::process::id());
        let path = path.map(Path::to_owned).unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("output")
                .join(format!("{}-{run_id}.jsonl", mode.name()))
        });
        if path.extension().is_none_or(|e| e != "jsonl") {
            return Err("--output must end in .jsonl".into());
        }
        // Results stay inside this prototype even when invoked from a different directory.
        let output = Path::new(env!("CARGO_MANIFEST_DIR")).join("output");
        std::fs::create_dir_all(&output).map_err(|e| e.to_string())?;
        let canonical_parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()
            .map_err(|e| e.to_string())?;
        if canonical_parent != output.canonicalize().map_err(|e| e.to_string())? {
            return Err(
                "--output must be a file directly inside prototypes/macos-shell/output/".into(),
            );
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| format!("{}: {e} (results are never overwritten)", path.display()))?;
        eprintln!("JSONL: {}", path.display());
        Ok(Self {
            sink: Arc::new(Mutex::new(Sink { file })),
            path,
            run_id,
            mode,
            entered,
        })
    }
    pub fn emit(&self, mut value: Value) -> Result<(), String> {
        let object = value
            .as_object_mut()
            .ok_or("JSONL record must be an object")?;
        object.insert("schema".into(), json!(1));
        object.insert("run_id".into(), json!(self.run_id));
        object.insert("mode".into(), json!(self.mode.name()));
        object.insert("pid".into(), json!(std::process::id()));
        object.insert("logged_main_ms".into(), json!(ms(self.entered.elapsed())));
        let bytes = json_line(&value)?;
        let mut sink = self.sink.lock().map_err(|_| "output lock poisoned")?;
        sink.file
            .write_all(&bytes)
            .and_then(|()| sink.file.flush())
            .map_err(|e| e.to_string())?;
        // A piped consumer may close stdout; the on-disk record remains authoritative.
        if let Err(e) = std::io::stdout().lock().write_all(&bytes)
            && e.kind() != std::io::ErrorKind::BrokenPipe
        {
            return Err(e.to_string());
        }
        Ok(())
    }
    pub fn rss(&self, phase: &str, details: Value) -> Result<(), String> {
        self.emit(memory_record(
            phase,
            details,
            native::rss_bytes(),
            native::phys_footprint_bytes(),
        ))
    }
}
fn memory_record(
    phase: &str,
    details: Value,
    rss: Result<u64, String>,
    footprint: Result<u64, String>,
) -> Value {
    json!({
        "kind":"memory", "phase":phase, "details":details,
        "rss_bytes":rss.as_ref().ok(), "rss_error":rss.err(),
        "method":"task_info(MACH_TASK_BASIC_INFO).resident_size",
        "phys_footprint_bytes":footprint.as_ref().ok(), "phys_footprint_error":footprint.err(),
        "phys_footprint_method":"task_info(TASK_VM_INFO, REV1).phys_footprint"
    })
}
fn json_line(value: &Value) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// A separate thread also bounds initialization if a native API never returns.
pub struct Watchdog {
    done: Arc<(Mutex<bool>, Condvar)>,
}
impl Watchdog {
    pub fn start(seconds: u64, reporter: Reporter) -> Self {
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        let child = Arc::clone(&done);
        std::thread::spawn(move || {
            let (lock, cv) = &*child;
            let guard = lock.lock().expect("watchdog lock");
            let (guard, timed) = cv
                .wait_timeout_while(guard, Duration::from_secs(seconds), |done| !*done)
                .expect("watchdog wait");
            if !*guard && timed.timed_out() {
                let _=reporter.emit(json!({"kind":"error","stage":"watchdog","message":format!("no completion within {seconds} seconds; check visibility, Metal, and event-loop progress"),"complete":false}));
                eprintln!("benchmark watchdog expired");
                std::process::exit(2);
            }
        });
        Self { done }
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        let (lock, cv) = &*self.done;
        if let Ok(mut done) = lock.lock() {
            *done = true;
            cv.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_json_keeps_rss_and_footprint_distinct_with_independent_errors() {
        let record = memory_record("first", json!({"lines":5000}), Ok(100), Ok(60));
        assert_eq!(record["rss_bytes"], 100);
        assert_eq!(record["phys_footprint_bytes"], 60);
        assert!(record["rss_error"].is_null());
        assert!(record["phys_footprint_error"].is_null());
        let failed = memory_record("first", json!({}), Ok(100), Err("short reply".into()));
        assert_eq!(failed["rss_bytes"], 100);
        assert!(failed["phys_footprint_bytes"].is_null());
        assert_eq!(failed["phys_footprint_error"], "short reply");
        let failed_rss = memory_record("first", json!({}), Err("denied".into()), Ok(60));
        assert!(failed_rss["rss_bytes"].is_null());
        assert_eq!(failed_rss["phys_footprint_bytes"], 60);
    }
    #[test]
    fn jsonl_round_trip_is_one_line_with_unicode_and_embedded_newlines() {
        let v = json!({"kind":"ime","text":"你好\n🦀","stats":null});
        let b = json_line(&v).unwrap();
        assert_eq!(b.iter().filter(|b| **b == b'\n').count(), 1);
        assert_eq!(serde_json::from_slice::<Value>(&b).unwrap(), v);
    }
    #[test]
    fn startup_times_are_monotonic_and_relative_to_main() {
        let mut c = Clock::new(Instant::now(), SystemTime::now());
        c.mark("test");
        assert_eq!(c.milestones[0].main_ms, 0.);
        assert!(c.milestones[1].main_ms >= 0.);
        if let Some(before) = c.before_main_ms {
            assert_eq!(
                c.milestones[1].process_ms,
                Some(before + c.milestones[1].main_ms)
            );
        }
    }
}
