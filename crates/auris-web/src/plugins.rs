//! API REST du système de plugins communautaires (#8 — spec docs/PLUGIN_SYSTEM.md).
//! Délègue à `auris_plugins::PluginHost` : liste, installation depuis un
//! répertoire local, activation, appel JSON-RPC générique, test de santé.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::Deserialize;

use auris_plugins::{PluginHost, PluginInfo};

use crate::{offload_io, ApiResult, AppState};

fn host(s: &AppState) -> Arc<PluginHost> {
    s.plugins.clone()
}

pub async fn get_plugins(State(s): State<AppState>) -> ApiResult<Json<Vec<PluginInfo>>> {
    let h = host(&s);
    offload_io(move || Ok(Json(h.list()))).await
}

#[derive(Debug, Deserialize)]
pub struct InstallBody {
    pub path: String,
}

pub async fn post_install(
    State(s): State<AppState>,
    Json(body): Json<InstallBody>,
) -> ApiResult<Json<PluginInfo>> {
    let h = host(&s);
    let path = body.path.clone();
    offload_io(move || {
        let info = h.install_from_dir(std::path::Path::new(&path))?;
        Ok(Json(info))
    })
    .await
}

pub async fn post_enable(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PluginInfo>> {
    let h = host(&s);
    offload_io(move || Ok(Json(h.set_enabled(&id, true)?))).await
}

pub async fn post_disable(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PluginInfo>> {
    let h = host(&s);
    offload_io(move || Ok(Json(h.set_enabled(&id, false)?))).await
}

pub async fn delete_plugin(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let h = host(&s);
    offload_io(move || {
        h.uninstall(&id)?;
        Ok(Json(serde_json::json!({ "uninstalled": id })))
    })
    .await
}

#[derive(Debug, Deserialize)]
pub struct CallBody {
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

pub async fn post_call(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CallBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let h = host(&s);
    offload_io(move || Ok(Json(h.call(&id, &body.method, body.params)?))).await
}

pub async fn post_test(
    State(s): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let h = host(&s);
    offload_io(move || Ok(Json(h.test(&id)?))).await
}
