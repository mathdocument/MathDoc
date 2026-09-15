use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, Semaphore},
};

const REQUIREMENTS: &str = include_str!("requirements.txt");
const MAX_FRAME: usize = 32 * 1024 * 1024;

/// One lazy worker per branch, with isolated document contexts and bounded caches.
/// A disconnected browser drops its result, not the worker's protocol frame.
pub struct LatexService {
    worker: Mutex<Option<Worker>>,
    slots: Arc<Semaphore>,
    stopping: tokio::sync::watch::Sender<bool>,
}

impl LatexService {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            worker: Mutex::new(None),
            slots: Arc::new(Semaphore::new(8)),
            stopping: tokio::sync::watch::channel(false).0,
        })
    }

    pub async fn request(self: &Arc<Self>, request: Value) -> Result<Value> {
        let service = self.clone();
        let permit = self
            .slots
            .clone()
            .try_acquire_owned()
            .context("LaTeX preview is busy; retry shortly")?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _permit = permit;
            let mut stopping = service.stopping.subscribe();
            if *stopping.borrow() {
                return;
            }
            let operation = async {
                let mut slot = service.worker.lock().await;
                if sender.is_closed() {
                    bail!("preview cancelled");
                }
                if slot.is_none() {
                    *slot = Some(Worker::spawn().await?);
                }
                let result = tokio::time::timeout(
                    Duration::from_secs(10),
                    slot.as_mut().unwrap().request(request),
                )
                .await;
                match result {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(error)) => {
                        *slot = None;
                        Err(error)
                    }
                    Err(_) => {
                        *slot = None;
                        bail!("LaTeX preview exceeded 10 seconds; shorten the block or check recursive macros")
                    }
                }
            };
            let result = tokio::select! {
                result = operation => result,
                _ = stopping.changed() => Err(anyhow::anyhow!("LaTeX service stopped")),
            };
            let _ = sender.send(result);
        });
        let mut response = receiver.await.context("LaTeX preview worker stopped")??;
        if let Some(error) = response.get("error").and_then(Value::as_str) {
            bail!("{error}");
        }
        Ok(response["result"].take())
    }

    pub async fn shutdown(&self) {
        self.stopping.send_replace(true);
        self.slots.close();
        *self.worker.lock().await = None;
    }
}

struct Worker {
    _child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Worker {
    async fn spawn() -> Result<Self> {
        let python = python().await?;
        let mut child = Command::new(python)
            .args(["-I", "-B", "-u", "-c", include_str!("renderer.py")])
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("start LaTeX renderer")?;
        Ok(Self {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            _child: child,
        })
    }
    async fn request(&mut self, request: Value) -> Result<Value> {
        let mut bytes = serde_json::to_vec(&request)?;
        ensure!(
            bytes.len() < MAX_FRAME,
            "LaTeX dependency context exceeds 32 MiB"
        );
        bytes.push(b'\n');
        self.input.write_all(&bytes).await?;
        self.input.flush().await?;
        let mut line = Vec::new();
        (&mut self.output)
            .take(MAX_FRAME as u64)
            .read_until(b'\n', &mut line)
            .await?;
        ensure!(
            line.last() == Some(&b'\n'),
            "LaTeX renderer stopped or exceeded its response limit"
        );
        serde_json::from_slice(&line).context("invalid LaTeX renderer response")
    }
}

async fn python() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("MDC_LATEX_PYTHON") {
        return Ok(path.into());
    }
    static INSTALL: OnceLock<Mutex<()>> = OnceLock::new();
    let _lock = INSTALL.get_or_init(|| Mutex::new(())).lock().await;
    let root = crate::config::Settings::load()?
        .cache_root()?
        .join("runtime");
    let runtime = root.join("plastex-3.1-pybtex-0.26.1");
    let python = runtime.join("bin/python");
    if runtime.join(".ready").is_file() {
        return Ok(python);
    }
    tokio::fs::create_dir_all(&root).await?;
    // Install outside project caches, once for all branches. The marker is written
    // only after pip succeeds; interrupted setup is safe to retry.
    let operation = async {
        let output = Command::new("python3")
            .args(["-m", "venv"])
            .arg(&runtime)
            .kill_on_drop(true)
            .output()
            .await
            .context("LaTeX preview requires Python 3 with venv support")?;
        ensure!(
            output.status.success(),
            "create LaTeX runtime: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = Command::new(&python)
            .args([
                "-m",
                "pip",
                "install",
                "--disable-pip-version-check",
                "--no-input",
                "--no-cache-dir",
            ])
            .args(REQUIREMENTS.lines())
            .kill_on_drop(true)
            .output()
            .await?;
        ensure!(
            output.status.success(),
            "install LaTeX runtime: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        tokio::fs::write(runtime.join(".ready"), b"1").await?;
        Ok::<_, anyhow::Error>(())
    };
    tokio::time::timeout(Duration::from_secs(180), operation)
        .await
        .context("LaTeX runtime installation timed out; retry with network access")??;
    Ok(python)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_requests_keep_the_queue_bounded() {
        let service = LatexService::new();
        let worker = service.worker.lock().await;
        let mut requests = Vec::new();
        for _ in 0..8 {
            let service = service.clone();
            requests.push(tokio::spawn(
                async move { service.request(Value::Null).await },
            ));
        }
        while service.slots.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
        for request in requests {
            request.abort();
            let _ = request.await;
        }
        assert!(service
            .request(Value::Null)
            .await
            .unwrap_err()
            .to_string()
            .contains("busy"));
        drop(worker);
        tokio::time::timeout(Duration::from_secs(1), async {
            while service.slots.available_permits() != 8 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
