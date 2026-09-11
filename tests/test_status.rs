mod common;
use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mdc"));
    command
        .current_dir(root)
        .env("MDC_CACHE_DIR", root)
        // Status must not depend on the client URL or an active HTTP service.
        .env("MDC_URL", "http://127.0.0.1:1")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command
}

async fn status(root: &Path) -> BTreeMap<String, Option<u16>> {
    let output = command(root).arg("status").output().await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let mut lines = text.lines();
    assert_eq!(
        lines.next().unwrap().split_whitespace().collect::<Vec<_>>(),
        ["Project", "Port"]
    );
    lines
        .map(|line| {
            let mut columns = line.split_whitespace();
            let name = columns.next().unwrap().into();
            let port = columns.next().map(|p| p.parse().unwrap());
            assert!(columns.next().is_none());
            (name, port)
        })
        .collect()
}

async fn serve(root: &Path, database: &str, branch: &str) -> (Child, u16) {
    let mut child = command(root)
        .args([
            "serve",
            database,
            "--branch",
            branch,
            "--bind",
            "127.0.0.1:0",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(15), stderr.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let port = line
        .trim()
        .strip_prefix("MathDoc → http://127.0.0.1:")
        .expect(&line)
        .parse()
        .unwrap();
    (child, port)
}

#[tokio::test]
#[ignore = "requires local TerminusDB and MDC_TERMINUS_PASSWORD"]
async fn status_lists_stopped_branches_and_tracks_multiple_services_without_stale_ports() {
    let first = common::TestDatabase::new("mdcstatus").await;
    let second = common::TestDatabase::new("mdcstatus").await;
    first.db.create_branch("agent").await.unwrap();
    let cache = tempfile::tempdir().unwrap();
    let main = format!("{}/main", first.db.database);
    let agent = format!("{}/agent", first.db.database);
    let unused = format!("{}/main", second.db.database);
    let before = status(cache.path()).await;
    assert_eq!(
        (before[&main], before[&agent], before[&unused]),
        (None, None, None)
    );
    assert_eq!(
        std::fs::read_dir(cache.path()).unwrap().count(),
        0,
        "status must not create compiler workspaces"
    );

    let (mut main_service, main_port) = serve(cache.path(), &first.db.database, "main").await;
    let (mut agent_service, agent_port) = serve(cache.path(), &first.db.database, "agent").await;
    assert_ne!(main_port, agent_port);
    let running = status(cache.path()).await;
    assert_eq!(
        (running[&main], running[&agent], running[&unused]),
        (Some(main_port), Some(agent_port), None)
    );

    unsafe {
        libc::kill(main_service.id().unwrap() as i32, libc::SIGTERM);
    }
    assert!(
        tokio::time::timeout(Duration::from_secs(10), main_service.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    let stopped = status(cache.path()).await;
    assert_eq!((stopped[&main], stopped[&agent]), (None, Some(agent_port)));

    agent_service.kill().await.unwrap(); // No cleanup handler: the stale address remains.
    let crashed = status(cache.path()).await;
    assert_eq!((crashed[&main], crashed[&agent]), (None, None));
    let rejected = command(cache.path())
        .args(["status", "--url", "http://127.0.0.1:1"])
        .output()
        .await
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("status reads TerminusDB directly"));
}
