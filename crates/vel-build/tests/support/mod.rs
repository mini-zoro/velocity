//! Explicit runtime login fixture; compiled only into test executables.
use std::{collections::BTreeMap, sync::OnceLock};
/// Installs the private environment before any test can start a capture.
pub fn install() {
    vel_build::set_login_capture_provider(environment);
}
fn environment() -> BTreeMap<String, String> {
    static FIXTURE: OnceLock<tempfile::TempDir> = OnceLock::new();
    let mut env = std::env::vars().collect::<BTreeMap<String, String>>();
    let (home, shell) = if let Some(root) = std::env::var_os("VELOCITY_R18_LOGIN_FIXTURE") {
        // Explicit profile tests run as isolated children, never change the
        // harness's process environment. Only shells inside that fixture qualify.
        let root = std::path::PathBuf::from(root);
        let shell = std::path::PathBuf::from(env.get("SHELL").expect("fixture SHELL"));
        assert_eq!(env.get("HOME"), Some(&root.to_string_lossy().into_owned()));
        assert!(shell.starts_with(&root) && shell.is_file());
        (root, shell)
    } else {
        let root = FIXTURE
            .get_or_init(|| {
                let dir = tempfile::tempdir().expect("test login HOME");
                let shell = dir.path().join("login-shell");
                std::fs::write(&shell, "#!/bin/sh\nexec /bin/sh -c \"$3\"\n")
                    .expect("fixture shell");
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700))
                        .unwrap();
                }
                dir
            })
            .path()
            .to_owned();
        let shell = root.join("login-shell");
        (root, shell)
    };
    env.insert("HOME".into(), home.to_string_lossy().into_owned());
    env.insert("SHELL".into(), shell.to_string_lossy().into_owned());
    env.insert("ZDOTDIR".into(), home.to_string_lossy().into_owned());
    env.remove("BASH_ENV");
    env.remove("ENV");
    env
}
