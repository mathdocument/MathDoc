use super::CompilerRes;
use anyhow::{bail, Result};
use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(super) fn require_tool(name: &str) -> Result<PathBuf> {
    which::which(name).map_err(|_| anyhow::anyhow!("{name} not found in PATH"))
}

/// At most 1 MiB of raw bytes is retained for each output stream. The first and
/// last halves are kept so both startup context and final diagnostics survive;
/// the pipes continue to be drained after this limit is reached.
const OUTPUT_CAPTURE_LIMIT_BYTES: usize = 1024 * 1024;
const OUTPUT_CAPTURE_HEAD_BYTES: usize = OUTPUT_CAPTURE_LIMIT_BYTES / 2;
const OUTPUT_CAPTURE_TAIL_BYTES: usize = OUTPUT_CAPTURE_LIMIT_BYTES - OUTPUT_CAPTURE_HEAD_BYTES;
const PIPE_READ_SIZE: usize = 16 * 1024;
const DRAIN_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

pub(super) fn ensure_complete_machine_output(stdout: &str, stderr: &str) -> Result<()> {
    if stdout.contains("\n[stdout truncated: omitted ")
        || stderr.contains("\n[stderr truncated: omitted ")
    {
        bail!("compiler dependency output exceeded the capture limit");
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum ProcessControlError {
    #[error("{tool} timed out after {timeout_sec} seconds{diagnostics}")]
    Timeout {
        tool: String,
        timeout_sec: u64,
        diagnostics: String,
    },
    #[error("{tool} interrupted by signal {signal}{diagnostics}")]
    Interrupted {
        tool: String,
        signal: i32,
        diagnostics: String,
    },
}

struct BoundedOutput {
    total_bytes: u64,
    head: Vec<u8>,
    tail: VecDeque<u8>,
}

impl BoundedOutput {
    fn new() -> Self {
        Self {
            total_bytes: 0,
            head: Vec::with_capacity(OUTPUT_CAPTURE_HEAD_BYTES),
            tail: VecDeque::with_capacity(OUTPUT_CAPTURE_TAIL_BYTES),
        }
    }

    fn push(&mut self, mut bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);

        let head_remaining = OUTPUT_CAPTURE_HEAD_BYTES - self.head.len();
        let head_len = head_remaining.min(bytes.len());
        self.head.extend_from_slice(&bytes[..head_len]);
        bytes = &bytes[head_len..];

        if bytes.len() >= OUTPUT_CAPTURE_TAIL_BYTES {
            self.tail.clear();
            self.tail
                .extend(&bytes[bytes.len() - OUTPUT_CAPTURE_TAIL_BYTES..]);
            return;
        }

        let excess = self
            .tail
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(OUTPUT_CAPTURE_TAIL_BYTES);
        if excess > 0 {
            self.tail.drain(..excess);
        }
        self.tail.extend(bytes);
    }

    fn into_string(self, stream_name: &str) -> String {
        if self.total_bytes <= OUTPUT_CAPTURE_LIMIT_BYTES as u64 {
            let mut bytes = self.head;
            bytes.extend(self.tail);
            return String::from_utf8_lossy(&bytes).into_owned();
        }

        let omitted = self
            .total_bytes
            .saturating_sub((self.head.len() + self.tail.len()) as u64);
        let mut output = String::with_capacity(OUTPUT_CAPTURE_LIMIT_BYTES + 128);
        output.push_str(&String::from_utf8_lossy(&self.head));
        output.push_str(&format!(
            "\n[{stream_name} truncated: omitted {omitted} bytes; showing first {} and last {} bytes]\n",
            self.head.len(),
            self.tail.len()
        ));
        let tail: Vec<u8> = self.tail.into_iter().collect();
        output.push_str(&String::from_utf8_lossy(&tail));
        output
    }
}

struct DrainResult {
    output: String,
    error: Option<String>,
}

struct PipeDrain {
    stream_name: &'static str,
    receiver: std::sync::mpsc::Receiver<DrainResult>,
    handle: Option<std::thread::JoinHandle<()>>,
    result: Option<DrainResult>,
    stop: Arc<AtomicBool>,
}

impl PipeDrain {
    fn spawn<R>(mut pipe: R, stream_name: &'static str) -> std::io::Result<Self>
    where
        R: Read + Send + std::os::fd::AsRawFd + 'static,
    {
        let fd = pipe.as_raw_fd();
        let (sender, receiver) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name(format!("mdc-{stream_name}-drain"))
            .spawn(move || {
                let mut capture = BoundedOutput::new();
                let mut chunk = [0_u8; PIPE_READ_SIZE];
                let error = loop {
                    if thread_stop.load(Ordering::Relaxed) {
                        break None;
                    }
                    let read_limit = chunk.len();

                    match pipe.read(&mut chunk[..read_limit]) {
                        Ok(0) => break None,
                        Ok(read) => capture.push(&chunk[..read]),
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            let mut descriptor = libc::pollfd {
                                fd,
                                events: libc::POLLIN,
                                revents: 0,
                            };
                            // SAFETY: descriptor contains the valid pipe fd owned by this thread.
                            unsafe { libc::poll(&mut descriptor, 1, 100) };
                        }
                        Err(e) => break Some(e.to_string()),
                    }
                };
                let _ = sender.send(DrainResult {
                    output: capture.into_string(stream_name),
                    error,
                });
            })?;
        Ok(Self {
            stream_name,
            receiver,
            handle: Some(handle),
            result: None,
            stop,
        })
    }

    fn poll(&mut self) {
        if self.result.is_some() {
            return;
        }
        match self.receiver.try_recv() {
            Ok(mut result) => {
                if self
                    .handle
                    .take()
                    .is_some_and(|handle| handle.join().is_err())
                {
                    result.error = Some("output drain thread panicked".to_string());
                }
                self.result = Some(result);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
                self.result = Some(DrainResult {
                    output: String::new(),
                    error: Some("output drain thread stopped unexpectedly".to_string()),
                });
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    fn is_done(&self) -> bool {
        self.result.is_some()
    }

    fn error(&self) -> Option<String> {
        self.result
            .as_ref()
            .and_then(|result| result.error.as_ref())
            .map(|error| format!("failed to read {}: {error}", self.stream_name))
    }

    fn output(&self) -> &str {
        self.result
            .as_ref()
            .map_or("", |result| result.output.as_str())
    }

    fn mark_unavailable(&mut self) {
        if self.result.is_some() {
            return;
        }
        self.stop.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.result.is_none() && Instant::now() < deadline {
            self.poll();
            if self.result.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if self.result.is_some() {
            return;
        }
        self.handle.take();
        self.result = Some(DrainResult {
            output: format!(
                "[{} unavailable: pipe remained open after process termination]",
                self.stream_name
            ),
            error: None,
        });
    }

    fn into_output(mut self) -> String {
        self.poll();
        self.result
            .expect("completed pipe drain must have a result")
            .output
    }
}

fn wait_for_drains(stdout: &mut PipeDrain, stderr: &mut PipeDrain) {
    let started = Instant::now();
    while !stdout.is_done() || !stderr.is_done() {
        stdout.poll();
        stderr.poll();
        if stdout.is_done() && stderr.is_done() {
            return;
        }
        if started.elapsed() >= DRAIN_SHUTDOWN_GRACE {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    stdout.mark_unavailable();
    stderr.mark_unavailable();
}

fn output_diagnostics(stdout: &str, stderr: &str) -> String {
    let mut diagnostics = String::new();
    if !stdout.is_empty() {
        diagnostics.push_str("\nstdout:\n");
        diagnostics.push_str(stdout);
    }
    if !stderr.is_empty() {
        diagnostics.push_str("\nstderr:\n");
        diagnostics.push_str(stderr);
    }
    diagnostics
}

fn drain_diagnostics(stdout: &PipeDrain, stderr: &PipeDrain) -> String {
    output_diagnostics(stdout.output(), stderr.output())
}

fn terminate_process(child: &mut std::process::Child, leader_reaped: bool) {
    let process_group = child.id() as libc::pid_t;
    // The command is placed in a group whose id is its pid before exec.
    unsafe {
        libc::killpg(process_group, libc::SIGKILL);
    }

    if !leader_reaped {
        // Also try the direct process in case group termination failed, then reap it.
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn set_pipe_nonblocking<T: std::os::fd::AsRawFd>(pipe: &T) -> std::io::Result<()> {
    let fd = pipe.as_raw_fd();
    // SAFETY: fcntl reads and updates flags on a valid child-pipe descriptor.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

struct SignalListener {
    previous: libc::sigset_t,
    signal: Arc<std::sync::atomic::AtomicI32>,
    stop: Arc<AtomicBool>,
    thread_id: libc::pthread_t,
    thread: Option<std::thread::JoinHandle<()>>,
    restored: bool,
}

impl SignalListener {
    fn new() -> Result<Self> {
        // Block only this thread. Drain threads inherit the mask, while the
        // child resets it before exec. No process-global handler is replaced.
        let mut signals = unsafe { std::mem::zeroed() };
        let mut previous = unsafe { std::mem::zeroed() };
        // SAFETY: both sigset_t values are initialized and valid for these APIs.
        unsafe {
            libc::sigemptyset(&mut signals);
            libc::sigaddset(&mut signals, libc::SIGINT);
            libc::sigaddset(&mut signals, libc::SIGTERM);
            let result = libc::pthread_sigmask(libc::SIG_BLOCK, &signals, &mut previous);
            if result != 0 {
                return Err(std::io::Error::from_raw_os_error(result).into());
            }
        }
        let signal = Arc::new(std::sync::atomic::AtomicI32::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_signal = Arc::clone(&signal);
        let thread_stop = Arc::clone(&stop);
        let (id_sender, id_receiver) = std::sync::mpsc::sync_channel(1);
        let thread = match std::thread::Builder::new()
            .name("mdc-compiler-signals".to_string())
            .spawn(move || {
                // SIGUSR1 is used only to wake this specific listener on drop.
                unsafe {
                    libc::sigaddset(&mut signals, libc::SIGUSR1);
                    libc::pthread_sigmask(libc::SIG_BLOCK, &signals, std::ptr::null_mut());
                }
                // SAFETY: pthread_self returns the identifier for this live thread.
                let _ = id_sender.send(unsafe { libc::pthread_self() });
                loop {
                    let mut received = 0;
                    // SAFETY: signals is initialized and blocked in this thread.
                    let result = unsafe { libc::sigwait(&signals, &mut received) };
                    if result != 0 {
                        break;
                    }
                    if received == libc::SIGUSR1 && thread_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if matches!(received, libc::SIGINT | libc::SIGTERM) {
                        thread_signal.store(received, Ordering::Relaxed);
                    }
                }
            }) {
            Ok(thread) => thread,
            Err(error) => {
                // SAFETY: previous was saved above for this thread.
                unsafe {
                    libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut());
                }
                return Err(error.into());
            }
        };
        let thread_id = id_receiver.recv()?;
        Ok(Self {
            previous,
            signal,
            stop,
            thread_id,
            thread: Some(thread),
            restored: false,
        })
    }

    fn received(&self) -> Option<i32> {
        match self.signal.swap(0, Ordering::Relaxed) {
            0 => None,
            signal => Some(signal),
        }
    }

    fn child_mask(&self) -> libc::sigset_t {
        let mut mask = self.previous;
        // SAFETY: mask is a valid copy of the caller's prior signal mask.
        unsafe {
            libc::sigdelset(&mut mask, libc::SIGINT);
            libc::sigdelset(&mut mask, libc::SIGTERM);
        }
        mask
    }

    fn shutdown(&mut self) -> Option<i32> {
        if self.restored {
            return self.received();
        }
        self.stop.store(true, Ordering::Relaxed);
        // SAFETY: thread_id names the listener until it is joined below.
        unsafe { libc::pthread_kill(self.thread_id, libc::SIGUSR1) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let received = self.received();
        // SAFETY: previous is the exact mask saved by pthread_sigmask in new().
        unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.previous, std::ptr::null_mut());
        }
        self.restored = true;
        received
    }
}

impl Drop for SignalListener {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn interrupt_process(child: &mut std::process::Child, signal: i32, listener: &SignalListener) {
    let process_group = child.id() as libc::pid_t;
    // SAFETY: the child was placed in this process group before exec.
    unsafe { libc::killpg(process_group, signal) };
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if listener.received().is_some() {
            terminate_process(child, false);
            return;
        }
        if child.try_wait().ok().flatten().is_some() {
            terminate_process(child, true);
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    terminate_process(child, false);
}

/// Run a subprocess and wait, with a polling timeout. Returns `(rtcode, stdout, stderr)`.
///
/// stdout and stderr are drained in background threads immediately after spawn.
/// Without this, a child that fills an OS pipe would block while the parent waits.
/// On Unix the command has its own process group so timeout and I/O error cleanup
/// also terminates descendants that inherited either pipe.
pub(super) fn run_process<P, I, S>(
    program: P,
    args: I,
    tool_name: &str,
    timeout_sec: u64,
    cwd: Option<&crate::workspace::DirectoryGeneration>,
) -> Result<(i32, String, String)>
where
    P: AsRef<std::ffi::OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    run_process_inner(program, args, tool_name, timeout_sec, cwd, None)
}

pub(super) fn run_process_with_inherited_fd<P, I, S>(
    program: P,
    args: I,
    tool_name: &str,
    timeout_sec: u64,
    cwd: &crate::workspace::DirectoryGeneration,
    inherited_fd: std::os::fd::RawFd,
) -> Result<(i32, String, String)>
where
    P: AsRef<std::ffi::OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    run_process_inner(
        program,
        args,
        tool_name,
        timeout_sec,
        Some(cwd),
        Some(inherited_fd),
    )
}

fn run_process_inner<P, I, S>(
    program: P,
    args: I,
    tool_name: &str,
    timeout_sec: u64,
    cwd: Option<&crate::workspace::DirectoryGeneration>,
    inherited_fd: Option<std::os::fd::RawFd>,
) -> Result<(i32, String, String)>
where
    P: AsRef<std::ffi::OsStr>,
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    use std::process::Stdio;

    let mut signal_listener = SignalListener::new()?;
    let result = (|| -> Result<(i32, String, String)> {
        let started = Instant::now();
        let timeout = Duration::from_secs(timeout_sec);

        if let Some(cwd) = cwd {
            cwd.require_current()?;
            crate::workspace::run_test_hook(crate::workspace::TestHookPoint::ProcessAfterCwdOpen);
        }
        let process_cwd_fd = cwd.map(crate::workspace::DirectoryGeneration::raw_fd);

        let mut cmd = std::process::Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        {
            use std::os::unix::process::CommandExt;
            let child_signal_mask = signal_listener.child_mask();
            let cwd_fd = process_cwd_fd;
            cmd.process_group(0);
            // SAFETY: pre_exec uses async-signal-safe operations before exec. The
            // directory descriptor remains alive in the parent through spawn.
            unsafe {
                cmd.pre_exec(move || {
                    if let Some(fd) = inherited_fd {
                        let flags = libc::fcntl(fd, libc::F_GETFD);
                        if flags < 0
                            || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                        {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    if let Some(cwd_fd) = cwd_fd {
                        if libc::fchdir(cwd_fd) != 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    let result = libc::pthread_sigmask(
                        libc::SIG_SETMASK,
                        &child_signal_mask,
                        std::ptr::null_mut(),
                    );
                    if result == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::from_raw_os_error(result))
                    }
                });
            }
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to run {tool_name}: {e}"))?;

        // Take the pipes before entering the wait loop so the drain threads hold
        // the only read ends; the child can write freely without blocking.
        let stdout_pipe = child.stdout.take().expect("stdout is piped");
        let stderr_pipe = child.stderr.take().expect("stderr is piped");
        if let Err(error) =
            set_pipe_nonblocking(&stdout_pipe).and_then(|_| set_pipe_nonblocking(&stderr_pipe))
        {
            terminate_process(&mut child, false);
            bail!("failed to configure compiler output pipes: {error}");
        }
        let mut stdout_drain = match PipeDrain::spawn(stdout_pipe, "stdout") {
            Ok(drain) => drain,
            Err(error) => {
                terminate_process(&mut child, false);
                bail!("failed to start stdout drain: {error}");
            }
        };
        let mut stderr_drain = match PipeDrain::spawn(stderr_pipe, "stderr") {
            Ok(drain) => drain,
            Err(error) => {
                terminate_process(&mut child, false);
                stdout_drain.mark_unavailable();
                bail!("failed to start stderr drain: {error}");
            }
        };

        let status = loop {
            stdout_drain.poll();
            stderr_drain.poll();

            if let Some(signal) = signal_listener.received() {
                interrupt_process(&mut child, signal, &signal_listener);
                wait_for_drains(&mut stdout_drain, &mut stderr_drain);
                return Err(ProcessControlError::Interrupted {
                    tool: tool_name.to_string(),
                    signal,
                    diagnostics: drain_diagnostics(&stdout_drain, &stderr_drain),
                }
                .into());
            }

            match child.try_wait() {
                Ok(Some(exit_status)) => {
                    // Completion is recorded before descendant/drain cleanup so
                    // cleanup latency cannot turn a completed process into a timeout.
                    terminate_process(&mut child, true);
                    wait_for_drains(&mut stdout_drain, &mut stderr_drain);
                    if let Some(signal) = signal_listener.received() {
                        return Err(ProcessControlError::Interrupted {
                            tool: tool_name.to_string(),
                            signal,
                            diagnostics: drain_diagnostics(&stdout_drain, &stderr_drain),
                        }
                        .into());
                    }
                    if let Some(error) = stdout_drain.error().or_else(|| stderr_drain.error()) {
                        bail!("{error}{}", drain_diagnostics(&stdout_drain, &stderr_drain));
                    }
                    break exit_status;
                }
                Ok(None) => {}
                Err(e) => {
                    terminate_process(&mut child, false);
                    wait_for_drains(&mut stdout_drain, &mut stderr_drain);
                    bail!(
                        "failed while waiting for {tool_name}: {e}{}",
                        drain_diagnostics(&stdout_drain, &stderr_drain)
                    );
                }
            }

            if let Some(error) = stdout_drain.error().or_else(|| stderr_drain.error()) {
                terminate_process(&mut child, false);
                wait_for_drains(&mut stdout_drain, &mut stderr_drain);
                bail!("{error}{}", drain_diagnostics(&stdout_drain, &stderr_drain));
            }

            if started.elapsed() >= timeout {
                terminate_process(&mut child, false);
                wait_for_drains(&mut stdout_drain, &mut stderr_drain);
                return Err(ProcessControlError::Timeout {
                    tool: tool_name.to_string(),
                    timeout_sec,
                    diagnostics: drain_diagnostics(&stdout_drain, &stderr_drain),
                }
                .into());
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        let stdout = stdout_drain.into_output();
        let stderr = stderr_drain.into_output();
        Ok((status.code().unwrap_or(-1), stdout, stderr))
    })();

    let result = match (
        result,
        cwd.map(crate::workspace::DirectoryGeneration::require_current),
    ) {
        (Err(process_error), Some(Err(cwd_error))) => Err(process_error.context(format!(
            "compiler working directory validation also failed: {cwd_error:#}"
        ))),
        (Ok((code, _, _)), Some(Err(cwd_error))) => {
            Err(cwd_error.context(format!("process exited with code {code}")))
        }
        (result, _) => result,
    };

    if let Some(signal) = signal_listener.shutdown() {
        let diagnostics = match &result {
            Ok((_, stdout, stderr)) => output_diagnostics(stdout, stderr),
            Err(error) => format!("\ncleanup result:\n{error}"),
        };
        return Err(ProcessControlError::Interrupted {
            tool: tool_name.to_string(),
            signal,
            diagnostics,
        }
        .into());
    }
    result
}

pub(super) fn process_error_result(error: anyhow::Error, fallback: i32) -> CompilerRes {
    let (rtcode, interrupted) = match error.downcast_ref::<ProcessControlError>() {
        Some(ProcessControlError::Timeout { .. }) => (124, false),
        Some(ProcessControlError::Interrupted { signal, .. }) => (128 + signal, true),
        None => (fallback, false),
    };
    CompilerRes {
        stdout: String::new(),
        stderr: format!("{error:#}"),
        rtcode,
        interrupted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_large_output_is_bounded_without_blocking() {
        if which::which("yes").is_err() || which::which("head").is_err() {
            return;
        }
        let output_bytes = OUTPUT_CAPTURE_LIMIT_BYTES + 64 * 1024;
        let script = format!(
            "printf 'stdout-start\\n'; yes x | head -c {output_bytes}; printf '\\nstdout-end\\n'; \
             {{ printf 'stderr-start\\n'; yes y | head -c {output_bytes}; printf '\\nstderr-end\\n'; }} >&2"
        );
        let (code, stdout, stderr) = run_process(
            "/bin/sh",
            ["-c", script.as_str()],
            "large-output helper",
            10,
            None,
        )
        .expect("large output must not be misreported as timeout");
        assert_eq!(code, 0);
        assert!(stdout.contains("stdout-start"));
        assert!(stdout.contains("stdout-end"));
        assert!(stdout.contains("[stdout truncated: omitted "));
        assert!(stderr.contains("stderr-start"));
        assert!(stderr.contains("stderr-end"));
        assert!(stderr.contains("[stderr truncated: omitted "));
        assert!(stdout.len() <= OUTPUT_CAPTURE_LIMIT_BYTES + 256);
        assert!(stderr.len() <= OUTPUT_CAPTURE_LIMIT_BYTES + 256);
    }

    #[test]
    fn test_non_utf8_output_is_preserved_lossily() {
        let (code, stdout, stderr) = run_process(
            "/bin/sh",
            [
                "-c",
                "printf 'valid-before:\\377:valid-after\\n'; printf 'error-before:\\376:error-after\\n' >&2",
            ],
            "non-utf8 helper",
            5,
            None,
        )
        .expect("non-UTF-8 output must not be discarded");
        assert_eq!(code, 0);
        assert!(stdout.contains("valid-before:\u{fffd}:valid-after"));
        assert!(stderr.contains("error-before:\u{fffd}:error-after"));
    }

    #[test]
    fn test_stale_working_directory_is_rejected_before_execution() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let cwd = root.join("cwd");
        let displaced = root.join("displaced-cwd");
        std::fs::create_dir(&cwd).unwrap();
        let generation = crate::workspace::DirectoryGeneration::open_beneath(&root, &cwd).unwrap();
        std::fs::rename(&cwd, &displaced).unwrap();
        std::fs::create_dir(&cwd).unwrap();

        let error = run_process(
            "/bin/sh",
            ["-c", "printf executed > marker"],
            "cwd helper",
            5,
            Some(&generation),
        )
        .unwrap_err();

        assert!(crate::workspace::error_has_file_conflict(&error));
        assert!(!cwd.join("marker").exists());
        assert!(!displaced.join("marker").exists());
    }

    #[test]
    fn test_replaced_working_directory_generation_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let cwd = root.join("cwd");
        let displaced = root.join("displaced-cwd");
        std::fs::create_dir(&cwd).unwrap();
        let generation = crate::workspace::DirectoryGeneration::open_beneath(&root, &cwd).unwrap();
        crate::workspace::set_test_hook(crate::workspace::TestHookPoint::ProcessAfterCwdOpen, {
            let cwd = cwd.clone();
            move || {
                std::fs::rename(&cwd, displaced).unwrap();
                std::fs::create_dir(&cwd).unwrap();
            }
        });

        let error = run_process(
            "/bin/sh",
            ["-c", "exit 0"],
            "cwd helper",
            5,
            Some(&generation),
        )
        .unwrap_err();

        assert!(
            crate::workspace::error_has_file_conflict(&error),
            "{error:#}"
        );
    }

    #[test]
    fn test_symlinked_working_directory_ancestor_cannot_redirect_execution() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let managed = root.join("managed");
        let cwd = managed.join("cwd");
        let displaced = root.join("displaced-managed");
        std::fs::create_dir_all(&cwd).unwrap();
        let generation = crate::workspace::DirectoryGeneration::open_beneath(&root, &cwd).unwrap();
        let outside_path = outside.path().canonicalize().unwrap();
        std::fs::create_dir(outside_path.join("cwd")).unwrap();
        crate::workspace::set_test_hook(crate::workspace::TestHookPoint::ProcessAfterCwdOpen, {
            let managed = managed.clone();
            let outside_path = outside_path.clone();
            move || {
                std::fs::rename(&managed, displaced).unwrap();
                symlink(outside_path, managed).unwrap();
            }
        });

        let error = run_process(
            "/bin/sh",
            ["-c", "printf executed > marker"],
            "cwd helper",
            5,
            Some(&generation),
        )
        .unwrap_err();

        assert!(crate::workspace::error_has_file_conflict(&error));
        assert!(!outside.path().join("cwd/marker").exists());
        assert_eq!(
            std::fs::read(root.join("displaced-managed/cwd/marker")).unwrap(),
            b"executed"
        );
    }

    #[test]
    fn test_cwd_conflict_does_not_hide_a_process_timeout() {
        if which::which("sleep").is_err() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let cwd = root.join("cwd");
        let displaced = root.join("displaced-cwd");
        std::fs::create_dir(&cwd).unwrap();
        let generation = crate::workspace::DirectoryGeneration::open_beneath(&root, &cwd).unwrap();
        crate::workspace::set_test_hook(crate::workspace::TestHookPoint::ProcessAfterCwdOpen, {
            let cwd = cwd.clone();
            move || {
                std::fs::rename(&cwd, displaced).unwrap();
                std::fs::create_dir(&cwd).unwrap();
            }
        });

        let error = run_process("sleep", ["60"], "sleep", 1, Some(&generation)).unwrap_err();
        let result = process_error_result(error, 1);

        assert_eq!(result.rtcode, 124);
        assert!(!result.interrupted);
        assert!(result.stderr.contains("working directory validation"));
    }

    /// Regression: a genuinely slow process must still be killed and reported as timed out.
    #[test]
    fn test_real_timeout_is_reported() {
        if which::which("sleep").is_err() {
            return;
        }
        let result = run_process("sleep", ["60"], "sleep", 1, None);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("timed out after"),
            "timed-out process must produce a timed-out error"
        );
    }

    #[test]
    fn test_completion_near_timeout_is_not_overwritten() {
        let started = Instant::now();
        let result = run_process(
            "/bin/sh",
            ["-c", "sleep 0.8; exit 0"],
            "near-deadline helper",
            1,
            None,
        );
        assert_eq!(
            result
                .expect("completed process must win the timeout race")
                .0,
            0
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn test_slow_drain_does_not_overwrite_completed_status() {
        let Some(python) = ["python3", "python"]
            .into_iter()
            .find_map(|name| which::which(name).ok())
        else {
            return;
        };
        let python = python.to_string_lossy();
        let tmp = tempfile::tempdir().unwrap();
        let pid_path = tmp.path().join("slow-drain.pid");
        let pid_path_text = pid_path.to_string_lossy();
        let started = Instant::now();
        let result = run_process(
            "/bin/sh",
            [
                "-c",
                "\"$1\" -c 'import os,sys,time; os.setsid(); p=open(sys.argv[1],\"w\"); p.write(str(os.getpid())); p.close(); time.sleep(10)' \"$2\" & while [ ! -s \"$2\" ]; do :; done; exit 0",
                "sh",
                python.as_ref(),
                pid_path_text.as_ref(),
            ],
            "slow-drain helper",
            1,
            None,
        );
        assert_eq!(result.expect("leader exited before drain cleanup").0, 0);
        assert!(started.elapsed() < Duration::from_secs(4));

        if let Ok(Ok(pid)) = std::fs::read_to_string(&pid_path).map(|pid| pid.trim().parse::<i32>())
        {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }

    #[test]
    fn test_normal_exit_kills_descendant_holding_output_pipes() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_path = tmp.path().join("descendant.pid");
        let survived_path = tmp.path().join("descendant-survived");
        let pid_path = pid_path.to_string_lossy();
        let survived_path = survived_path.to_string_lossy();
        let started = Instant::now();
        let result = run_process(
            "/bin/sh",
            [
                "-c",
                "trap '' HUP; (sleep 2; printf survived > \"$2\") & echo $! > \"$1\"; exit 0",
                "sh",
                pid_path.as_ref(),
                survived_path.as_ref(),
            ],
            "descendant helper",
            5,
            None,
        );

        assert_eq!(result.expect("leader should exit normally").0, 0);
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(std::path::Path::new(pid_path.as_ref()).is_file());

        std::thread::sleep(Duration::from_millis(1500));
        assert!(
            !std::path::Path::new(survived_path.as_ref()).exists(),
            "the descendant survived process-group termination"
        );
    }

    #[test]
    fn test_normal_exit_kills_descendant_with_redirected_output() {
        let tmp = tempfile::tempdir().unwrap();
        let survived_path = tmp.path().join("redirected-descendant-survived");
        let survived_path = survived_path.to_string_lossy();
        let result = run_process(
            "/bin/sh",
            [
                "-c",
                "(sleep 1; printf survived > \"$1\") >/dev/null 2>&1 & exit 0",
                "sh",
                survived_path.as_ref(),
            ],
            "redirected descendant helper",
            5,
            None,
        );
        assert_eq!(result.unwrap().0, 0);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(!std::path::Path::new(survived_path.as_ref()).exists());
    }

    #[test]
    fn test_escaped_descendant_does_not_hold_drain_threads_forever() {
        if which::which("setsid").is_err() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let pid_path = tmp.path().join("escaped.pid");
        let pid_path_text = pid_path.to_string_lossy();
        let started = Instant::now();
        let result = run_process(
            "/bin/sh",
            [
                "-c",
                "setsid sh -c 'echo $$ > \"$1\"; sleep 10' sh \"$1\" & exit 0",
                "sh",
                pid_path_text.as_ref(),
            ],
            "escaped descendant helper",
            5,
            None,
        );
        assert_eq!(result.unwrap().0, 0);
        assert!(started.elapsed() < Duration::from_secs(4));

        if let Ok(Ok(pid)) = std::fs::read_to_string(&pid_path).map(|pid| pid.trim().parse::<i32>())
        {
            // SAFETY: the test owns this escaped helper pid.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }

    #[test]
    fn test_process_control_errors_have_typed_exit_status() {
        let timeout = anyhow::Error::new(ProcessControlError::Timeout {
            tool: "tool".to_string(),
            timeout_sec: 1,
            diagnostics: " interrupted by signal 2".to_string(),
        });
        let timeout_result = process_error_result(timeout, 1);
        assert_eq!(timeout_result.rtcode, 124);
        assert!(!timeout_result.interrupted);

        let interrupted = anyhow::Error::new(ProcessControlError::Interrupted {
            tool: "tool".to_string(),
            signal: libc::SIGTERM,
            diagnostics: " timed out after 1 seconds".to_string(),
        });
        let interrupted_result = process_error_result(interrupted, 1);
        assert_eq!(interrupted_result.rtcode, 128 + libc::SIGTERM);
        assert!(interrupted_result.interrupted);
    }
}
