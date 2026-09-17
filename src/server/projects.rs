//! Browser project management shares the CLI's database and service operations.
use super::{ApiError, Server};
use crate::store::Database;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::{atomic::Ordering, Arc};

pub(super) async fn list(State(state): State<Arc<Server>>) -> Result<Json<Value>, ApiError> {
    let mut status = super::status().await?;
    let services: Vec<_> = state
        .branches
        .lock()
        .await
        .iter()
        .filter_map(|(project, branch)| {
            branch
                .as_ref()
                .map(|b| (project.clone(), b.service.clone()))
        })
        .collect();
    for (project, service) in services {
        if status["projects"][&project]["running"] != true {
            continue;
        }
        // Polling the directory must not reload sources or start stopped branches.
        let snapshot = service.snapshot.lock().await;
        let counts = crate::service::graph_report(&snapshot);
        status["projects"][&project]["nodes"] = counts["nodes"].clone();
        status["projects"][&project]["edges"] = counts["edges"].clone();
    }
    Ok(Json(status))
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    Init { name: String },
    Remove { database: String },
    Start { project: String },
    Stop { project: String },
    NewBranch { project: String, name: String },
    DeleteBranch { project: String },
}

pub(super) async fn manage(
    State(state): State<Arc<Server>>,
    headers: HeaderMap,
    Json(action): Json<Action>,
) -> Result<Json<Value>, ApiError> {
    // check_origin already validates Host and Origin. Require a browser Origin
    // here as well; local CLI service endpoints keep their private token guards.
    if !headers.contains_key("origin") {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "a same-origin browser request is required".into(),
        ));
    }
    // Finish mutations even if the page closes halfway through an operation.
    tokio::spawn(async move {
        match action {
            Action::Start { project } => Ok(state.start_branch(project).await?),
            Action::Stop { project } => state.stop_branch(project, None).await,
            Action::Remove { database } => state.remove_database(database).await,
            action => {
                let _operation = state.operations.read().await;
                if state.closing.load(Ordering::Acquire) {
                    return Err(super::stopped("server"));
                }
                match action {
                    Action::Init { name } => {
                        Database::from_env(name.clone(), "main".into())?
                            .initialize()
                            .await?;
                        Ok(json!({"project":format!("{name}/main"),"initialized":true}))
                    }
                    Action::NewBranch { project, name } => {
                        let (database, branch) = crate::config::project_parts(&project)?;
                        Database::from_env(database.into(), branch.into())?
                            .create_branch(&name)
                            .await?;
                        Ok(json!({"project":format!("{database}/{name}"),"created":true}))
                    }
                    Action::DeleteBranch { project } => {
                        Ok(crate::service::delete_branch(&project).await?)
                    }
                    Action::Start { .. } | Action::Stop { .. } | Action::Remove { .. } => {
                        unreachable!()
                    }
                }
            }
        }
    })
    .await
    .map_err(anyhow::Error::from)?
    .map(Json)
}
