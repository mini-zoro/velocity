//! Cached login-shell environment; shell startup and timeouts run on build workers.
use crate::{Exec, Variables};
use std::{
    collections::BTreeMap,
    sync::{OnceLock, atomic::AtomicBool},
};
#[cfg(unix)]
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::{Mutex, atomic::Ordering},
    time::{Duration, Instant},
};
type Environment = BTreeMap<String, String>;
#[cfg(all(test, unix))]
#[path = "../tests/support/detached.rs"]
mod detached_fixture;
#[cfg(all(test, unix))]
#[path = "../tests/support/timing.rs"]
mod test_timing;
#[cfg(unix)]
#[derive(Debug)]
struct Capture {
    rx: std::sync::mpsc::Receiver<Result<Environment, String>>,
    stop: std::sync::Arc<AtomicBool>,
}
#[cfg(unix)]
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
#[cfg(unix)]
#[derive(Debug, Default)]
struct Cache {
    key: Environment,
    profiles: Vec<(std::path::PathBuf, Option<std::time::SystemTime>)>,
    value: Option<Result<Environment, String>>,
    retry_at: Option<Instant>,
    capture: Option<Capture>,
    retried: bool,
}
#[cfg(unix)]
static LOGIN: OnceLock<Mutex<Cache>> = OnceLock::new();
#[cfg(unix)]
const WAIT: Duration = Duration::from_secs(2);
#[cfg(unix)]
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(unix)]
const BACKOFF: Duration = Duration::from_secs(300);
// Login/startup and logout files: https://zsh.sourceforge.io/Doc/Release/Files.html
// Both sentinels below delimit the environment before any logout/EXIT output.
#[cfg(unix)]
fn profile_stamp(
    inherited: &Environment,
) -> Vec<(std::path::PathBuf, Option<std::time::SystemTime>)> {
    let home = inherited
        .get("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let zdot = inherited
        .get("ZDOTDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.clone());
    let mut paths = vec![
        "/etc/profile".into(),
        "/etc/bashrc".into(),
        "/etc/zshenv".into(),
        "/etc/zprofile".into(),
        "/etc/zshrc".into(),
        "/etc/zlogin".into(),
        "/etc/zlogout".into(),
    ];
    paths.extend(
        [".profile", ".bash_profile", ".bash_login", ".bashrc"].map(|name| home.join(name)),
    );
    paths.extend(
        [".zshenv", ".zprofile", ".zshrc", ".zlogin", ".zlogout"].map(|name| zdot.join(name)),
    );
    if let Some(shell) = inherited.get("SHELL") {
        paths.push(shell.into());
    }
    paths
        .into_iter()
        .map(|path: std::path::PathBuf| {
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            (path, modified)
        })
        .collect()
}
#[cfg(unix)]
impl Cache {
    fn refresh(&mut self) {
        let result = self
            .capture
            .as_ref()
            .and_then(|capture| capture.rx.try_recv().ok());
        if let Some(result) = result {
            self.retry_at = result.is_err().then(|| Instant::now() + BACKOFF);
            self.value = Some(result);
            self.capture = None;
        }
    }
    fn start(&mut self, inherited: Environment) {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        self.capture = Some(Capture { rx, stop });
        std::thread::spawn(move || {
            let _ = tx.send(login_environment(&inherited, &cancelled));
        });
    }
    fn prepare(&mut self, key: Environment, inherited: &Environment) -> bool {
        let profiles = profile_stamp(inherited);
        let changed = self.key != key || self.profiles != profiles;
        if changed {
            *self = Self {
                key,
                profiles,
                ..Default::default()
            };
        }
        self.refresh();
        let initial = self.value.is_none();
        let expired = self.retry_at.is_some_and(|at| Instant::now() >= at);
        let background_retry = self.value.as_ref().is_some_and(Result::is_err) && !self.retried;
        if self.capture.is_none() && (initial || expired || background_retry) {
            self.retried = !initial;
            self.start(inherited.clone());
        }
        initial
    }
}
/// Starts login-environment capture before a build, without waiting on the UI thread.
pub(crate) fn warm_login_environment() {
    #[cfg(unix)]
    {
        let inherited = inherited_environment();
        let key = cache_key(&inherited);
        LOGIN
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prepare(key, &inherited);
    }
}
// Injection is explicit runtime state. Cargo feature unification must never choose
// a fixture for a production binary, including `build --all-targets`.
static PROVIDER: OnceLock<fn() -> Environment> = OnceLock::new();
#[doc(hidden)]
pub fn set_login_capture_provider(provider: fn() -> Environment) {
    if let Some(installed) = PROVIDER.get() {
        assert!(
            std::ptr::fn_addr_eq(*installed, provider),
            "login provider already installed"
        );
    } else if PROVIDER.set(provider).is_err() {
        assert!(
            std::ptr::fn_addr_eq(*PROVIDER.get().unwrap(), provider),
            "login provider already installed"
        );
    }
}
fn inherited_environment() -> Environment {
    #[cfg(test)]
    crate::test_login::install();
    PROVIDER
        .get()
        .map_or_else(system_environment, |provider| provider())
}

pub(crate) fn system_environment() -> Environment {
    #[cfg(not(windows))]
    return std::env::vars().collect();
    #[cfg(windows)]
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}
// Build-specific env/path overrides are applied after capture, never part of its identity.
#[cfg(unix)]
fn cache_key(inherited: &Environment) -> Environment {
    ["SHELL", "HOME", "ZDOTDIR"]
        .into_iter()
        .filter_map(|name| {
            inherited
                .get(name)
                .map(|value| (name.into(), value.clone()))
        })
        .collect()
}
#[cfg(unix)]
pub(crate) fn launch_environment(exec: &Exec, stop: &AtomicBool) -> (Environment, Option<String>) {
    launch_environment_with(
        exec,
        stop,
        inherited_environment(),
        LOGIN.get_or_init(Default::default),
    )
}
#[cfg(not(unix))]
pub(crate) fn launch_environment(exec: &Exec, _stop: &AtomicBool) -> (Environment, Option<String>) {
    let inherited = inherited_environment();
    match expand_environment(&inherited, &exec.env_source, &exec.variables) {
        Ok(env) => (env, None),
        Err(error) => (inherited, Some(format!("Build environment: {error}"))),
    }
}

#[cfg(unix)]
fn launch_environment_with(
    exec: &Exec,
    stop: &AtomicBool,
    inherited: Environment,
    cache: &Mutex<Cache>,
) -> (Environment, Option<String>) {
    let mut warning = None;
    let mut base = inherited.clone();
    if cfg!(unix) {
        // Capture in the worker can outlive the first build's two-second wait. A
        // slow healthy profile then serves later builds, without repeated stalls.
        // https://www.sublimetext.com/docs/build_systems.html#exec-target-options
        let key = cache_key(&inherited);
        let initial = cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prepare(key.clone(), &inherited);
        let deadline = Instant::now() + WAIT;
        let result = loop {
            let mut state = cache.lock().unwrap_or_else(|e| e.into_inner());
            state.refresh();
            if stop.load(Ordering::Acquire) {
                // This worker stops waiting; the cache owns the capture and
                // a replacement build can still use the same in-flight result.
                break Err("startup was cancelled".into());
            }
            if state.key != key {
                break Err("login environment changed".into());
            }
            if let Some(result) = &state.value {
                break result.clone();
            }
            if !initial || Instant::now() >= deadline {
                let error =
                    "startup still running; using inherited environment while capture continues"
                        .to_owned();
                state.value = Some(Err(error.clone()));
                state.retry_at = Some(Instant::now() + BACKOFF);
                break Err(error);
            }
            drop(state);
            std::thread::sleep(Duration::from_millis(5));
        };
        match result {
            Ok(env) => base = env,
            Err(error) => {
                warning = Some(format!(
                    "Login shell unavailable; using inherited environment: {error}"
                ))
            }
        }
    }
    match expand_environment(&base, &exec.env_source, &exec.variables) {
        Ok(env) => (env, warning),
        Err(error) => (base, Some(format!("Build environment: {error}"))),
    }
}
/// path overrides env.PATH; each supplied value expands the base environment and file variables.
pub(crate) fn expand_environment(
    base: &Environment,
    overrides: &Environment,
    variables: &Variables,
) -> Result<Environment, String> {
    #[cfg(windows)]
    let canonical_key = |key: &str| key.to_uppercase();
    #[cfg(not(windows))]
    let canonical_key = |key: &str| key.to_owned();
    #[cfg(windows)]
    let mut result: Environment = base
        .iter()
        .map(|(k, v)| (canonical_key(k), v.clone()))
        .collect();
    #[cfg(not(windows))]
    let mut result = base.clone();
    let mut context = Variables(result.clone());
    context.0.extend(variables.0.clone());
    for (key, value) in overrides {
        result.insert(canonical_key(key), context.expand(value)?);
    }
    Ok(result)
}
// Public build docs require the user's PATH for both cmd and shell_cmd:
// https://www.sublimetext.com/docs/build_systems.html#exec-target-options
// Capture login startup once, then run individual commands without repeating it.
#[cfg(unix)]
fn login_environment(inherited: &Environment, stop: &AtomicBool) -> Result<Environment, String> {
    capture_shell_in(
        inherited
            .get("SHELL")
            .map(String::as_str)
            .unwrap_or("/bin/bash"),
        stop,
        CAPTURE_TIMEOUT,
        inherited,
    )
}
#[cfg(all(test, unix))]
fn capture_shell(shell: &str, stop: &AtomicBool, timeout: Duration) -> Result<Environment, String> {
    // No process-global environment or cache is shared between these fixtures.
    let inherited = [
        ("SHELL".into(), shell.into()),
        (
            "HOME".into(),
            std::path::Path::new(shell)
                .parent()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        ),
        ("PATH".into(), "/usr/bin:/bin".into()),
    ]
    .into();
    capture_shell_in(shell, stop, timeout, &inherited)
}
#[cfg(unix)]
fn capture_shell_in(
    shell: &str,
    stop: &AtomicBool,
    timeout: Duration,
    inherited: &Environment,
) -> Result<Environment, String> {
    use std::sync::atomic::AtomicU64;
    static CAPTURE: AtomicU64 = AtomicU64::new(0);
    let sentinel = format!(
        "VELOCITY_LOGIN_{}_{}",
        std::process::id(),
        CAPTURE.fetch_add(1, Ordering::Relaxed)
    );
    let end = format!("{sentinel}_END");
    let source = format!("printf '\\0{sentinel}\\0'; /usr/bin/env -0; printf '\\0{end}\\0'");
    let deadline = Instant::now() + timeout;
    let mut shells = vec![shell];
    if shell != "/bin/bash" {
        shells.push("/bin/bash");
    }
    let mut last = Err("no usable login shell".into());
    for candidate in shells {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            break;
        }
        let mut command = Command::new(candidate);
        command
            .env_clear()
            .envs(inherited)
            .args(["-l", "-c", &source]);
        let result = collect_login(
            &mut command,
            stop,
            deadline.saturating_duration_since(Instant::now()),
            &sentinel,
        );
        let retry = result.as_ref().is_err_and(|error| {
            error.contains("failed") || error.contains("marker") || error.contains("No such file")
        });
        if result.is_ok() {
            return result;
        }
        last = result;
        if !retry {
            break;
        }
    }
    last
}

#[cfg(unix)]
fn collect_login(
    command: &mut Command,
    stop: &AtomicBool,
    timeout: Duration,
    sentinel: &str,
) -> Result<Environment, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let deadline = Instant::now() + timeout;
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().expect("pipe");
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(512 * 1024)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let status = loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            crate::process::kill_tree(child.id());
            let _ = child.kill();
            let _ = child.wait();
            // Dropping a JoinHandle detaches it. A detached descendant may hold
            // the pipe, but cannot extend the profile deadline or hold LOGIN.
            return Err("startup timed out or was cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => {
                crate::process::kill_tree(child.id());
                let _ = child.wait();
                return Err(e.to_string());
            }
        }
    };
    crate::process::kill_tree(child.id());
    // The shell has exited; only its output is left to drain. The grace is bounded
    // (a detached descendant holding the pipe must not hang the caller) but long
    // enough for the reader thread to be scheduled on a heavily loaded machine: 1 s
    // expired on a loaded CI runner. This runs on the capture worker, never the UI.
    let grace = deadline.min(Instant::now() + Duration::from_secs(5));
    let bytes = loop {
        if stop.load(Ordering::Acquire) || Instant::now() >= grace {
            return Err("login pipe did not close before deadline".into());
        }
        match rx.recv_timeout(Duration::from_millis(5)) {
            Ok(bytes) => break bytes.map_err(|e| e.to_string())?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err("login reader failed".into()),
        }
    };
    if reader.is_finished() {
        let _ = reader.join();
    }
    if !status.success() || bytes.len() >= 512 * 1024 {
        return Err("login shell failed or exceeded output limit".into());
    }
    let marker = format!("\0{sentinel}\0");
    let start = bytes
        .windows(marker.len())
        .position(|w| w == marker.as_bytes())
        .ok_or("login shell returned no environment marker")?
        + marker.len();
    let end_marker = format!("\0{sentinel}_END\0");
    let end = bytes[start..]
        .windows(end_marker.len())
        .position(|w| w == end_marker.as_bytes())
        .map(|at| start + at)
        .ok_or("login shell returned no end marker")?;
    let mut env = Environment::new();
    for entry in bytes[start..end].split(|b| *b == 0) {
        if let Ok(entry) = std::str::from_utf8(entry)
            && let Some((key, value)) = entry.split_once('=')
            && !key.is_empty()
            && !key.contains('\n')
        {
            env.insert(key.into(), value.into());
        }
    }
    if !env.contains_key("PATH") {
        return Err("login shell returned no PATH".into());
    }
    Ok(env)
}
#[cfg(all(test, unix))]
mod tests {
    use super::test_timing::{HANG_GUARD, budget};
    use super::*;
    fn await_login(cache: &Mutex<Cache>, env: &Environment) {
        cache.lock().unwrap().prepare(cache_key(env), env);
        let deadline = Instant::now() + HANG_GUARD;
        loop {
            let mut state = cache.lock().unwrap();
            state.refresh();
            if let Some(value) = &state.value {
                if value.is_ok() {
                    return;
                }
                assert!(state.capture.is_some(), "{value:?}");
            }
            assert!(Instant::now() < deadline, "capture did not complete");
            drop(state);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn await_started(path: &std::path::Path) {
        let deadline = Instant::now() + HANG_GUARD;
        while std::fs::read_to_string(path).unwrap_or_default().is_empty() {
            assert!(
                Instant::now() < deadline,
                "profile did not start: {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn round18_environment_expands_against_base_and_file_variables_without_recursion() {
        let base: Environment = [
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("SDK".into(), "/sdk".into()),
        ]
        .into();
        let vars = Variables::new(Some(std::path::Path::new("/src/你好.rs")), None, None, None);
        for (overrides, expected) in [
            (
                [
                    ("PATH".into(), "/opt/bin:$PATH".into()),
                    ("NAME".into(), "${SDK}:$file_base_name".into()),
                ],
                ["/opt/bin:/usr/bin:/bin", "/sdk:你好"],
            ),
            (
                [
                    ("PATH".into(), "$PATH".into()),
                    ("NAME".into(), "${missing:fallback}".into()),
                ],
                ["/usr/bin:/bin", "fallback"],
            ),
        ] {
            let result = expand_environment(&base, &overrides.into(), &vars).unwrap();
            assert_eq!([result["PATH"].as_str(), result["NAME"].as_str()], expected);
        }
    }
    #[test]
    fn round18_login_failure_and_timeout_are_bounded_and_do_not_block_the_caller() {
        let stop = AtomicBool::new(false);
        let mut bad = Command::new("/bin/sh");
        bad.args(["-c", "exit 3"])
            .env_clear()
            .envs(inherited_environment());
        assert!(collect_login(&mut bad, &stop, Duration::from_secs(1), "TEST").is_err());
        let mut hung = Command::new("/bin/sh");
        hung.args(["-c", "sleep 30 & wait"])
            .env_clear()
            .envs(inherited_environment());
        let at = Instant::now();
        assert!(collect_login(&mut hung, &stop, Duration::from_millis(40), "TEST").is_err());
        // The hung shell lives 30 s; returning well before that shows the 40 ms deadline
        // was obeyed. The 2 s bound is a performance contract (timing runs only).
        let elapsed = at.elapsed();
        assert!(
            elapsed < budget(Duration::from_secs(2), Duration::from_secs(20)),
            "{elapsed:?}"
        );
    }
    #[cfg(unix)]
    #[test]
    fn round18_login_user_shell_banner_cmd_path_cache_and_retry() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("login-shell");
        let count = dir.path().join("calls");
        let tool = dir.path().join("dock-tool");
        std::fs::write(&tool, "#!/bin/sh\nprintf 'CMD_OK'\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let stop = AtomicBool::new(false);
        let cache = Mutex::new(Cache::default());
        let inherited: Environment = [
            ("SHELL".into(), shell.to_string_lossy().into_owned()),
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("HOME".into(), dir.path().to_string_lossy().into_owned()),
        ]
        .into();
        let exec = crate::BuildSystem::from_value(
            "dock",
            serde_json::json!({"cmd":["dock-tool"],"path":"$PATH:/extra"}),
        )
        .unwrap()
        .resolve(&Variables::default())
        .unwrap();
        // An unsupported shell falls back once to bash; changing the shell
        // file's mtime below invalidates that result.
        std::fs::write(&shell, "#!/bin/sh\nexit 3\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        await_login(&cache, &inherited);
        assert!(
            launch_environment_with(&exec, &stop, inherited.clone(), &cache)
                .1
                .is_none()
        );
        for banner in ["profile banner", "profile banner\n"] {
            std::fs::write(&shell, format!("#!/bin/sh\n[ \"$1\" = -l ] && [ \"$2\" = -c ] || exit 5\nprintf x >> '{}'\nprintf '%s' '{}'\nexport PATH='{}:/usr/bin:/bin'\nexec /bin/sh -c \"$3\"\n", count.display(), banner, dir.path().display())).unwrap();
            await_login(&cache, &inherited);
            let (env, warning) = launch_environment_with(&exec, &stop, inherited.clone(), &cache);
            assert!(warning.is_none(), "{warning:?}");
            assert_eq!(
                env["PATH"],
                format!("{}:/usr/bin:/bin:/extra", dir.path().display())
            );
            assert!(env.keys().all(|k| !k.contains("banner")));
            let output = Command::new(&exec.cmd[0])
                .env_clear()
                .envs(&env)
                .output()
                .unwrap();
            assert_eq!(output.stdout, b"CMD_OK");
            assert_eq!(std::fs::read_to_string(&count).unwrap(), "x");
            // Clear only for the second banner form; within each form capture is cached.
            launch_environment_with(&exec, &stop, inherited.clone(), &cache);
            *cache.lock().unwrap() = Cache::default();
            std::fs::write(&count, "").unwrap();
        }
    }
    #[cfg(unix)]
    #[test]
    fn round18_login_detached_pipe_obeys_deadline_and_retries() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = detached_fixture::DetachedFixture::new();
        let shell = fixture.root().join("shell");
        std::fs::write(
            &shell,
            format!(
                "#!/bin/sh\n{}",
                fixture.script(false, "exec /bin/sh -c \"$3\"")
            ),
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let at = Instant::now();
        let result = capture_shell(
            shell.to_str().unwrap(),
            &AtomicBool::new(false),
            Duration::from_millis(100),
        );
        let elapsed = at.elapsed();
        assert!(result.is_err());
        // The detached holder keeps the pipe open for 60 s; returning well before that
        // shows the 100 ms deadline was obeyed. The 1 s bound is a performance contract.
        assert!(
            elapsed < budget(Duration::from_secs(1), Duration::from_secs(30)),
            "{elapsed:?}"
        );
    }
    #[cfg(unix)]
    fn fake_profile(dir: &std::path::Path, body: &str) -> (crate::Exec, Environment) {
        use std::os::unix::fs::PermissionsExt;
        let shell = dir.join("shell");
        std::fs::write(
            &shell,
            format!("#!/bin/sh\n{body}\nexec /bin/sh -c \"$3\"\n"),
        )
        .unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let exec = crate::BuildSystem::from_value(
            "profile",
            serde_json::json!({"cmd":["/usr/bin/env"],"env":{"SHELL":shell}}),
        )
        .unwrap()
        .resolve(&Variables::default())
        .unwrap();
        let env = [
            ("SHELL".into(), shell.to_string_lossy().into_owned()),
            ("HOME".into(), dir.to_string_lossy().into_owned()),
            ("PATH".into(), "/usr/bin:/bin".into()),
        ]
        .into();
        (exec, env)
    }
    #[cfg(unix)]
    #[test]
    fn round18_verify_login_failure_backoff_slow_success_and_profile_edit() {
        let hanging = tempfile::tempdir().unwrap();
        let (exec, env) = fake_profile(
            hanging.path(),
            &format!(
                "[ -f '{}/.profile' ] || sleep 30\nexport RETRIED=yes",
                hanging.path().display()
            ),
        );
        let cache = Mutex::new(Cache::default());
        let stop = AtomicBool::new(false);
        let at = Instant::now();
        assert!(
            launch_environment_with(&exec, &stop, env.clone(), &cache)
                .1
                .is_some()
        );
        // The first build waits for the two-second fallback (a lower bound, so load cannot
        // break it) and returns long before the 30 s profile would finish. The 4 s bound is
        // a performance contract (timing runs only).
        assert!(at.elapsed() >= Duration::from_millis(1900));
        let elapsed = at.elapsed();
        assert!(
            elapsed < budget(Duration::from_secs(4), Duration::from_secs(20)),
            "{elapsed:?}"
        );
        for _ in 0..4 {
            let at = Instant::now();
            assert!(
                launch_environment_with(&exec, &stop, env.clone(), &cache)
                    .1
                    .is_some()
            );
            // Cached: no second two-second fallback wait. 200 ms is the timing-run contract.
            let elapsed = at.elapsed();
            assert!(
                elapsed < budget(Duration::from_millis(200), Duration::from_millis(1500)),
                "cached failures must be immediate: {elapsed:?}"
            );
        }
        // A profile edit bypasses the negative cache and cancels the old capture.
        std::fs::write(hanging.path().join(".profile"), "retry").unwrap();
        await_login(&cache, &env);
        let (values, warning) = launch_environment_with(&exec, &stop, env.clone(), &cache);
        assert!(warning.is_none());
        assert_eq!(values["RETRIED"], "yes");
        drop(cache);
        let slow = tempfile::tempdir().unwrap();
        let count = slow.path().join("calls");
        let (exec, env) = fake_profile(
            slow.path(),
            &format!(
                "printf x >> '{}'\nsleep 3\nexport PROFILE_READY=late",
                count.display()
            ),
        );
        let cache = Mutex::new(Cache::default());
        // The two-second first-build fallback may already have completed under
        // load. Synchronize on publication, not the duration of a healthy shell.
        launch_environment_with(&exec, &stop, env.clone(), &cache);
        await_login(&cache, &env);
        let (values, warning) = launch_environment_with(&exec, &stop, env.clone(), &cache);
        assert!(warning.is_none());
        assert_eq!(values["PROFILE_READY"], "late");
        assert_eq!(std::fs::read_to_string(&count).unwrap(), "x");
        std::fs::write(slow.path().join(".zprofile"), "changed").unwrap();
        await_login(&cache, &env);
        assert_eq!(std::fs::read_to_string(&count).unwrap(), "xx");
    }
    #[cfg(unix)]
    #[test]
    fn round18_verify_login_warmup_fallback_and_end_sentinel() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("shell");
        let stop = AtomicBool::new(false);
        for body in [
            "exit 9",
            "printf 'no start marker'",
            "printf 'banner=prefix'; trap \"printf 'EVIL_LOGOUT=injected\\0'\" EXIT; /bin/sh -c \"$3\"",
        ] {
            std::fs::write(&shell, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
            let env =
                capture_shell(shell.to_str().unwrap(), &stop, Duration::from_secs(30)).unwrap();
            assert!(env.contains_key("PATH"));
            assert!(!env.contains_key("EVIL_LOGOUT"));
            assert!(!env.contains_key("banner"));
        }
        let (exec, env) = fake_profile(dir.path(), "sleep .3\nexport WARMED=yes");
        let cache = Mutex::new(Cache::default());
        cache.lock().unwrap().prepare(cache_key(&env), &env);
        await_login(&cache, &env);
        let (values, warning) = launch_environment_with(&exec, &stop, env, &cache);
        assert!(warning.is_none());
        assert_eq!(values["WARMED"], "yes");
        assert!(cache.lock().unwrap().capture.is_none());
    }
    #[cfg(unix)]
    #[test]
    fn round18_final_login_cache_shared_across_build_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("calls");
        let (_, env) = fake_profile(
            dir.path(),
            &format!("printf x >> '{}'\nsleep 30", count.display()),
        );
        let cache = Mutex::new(Cache::default());
        let stop = AtomicBool::new(false);
        let builds=[("A","/a"),("B","/b")].map(|(name,path)|crate::BuildSystem::from_value(name,serde_json::json!({"cmd":["/usr/bin/env"],"env":{"FLAG":name,"SHELL":"/does/not/exist"},"path":format!("$PATH:{path}")})).unwrap().resolve(&Variables::default()).unwrap());
        // Exclude scheduling the fixture shell from the intentionally measured
        // negative-cache wait; the production first-build budget stays two seconds.
        cache.lock().unwrap().prepare(cache_key(&env), &env);
        await_started(&count);
        let at = Instant::now();
        assert!(
            launch_environment_with(&builds[0], &stop, env.clone(), &cache)
                .1
                .is_some()
        );
        assert!(at.elapsed() >= Duration::from_millis(1900));
        for i in 0..8 {
            let at = Instant::now();
            let (values, warning) =
                launch_environment_with(&builds[i % 2], &stop, env.clone(), &cache);
            assert!(warning.is_some());
            // No second two-second fallback wait; 200 ms is the timing-run contract.
            let elapsed = at.elapsed();
            assert!(
                elapsed < budget(Duration::from_millis(200), Duration::from_millis(1500)),
                "shared negative cache must not wait: {elapsed:?}"
            );
            assert_eq!(values["FLAG"], if i % 2 == 0 { "A" } else { "B" });
            assert_eq!(
                values["PATH"],
                format!("/usr/bin:/bin:{}", if i % 2 == 0 { "/a" } else { "/b" })
            );
        }
        assert_eq!(std::fs::read_to_string(count).unwrap(), "x");
    }
    #[cfg(unix)]
    #[test]
    fn round18_final_login_cancelled_waiter_preserves_capture() {
        let dir = tempfile::tempdir().unwrap();
        let count = dir.path().join("calls");
        let release = dir.path().join("release");
        let (exec, env) = fake_profile(
            dir.path(),
            &format!(
                "printf x >> '{}'\nwhile [ ! -f '{}' ]; do sleep .01; done\nexport PROFILE_READY=shared",
                count.display(),
                release.display()
            ),
        );
        let cache = Mutex::new(Cache::default());
        cache.lock().unwrap().prepare(cache_key(&env), &env);
        await_started(&count);
        // Enter the real cancelled-worker branch with a known running capture.
        // A wall-time race against process startup would test scheduling instead.
        let (_, warning) =
            launch_environment_with(&exec, &AtomicBool::new(true), env.clone(), &cache);
        assert!(warning.unwrap().contains("cancelled"));
        assert!(cache.lock().unwrap().capture.is_some());
        // Release the same profile after detaching the cancelled first waiter.
        std::fs::write(&release, "").unwrap();
        await_login(&cache, &env);
        let (values, warning) =
            launch_environment_with(&exec, &AtomicBool::new(false), env, &cache);
        assert!(warning.is_none());
        assert_eq!(values["PROFILE_READY"], "shared");
        assert_eq!(std::fs::read_to_string(count).unwrap(), "x");
    }
}
