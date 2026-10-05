//! Reclaim idle native Lake objects, independently of branch service lifetime.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

struct File {
    path: PathBuf,
    meta: fs::Metadata,
}
fn files(root: &Path, output: &mut Vec<File>) -> Result<()> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "unexpected symlink in shared pool: {}",
            entry.path().display()
        );
        if kind.is_dir() {
            files(&entry.path(), output)?;
        } else if kind.is_file() {
            match entry.metadata() {
                Ok(meta) => output.push(File {
                    path: entry.path(),
                    meta,
                }),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
    }
    Ok(())
}

fn directories(root: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let mut paths = vec![];
    for entry in entries {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with('.') && entry.file_type()?.is_dir() {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

fn automatic(root: &Path, budget: u64, since: SystemTime) -> Result<Option<Value>> {
    // Coordinate all entry servers sharing this cache_dir. Manual GC uses the
    // same per-pool leases and therefore cannot race this collector either.
    let _collector = match crate::file_lock::acquire(&root.join("gc.lock"), true) {
        Ok(lease) => lease,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut pools = vec![];
    for endpoint in directories(root)? {
        for database in directories(&endpoint)? {
            let shared = database.join(".shared");
            if fs::symlink_metadata(&shared).is_ok_and(|m| m.is_dir()) {
                pools.push(shared);
            }
        }
    }
    pools.sort();
    let due = since
        .elapsed()
        .map_or(true, |age| age >= Duration::from_secs(300))
        || pools.iter().any(|p| p.join("gc.pending").exists());
    if !due {
        return Ok(None);
    }
    // Avoid multiplication overflow for valid, unusually large configured budgets.
    let target = budget / 5 * 4 + budget % 5 * 4 / 5;
    let result = collect_pools(&pools, 0, Some(target), false, false, Some(budget))?;
    Ok(Some(result))
}

pub(crate) async fn run(
    root: PathBuf,
    budget: u64,
    mut stopped: tokio::sync::watch::Receiver<bool>,
) {
    if budget == 0 {
        return;
    }
    let mut interval = tokio::time::interval(Duration::from_secs(30));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_scan = SystemTime::UNIX_EPOCH;
    loop {
        tokio::select! {
            _ = stopped.changed() => return,
            _ = interval.tick() => (),
        }
        let root = root.clone();
        let started = SystemTime::now();
        // Keep scans, JSON parsing and deletion off Tokio's request workers.
        match tokio::task::spawn_blocking(move || automatic(&root, budget, last_scan)).await {
            Ok(Ok(Some(report))) => {
                last_scan = started;
                if report["selected"]["files"].as_u64().unwrap_or(0) > 0
                    || report["budget_met"] == false
                {
                    eprintln!("Lean cache GC: {report}");
                }
            }
            Ok(Ok(None)) => (),
            Ok(Err(error)) => eprintln!("Lean cache GC: {error:#}"),
            Err(error) => eprintln!("Lean cache GC worker: {error}"),
        }
    }
}
fn totals<'a>(files: impl Iterator<Item = &'a File>) -> Value {
    let (mut count, mut logical, mut allocated) = (0u64, 0u64, 0u64);
    let mut inodes = HashSet::new();
    for file in files {
        count += 1;
        logical += file.meta.len();
        if inodes.insert((file.meta.dev(), file.meta.ino())) {
            allocated += file.meta.blocks() * 512;
        }
    }
    json!({"files":count,"logical_size":size(logical),"allocated_size":size(allocated)})
}
fn size(bytes: u64) -> String {
    let mut value = bytes as f64;
    for unit in ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] {
        if value < 1024.0 || unit == "EiB" {
            return if unit == "B" {
                format!("{bytes} B")
            } else {
                format!("{value:.2} {unit}")
            };
        }
        value /= 1024.0;
    }
    unreachable!()
}
fn parts<'a>(root: &Path, file: &'a File) -> Vec<&'a std::ffi::OsStr> {
    file.path.strip_prefix(root).unwrap().iter().collect()
}
fn is_artifact(root: &Path, f: &File) -> bool {
    let p = parts(root, f);
    p.len() == 6 && p[0] == "v1" && p[3] == "lake" && p[4] == "artifacts"
}
fn is_mapping(root: &Path, f: &File) -> bool {
    let p = parts(root, f);
    p.len() >= 6
        && p[0] == "v1"
        && p[3] == "lake"
        && p[4] == "outputs"
        && f.path.extension().is_some_and(|e| e == "json")
}
fn is_certificate(root: &Path, f: &File) -> bool {
    let p = parts(root, f);
    p.len() == 6
        && p[0] == "v1"
        && p[3] == "certificates-v2"
        && f.path.extension().is_some_and(|e| e == "json")
}
pub fn stats(root: &Path) -> Result<Value> {
    let _lease = super::cache::lease(root, false)?;
    let _live = super::cache::lifetime(root, false)?;
    let mut all = vec![];
    files(root, &mut all)?;
    Ok(json!({"path":root,"total":totals(all.iter()),
        "artifacts":totals(all.iter().filter(|f| is_artifact(root, f))),
        "mappings":totals(all.iter().filter(|f| is_mapping(root, f))),
        "certificates":totals(all.iter().filter(|f| is_certificate(root, f)))}))
}

