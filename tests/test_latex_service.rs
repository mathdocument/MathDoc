mod common;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use mathdoc::{
    service::{self, Service},
    store::{Block, Node},
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
    let body = serde_json::from_slice(
        &to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    (status, body)
}

#[tokio::test]
#[ignore = "requires TerminusDB and Python with src/latex/requirements.txt"]
async fn latex_project_previews_follow_dependencies_and_preserve_configuration() {
    let fixture = common::TestDatabase::new("mdclatex").await;
    let service = Service::open(fixture.db.clone()).await.unwrap();
    let app = service::router(service.clone());
    let (_, original) = call(&app, "GET", "/api/project/latex", Value::Null, None).await;
    // Configuration stores editable source, even before it can be rendered.
    let unfinished = json!({"preamble_name":"macros.cls", "preamble":"\\iftrue",
        "bibliography_name":"refs.bib", "bibliography":"@book{unfinished"});
    let (status, original) = call(
        &app,
        "PUT",
        "/api/project/latex",
        unfinished.clone(),
        original["revision"].as_str(),
    )
    .await;
    assert_eq!(status, 200, "{original}");
    let (_, saved) = call(&app, "GET", "/api/project/latex", Value::Null, None).await;
    assert_eq!(saved["project"], unfinished);
    let project = json!({"preamble_name":"macros.cls", "preamble":"\\ProvidesClass{macros}\n\\newcommand{\\cA}{\\mathcal{A}}\n\\newtheorem{thm}{Theorem}",
        "bibliography_name":"refs.bib", "bibliography":"@article{paper,title={A paper},author={Author, A.},journal={Journal},year={2020}}"});
    let (status, configured) = call(
        &app,
        "PUT",
        "/api/project/latex",
        project.clone(),
        original["revision"].as_str(),
    )
    .await;
    assert_eq!(status, 200, "{configured}");
    assert_eq!(configured["project"], project);
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/project/latex",
            project.clone(),
            original["revision"].as_str()
        )
        .await
        .0,
        412
    );
    let mut a = Node::new("First result".into()).unwrap();
    a.blocks.push(Block {
        srctype: "latex".into(),
        content: "\\begin{thm}[Named result]\\label{thm:main}$\\cA$\\end{thm}".into(),
        ..Default::default()
    });
    let mut b = Node::new("Second result".into()).unwrap();
    b.depens.push(a.fnode.clone());
    b.blocks.push(Block {
        srctype: "latex".into(),
        content: "By \\nameref{thm:main}, see \\cite{paper}.".into(),
        ..Default::default()
    });
    let mut outsider = Node::new("Unrelated result".into()).unwrap();
    outsider.blocks.push(Block {
        srctype: "latex".into(),
        content: "\\section{Hidden}\\label{hidden}".into(),
        ..Default::default()
    });
    fixture
        .db
        .put(
            &[a.clone(), b.clone(), outsider.clone()],
            configured["revision"].as_str().unwrap(),
            "Create LaTeX nodes",
        )
        .await
        .unwrap();
    let preview_path = format!("/api/node/{}/latex/preview", b.fnode);
    let context_path = format!("/api/node/{}/latex/context", b.fnode);
    let draft = json!({"source": b.source("latex").unwrap()});
    let (status, preview) = call(&app, "POST", &preview_path, draft.clone(), None).await;
    assert_eq!(status, 200, "{preview}");
    assert_eq!(preview["diagnostics"], json!([]));
    assert!(preview["html"]
        .as_str()
        .unwrap()
        .contains(&format!("{}::Theorem 1</a>", &a.fnode[..8])));
    assert!(preview["html"].as_str().unwrap().contains("Aut20"));
    let (_, context) = call(&app, "GET", &context_path, Value::Null, None).await;
    assert!(preview["context_key"].is_string());
    assert_eq!(preview["context_key"], context["context_key"]);
    assert_eq!(context["imports"].as_array().unwrap().len(), 1);
    assert_eq!(
        context["references"][0]["key"],
        format!("{}::thm:main", a.fnode)
    );
    let (_, unchanged) = call(
        &app,
        "GET",
        &format!(
            "{context_path}?known={}",
            context["context_key"].as_str().unwrap()
        ),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(unchanged["unchanged"], true);
    let (_, catalog) = call(&app, "GET", "/api/project/latex/catalog", Value::Null, None).await;
    assert_eq!(catalog["citations"][0]["key"], "paper");
    assert!(catalog["commands"]
        .as_array()
        .unwrap()
        .contains(&json!("cA")));
    let (status, _) = call(
        &app,
        "POST",
        &preview_path,
        json!({"source":"\\externaldocument{outside}"}),
        None,
    )
    .await;
    assert_eq!(status, 422);
    assert_eq!(
        call(&app, "POST", &preview_path, draft.clone(), None)
            .await
            .0,
        200
    );
    let mut changed = a.clone();
    changed.blocks[0].content = format!(
        "\\begin{{thm}}Earlier result.\\end{{thm}}{}",
        changed.blocks[0].content
    );
    fixture
        .db
        .put(
            &[changed],
            &fixture.db.version().await.unwrap(),
            "Update theorem numbering",
        )
        .await
        .unwrap();
    let (_, refreshed) = call(&app, "POST", &preview_path, draft.clone(), None).await;
    assert_eq!(refreshed["project_key"], preview["project_key"]);
    assert_ne!(refreshed["context_key"], preview["context_key"]);
    assert!(refreshed["html"]
        .as_str()
        .unwrap()
        .contains(&format!("{}::Theorem 2</a>", &a.fnode[..8])));
    b.depens.clear();
    fixture
        .db
        .put(
            &[b.clone()],
            &fixture.db.version().await.unwrap(),
            "Remove dependency",
        )
        .await
        .unwrap();
    let (_, missing) = call(&app, "POST", &preview_path, draft, None).await;
    assert!(missing["diagnostics"][0]
        .as_str()
        .unwrap()
        .contains("undeclared"));
    assert!(!missing["html"]
        .as_str()
        .unwrap()
        .contains("data-latex-node"));
    let (_, exported) = call(&app, "GET", "/api/export", Value::Null, None).await;
    assert_eq!(exported["latex_project"], project);
    let restored = common::TestDatabase::new("mdclatexrestore").await;
    let restored_service = Service::open(restored.db.clone()).await.unwrap();
    let restored_app = service::router(restored_service.clone());
    assert_eq!(
        call(&restored_app, "POST", "/api/import", exported, None)
            .await
            .0,
        200
    );
    let loaded = restored.db.load().await.unwrap();
    assert_eq!(
        serde_json::to_value(&*loaded.latex_project).unwrap(),
        project
    );
    assert_eq!(loaded.nodes[&b.fnode].blocks, b.blocks);
    // Each branch has its own resident configuration. Ordinary graph commits
    // retain its key; changing configuration must replace it without a restart.
    let mut updated_project = project.clone();
    updated_project["bibliography"] = json!(
        "@article{paper,title={Updated paper},author={Author, A.},journal={Journal},year={2021}}"
    );
    let (status, _) = call(
        &restored_app,
        "PUT",
        "/api/project/latex",
        updated_project,
        Some(&loaded.version),
    )
    .await;
    assert_eq!(status, 200);
    let cited = json!({"source":"\\cite{paper}"});
    let (status, updated) = call(&restored_app, "POST", &preview_path, cited.clone(), None).await;
    assert_eq!(status, 200, "{updated}");
    assert!(updated["html"].as_str().unwrap().contains("Aut21"));
    let (status, original) = call(&app, "POST", &preview_path, cited.clone(), None).await;
    assert_eq!(status, 200, "{original}");
    assert!(original["html"].as_str().unwrap().contains("Aut20"));
    assert_ne!(original["project_key"], updated["project_key"]);
    // Restore the old configuration in an already-running worker.
    let revision = restored.db.version().await.unwrap();
    let (status, _) = call(
        &restored_app,
        "PUT",
        "/api/project/latex",
        project,
        Some(&revision),
    )
    .await;
    assert_eq!(status, 200);
    let (_, reverted) = call(&restored_app, "POST", &preview_path, cited, None).await;
    assert_eq!(reverted, original);
    service.latex.shutdown().await;
    restored_service.latex.shutdown().await;
}
