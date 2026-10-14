//! Lifecycle management for the bundled local speech-to-text server.
//!
//! When the user points UnoFlow at a local base URL (e.g.
//! `http://127.0.0.1:8000/v1`), we can start the Python STT server for them on
//! launch and shut it down on exit, so "local mode" is one setting rather than
//! a separate thing to remember to run.
//!
//! This is deliberately best-effort: if Python or the server folder is missing,
//! we do nothing and UnoFlow still works against whatever is already
//! listening on that URL (or against the cloud).
//!
//! Cleanup uses `taskkill /T /F`, which kills the whole process tree — important
//! because the Python server's children hold the port even after the launcher
//! exits.

use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Suppress the console window for a child process on Windows.
///
/// Every `Command` we run (Python probes, the server itself, `taskkill`) must
/// go through this, otherwise Windows flashes a console window on each spawn —
/// which looks like a stray cmd prompt appearing when the app starts.
#[cfg(windows)]
fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

#[cfg(not(windows))]
fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

/// A running local STT server, killed when this value is dropped.
pub struct LocalServer {
    child: Child,
}

impl LocalServer {
    /// Start the local server if the config asks for one and we can find it.
    ///
    /// Returns `Ok(None)` when the config points at a remote/cloud API or the
    /// server isn't installed — both are normal, not errors.
    pub fn start_if_local(
        base_url: &str,
        port: u16,
        dir: &Path,
        model: &str,
    ) -> Result<Option<Self>, String> {
        if !is_local_url(base_url) {
            return Ok(None);
        }

        // Two ways to run the server, in order of preference:
        //   1. `unoflow-stt.exe` — the bundled standalone build. Needs no Python.
        //   2. `server.py` — a source checkout, run with a system interpreter.
        let standalone = dir.join("unoflow-stt.exe");
        let script = dir.join("server.py");

        let mut cmd = if standalone.is_file() {
            let mut c = Command::new(&standalone);
            c.current_dir(dir);
            c
        } else if script.is_file() {
            let (exe, prefix) = python_launcher().ok_or_else(|| {
                "no Python interpreter found (tried `py`, `python`), and no \
                 bundled unoflow-stt.exe is present"
                    .to_string()
            })?;
            let mut c = Command::new(&exe);
            c.args(&prefix).arg(&script).current_dir(dir);
            c
        } else {
            eprintln!(
                "[local-stt] base URL is local but neither {} nor {} was found; \
                 not starting a server (expecting one to already be running)",
                standalone.display(),
                script.display()
            );
            return Ok(None);
        };

        // If something is already serving this port (e.g. a previous run was
        // force-killed and left its server behind), reuse it instead of
        // spawning a rival process that would fail to bind.
        if wait_until_ready(base_url, Duration::from_millis(400)) {
            eprintln!("[local-stt] a server is already listening on port {port}; reusing it");
            return Ok(None);
        }

        // Match the port we tell the app to use.
        cmd.env("UNOFLOW_STT_PORT", port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Only push the model through when it's a Whisper name; cloud model
        // names (gpt-4o-…) would make faster-whisper fail to load.
        if is_whisper_model(model) {
            cmd.env("UNOFLOW_STT_MODEL", model);
        }
        // Don't pop a console window on Windows.
        no_window(&mut cmd);

        match cmd.spawn() {
            Ok(child) => {
                eprintln!("[local-stt] started local STT server (pid {})", child.id());
                Ok(Some(Self { child }))
            }
            Err(e) => Err(format!("could not start the local STT server: {e}")),
        }
    }

    /// Stop the server and everything it spawned.
    pub fn stop(&mut self) {
        let pid = self.child.id();
        #[cfg(windows)]
        {
            let mut kill = Command::new("taskkill");
            kill.args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            no_window(&mut kill);
            let _ = kill.status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        eprintln!("[local-stt] stopped local STT server (pid {pid})");
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// True when `base_url` points at this machine.
pub fn is_local_url(base_url: &str) -> bool {
    let u = base_url.to_ascii_lowercase();
    u.contains("127.0.0.1") || u.contains("localhost") || u.contains("0.0.0.0") || u.contains("[::1]")
}

/// True for names faster-whisper understands (e.g. `base.en`, `small`, `large-v3`).
/// Cloud names like `gpt-4o-transcribe` are not, and would break model loading.
pub fn is_whisper_model(model: &str) -> bool {
    matches!(
        model,
        "tiny" | "tiny.en"
            | "base" | "base.en"
            | "small" | "small.en"
            | "medium" | "medium.en"
            | "large-v1" | "large-v2" | "large-v3"
            | "distil-small.en" | "distil-medium.en" | "distil-large-v3"
    )
}

/// The port to run the local server on, derived from the base URL when present.
/// `http://127.0.0.1:9000/v1` -> 9000; otherwise the default 8000.
pub fn port_from_url(base_url: &str) -> u16 {
    host_port(base_url).1
}

/// Split a base URL into `(host, port)` for a TCP readiness probe.
pub fn host_port(base_url: &str) -> (String, u16) {
    let after_scheme = base_url.split("://").nth(1).unwrap_or(base_url);
    let authority = after_scheme.split('/').next().unwrap_or("");
    // Drop any `user:pass@` prefix.
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(8000)),
        None => (authority.to_string(), 8000),
    }
}

/// Block until the server at `base_url` accepts a TCP connection, or the
/// timeout elapses. Returns true if it became reachable.
///
/// A TCP accept is enough: once uvicorn is listening, requests will be served
/// (the model loads lazily inside the first request, so that one is just slow,
/// not failed).
pub fn wait_until_ready(base_url: &str, timeout: Duration) -> bool {
    let (host, port) = host_port(base_url);
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(addrs) = (host.as_str(), port).to_socket_addrs() {
            for addr in addrs {
                if TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok() {
                    return true;
                }
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Where the bundled server lives: a `local-stt` folder next to the executable,
/// falling back to `%APPDATA%\UnoFlow\local-stt`.
///
/// Either entry point marks the folder as valid: the standalone
/// `unoflow-stt.exe` (release zip) or `server.py` (source checkout).
pub fn server_dir() -> Option<PathBuf> {
    let looks_like_server = |dir: &Path| {
        dir.join("unoflow-stt.exe").is_file() || dir.join("server.py").is_file()
    };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join("local-stt");
            if looks_like_server(&cand) {
                return Some(cand);
            }
        }
    }
    let appdata = std::env::var("APPDATA").ok()?;
    let cand = PathBuf::from(appdata).join("UnoFlow").join("local-stt");
    if looks_like_server(&cand) {
        Some(cand)
    } else {
        None
    }
}

/// Find a usable Python. Prefers the `py` launcher, which always points at a
/// real interpreter; the bare `python` on PATH can be the Microsoft Store stub
/// that redirects to WSL.
///
/// The returned vec is the argument prefix to place before the script path
/// (e.g. `["-3.10"]` for `py -3.10 server.py`).
fn python_launcher() -> Option<(String, Vec<String>)> {
    for ver in ["-3.10", "-3.11", "-3.12", "-3"] {
        if probe_ok("py", &[ver, "-c", "import faster_whisper"]) {
            return Some(("py".into(), vec![ver.into()]));
        }
    }
    // Last resort: a `python` that can import the dependency.
    if probe_ok("python", &["-c", "import faster_whisper"]) {
        return Some(("python".into(), vec![]));
    }
    None
}

/// Run `exe args…` silently and report whether it exited successfully.
///
/// Used for the interpreter probes above. The console window must be
/// suppressed here too: without it, each probe flashes a cmd window on startup.
fn probe_ok(exe: &str, args: &[&str]) -> bool {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    no_window(&mut cmd);
    matches!(cmd.status(), Ok(s) if s.success())
}
