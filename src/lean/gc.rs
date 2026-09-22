//! Explicit, offline reclamation of native Lake objects; no work on request paths.
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
    if !root.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
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
            output.push(File {
                path: entry.path(),
                meta: entry.metadata()?,
            });
        }
    }
    Ok(())
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
    let _lease = super::cache::lease(root, true)?;
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(
            days.checked_mul(86400).context("age is too large")?,
        ))
        .context("age is too large")?;
    let mut all = vec![];
    files(root, &mut all)?;
    let objects: HashMap<_, _> = all
        .iter()
        .filter(|f| is_artifact(root, f))
        .map(|f| (f.path.clone(), f))
        .collect();
    let mut references: HashMap<PathBuf, usize> = HashMap::new();
    let mut mappings = vec![];
    // Parse the complete plan before deleting anything. Unknown/corrupt mappings
    // abort rather than guessing whether an object is unreferenced.
    for file in all.iter().filter(|f| is_mapping(root, f)) {
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
        let base = root
            .join(p[..4].iter().collect::<PathBuf>())
            .join("artifacts");
        let paths: HashSet<_> = names.into_iter().map(|n| base.join(n)).collect();
        for path in &paths {
            *references.entry(path.clone()).or_default() += 1;
        }
        mappings.push((file, paths));
    }
    mappings.sort_by_key(|(f, _)| (f.meta.modified().ok(), &f.path));
    let before: u64 = objects.values().map(|f| f.meta.len()).sum();
    let mut remaining = before;
    let mut remove = HashSet::new();
    for (path, file) in &objects {
        if !references.contains_key(path) && file.meta.modified()? <= cutoff {
            remaining -= file.meta.len();
            remove.insert(path.clone());
        }
    }
    for (file, paths) in mappings {
        if file.meta.modified()? > cutoff || max_bytes.is_some_and(|limit| remaining <= limit) {
            continue;
        }
        remove.insert(file.path.clone());
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
    if certificates {
        for file in all.iter().filter(|f| is_certificate(root, f)) {
            if file.meta.modified()? <= cutoff {
                remove.insert(file.path.clone());
            }
        }
    }
    let selected: Vec<_> = all.iter().filter(|f| remove.contains(&f.path)).collect();
    // Pool accounting does not promise freed disk: private producer outputs can
    // still hold hardlinks. Count only last links as reclaimable file blocks.
    let reclaimable: u64 = selected
        .iter()
        .filter(|f| f.meta.nlink() == 1)
        .map(|f| f.meta.blocks() * 512)
        .sum();
    let result = json!({"path":root,"dry_run":dry_run,"older_than_days":days,"max_bytes":max_bytes,
        "selected":totals(selected.iter().copied()),"reclaimable_file_bytes":reclaimable,
        "artifact_bytes_before":before,"artifact_bytes_after":remaining,
        "budget_met":max_bytes.is_none_or(|limit| remaining <= limit)});
    if !dry_run {
        // Mappings/certificates disappear before their objects. Interruption can
        // leak orphan objects, never leave a retained mapping pointing at deleted data.
        for file in selected.iter().filter(|f| !is_artifact(root, f)) {
            fs::remove_file(&file.path)?;
        }
        for file in selected.iter().filter(|f| is_artifact(root, f)) {
            fs::remove_file(&file.path)?;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(collect(&root, 0, None, false, false).is_err());
        let report = stats(&root).unwrap();
        assert_eq!(report["artifacts"]["files"], 3);
        assert_eq!(report["artifacts"]["logical_size"], "12.00 KiB");
        assert_eq!(size(0), "0 B");
        assert_eq!(size(5 * 1024 * 1024 * 1024), "5.00 GiB");
        drop(pool);
        let preview = collect(&root, 0, Some(4096), false, true).unwrap();
        assert_eq!(preview["artifact_bytes_after"], 4096);
        assert!(a.exists() && b.exists());
        let bad = lake.join("outputs/test/corrupt.json");
        fs::write(&bad, b"{").unwrap();
        assert!(collect(&root, 0, None, false, false).is_err());
        assert!(a.exists() && lake.join("artifacts/a.olean").exists());
        fs::remove_file(bad).unwrap();
        let result = collect(&root, 0, Some(4096), false, false).unwrap();
        assert_eq!(result["artifact_bytes_after"], 4096);
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