pub fn collect(
    root: &Path,
    days: u64,
    max_bytes: Option<u64>,
    certificates: bool,
    dry_run: bool,
) -> Result<Value> {
    let mut result = collect_pools(
        &[root.to_owned()],
        days,
        max_bytes,
        certificates,
        dry_run,
        None,
    )?;
    result["path"] = json!(root);
    Ok(result)
}

fn collect_pools(
    roots: &[PathBuf],
    days: u64,
    max_bytes: Option<u64>,
    certificates: bool,
    dry_run: bool,
    automatic_budget: Option<u64>,
) -> Result<Value> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(
            days.checked_mul(86400).context("age is too large")?,
        ))
        .context("age is too large")?;
    let mut leases = vec![];
    let mut all = vec![];
    let mut skipped = vec![];
    let mut protected = 0u64;
    for root in roots {
        let guards = (|| -> Result<_> {
            // Older binaries do not participate in the online activity protocol.
            let legacy = super::cache::lease(root, true)?;
            let live = super::cache::lifetime(root, certificates)
                .context("certificate GC requires all branches of this database to be stopped")?;
            let access = super::cache::try_access(root, true)
                .context("Lean cache has active checks/builds; retry when they finish")?;
            Ok((legacy, live, access))
        })();
        let mut entries = vec![];
        match guards {
            Ok(guards) => {
                leases.push((root, guards));
                files(root, &mut entries)?;
                all.extend(entries.into_iter().map(|f| (root, f)));
            }
            Err(error) if automatic_budget.is_some() => {
                // Only count busy pools; never parse a mapping Lake might still
                // be publishing. Concurrent growth makes this a soft budget.
                if let Ok(_live) = super::cache::lifetime(root, false) {
                    files(root, &mut entries)?;
                    protected += entries
                        .iter()
                        .filter(|f| is_artifact(root, f))
                        .map(|f| f.meta.len())
                        .sum::<u64>();
                }
                skipped.push(json!({"path":root,"reason":error.to_string()}));
            }
            Err(error) => return Err(error),
        }
    }
    let objects: HashMap<_, _> = all
        .iter()
        .filter(|(r, f)| is_artifact(r, f))
        .map(|(_, f)| (f.path.clone(), f))
        .collect();
    let before = protected + objects.values().map(|f| f.meta.len()).sum::<u64>();
    let needed = automatic_budget.is_none_or(|budget| before > budget);
    let mut references: HashMap<PathBuf, usize> = HashMap::new();
    let mut mappings = vec![];
    // Plan everything before deleting anything; corrupt/unknown schemas fail closed.
    for (root, file) in all.iter().filter(|(r, f)| needed && is_mapping(r, f)) {
        let value: Value = serde_json::from_slice(&fs::read(&file.path)?).with_context(|| {
            format!(
                "invalid Lake mapping {}; no files removed",
                file.path.display()
            )
        })?;
        ensure!(
            value["schemaVersion"] == "2026-02-25" && value.get("data").is_some(),
            "unsupported Lake mapping {}; no files removed",
            file.path.display()
        );
        let mut names = vec![];
        super::cache::object_names(&value["data"], &mut names);
        ensure!(
            names.iter().all(|n| super::cache::valid_object(n)),
            "invalid Lake object path; no files removed"
        );
        let p = parts(root, file);
        let lake = root.join(p[..4].iter().collect::<PathBuf>());
        let paths: HashSet<_> = names
            .into_iter()
            .map(|n| lake.join("artifacts").join(n))
            .collect();
        for path in &paths {
            *references.entry(path.clone()).or_default() += 1;
        }
        let access = lake.join("access").join(file.path.file_stem().unwrap());
        let touched = fs::metadata(&access).and_then(|m| m.modified()).ok();
        let last_use = touched.max(file.meta.modified().ok());
        mappings.push((file, paths, access, last_use));
    }
    mappings.sort_by_key(|(file, _, _, used)| (*used, file.path.clone()));
    let mut remaining = before;
    let mut remove = HashSet::new();
    if needed {
        for (path, file) in &objects {
            if !references.contains_key(path) && file.meta.modified()? <= cutoff {
                remaining -= file.meta.len();
                remove.insert(path.clone());
            }
        }
    }
    let mut retained_access = HashSet::new();
    let mut discarded_access = HashSet::new();
    for (file, paths, access, _) in mappings {
        if file.meta.modified()? > cutoff || max_bytes.is_some_and(|limit| remaining <= limit) {
            retained_access.insert(access);
            continue;
        }
        remove.insert(file.path.clone());
        discarded_access.insert(access);
        for path in paths {
            let count = references.get_mut(&path).unwrap();
            *count -= 1;
            if *count == 0 {
                if let Some(object) = objects.get(&path) {
                    if object.meta.modified()? <= cutoff && remove.insert(path) {
                        remaining -= object.meta.len();
                    }
                }
            }
        }
    }
    remove.extend(discarded_access.difference(&retained_access).cloned());
    if certificates {
        for (_, file) in all.iter().filter(|(r, f)| is_certificate(r, f)) {
            if file.meta.modified()? <= cutoff {
                remove.insert(file.path.clone());
            }
        }
    }
    let selected: Vec<_> = all
        .iter()
        .filter(|(_, f)| remove.contains(&f.path))
        .collect();
    // Named workspace outputs can keep hardlinks alive after pool eviction.
    let reclaimable: u64 = selected
        .iter()
        .filter(|(_, f)| f.meta.nlink() == 1)
        .map(|(_, f)| f.meta.blocks() * 512)
        .sum();
    let result = json!({"paths":roots,"dry_run":dry_run,"older_than_days":days,"max_size":max_bytes.map(size),
        "budget_size":automatic_budget.map(size),
        "selected":totals(selected.iter().map(|(_, f)| f)),"reclaimable_file_size":size(reclaimable),
        "artifact_size_before":size(before),"artifact_size_after":size(remaining),
        "budget_met":automatic_budget.or(max_bytes).is_none_or(|limit| remaining <= limit),
        "target_met":!needed || max_bytes.is_none_or(|limit| remaining <= limit),
        "protected_artifact_size":size(protected),"skipped_pools":skipped});
    if !dry_run {
        for (_, file) in selected.iter().filter(|(r, f)| !is_artifact(r, f)) {
            fs::remove_file(&file.path)?;
        }
        for (_, file) in selected.iter().filter(|(r, f)| is_artifact(r, f)) {
            fs::remove_file(&file.path)?;
        }
        if automatic_budget.is_some() {
            // Acknowledge only scanned pools while holding their activity locks.
            // Writers cannot lose a new request, and busy pools retry next tick.
            // Comparing mtimes to the process clock can miss writes on Linux.
            for (root, _) in &leases {
                match fs::remove_file(root.join("gc.pending")) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn background_gc_runs_on_startup_and_zero_disables_it() {
        let tmp = tempfile::tempdir().unwrap();
        let object = tmp
            .path()
            .join("endpoint/db/.shared/v1/test/project/lake/artifacts/old.olean");
        fs::create_dir_all(object.parent().unwrap()).unwrap();
        fs::write(&object, [0; 4096]).unwrap();
        let (stop, stopped) = tokio::sync::watch::channel(false);
        run(tmp.path().to_owned(), 0, stopped.clone()).await;
        assert!(object.exists());
        let task = tokio::spawn(run(tmp.path().to_owned(), 1, stopped));
        tokio::time::timeout(Duration::from_secs(5), async {
            while object.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        stop.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn automatic_gc_shares_one_budget_uses_global_lru_and_skips_active_pools() {
        let tmp = tempfile::tempdir().unwrap();
        let roots: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|db| tmp.path().join("endpoint").join(db).join(".shared"))
            .collect();
        let old = SystemTime::now() - Duration::from_secs(600);
        let mut objects = vec![];
        for (i, root) in roots.iter().enumerate() {
            let lake = root.join("v1/test/project/lake");
            fs::create_dir_all(lake.join("artifacts")).unwrap();
            fs::create_dir_all(lake.join("outputs/pkg")).unwrap();
            for j in 0..2 {
                let key = format!("{:016x}", i * 2 + j);
                let object = lake.join("artifacts").join(format!("{key}.olean"));
                fs::write(&object, [0; 4096]).unwrap();
                objects.push(object);
                let mapping = lake.join("outputs/pkg").join(format!("{key}.json"));
                fs::write(
                    &mapping,
                    json!({"schemaVersion":"2026-02-25","data":format!("{key}.olean")}).to_string(),
                )
                .unwrap();
                fs::File::open(mapping)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(old))
                    .unwrap();
                super::super::cache::touch(&lake, &key).unwrap();
                // Interleave ages across databases: entries 0 and 3 are oldest.
                let age = [4, 1, 2, 3][i * 2 + j];
                fs::File::open(lake.join("access").join(key))
                    .unwrap()
                    .set_times(
                        fs::FileTimes::new().set_modified(old + Duration::from_secs(10 - age)),
                    )
                    .unwrap();
            }
        }
        let now = SystemTime::now();
        assert!(automatic(tmp.path(), 16384, now).unwrap().is_none());
        let pending = roots[0].join("gc.pending");
        fs::write(&pending, []).unwrap();
        // Filesystem timestamps can lag the process clock (notably on Linux).
        fs::File::open(&pending)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        let below = automatic(tmp.path(), 16384, now).unwrap().unwrap();
        assert_eq!(
            below["selected"]["files"], 0,
            "do not trim to 80% until over budget"
        );
        assert!(!pending.exists());
        assert!(automatic(tmp.path(), 16384, now).unwrap().is_none());
        let report = automatic(tmp.path(), 12288, SystemTime::UNIX_EPOCH)
            .unwrap()
            .unwrap();
        assert_eq!(report["artifact_size_after"], "8.00 KiB");
        assert!(!objects[0].exists() && !objects[3].exists());
        assert!(objects[1].exists() && objects[2].exists());
        let active = super::super::cache::try_access(&roots[0], false).unwrap();
        fs::write(&pending, []).unwrap();
        let report = automatic(tmp.path(), 2048, SystemTime::UNIX_EPOCH)
            .unwrap()
            .unwrap();
        assert_eq!(report["budget_met"], false);
        assert_eq!(report["protected_artifact_size"], "4.00 KiB");
        assert_eq!(report["skipped_pools"].as_array().unwrap().len(), 1);
        assert!(objects[1].exists() && !objects[2].exists());
        assert!(pending.exists(), "a busy pool must retain its GC request");
        drop(active);
        let report = automatic(tmp.path(), 2048, now).unwrap().unwrap();
        assert_eq!(report["budget_met"], true);
        assert!(!objects[1].exists());
        assert!(!pending.exists());
    }

    #[test]
    fn gc_evicts_least_recently_used_mapping_without_touching_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let lake = root.join("v1/test/project/lake");
        fs::create_dir_all(lake.join("outputs/pkg")).unwrap();
        fs::create_dir_all(lake.join("artifacts")).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3600);
        for key in ["1111111111111111", "2222222222222222"] {
            let object = lake.join("artifacts").join(format!("{key}.olean"));
            fs::write(&object, [0; 4096]).unwrap();
            let mapping = lake.join("outputs/pkg").join(format!("{key}.json"));
            fs::write(
                &mapping,
                json!({"schemaVersion":"2026-02-25","data":format!("{key}.olean")}).to_string(),
            )
            .unwrap();
            fs::File::open(mapping)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let object = lake.join("artifacts/1111111111111111.olean");
        let timestamp = fs::metadata(&object).unwrap().modified().unwrap();
        super::super::cache::touch(&lake, "1111111111111111").unwrap();
        collect(root, 0, Some(4096), false, false).unwrap();
        assert!(object.is_file());
        assert!(!lake.join("artifacts/2222222222222222.olean").exists());
        assert_eq!(fs::metadata(object).unwrap().modified().unwrap(), timestamp);
    }

    #[test]
    fn gc_preserves_shared_references_obeys_leases_and_plans_before_deletion() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join(".shared");
        let lake = root.join("v1/test/project/lake");
        fs::create_dir_all(lake.join("artifacts")).unwrap();
        fs::create_dir_all(lake.join("outputs/test")).unwrap();
        for name in ["a.olean", "b.olean", "orphan.olean"] {
            fs::write(lake.join("artifacts").join(name), [0u8; 4096]).unwrap();
        }
        let mapping = |name: &str, data: Value| {
            let path = lake.join("outputs/test").join(name);
            fs::write(
                &path,
                serde_json::to_vec(&json!({"schemaVersion":"2026-02-25", "data":data})).unwrap(),
            )
            .unwrap();
            path
        };
        let a = mapping("a.json", json!(["a.olean", "b.olean"]));
        let b = mapping("b.json", json!("b.olean"));
        let old = SystemTime::now() - Duration::from_secs(86400 * 2);
        for path in [&a, &b] {
            fs::File::open(path)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(old))
                .unwrap();
        }
        let pool = super::super::cache::Pool::open(root.clone()).unwrap();
        assert!(
            collect(&root, 7, None, false, true).is_ok(),
            "idle running branches allow GC"
        );
        assert!(
            collect(&root, 0, None, true, true).is_err(),
            "certificate deletion still requires stopped branches"
        );
        let active = super::super::cache::try_access(&root, false).unwrap();
        assert!(collect(&root, 0, None, false, false).is_err());
        drop(active);
        let legacy = super::super::cache::lease(&root, false).unwrap();
        assert!(
            collect(&root, 0, None, false, false).is_err(),
            "old binaries have no online-GC protocol"
        );
        drop(legacy);
        let report = stats(&root).unwrap();
        assert_eq!(report["artifacts"]["files"], 3);
        assert_eq!(report["artifacts"]["logical_size"], "12.00 KiB");
        assert_eq!(size(0), "0 B");
        assert_eq!(size(5 * 1024 * 1024 * 1024), "5.00 GiB");
        drop(pool);
        let preview = collect(&root, 0, Some(4096), false, true).unwrap();
        assert_eq!(preview["max_size"], "4.00 KiB");
        assert_eq!(preview["artifact_size_before"], "12.00 KiB");
        assert_eq!(preview["artifact_size_after"], "4.00 KiB");
        assert_eq!(
            preview["reclaimable_file_size"],
            preview["selected"]["allocated_size"]
        );
        assert!(a.exists() && b.exists());
        let bad = lake.join("outputs/test/corrupt.json");
        fs::write(&bad, b"{").unwrap();
        assert!(collect(&root, 0, None, false, false).is_err());
        assert!(a.exists() && lake.join("artifacts/a.olean").exists());
        fs::remove_file(bad).unwrap();
        let result = collect(&root, 0, Some(4096), false, false).unwrap();
        assert_eq!(result["artifact_size_after"], "4.00 KiB");
        assert!(
            b.exists() && lake.join("artifacts/b.olean").exists(),
            "kept mapping pins the shared output"
        );
        assert!(!a.exists() && !lake.join("artifacts/a.olean").exists());
        assert!(collect(&root, 7, Some(0), false, false).unwrap()["budget_met"] == false);
        collect(&root, 0, None, false, false).unwrap();
        assert_eq!(stats(&root).unwrap()["artifacts"]["files"], 0);
    }
}
