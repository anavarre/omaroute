use std::{env, io::{Read, Write}, os::unix::net::UnixStream, time::Duration};

/// Window class of the focused Hyprland window: the best available hint for
/// "which app did this link come from". None outside Hyprland or with no focus.
pub fn focused_app() -> Option<String> {
    let sig = env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let runtime = env::var("XDG_RUNTIME_DIR").ok()?;
    let mut s = UnixStream::connect(format!("{runtime}/hypr/{sig}/.socket.sock")).ok()?;
    let t = Some(Duration::from_millis(500));
    s.set_read_timeout(t).ok()?;
    s.set_write_timeout(t).ok()?;
    s.write_all(b"j/activewindow").ok()?;
    let mut buf = String::new();
    // The reply is a small JSON object; the cap keeps a misbehaving peer from growing memory.
    s.take(64 * 1024).read_to_string(&mut buf).ok()?;
    let v: serde_json::Value = serde_json::from_str(&buf).ok()?;
    let class = v.get("class")?.as_str()?.trim().to_lowercase();
    (!class.is_empty()).then_some(class)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{TempDir, with_env};
    use std::{ffi::OsStr, fs, os::unix::net::UnixListener, path::Path, thread};

    /// Serve one connection on the Hyprland socket path; returns the request that was received.
    fn serve(root: &Path, sig: &str, reply: &'static str) -> thread::JoinHandle<String> {
        let dir = root.join("hypr").join(sig);
        fs::create_dir_all(&dir).unwrap();
        let listener = UnixListener::bind(dir.join(".socket.sock")).unwrap();
        thread::spawn(move || {
            let (mut c, _) = listener.accept().unwrap();
            let mut req = vec![0u8; 64];
            let n = c.read(&mut req).unwrap();
            c.write_all(reply.as_bytes()).unwrap();
            String::from_utf8_lossy(&req[..n]).into_owned()
        })
    }

    fn query(root: &Path, sig: Option<&str>) -> Option<String> {
        with_env(
            &[
                ("XDG_RUNTIME_DIR", Some(root.as_os_str())),
                ("HYPRLAND_INSTANCE_SIGNATURE", sig.map(OsStr::new)),
            ],
            focused_app,
        )
    }

    #[test]
    fn returns_lowercased_class_and_sends_activewindow_request() {
        let t = TempDir::new("src-ok");
        let h = serve(t.path(), "sig", r#"{"address":"0x1","class":"  Slack\n","title":"x"}"#);
        assert_eq!(query(t.path(), Some("sig")).as_deref(), Some("slack"));
        assert_eq!(h.join().unwrap(), "j/activewindow");
    }

    #[test]
    fn pwa_classes_are_preserved() {
        let t = TempDir::new("src-pwa");
        serve(t.path(), "s", r#"{"class":"chrome-app.hey.com__-Default"}"#);
        assert_eq!(query(t.path(), Some("s")).as_deref(), Some("chrome-app.hey.com__-default"));
    }

    #[test]
    fn no_focused_window_yields_none() {
        let t = TempDir::new("src-empty-obj");
        serve(t.path(), "s", "{}");
        assert_eq!(query(t.path(), Some("s")), None);
    }

    #[test]
    fn blank_class_yields_none() {
        let t = TempDir::new("src-blank");
        serve(t.path(), "s", r#"{"class":"   "}"#);
        assert_eq!(query(t.path(), Some("s")), None);
    }

    #[test]
    fn non_string_class_yields_none() {
        let t = TempDir::new("src-nonstr");
        serve(t.path(), "s", r#"{"class":5}"#);
        assert_eq!(query(t.path(), Some("s")), None);
    }

    #[test]
    fn invalid_json_yields_none() {
        let t = TempDir::new("src-badjson");
        serve(t.path(), "s", "not json");
        assert_eq!(query(t.path(), Some("s")), None);
    }

    #[test]
    fn empty_reply_yields_none() {
        let t = TempDir::new("src-emptyreply");
        serve(t.path(), "s", "");
        assert_eq!(query(t.path(), Some("s")), None);
    }

    #[test]
    fn missing_socket_yields_none() {
        let t = TempDir::new("src-nosock");
        assert_eq!(query(t.path(), Some("absent")), None);
    }

    #[test]
    fn outside_hyprland_yields_none() {
        let t = TempDir::new("src-nohypr");
        assert_eq!(query(t.path(), None), None);
        with_env(&[("HYPRLAND_INSTANCE_SIGNATURE", Some(OsStr::new("s"))), ("XDG_RUNTIME_DIR", None)], || {
            assert_eq!(focused_app(), None);
        });
    }

    #[test]
    fn unresponsive_compositor_times_out_instead_of_hanging() {
        let t = TempDir::new("src-hang");
        let dir = t.path().join("hypr/s");
        fs::create_dir_all(&dir).unwrap();
        let _listener = UnixListener::bind(dir.join(".socket.sock")).unwrap(); // accepts via backlog, never replies
        let start = std::time::Instant::now();
        assert_eq!(query(t.path(), Some("s")), None);
        assert!(start.elapsed() < Duration::from_secs(5));
    }
}
