//! Saved compiler evidence, shared by content key and revalidated against each graph.
use super::CheckResult;
use anyhow::{ensure, Context, Result};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock, RwLock, Weak,
    },
};

pub struct Certificates {
    pub root: PathBuf,
    pub results: RwLock<HashMap<String, CheckResult>>,
    generation: AtomicU64,
}

impl Certificates {
    pub async fn open(root: PathBuf, toolchain: &str) -> Result<Arc<Self>> {
        // Resolve without installing a toolchain merely to display graph status.
        let located = tokio::process::Command::new("elan")
            .args(["which", "lean"])
            .env("ELAN_TOOLCHAIN", toolchain)
            .kill_on_drop(true)
            .output()
            .await?;
        ensure!(
            located.status.success(),
            "pinned Lean toolchain is not installed"
        );
        let path = String::from_utf8(located.stdout)?;
        let version = tokio::process::Command::new(path.trim())
            .arg("--version")
            .kill_on_drop(true)
            .output()
            .await?;
        ensure!(version.status.success(), "cannot identify Lean compiler");
        let root = root
            .join("certificates-v2")
            .join(crate::store::digest(&version.stdout));
        tokio::fs::create_dir_all(&root).await?;
        let root = tokio::fs::canonicalize(root).await?;
        tokio::task::spawn_blocking(move || {
            static OPEN: OnceLock<Mutex<HashMap<PathBuf, Weak<Certificates>>>> = OnceLock::new();
            let mut open = OPEN.get_or_init(Mutex::default).lock().unwrap();
            if let Some(existing) = open.get(&root).and_then(Weak::upgrade) {
                return Ok(existing);
            }
            open.retain(|_, value| value.strong_count() > 0);
            let mut results = HashMap::new();
            for entry in std::fs::read_dir(&root)? {
                let path = entry?.path();
                let Some(key) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if path.extension().is_some_and(|s| s == "json") {
                    if let Some(result) = std::fs::read(&path)
                        .ok()
                        .and_then(|b| serde_json::from_slice::<CheckResult>(&b).ok())
                        .filter(|r| {
                            r.certified
                                && r.passed
                                && r.has_sorry.is_some()
                                && crate::store::digest(r.input_key.as_bytes()) == key
                        })
                    {
                        results.insert(result.input_key.clone(), result);
                    }
                }
            }
            let shared = Arc::new(Self {
                root: root.clone(),
                results: RwLock::new(results),
                generation: AtomicU64::new(1),
            });
            open.insert(root, Arc::downgrade(&shared));
            Ok(shared)
        })
        .await?
    }
    fn load(root: &std::path::Path, key: &str) -> Option<CheckResult> {
        let result: CheckResult = serde_json::from_slice(
            &std::fs::read(root.join(format!("{}.json", crate::store::digest(key.as_bytes()))))
                .ok()?,
        )
        .ok()?;
        (result.input_key == key && result.passed && result.certified && result.has_sorry.is_some())
            .then_some(result)
    }
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
    pub async fn refresh(&self, keys: Vec<String>) -> Result<()> {
        let root = self.root.clone();
        let results = tokio::task::spawn_blocking(move || {
            keys.iter()
                .filter_map(|k| Self::load(&root, k))
                .collect::<Vec<_>>()
        })
        .await?;
        let mut cached = self.results.write().unwrap();
        for result in results {
            cached.insert(result.input_key.clone(), result);
        }
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }
    pub async fn publish(&self, result: &CheckResult, workspace: &std::path::Path) -> Result<()> {
        if !result.certified || !result.passed || result.has_sorry.is_none() {
            return Ok(());
        }
        let text = crate::service::rewrite_uris(
            &serde_json::to_string(result)?,
            &super::file_uri(workspace)?,
            "file:///project",
        )?;
        let mut result: CheckResult = serde_json::from_str(&text)?;
        let root = self.root.clone();
        let key = crate::store::digest(result.input_key.as_bytes());
        let _lock = super::cache::lock(root.join(format!("{key}.lock"))).await?;
        // A later editor check must not erase an already published build capability.
        if let Some(old) = Self::load(&root, &result.input_key).filter(|r| r.built) {
            if !result.built {
                result.built = true;
                result.artifacts = old.artifacts;
            }
        }
        let bytes = serde_json::to_vec(&result)?;
        let lease = super::cache::lease(
            root.ancestors().nth(5).context("certificate pool path")?,
            false,
        )?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            use std::io::Write;
            let (_publication, _lease) = (_lock, lease);
            let mut temp = tempfile::NamedTempFile::new_in(&root)?;
            temp.write_all(&bytes)?;
            temp.persist(root.join(format!("{key}.json")))
                .context("publish Lean certificate")?;
            Ok(())
        })
        .await??;
        self.results
            .write()
            .unwrap()
            .insert(result.input_key.clone(), result);
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }
}
