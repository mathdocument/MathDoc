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
    Router::new().route("/project/latex", get(project).put(put_project))
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
    project.validate()?;
    let mut snapshot = service.read().await?;
    if expected(&headers)? != snapshot.version {
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
