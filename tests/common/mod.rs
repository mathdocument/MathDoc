use mathdoc::store::Database;

/// Own only this test's database and temporary compiler cache, including on panic.
pub struct TestDatabase {
    pub db: Database,
    _cache: tempfile::TempDir,
}
impl TestDatabase {
    pub async fn new(prefix: &str) -> Self {
        let cache = tempfile::tempdir().unwrap();
        let db = Database::from_env(
            format!("{prefix}{}", uuid::Uuid::new_v4().simple()),
            "main".into(),
        )
        .unwrap()
        .with_cache_root(cache.path().to_path_buf());
        let fixture = Self { db, _cache: cache };
        fixture.db.initialize().await.unwrap();
        fixture
    }
}
impl Drop for TestDatabase {
    fn drop(&mut self) {
        let name = self.db.database.clone();
        let result = std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let response = reqwest::Client::new()
                        .delete(format!(
                            "{}/api/db/admin/{name}",
                            std::env::var("MDC_TERMINUS_URL")
                                .unwrap_or("http://127.0.0.1:6363".into())
                        ))
                        .basic_auth(
                            std::env::var("MDC_TERMINUS_USER").unwrap_or("admin".into()),
                            Some(
                                std::env::var("MDC_TERMINUS_PASSWORD")
                                    .expect("test database password"),
                            ),
                        )
                        .send()
                        .await?;
                    anyhow::ensure!(
                        response.status().is_success() || response.status() == 404,
                        "test database cleanup: {}",
                        response.status()
                    );
                    Ok::<_, anyhow::Error>(())
                })
        })
        .join();
        if !matches!(result, Ok(Ok(()))) {
            eprintln!(
                "Could not remove test database {}: {result:?}",
                self.db.database
            );
            assert!(std::thread::panicking(), "test database cleanup failed");
        }
    }
}
