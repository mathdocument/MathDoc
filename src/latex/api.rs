use super::LatexProject;
use crate::service::{expected, ApiError, Service};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub(crate) fn routes() -> Router<Arc<Service>> {
    Router::new()
        .route("/project/latex", get(project).put(put_project))
        .route("/project/latex/catalog", get(catalog))
        .route("/node/:id/latex/context", get(context))
        .route("/node/:id/latex/preview", axum::routing::post(preview))
}

async fn project(State(service): State<Arc<Service>>) -> Result<Json<Value>, ApiError> {
    let snapshot = service.read().await?;
    Ok(Json(
        json!({"revision":snapshot.version, "project":snapshot.latex_project}),
    ))
}

async fn put_project(
    State(service): State<Arc<Service>>,
    headers: HeaderMap,
    Json(project): Json<LatexProject>,
) -> Result<Json<Value>, ApiError> {
    let expected_revision = expected(&headers)?.to_string();
    project.validate()?;
    service
        .latex
        .request(json!({"kind":"catalog","project":project}))
        .await?;
    let mut snapshot = service.read().await?;
    if expected_revision != snapshot.version {
        return Err(ApiError(
            StatusCode::PRECONDITION_FAILED,
            "project changed; reload and retry".into(),
        ));
    }
    let version = service
        .db
        .put_bundle(
            &[],
            None,
            Some(&project),
            &snapshot.version,
            "Update LaTeX project",
        )
        .await?;
    snapshot.version = version;
    snapshot.latex_project = project.into();
    Ok(Json(
        json!({"revision":snapshot.version, "project":snapshot.latex_project}),
    ))
}

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Known {
    known: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    source: String,
}

fn capture(
    snapshot: &crate::store::Snapshot,
    id: &str,
    source: Option<&str>,
    kind: &str,
) -> anyhow::Result<Value> {
    let node = snapshot.resolve(id)?;
    let target = json!({"fnode":node.fnode, "title":node.title, "source":source.unwrap_or_else(|| node.source("latex").unwrap_or(""))});
    let dependencies: Vec<_> = node.depens.iter().map(|id| {
        let node = &snapshot.nodes[id];
        json!({"fnode":node.fnode, "title":node.title, "source":node.source("latex").unwrap_or("")})
    }).collect();
    let key = crate::store::digest(&serde_json::to_vec(&(
        snapshot.latex_project.key(),
        &target,
        &dependencies,
    ))?);
    Ok(
        json!({"kind":kind, "project":snapshot.latex_project, "target":target, "dependencies":dependencies,
        "context_key":key, "project_key":snapshot.latex_project.key()}),
    )
}

async fn catalog(
    State(service): State<Arc<Service>>,
    axum::extract::Query(query): axum::extract::Query<Known>,
) -> Result<Json<Value>, ApiError> {
    let project = service.read().await?.latex_project.clone();
    let key = project.key();
    if query.known.as_deref() == Some(&key) {
        return Ok(Json(json!({"unchanged":true,"project_key":key})));
    }
    let mut result = service
        .latex
        .request(json!({"kind":"catalog","project":project}))
        .await?;
    result["project_key"] = json!(key);
    Ok(Json(result))
}

async fn context(
    State(service): State<Arc<Service>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<Known>,
) -> Result<Json<Value>, ApiError> {
    let input = {
        let snapshot = service.read().await?;
        capture(&snapshot, &id, None, "context")?
    };
    if query.known.as_deref() == input["context_key"].as_str() {
        return Ok(Json(
            json!({"unchanged":true,"context_key":input["context_key"],"project_key":input["project_key"]}),
        ));
    }
    let mut result = service.latex.request(input.clone()).await?;
    result["context_key"] = input["context_key"].clone();
    result["project_key"] = input["project_key"].clone();
    Ok(Json(result))
}

async fn preview(
    State(service): State<Arc<Service>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(draft): Json<Draft>,
) -> Result<Json<Value>, ApiError> {
    if draft.source.len() > 2 * 1024 * 1024 {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "LaTeX block exceeds 2 MiB".into(),
        ));
    }
    let input = {
        let snapshot = service.read().await?;
        capture(&snapshot, &id, Some(&draft.source), "preview")?
    };
    let mut result = service.latex.request(input.clone()).await?;
    let current = {
        let snapshot = service.read().await?;
        capture(&snapshot, &id, Some(&draft.source), "preview")?
    };
    if input["context_key"] != current["context_key"] {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "LaTeX dependencies or project changed; retry the preview".into(),
        ));
    }
    result["project_key"] = input["project_key"].clone();
    Ok(Json(result))
}
