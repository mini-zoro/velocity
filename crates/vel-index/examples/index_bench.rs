//! Five-run project indexing/query evidence; no sysctl calls.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use vel_index::{
    FileAnalysis, IndexOptions, IndexedSymbol, ProjectIndex, SymbolKind, WordStatistics,
};
use vel_syntax::{MetadataSet, SyntaxSet};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str).unwrap_or("queries") {
        "queries" => queries(),
        "prepare" => {
            syntax()?;
            Ok(())
        }
        "corpus" => corpus(args.get(2).ok_or("missing corpus directory")?),
        "edit" => edited_file(),
        _ => Err("expected queries | corpus DIRECTORY | edit".into()),
    }
}
fn queries() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let cache = tempfile::tempdir()?;
    let service = ProjectIndex::open(
        root.path(),
        syntax()?,
        Arc::new(MetadataSet::new()),
        IndexOptions {
            cache_dir: Some(cache.path().into()),
            index_workers: 4,
            watch: false,
            ..IndexOptions::default()
        },
    )?;
    service.flush()?;
    let shared = service.shared_index();
    let mut index = shared.write().unwrap();
    println!(
        "{}",
        serde_json::json!({"case":"empty_service_memory","rss_bytes":resident_rss(),"peak_rss_bytes":peak_rss()})
    );
    let start = Instant::now();
    // One million distinct names; construction/input allocations are included in process peak RSS.
    for file in 0..1000 {
        let mut words = WordStatistics::default();
        words.words.insert("fn".into(), 1000);
        let symbols = (0..1000)
            .map(|n| {
                let id = file * 1000 + n;
                let name = format!("render_widget_{id:07}");
                words.words.insert(name.clone(), 1);
                words.sequences.insert(vec!["fn".into(), name.clone()], 1);
                IndexedSymbol {
                    name,
                    row: n,
                    column: 3,
                    range: n * 32..n * 32 + 24,
                    kind: SymbolKind::Function,
                    definition: true,
                    reference: false,
                }
            })
            .collect();
        index.replace_file(
            format!("/fixture/{file:04}.rs"),
            FileAnalysis { symbols, words },
        );
    }
    println!(
        "{}",
        serde_json::json!({"case":"million_build","seconds":start.elapsed().as_secs_f64(),"symbols":index.symbol_count(),"storage_bytes":index.storage_bytes()})
    );
    println!(
        "{}",
        serde_json::json!({"case":"million_memory","rss_bytes":resident_rss(),"peak_rss_bytes":peak_rss()})
    );
    for run in 0..5 {
        let start = Instant::now();
        for n in 0..1000 {
            std::hint::black_box(index.lookup_definition(&format!("render_widget_{:07}", n * 997)));
        }
        println!(
            "{}",
            serde_json::json!({"case":"definition_million","run":run,"ms_per_query":start.elapsed().as_secs_f64()})
        );
        for query in ["rw99", "widget42", "render", "zzzz", ""] {
            let start = Instant::now();
            let result = index.fuzzy_symbols(query, 50);
            println!(
                "{}",
                serde_json::json!({"case":"fuzzy_million","run":run,"query":query,"ms":start.elapsed().as_secs_f64()*1000.,"hits":result.len()})
            );
            std::hint::black_box(result);
        }
    }
    Ok(())
}
fn syntax() -> Result<Arc<SyntaxSet>, Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/sublime-packages");
    Ok(Arc::new(
        SyntaxSet::load_cached(
            &root,
            &std::env::temp_dir().join("velocity-lane-b-r6-syntax-cache"),
        )?
        .0,
    ))
}
fn corpus(root: &str) -> Result<(), Box<dyn std::error::Error>> {
    let set = syntax()?;
    let metadata = Arc::new(MetadataSet::load_directories(&[PathBuf::from(env!(
        "CARGO_MANIFEST_DIR"
    ))
    .join("../../third_party/sublime-packages")])?);
    for run in 0..5 {
        let cache = tempfile::tempdir()?;
        let options = IndexOptions {
            index_workers: 4,
            cache_dir: Some(cache.path().into()),
            watch: false,
            ..IndexOptions::default()
        };
        let start = Instant::now();
        let service = ProjectIndex::open(root, set.clone(), metadata.clone(), options.clone())?;
        service.flush()?;
        let status = service.status();
        let count = service.shared_index().read().unwrap().symbol_count();
        println!(
            "{}",
            serde_json::json!({"case":"corpus_cold","run":run,"seconds":start.elapsed().as_secs_f64(),"files":status.total_files,"indexed":status.indexed_files,"symbols":count,"skipped":status.skipped_files,"logs":status.logs})
        );
        service.cancel();
        drop(service);
        let start = Instant::now();
        let service = ProjectIndex::open(root, set.clone(), metadata.clone(), options)?;
        service.flush()?;
        println!(
            "{}",
            serde_json::json!({"case":"corpus_restart","run":run,"seconds":start.elapsed().as_secs_f64(),"cached":service.status().cached_files,"indexed":service.status().indexed_files})
        );
    }
    Ok(())
}
fn edited_file() -> Result<(), Box<dyn std::error::Error>> {
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spikes/data/rust-10mb.rs"),
    )?;
    let sample: String = source
        .split_inclusive('\n')
        .scan(0, |bytes, line| {
            *bytes += line.len();
            (*bytes < 8192).then_some(line)
        })
        .collect();
    let root = tempfile::tempdir()?;
    let cache = tempfile::tempdir()?;
    std::fs::write(root.path().join("hot.rs"), &sample)?;
    let set = syntax()?;
    let service = ProjectIndex::open(
        root.path(),
        set,
        Arc::new(MetadataSet::new()),
        IndexOptions {
            cache_dir: Some(cache.path().into()),
            watch: false,
            ..IndexOptions::default()
        },
    )?;
    service.flush()?;
    for run in 0..5 {
        let text = vel_text::Text::from(format!("// revision {run}\n{sample}").as_str());
        let start = Instant::now();
        service.update_buffer("hot.rs", run + 1, text);
        // Observe normal debounce instead of bypassing it with the flush barrier.
        let before = service.status().indexed_files;
        while service.status().indexed_files == before {
            if start.elapsed() > Duration::from_secs(10) {
                return Err("edit timed out".into());
            }
            std::thread::park_timeout(Duration::from_micros(100));
        }
        println!(
            "{}",
            serde_json::json!({"case":"edited_rust_file","run":run,"bytes":sample.len(),"ms":start.elapsed().as_secs_f64()*1000.})
        );
    }
    Ok(())
}

fn peak_rss() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage writes the correctly aligned rusage on a successful call.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    assert_eq!(result, 0);
    // SAFETY: the successful call initialized this rusage.
    let bytes = unsafe { usage.assume_init() }.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        bytes
    } else {
        bytes * 1024
    }
}
#[cfg(target_os = "macos")]
fn resident_rss() -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::uninit();
    // SAFETY: the current PID and matching v2 flavor initialize this aligned struct.
    let result = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V2,
            info.as_mut_ptr().cast(),
        )
    };
    if result != 0 {
        return None;
    }
    // SAFETY: successful proc_pid_rusage initialized all of the v2 struct.
    Some(unsafe { info.assume_init() }.ri_resident_size)
}
#[cfg(not(target_os = "macos"))]
fn resident_rss() -> Option<u64> {
    None
}
