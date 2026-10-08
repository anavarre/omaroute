//! Helpers for tests that touch process-global state (environment variables, temp dirs).
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

static LOCK: Mutex<()> = Mutex::new(());
static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = env::temp_dir().join(format!("omaroute-test-{}-{tag}-{n}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Restore(Vec<(OsString, Option<OsString>)>);

impl Drop for Restore {
    fn drop(&mut self) {
        for (k, v) in self.0.drain(..).rev() {
            // SAFETY: callers hold LOCK, so no other test thread touches the environment.
            unsafe {
                match v {
                    Some(v) => env::set_var(&k, v),
                    None => env::remove_var(&k),
                }
            }
        }
    }
}

/// Run `f` with the given variables set (`None` unsets), serialized against every other
/// `with_env` caller and restored afterwards, even if `f` panics.
pub fn with_env<R>(vars: &[(&str, Option<&OsStr>)], f: impl FnOnce() -> R) -> R {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut restore = Restore(Vec::new());
    for (k, v) in vars {
        restore.0.push((OsString::from(k), env::var_os(k)));
        // SAFETY: LOCK is held for the duration of the call.
        unsafe {
            match v {
                Some(v) => env::set_var(k, v),
                None => env::remove_var(k),
            }
        }
    }
    f()
}

/// Point HOME and every XDG base dir at a fresh sandbox under `root`.
pub fn with_home<R>(root: &Path, f: impl FnOnce() -> R) -> R {
    let (c, s, d) = (root.join("config"), root.join("state"), root.join("data"));
    with_env(
        &[
            ("HOME", Some(root.as_os_str())),
            ("XDG_CONFIG_HOME", Some(c.as_os_str())),
            ("XDG_STATE_HOME", Some(s.as_os_str())),
            ("XDG_DATA_HOME", Some(d.as_os_str())),
        ],
        f,
    )
}
