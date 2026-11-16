//! Local headless peer for the existing CLI wire protocol. No native app initialization.
use std::{
    hash::{Hash, Hasher},
    io::{Read, Write},
    os::unix::net::UnixListener,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    let data = Some(PathBuf::from(&args[1]));
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hash);
    let uid = args[2].to_str().unwrap();
    let path = std::env::temp_dir().join(format!("velocity-{uid}-{:016x}.sock", hash.finish()));
    let listener = UnixListener::bind(&path).unwrap();
    listener.set_nonblocking(true).unwrap();
    println!("{}", path.display());
    std::io::stdout().flush().unwrap();
    let result = std::panic::catch_unwind(|| {
        for _ in 0..2 {
            let deadline = Instant::now() + Duration::from_secs(15);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "CLI never connected");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut size = [0; 8];
            stream.read_exact(&mut size).unwrap();
            let size = u64::from_le_bytes(size);
            assert!(size < 1024 * 1024);
            let mut bytes = vec![0; size as usize];
            stream.read_exact(&mut bytes).unwrap();
            println!("{}", String::from_utf8_lossy(&bytes));
            std::io::stdout().flush().unwrap();
            stream.write_all(&[1]).unwrap();
            if bytes
                .windows(b"\"wait\":true".len())
                .any(|w| w == b"\"wait\":true")
            {
                while !Path::new(&args[3]).exists() {
                    assert!(
                        Instant::now() < deadline,
                        "wait test never closed the sheet"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                stream.write_all(&[2]).unwrap();
            }
        }
    });
    std::fs::remove_file(path).unwrap();
    if result.is_err() {
        std::process::exit(1);
    }
}
