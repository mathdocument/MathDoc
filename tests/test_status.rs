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

async fn input(root: &Path, args: &[&str], content: &str) -> std::process::Output {
    use tokio::io::AsyncWriteExt;
    let mut child = command(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(content.as_bytes())
        .await
        .unwrap();
    child.wait_with_output().await.unwrap()
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

async fn status(root: &Path) -> BTreeMap<String, bool> {
    let value = run(root, &["status"]).await;
    value["projects"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(project, state)| {
            assert_eq!(state.as_object().unwrap().len(), 2);
            (project.clone(), state["running"].as_bool().unwrap())
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
        let state = run(root, &["status"]).await;
        let port = port.or_else(|| {
            if state["server"]["running"] == true {
                None
            } else {
                Some(
                    std::net::TcpListener::bind("127.0.0.1:0")
                        .unwrap()
                        .local_addr()
                        .unwrap()
                        .port(),
                )
            }
        });
        let value = port.map(|p| p.to_string());
        let mut args = vec!["start", project];
        if let Some(value) = &value {
            args.extend(["--port", value]);
        }
        // The background child changes cwd; explicit relative config paths must still work.
        std::fs::write(root.join("client.toml"), "").unwrap();
        // Node.js uses a socket pair for piped stdin; it is not our bootstrap.
        let (input, child_input) = std::os::unix::net::UnixStream::pair().unwrap();
        let child_input: std::os::fd::OwnedFd = child_input.into();
        let output = command(root)
            .env("MDC_CONFIG", "client.toml")
            .stdin(Stdio::from(child_input))
            .args(&args)
            .output()
            .await
            .unwrap();
        drop(input);
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
        let state = std::process::Command::new(env!("CARGO_BIN_EXE_mdc"))
            .arg("status")
            .env("MDC_CACHE_DIR", &self.root)
            .output()
            .unwrap();
        if let Ok(state) = serde_json::from_slice::<Value>(&state.stdout) {
            if state["projects"]
                .as_object()
                .is_some_and(|p| p.values().all(|v| v["running"] == false))
            {
                let _ = std::process::Command::new(env!("CARGO_BIN_EXE_mdc"))
                    .arg("stop")
                    .env("MDC_CACHE_DIR", &self.root)
                    .output();
            }
        }
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
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    // A closed bootstrap peer must still be recognized; never recursively launch start.
    let (mut bootstrap, child_input) = std::os::unix::net::UnixStream::pair().unwrap();
    std::io::Write::write_all(&mut bootstrap, b"mdc-start").unwrap();
    std::io::Write::write_all(
        &mut bootstrap,
        &occupied.local_addr().unwrap().port().to_be_bytes(),
    )
    .unwrap();
    let child_input: std::os::fd::OwnedFd = child_input.into();
    let child = command(root)
        .args([
            "start",
            "mdcbootstrapmissing/main",
            "--port",
            &occupied.local_addr().unwrap().port().to_string(),
        ])
        .stdin(Stdio::from(child_input))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(bootstrap);
    let failed = child.wait_with_output().await.unwrap();
    assert!(!failed.status.success());
    let failed: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert!(failed["error"]
        .as_str()
        .unwrap()
        .contains("cannot bind port"));
    let main = format!("{}/main", first.db.database);
    let agent = format!("{}/agent", first.db.database);
    let unused = format!("{}/main", second.db.database);
    let before = status(root).await;
    assert_eq!(
        (before[&main], before[&agent], before[&unused]),
        (false, false, false)
    );
    assert_eq!(
        std::fs::read_dir(root).unwrap().count(),
        0,
        "status must not create compiler workspaces"
    );

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
    let process = std::process::Command::new("ps")
        .args(["-p", &a.info["pid"].to_string(), "-o", "command="])
        .output()
        .unwrap();
    let process = String::from_utf8(process.stdout).unwrap();
    assert!(process.contains(&format!("mdc start {main}")), "{process}");
    assert!(!process.contains("__run"));
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let explicit_port = reserved.local_addr().unwrap().port();
    drop(reserved);
    reject(
        root,
        &["start", &agent, "--port", &explicit_port.to_string()],
        "entry server is already using port",
    )
    .await;
    let b = Started::start(root, &agent, Some(a.port())).await;
    assert_eq!(a.port(), b.port());
    assert_ne!(a.info["url"], b.info["url"]);
    assert!(a.info["url"]
        .as_str()
        .unwrap()
        .ends_with(&format!("/p/{main}/")));
    let running = status(root).await;
    for option in ["--meas", "-m"] {
        let measured = command(root)
            .args(["status", option])
            .output()
            .await
            .unwrap();
        assert!(measured.status.success());
        let value: Value = serde_json::from_slice(&measured.stdout).unwrap();
        // Other tests may create or delete their own branches between status requests.
        for project in [&main, &agent, &unused] {
            assert_eq!(value["projects"][project]["running"], running[project]);
        }
        assert!(String::from_utf8_lossy(&measured.stderr).contains("service.request"));
    }
    assert_eq!(
        (running[&main], running[&agent], running[&unused]),
        (true, true, false)
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
        run(root, &["graph", "-p", &agent, "check"]).await["nodes"],
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
    let url = a.info["url"].as_str().unwrap().trim_end_matches('/');
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
    assert_eq!((stopped[&main], stopped[&agent]), (false, true));
    reject(root, &["stop", &main], "not running").await;
    reject(root, &["graph", "check", "--proj", &main], "mdc start").await;

    // Stopping/restarting the entry server must retain branch processes and URLs.
    let entry = format!("http://127.0.0.1:{}", b.port());
    let inventory: Value = http
        .get(format!("{entry}/api/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(inventory["projects"][&agent]["running"], true);
    assert_eq!(inventory["projects"][&agent]["url"], b.info["url"]);
    assert_eq!(
        http.get(format!("{entry}/api/status"))
            .header("origin", "https://attacker.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.get(format!("{entry}/api/status"))
            .header("host", "attacker.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        http.get(format!("{entry}/p/{main}/api/graph/check"))
            .send()
            .await
            .unwrap()
            .status(),
        503
    );
    run(root, &["stop"]).await;
    let stopped_entry = run(root, &["status"]).await;
    assert_eq!(
        stopped_entry["server"],
        serde_json::json!({"running":false,"port":null,"url":null})
    );
    assert_eq!(
        stopped_entry["projects"][&agent],
        serde_json::json!({"running":true,"url":null})
    );
    reject(
        root,
        &["graph", "check", "-p", &agent],
        "entry server is not running",
    )
    .await;
    run(root, &["start", "--port", &b.port().to_string()]).await;
    assert_eq!(
        run(root, &["graph", "check", "-p", &agent]).await["nodes"],
        0
    );

    unsafe {
        assert_eq!(
            libc::kill(b.info["pid"].as_u64().unwrap() as i32, libc::SIGKILL),
            0
        );
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while status(root).await[&agent] {
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
    assert!(!status(root).await[&agent]);
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

#[tokio::test]
#[ignore = "requires local TerminusDB and MDC_TERMINUS_PASSWORD"]
async fn cli_editing_matches_backend_transactions_and_revision_guards() {
    let fixture = common::TestDatabase::new("mdccli").await;
    let cache = tempfile::tempdir().unwrap();
    let root = cache.path();
    let project = format!("{}/main", fixture.db.database);
    let _service = Started::start(root, &project, None).await;
    let parent = run(root, &["new", "-p", &project, "-t", "Parent"]).await;
    let original_rev = parent["revision"].as_str().unwrap();
    let child = run(
        root,
        &[
            "new",
            "-p",
            &project,
            "-t",
            "Child",
            "--parent",
            "Parent",
            "--revision",
            original_rev,
        ],
    )
    .await;
    assert_eq!(child["title"], "Child");
    let linked = run(root, &["show", "Parent", "-p", &project]).await;
    assert_eq!(linked["depens"], serde_json::json!([child["fnode"]]));
    reject(
        root,
        &[
            "new",
            "-p",
            &project,
            "-t",
            "Stale",
            "--parent",
            "Parent",
            "--revision",
            original_rev,
        ],
        "HTTP 412",
    )
    .await;
    reject(
        root,
        &[
            "rename",
            "Parent",
            "Renamed",
            "-p",
            &project,
            "--revision",
            original_rev,
        ],
        "HTTP 412",
    )
    .await;
    let renamed = run(
        root,
        &[
            "rename",
            "Parent",
            "Renamed",
            "-p",
            &project,
            "--revision",
            linked["revision"].as_str().unwrap(),
        ],
    )
    .await;
    run(root, &["new", "-p", &project, "-t", "Other"]).await;
    reject(
        root,
        &[
            "dep",
            "add",
            "Renamed",
            "-t",
            "Other",
            "-p",
            &project,
            "--revision",
            original_rev,
        ],
        "HTTP 412",
    )
    .await;
    let added = run(
        root,
        &[
            "dep",
            "add",
            "Renamed",
            "-t",
            "Other",
            "-p",
            &project,
            "--revision",
            renamed["revision"].as_str().unwrap(),
        ],
    )
    .await;
    let candidates = run(root, &["dep", "candidates", "Renamed", "-p", &project]).await;
    assert_eq!(candidates["nodes"], serde_json::json!([]));
    reject(
        root,
        &[
            "dep", "rm", "Renamed", "-t", "Child", "Missing", "-p", &project,
        ],
        "node not found",
    )
    .await;
    assert_eq!(
        run(root, &["show", "Renamed", "-p", &project]).await["revision"],
        added["revision"]
    );
    reject(
        root,
        &[
            "dep",
            "rm",
            "Renamed",
            "-t",
            "Child",
            "Other",
            "-p",
            &project,
            "--revision",
            original_rev,
        ],
        "HTTP 412",
    )
    .await;
    let removed = run(
        root,
        &[
            "dep",
            "rm",
            "Renamed",
            "-t",
            "Child",
            "Other",
            "-p",
            &project,
            "--revision",
            added["revision"].as_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(removed["depens"], serde_json::json!([]));
    assert_eq!(
        run(
            root,
            &[
                "dep",
                "candidates",
                "Renamed",
                "Child",
                "-p",
                &project,
                "-n",
                "1"
            ]
        )
        .await["nodes"][0]["fnode"],
        child["fnode"]
    );
    let output = input(
        root,
        &["edit", "Child", "--type", "text", "-p", &project],
        "saved text",
    )
    .await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved: Value = serde_json::from_slice(&output.stdout).unwrap();
    reject(
        root,
        &[
            "edit",
            "Child",
            "--type",
            "text",
            "--delete",
            "-p",
            &project,
            "--revision",
            child["revision"].as_str().unwrap(),
        ],
        "HTTP 412",
    )
    .await;
    let deleted = run(
        root,
        &[
            "edit",
            "Child",
            "--type",
            "text",
            "--delete",
            "-p",
            &project,
            "--revision",
            saved["revision"].as_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(deleted["blocks"], serde_json::json!([]));
    reject(
        root,
        &[
            "lean",
            "goals",
            "Child",
            "--line",
            "0",
            "-p",
            &project,
            "--revision",
            saved["revision"].as_str().unwrap(),
        ],
        "HTTP 412",
    )
    .await;
    let config = run(root, &["project", "show", "-p", &project]).await;
    let mut changed = config["project"].clone();
    changed["lakefile"] = serde_json::json!(format!(
        "{}\n# updated\n",
        changed["lakefile"].as_str().unwrap()
    ));
    let args = [
        "project",
        "set",
        "-p",
        &project,
        "--revision",
        config["revision"].as_str().unwrap(),
    ];
    let output = input(root, &args, &changed.to_string()).await;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stale = input(root, &args, &config["project"].to_string()).await;
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("HTTP 412"));
    assert_eq!(
        run(root, &["project", "show", "-p", &project]).await["project"],
        changed
    );
    assert_eq!(
        run(root, &["graph", "check", "-p", &project]).await["nodes"],
        3
    );
}
