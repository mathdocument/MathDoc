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
#[ignore = "requires TerminusDB and native Lean v4.33.1"]
async fn api_mutations_use_database_revisions_without_workspace_files() {
    let db = Database::from_env(
        format!("mdcapi{}", uuid::Uuid::new_v4().simple()),
        "main".into(),
    )
    .unwrap();
    eprintln!("test database: {}", db.database);
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
    let (_, view) = call(
        &app,
        "GET",
        &format!("/api/node/{id}/view"),
        Value::Null,
        None,
    )
    .await;
    let (status, parent) = call(
        &app,
        "POST",
        "/api/node/new",
        json!({"title":"linked","parent_fnode":id}),
        view["node"]["revision"].as_str(),
    )
    .await;
    assert_eq!(status, 200, "{parent}");
    assert_eq!(parent["fnode"], id);
    assert_eq!(parent["depens"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(
            &app,
            "PUT",
            &format!("/api/node/{id}/block/python"),
            json!({"content":"print(1)"}),
            parent["revision"].as_str()
        )
        .await
        .0,
        422
    );
    let (_, excluded) = call(
        &app,
        "GET",
        &format!("/api/node/{id}/dep/candidates?q=linked"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(excluded["empty"]["kind"], "excluded");
    assert_eq!(excluded["empty"]["existing_dependencies"], 1);
    let child = parent["depens"][0].as_str().unwrap();
    let (_, child_view) = call(
        &app,
        "GET",
        &format!("/api/node/{child}/view"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(child_view["referrers"][0]["fnode"], id);
    let (_, child_saved) = call(
        &app,
        "PUT",
        &format!("/api/node/{child}/block/lean"),
        json!({"content":"theorem child : True := by trivial\n"}),
        child_view["node"]["revision"].as_str(),
    )
    .await;
    let (status, checked) = call(
        &app,
        "POST",
        &format!("/api/node/{child}/lean/check"),
        json!({"build":true}),
        child_saved["revision"].as_str(),
    )
    .await;
    assert_eq!(status, 200, "{checked}");
    assert_eq!(checked["certified"], true);
    for _ in 0..12 {
        let (status, session) = call(
            &app,
            "POST",
            &format!("/api/node/{child}/lean/session"),
            Value::Null,
            child_saved["revision"].as_str(),
        )
        .await;
        assert_eq!(status, 200, "{session}");
        let path = format!("/api/lean/session/{}", session["id"].as_str().unwrap());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&path)
                    .header("host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 204);
        assert_eq!(call(&app, "GET", &path, Value::Null, None).await.0, 404);
    }
    drop(app);
    let reopened = Service::open(db).await.unwrap();
    assert_eq!(
        reopened.read().await.unwrap().nodes[id].source("lean"),
        Some("#check Nat")
    );
    let app = service::router(reopened);
    let (_, restored) = call(
        &app,
        "GET",
        &format!("/api/node/{child}/view"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(restored["node"]["formalization"]["lean"], "verified");
}
