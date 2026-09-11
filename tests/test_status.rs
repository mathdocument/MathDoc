mod common;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mdc"));
    command
        .current_dir(root)
        .env("MDC_CACHE_DIR", root)
        // Obsolete URL settings must have no effect on project selection.
        .env("MDC_URL", "http://127.0.0.1:1")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command
}

async fn run(root: &Path, args: &[&str]) -> Value {
    let output = tokio::time::timeout(Duration::from_secs(40), command(root).args(args).output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn reject(root: &Path, args: &[&str], message: &str) {
    let output = command(root).args(args).output().await.unwrap();
    assert!(!output.status.success(), "{args:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn status(root: &Path) -> BTreeMap<String, Option<u16>> {
    let value = run(root, &["status"]).await;
    value
        .as_object()
        .unwrap()
        .iter()
        .map(|(project, state)| {
            assert_eq!(state.as_object().unwrap().len(), 1);
            (
                project.clone(),
                serde_json::from_value(state["port"].clone()).unwrap(),
            )
        })
        .collect()
}

struct Started {
    root: PathBuf,
    project: String,
    info: Value,
}
impl Started {
    async fn start(root: &Path, project: &str, port: Option<u16>) -> Self {
        let value = port.map(|p| p.to_string());
        let mut args = vec!["start", project];
        if let Some(value) = &value {
            args.extend(["--port", value]);
        }
        // The background child changes cwd; explicit relative config paths must still work.
        std::fs::write(root.join("client.toml"), "").unwrap();
        let output = command(root)
            .env("MDC_CONFIG", "client.toml")
            .args(&args)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let info = serde_json::from_slice(&output.stdout).unwrap();
        Self {
            root: root.into(),
            project: project.into(),
            info,
        }
    }
    fn port(&self) -> u16 {
        self.info["port"].as_u64().unwrap() as u16
    }
}
impl Drop for Started {
    fn drop(&mut self) {
        // Each guard owns one disposable branch; never stop production services.
        let _ = std::process::Command::new(env!("CARGO_BIN_EXE_mdc"))
            .args(["stop", &self.project])
            .env("MDC_CACHE_DIR", &self.root)
            .current_dir(&self.root)
            .output();
    }
}

#[tokio::test]
#[ignore = "requires local TerminusDB and MDC_TERMINUS_PASSWORD"]
async fn status_and_project_lifecycle_handle_conflicts_routing_and_crashes() {
    let first = common::TestDatabase::new("mdcstatus").await;
    let second = common::TestDatabase::new("mdcstatus").await;
    first.db.create_branch("agent").await.unwrap();
    let cache = tempfile::tempdir().unwrap();
    let root = cache.path();
    let main = format!("{}/main", first.db.database);
    let agent = format!("{}/agent", first.db.database);
    let unused = format!("{}/main", second.db.database);
    let before = status(root).await;
    assert_eq!(
        (before[&main], before[&agent], before[&unused]),
        (None, None, None)
    );
    assert_eq!(
        std::fs::read_dir(root).unwrap().count(),
        0,
        "status must not create compiler workspaces"
    );

    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    reject(
        root,
        &[
            "start",
            &main,
            "--port",
            &occupied.local_addr().unwrap().port().to_string(),
        ],
        "cannot bind port",
    )
    .await;
    reject(
        root,
        &["start", &format!("{}/absent", first.db.database)],
        "TerminusDB",
    )
    .await;
    let a = Started::start(root, &main, None).await;
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let explicit_port = reserved.local_addr().unwrap().port();
    drop(reserved);
    let b = Started::start(root, &agent, Some(explicit_port)).await;
    assert_eq!(b.port(), explicit_port);
    assert_ne!(a.port(), b.port());
    let running = status(root).await;
    let profiled = command(root)
        .args(["status", "--prof"])
        .output()
        .await
        .unwrap();
    assert!(profiled.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&profiled.stdout).unwrap(),
        run(root, &["status"]).await
    );
    assert!(String::from_utf8_lossy(&profiled.stderr).contains("service.request"));
    assert_eq!(
        (running[&main], running[&agent], running[&unused]),
        (Some(a.port()), Some(b.port()), None)
    );
    reject(root, &["start", &main], "already running").await;
    reject(root, &["graph", "check"], "--proj DATABASE/BRANCH").await;
    reject(
        root,
        &["status", "--url", "http://127.0.0.1:1"],
        "unexpected argument",
    )
    .await;
    reject(
        root,
        &["start", &main, "--proj", &agent],
        "unexpected argument",
    )
    .await;
    run(root, &["new", "--proj", &main, "-t", "Only in main"]).await;
    assert_eq!(
        run(root, &["graph", "check", "--proj", &main]).await["nodes"],
        1
    );
    assert_eq!(
        run(root, &["graph", "--proj", &agent, "check"]).await["nodes"],
        0
    );

    // Clients discover locally without contacting TerminusDB or reading its password.
    let without_password = command(root)
        .env_remove("MDC_TERMINUS_PASSWORD")
        .args(["graph", "check", "--proj", &main])
        .output()
        .await
        .unwrap();
    assert!(
        without_password.status.success(),
        "{}",
        String::from_utf8_lossy(&without_password.stderr)
    );
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = a.info["url"].as_str().unwrap();
    assert_eq!(
        http.post(format!("{url}/api/service/stop"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.post(format!("{url}/api/node/new"))
            .header("x-mdc-service", "old-service-token")
            .json(&serde_json::json!({"title":"Must not be created"}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );

    let stopped = command(root)
        .env_remove("MDC_TERMINUS_PASSWORD")
        .args(["stop", &main])
        .output()
        .await
        .unwrap();
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    let stopped = status(root).await;
    assert_eq!((stopped[&main], stopped[&agent]), (None, Some(b.port())));
    reject(root, &["stop", &main], "not running").await;
    reject(root, &["graph", "check", "--proj", &main], "mdc start").await;

    unsafe {
        assert_eq!(
            libc::kill(b.info["pid"].as_u64().unwrap() as i32, libc::SIGKILL),
            0
        );
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while status(root).await[&agent].is_some() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    let c = Started::start(root, &agent, Some(b.port())).await;
    assert_eq!(
        run(root, &["graph", "--proj", &agent, "check"]).await["nodes"],
        0
    );
    run(root, &["stop", &agent]).await;
    assert_eq!(status(root).await[&agent], None);
    drop(c);
}

#[tokio::test]
#[ignore = "requires local TerminusDB and MDC_TERMINUS_PASSWORD"]
async fn branch_deletion_requires_stopped_service_and_cleans_only_its_cache() {
    let fixture = common::TestDatabase::new("mdcbranch").await;
    let cache = tempfile::tempdir().unwrap();
    let root = cache.path();
    let main = format!("{}/main", fixture.db.database);
    let copy = format!("{}/copy", fixture.db.database);
    let _source = Started::start(root, &main, None).await;
    run(root, &["new", "--proj", &main, "-t", "Original"]).await;
    let original = run(root, &["export", "--proj", &main]).await;
    run(root, &["branch", "new", "copy", "--proj", &main]).await;
    let copied = mathdoc::store::Database::from_env(fixture.db.database.clone(), "copy".into())
        .unwrap()
        .with_cache_root(root.into());
    let copy_root = copied.cache_path().unwrap();
    let cache_file = copy_root.join("projects/toolchain/.lake/build/Lib.olean");
    std::fs::create_dir_all(cache_file.parent().unwrap()).unwrap();
    std::fs::write(&cache_file, "cached olean").unwrap();
    let outside = root.join("shared-library");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep.olean"), "shared").unwrap();
    std::os::unix::fs::symlink(&outside, copy_root.join("library-link")).unwrap();

    // Startup also holds the lease before it records a listening port.
    let starting = mathdoc::lean::LeanService::new(copy_root.clone()).unwrap();
    reject(
        root,
        &["branch", "del", "--proj", &copy],
        &format!("mdc stop {copy}"),
    )
    .await;
    assert!(cache_file.exists());
    drop(starting);

    let target = Started::start(root, &copy, None).await;
    assert_eq!(run(root, &["export", "--proj", &copy]).await, original);
    reject(
        root,
        &["branch", "del", "--proj", &copy],
        &format!("mdc stop {copy}"),
    )
    .await;
    assert!(cache_file.exists());
    assert!(copied.version().await.is_ok());
    run(root, &["stop", &copy]).await;
    drop(target);
    assert_eq!(
        run(root, &["branch", "del", "--proj", &copy]).await,
        serde_json::json!({"project":copy,"deleted":true})
    );
    assert!(copied.version().await.is_err());
    assert!(!status(root).await.contains_key(&copy));
    let remaining: Vec<_> = std::fs::read_dir(&copy_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(remaining, vec![std::ffi::OsString::from("service.lock")]);
    assert_eq!(
        std::fs::metadata(copy_root.join("service.lock"))
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        std::fs::read_to_string(outside.join("keep.olean")).unwrap(),
        "shared"
    );
    assert_eq!(run(root, &["export", "--proj", &main]).await, original);
    reject(root, &["branch", "del", "--proj", &copy], "TerminusDB").await;

    run(root, &["branch", "new", "copy", "--proj", &main]).await;
    let _recreated = Started::start(root, &copy, None).await;
    assert!(!cache_file.exists());
    assert_eq!(run(root, &["export", "--proj", &copy]).await, original);
}
