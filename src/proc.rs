//! Subprocess execution with a timeout, and PATH lookup.
//!
//! Python got `subprocess.run(input=..., capture_output=True, timeout=N)` for
//! free. Rust's standard library has neither a wait-with-timeout nor a safe way
//! to write one pipe while reading two others: servicing them on one thread
//! deadlocks the moment the child fills a buffer nobody is draining. Every
//! subprocess in the Core goes through here - nvidia-smi in `sysinfo` and
//! systemctl in `doctor`.

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// How often the wait loop checks for exit. Irrelevant against the 10s and
/// 300s budgets this is used with, and it keeps the module dependency-free.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone)]
pub struct Output {
    /// `None` when the child was killed by a signal.
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn succeeded(&self) -> bool {
        self.status == Some(0)
    }
}

#[derive(Debug)]
pub enum RunError {
    /// The program is not on PATH. Callers distinguish this because "the
    /// command is not installed" is worth saying differently from "the command
    /// failed".
    NotFound(String),
    Timeout,
    Io(std::io::Error),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::NotFound(program) => write!(f, "command not found: {program}"),
            RunError::Timeout => write!(f, "command timed out"),
            RunError::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RunError {}

/// The seam every caller takes, so tests can supply canned output without
/// spawning anything.
pub trait CommandRunner {
    fn run(
        &self,
        argv: &[String],
        input: Option<&str>,
        timeout: Duration,
    ) -> Result<Output, RunError>;
}

pub struct RealRunner;

impl CommandRunner for RealRunner {
    fn run(
        &self,
        argv: &[String],
        input: Option<&str>,
        timeout: Duration,
    ) -> Result<Output, RunError> {
        run(argv, input, timeout)
    }
}

pub fn run(argv: &[String], input: Option<&str>, timeout: Duration) -> Result<Output, RunError> {
    let Some((program, args)) = argv.split_first() else {
        return Err(RunError::NotFound(String::new()));
    };

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => RunError::NotFound(program.clone()),
            _ => RunError::Io(err),
        })?;

    // One thread per pipe. The writer drops its handle on the way out, which
    // closes the pipe and is how the child sees EOF on stdin.
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let payload = input.map(str::to_owned);

    let writer = thread::spawn(move || {
        if let Some(mut handle) = stdin {
            if let Some(text) = payload {
                let _ = handle.write_all(text.as_bytes());
            }
        }
    });
    let out_reader = thread::spawn(move || read_all(stdout));
    let err_reader = thread::spawn(move || read_all(stderr));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(err) => return Err(RunError::Io(err)),
        }
        if Instant::now() >= deadline {
            // Killing closes the child's end of every pipe, which unblocks all
            // three helper threads: the writer gets EPIPE, the readers get EOF.
            let _ = child.kill();
            let _ = child.wait();
            let _ = writer.join();
            let _ = out_reader.join();
            let _ = err_reader.join();
            return Err(RunError::Timeout);
        }
        thread::sleep(POLL_INTERVAL);
    };

    let _ = writer.join();
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();

    Ok(Output {
        status: status.code(),
        stdout,
        stderr,
    })
}

fn read_all<R: Read>(handle: Option<R>) -> String {
    let mut buf = Vec::new();
    if let Some(mut handle) = handle {
        let _ = handle.read_to_end(&mut buf);
    }
    // Lossy conversion is correct here and only here: this is a program's
    // output, shown to a human or parsed as JSON. Nothing read through this
    // function is ever written back to a file.
    String::from_utf8_lossy(&buf).into_owned()
}

/// Absolute path to `name` on `path_env` (or `$PATH`), or None.
///
/// Stands in for `shutil.which`. An empty PATH segment is skipped rather than
/// treated as the working directory - `doctor` uses this to decide which
/// `steamtrain` a shell would actually run, and "whatever is in the current
/// directory" is not an answer worth acting on.
pub fn which(name: &str, path_env: Option<&str>) -> Option<PathBuf> {
    if name.contains('/') {
        let candidate = Path::new(name);
        return is_executable(candidate).then(|| candidate.to_path_buf());
    }
    let path = match path_env {
        Some(value) => value.to_string(),
        None => std::env::var("PATH").ok()?,
    };
    path.split(':')
        .filter(|segment| !segment.is_empty())
        .map(|segment| Path::new(segment).join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn captures_stdout_and_status() {
        let out = RealRunner
            .run(
                &argv(&["sh", "-c", "printf hello; exit 3"]),
                None,
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(out.stdout, "hello");
        assert_eq!(out.status, Some(3));
        assert!(!out.succeeded());
    }

    #[test]
    fn feeds_stdin_and_captures_stderr() {
        let out = RealRunner
            .run(
                &argv(&["sh", "-c", "cat; printf oops >&2"]),
                Some("fed"),
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(out.stdout, "fed");
        assert_eq!(out.stderr, "oops");
        assert!(out.succeeded());
    }

    #[test]
    fn does_not_deadlock_on_large_output() {
        // A child that writes more than one pipe buffer while we are also
        // writing its stdin is exactly what a single-threaded implementation
        // hangs on.
        let out = RealRunner
            .run(
                &argv(&["sh", "-c", "cat; yes x | head -c 200000"]),
                Some("go"),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(out.stdout.len(), 200_002);
    }

    #[test]
    fn times_out_and_kills_the_child() {
        let err = RealRunner
            .run(&argv(&["sleep", "30"]), None, Duration::from_millis(200))
            .unwrap_err();
        assert!(matches!(err, RunError::Timeout));
    }

    #[test]
    fn reports_a_missing_program_distinctly() {
        let err = RealRunner
            .run(
                &argv(&["definitely-not-a-real-program-xyz"]),
                None,
                Duration::from_secs(5),
            )
            .unwrap_err();
        match err {
            RunError::NotFound(program) => {
                assert_eq!(program, "definitely-not-a-real-program-xyz")
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    fn make_executable(path: &Path) {
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn which_finds_an_executable_on_the_given_path() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fakeprog");
        make_executable(&exe);

        let path_env = dir.path().to_str().unwrap();
        assert_eq!(
            which("fakeprog", Some(path_env)).as_deref(),
            Some(exe.as_path())
        );
        assert_eq!(which("nope", Some(path_env)), None);
    }

    #[test]
    fn which_ignores_a_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("plain"), "").unwrap();
        assert_eq!(which("plain", Some(dir.path().to_str().unwrap())), None);
    }

    #[test]
    fn which_takes_the_first_matching_path_segment() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        make_executable(&first.path().join("dup"));
        make_executable(&second.path().join("dup"));

        let path_env = format!("{}:{}", first.path().display(), second.path().display());
        assert_eq!(
            which("dup", Some(&path_env)).as_deref(),
            Some(first.path().join("dup").as_path())
        );
    }

    #[test]
    fn which_accepts_an_explicit_path() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("direct");
        make_executable(&exe);

        assert_eq!(
            which(exe.to_str().unwrap(), Some("")).as_deref(),
            Some(exe.as_path())
        );
        assert_eq!(which("/nonexistent/thing", Some("")), None);
    }
}
