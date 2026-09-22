//! Lake process lifetime and opaque, bounded Lean LSP transport.
use anyhow::{bail, Context, Result};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::mpsc,
};

const MAX_MESSAGE: usize = 32 * 1024 * 1024;

/// Kill the entire Lake/Lean process group on timeout, disconnect or shutdown.
pub struct Process {
    pub child: Child,
    pid: i32,
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.pid > 0 {
            unsafe {
                libc::kill(-self.pid, libc::SIGKILL);
            }
        }
    }
}
pub fn command(root: &Path, args: &[&str]) -> Command {
    let mut c = Command::new("lake");
    c.args(args)
        .current_dir(root)
        // Lake keys artifacts by compiler inputs, including transitive imports.
        // Every workspace links this to its database/project pool.
        .env("LAKE_ARTIFACT_CACHE", "true")
        .env("LAKE_RESTORE_ARTIFACTS", "false")
        .env("LAKE_CACHE_DIR", root.join(".lake/cache"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    c.process_group(0);
    // Compiler metaprograms run as the local author, without database credentials.
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("MDC_")
            || name.contains("TOKEN")
            || name.contains("PASSWORD")
            || name.contains("SECRET")
            || name.ends_with("API_KEY")
        {
            c.env_remove(key);
        }
    }
    c
}
pub fn spawn(root: &Path, args: &[&str]) -> Result<Process> {
    let child = command(root, args)
        .spawn()
        .context("start Lake; install the pinned Lean toolchain first")?;
    let pid = child.id().context("compiler process has no PID")? as i32;
    Ok(Process { child, pid })
}
/// One transport for CLI and browser clients. Dropping it asks Lean's watchdog
/// to terminate its workers, which use separate process groups of their own.
pub struct Server {
    input: mpsc::Sender<String>,
    output: mpsc::Receiver<Result<String>>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    pub fn start(root: &Path) -> Result<Self> {
        let mut process = spawn(root, &["serve"])?;
        let mut writer = process
            .child
            .stdin
            .take()
            .context("Lean stdin unavailable")?;
        let mut reader = BufReader::new(
            process
                .child
                .stdout
                .take()
                .context("Lean stdout unavailable")?,
        );
        let (input, mut incoming) = mpsc::channel::<String>(32);
        let (outgoing, output) = mpsc::channel(32);
        let errors = outgoing.clone();
        // Never cancel a partially read frame. After the client disconnects,
        // continue draining stdout until Lean finishes its shutdown handshake.
        tokio::spawn(async move {
            loop {
                let message = read_frame(&mut reader).await;
                let failed = message.is_err();
                let _ = outgoing.send(message).await;
                if failed {
                    break;
                }
            }
        });
        let task = tokio::spawn(async move {
            let result = {
                let forward = async {
                    while let Some(text) = incoming.recv().await {
                        write_frame(&mut writer, &text).await?;
                    }
                    Ok::<_, anyhow::Error>(())
                };
                tokio::pin!(forward);
                tokio::select! {
                    result = &mut forward => result,
                    _ = errors.closed() => {
                        // Finish queued frames before appending shutdown. Cancelling
                        // write_frame midway corrupts the stream seen by Lean.
                        tokio::time::timeout(Duration::from_secs(1), &mut forward)
                            .await.unwrap_or_else(|_| Err(anyhow::anyhow!("Lean input stalled during shutdown")))
                    },
                }
            };
            if let Err(error) = result {
                let _ = errors.try_send(Err(error));
                return; // Kill a broken/stalled transport without appending bytes.
            }
            // Lean processes these in order. Keep both pipes alive until it has
            // killed and reaped the file workers; dropping stdout first causes EPIPE.
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                write_frame(
                    &mut writer,
                    r#"{"jsonrpc":"2.0","id":"mdc-shutdown","method":"shutdown","params":null}"#,
                )
                .await?;
                write_frame(&mut writer, r#"{"jsonrpc":"2.0","method":"exit"}"#).await?;
                process.child.wait().await?;
                Ok::<_, anyhow::Error>(())
            })
            .await;
            // Process::drop is the fallback for a stuck or crashed server.
        });
        Ok(Self {
            input,
            output,
            task,
        })
    }
    pub async fn send(&self, text: String) -> Result<()> {
        self.input
            .send(text)
            .await
            .context("Lean server closed its input")
    }
    /// Stream clients must drive this sender independently of receive().
    pub(crate) fn sender(&self) -> mpsc::Sender<String> {
        self.input.clone()
    }
    pub async fn receive(&mut self) -> Result<String> {
        self.output
            .recv()
            .await
            .context("Lean server closed its output")?
    }
    pub async fn shutdown(self) {
        drop(self.input);
        drop(self.output);
        let _ = self.task.await;
    }
}

/// Keep editor RPC payloads opaque: Lean's tagged expressions can exceed JSON
/// tree deserializers' recursion limits even for ordinary mathematical terms.
async fn read_frame(reader: &mut (impl tokio::io::AsyncBufRead + Unpin)) -> Result<String> {
    let mut length = None;
    let mut header_bytes = 0;
    loop {
        let mut line = String::new();
        let n = (&mut *reader)
            .take((8193 - header_bytes) as u64)
            .read_line(&mut line)
            .await?;
        if n == 0 {
            bail!("Lean server exited");
        }
        header_bytes += n;
        if header_bytes > 8192 {
            bail!("Lean protocol header too large");
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            if key.eq_ignore_ascii_case("content-length") {
                length = Some(value.trim().parse::<usize>()?);
            }
        }
    }
    let length = length.context("Lean response omitted Content-Length")?;
    if length > MAX_MESSAGE {
        bail!("Lean protocol message too large");
    }
    let mut data = vec![0; length];
    reader.read_exact(&mut data).await?;
    Ok(String::from_utf8(data)?)
}
async fn write_frame(writer: &mut (impl tokio::io::AsyncWrite + Unpin), data: &str) -> Result<()> {
    if data.len() > MAX_MESSAGE {
        bail!("Lean request too large");
    }
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", data.len()).as_bytes())
        .await?;
    writer.write_all(data.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn editor_frames_preserve_deep_json_and_bound_protocol_input() {
        let text = format!("{}0{}", "[".repeat(2048), "]".repeat(2048));
        let mut wire = Vec::new();
        write_frame(&mut wire, &text).await.unwrap();
        let mut reader = BufReader::new(wire.as_slice());
        assert_eq!(read_frame(&mut reader).await.unwrap(), text);
        for invalid in [
            "x".repeat(8193),
            format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE + 1),
            "Content-Length: 10\r\n\r\n{}".into(),
        ] {
            assert!(read_frame(&mut BufReader::new(invalid.as_bytes()))
                .await
                .is_err());
        }
    }
}
