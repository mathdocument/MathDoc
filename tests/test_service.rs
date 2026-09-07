use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use mathdoc::{
    service::{self, Service},
    store::Database,
};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: Value,
    revision: Option<&str>,
) -> (u16, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("content-type", "application/json");
    if let Some(rev) = revision {
        request = request.header("if-match", format!("\"{rev}\""));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
#[ignore = "requires TerminusDB"]
async fn api_mutations_use_database_revisions_without_workspace_files() {
    let db = Database::from_env(
        format!("mdcapi{}", uuid::Uuid::new_v4().simple()),
        "main".into(),
    )
    .unwrap();
    db.initialize().await.unwrap();
    let app = service::router(Service::open(db.clone()).await.unwrap());
    let (status, a) = call(&app, "POST", "/api/node/new", json!({"title":"A"}), None).await;
    assert_eq!(status, 200, "{a}");
    let id = a["fnode"].as_str().unwrap();
    let rev = a["revision"].as_str().unwrap();
    let path = format!("/api/node/{id}/block/lean");
    assert_eq!(
        call(&app, "PUT", &path, json!({"content":"#check Nat"}), None)
            .await
            .0,
        428
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &path,
            json!({"content":"#check Nat"}),
            Some(rev)
        )
        .await
        .0,
        200
    );
    assert_eq!(
        call(&app, "PUT", &path, json!({"content":"stale"}), Some(rev))
            .await
            .0,
        412
    );
    assert_eq!(
        call(&app, "GET", "/api/resolve?ref=A.mdoc", Value::Null, None)
            .await
            .0,
        404
    );
    assert_eq!(
        call(&app, "GET", "/api/resolve?ref=A", Value::Null, None)
            .await
            .0,
        200
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/node/new",
            json!({"title":"B","file":"b.mdoc"}),
            None
        )
        .await
        .0,
        422
    );
    let reopened = Service::open(db).await.unwrap();
    assert_eq!(
        reopened.read().await.unwrap().nodes[id].source("lean"),
        Some("#check Nat")
    );
}
