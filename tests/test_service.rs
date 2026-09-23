mod common;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use mathdoc::service::{self, Service};
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
#[ignore = "requires local TerminusDB and MDC_TERMINUS_PASSWORD"]
async fn delete_node_atomically_detaches_referrers_and_invalidates_affected_keys() {
    use mathdoc::store::Node;
    let fixture = common::TestDatabase::new("mdcdelete").await;
    let db = &fixture.db;
    let before = db.load().await.unwrap();
    let dependency = Node::new("Dependency".into()).unwrap();
    let mut target = Node::new("Target".into()).unwrap();
    target.depens.push(dependency.fnode.clone());
    let mut parent = Node::new("Parent".into()).unwrap();
    parent.depens = vec![target.fnode.clone(), dependency.fnode.clone()];
    let mut ancestor = Node::new("Ancestor".into()).unwrap();
    ancestor.depens = vec![parent.fnode.clone(), target.fnode.clone()];
    db.put(
        &[
            dependency.clone(),
            target.clone(),
            parent.clone(),
            ancestor.clone(),
        ],
        &before.version,
        "Deletion fixture",
    )
    .await
    .unwrap();
    let before = db.load().await.unwrap();
    db.create_branch("preserved").await.unwrap();
    let service = Service::open(db.clone()).await.unwrap();
    let app = service::router(service.clone());
    let path = format!("/api/node/{}", target.fnode);
    assert_eq!(call(&app, "DELETE", &path, Value::Null, None).await.0, 428);
    assert_eq!(
        call(&app, "DELETE", &path, Value::Null, Some("stale"))
            .await
            .0,
        412
    );
    assert_eq!(db.version().await.unwrap(), before.version);
    let commits = db.history().await.unwrap().as_array().unwrap().len();
    let (status, result) = call(&app, "DELETE", &path, Value::Null, Some(&target.revision())).await;
    assert_eq!(status, 200, "{result}");
    assert_eq!(result["removed_edges"], 3);
    assert_eq!(
        db.history().await.unwrap().as_array().unwrap().len(),
        commits + 1
    );
    let after = db.load().await.unwrap();
    let live = service.read().await.unwrap();
    assert!(!after.nodes.contains_key(&target.fnode));
    assert_eq!(
        after.nodes[&parent.fnode].depens,
        *std::slice::from_ref(&dependency.fnode)
    );
    assert_eq!(after.nodes[&ancestor.fnode].depens, [parent.fnode.clone()]);
    assert_eq!(after.nodes, live.nodes);
    assert_eq!(after.lean_keys, live.lean_keys);
    assert_eq!(after.depths, live.depths);
    assert_eq!(after.modules, live.modules);
    assert_eq!(
        after.lean_keys[&dependency.fnode],
        before.lean_keys[&dependency.fnode]
    );
    for id in [&parent.fnode, &ancestor.fnode] {
        assert_ne!(after.lean_keys[id], before.lean_keys[id]);
        assert_ne!(after.nodes[id].revision(), before.nodes[id].revision());
    }
    drop(live);
    assert!(db
        .delete_node(
            &dependency.fnode,
            std::slice::from_ref(&parent.fnode),
            &before.version
        )
        .await
        .is_err());
    assert_eq!(db.version().await.unwrap(), after.version);
    assert_eq!(
        call(&app, "DELETE", &path, Value::Null, Some(&target.revision()))
            .await
            .0,
        404
    );
    let mut branch = db.clone();
    branch.branch = "preserved".into();
    assert_eq!(branch.load().await.unwrap().nodes, before.nodes);
    for id in [&ancestor.fnode, &parent.fnode, &dependency.fnode] {
        let snapshot = db.load().await.unwrap();
        let (status, result) = call(
            &app,
            "DELETE",
            &format!("/api/node/{id}"),
            Value::Null,
            Some(&snapshot.nodes[id].revision()),
        )
        .await;
        assert_eq!(status, 200, "{result}");
    }
    assert!(service.read().await.unwrap().nodes.is_empty());
    assert!(db.load().await.unwrap().nodes.is_empty());
}

#[tokio::test]
#[ignore = "requires TerminusDB and native Lean v4.33.1"]
async fn api_mutations_use_database_revisions_without_workspace_files() {
    let fixture = common::TestDatabase::new("mdcapi").await;
    let db = &fixture.db;
    let app = service::router(Service::open(db.clone()).await.unwrap());
    let (status, a) = call(&app, "POST", "/api/node/new", json!({"title":"A"}), None).await;
    assert_eq!(status, 200, "{a}");
    for query in ["mode=unknown&depth=1", "mode=show&depth=-2"] {
        let (status, _) = call(
            &app,
            "GET",
            &format!("/api/node/{}/dep?{query}", a["fnode"].as_str().unwrap()),
            Value::Null,
            None,
        )
        .await;
        assert!((400..500).contains(&status));
    }

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
    assert_eq!(
        child_view["referrers"][0]["formalization"],
        json!({"lean":"unverified","rocq":"unverified"})
    );
    let before = db.version().await.unwrap();
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!("/api/node/{child}/block/python"),
            Value::Null,
            child_view["node"]["revision"].as_str()
        )
        .await
        .0,
        422
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/node/{child}/dep/rm"),
            json!({"dep_fnodes":[uuid::Uuid::new_v4().to_string()]}),
            child_view["node"]["revision"].as_str()
        )
        .await
        .0,
        404
    );
    assert_eq!(db.version().await.unwrap(), before);
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
    let reopened = Service::open(db.clone()).await.unwrap();
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
    let (_, bundle) = call(&app, "GET", "/api/export", Value::Null, None).await;
    let restore = common::TestDatabase::new("mdcrestore").await;
    let restored = service::router(Service::open(restore.db.clone()).await.unwrap());
    let empty_version = restore.db.version().await.unwrap();
    let mut missing_project = bundle.clone();
    missing_project.as_object_mut().unwrap().remove("project");
    let mut null_project = bundle.clone();
    null_project["project"] = Value::Null;
    let mut invalid_project = bundle.clone();
    invalid_project["project"]["toolchain"] = json!("unpinned");
    let mut dangling = bundle.clone();
    dangling["nodes"][0]["depens"] = json!([uuid::Uuid::new_v4().to_string()]);
    let mut duplicate = bundle.clone();
    let node = duplicate["nodes"][0].clone();
    duplicate["nodes"].as_array_mut().unwrap().push(node);
    for invalid in [
        missing_project,
        null_project,
        invalid_project,
        dangling,
        duplicate,
    ] {
        let (status, response) = call(&restored, "POST", "/api/import", invalid, None).await;
        assert_eq!(status, 422, "{response}");
        assert_eq!(restore.db.version().await.unwrap(), empty_version);
    }
    let (status, _) = call(&restored, "POST", "/api/import", bundle.clone(), None).await;
    assert_eq!(status, 200);
    let (_, roundtrip) = call(&restored, "GET", "/api/export", Value::Null, None).await;
    assert_eq!(bundle, roundtrip);
    let version = restore.db.version().await.unwrap();
    let disjoint = json!({
        "nodes":[mathdoc::store::Node::new("Disjoint".into()).unwrap()],
        "project":bundle["project"]
    });
    assert_eq!(
        call(&restored, "POST", "/api/import", disjoint, None)
            .await
            .0,
        409
    );
    assert_eq!(restore.db.version().await.unwrap(), version);
    assert_eq!(
        call(&restored, "POST", "/api/import", bundle, None).await.0,
        409
    );
}
