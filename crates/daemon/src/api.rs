use axum::{
    Json, Router,
    body::Body,
    extract::{Path as UrlPath, State},
    http::{HeaderMap, header},
    middleware,
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use futures::StreamExt;
use nimbus_vault::{
    error::VaultError,
    object::{Object, ObjectId},
    vault::Vault,
};
use std::{
    collections::HashMap,
    path::{Component, Path},
};

use crate::{auth, error::ApiError, state::AppState};

pub const VAULT_PREFIX: &str = "/v";

pub fn router(state: AppState) -> Router {
    let mut vaults = Router::new()
        .route(VAULT_PREFIX, get(index))
        .route(&format!("{VAULT_PREFIX}/"), get(index));

    for suffix in ["", "/", "/{*id}"] {
        let route = |operation: &str| format!("{VAULT_PREFIX}/{{vault}}/{operation}{suffix}");
        vaults = vaults
            .route(&route("list"), get(list))
            .route(&route("get"), get(get_object))
            .route(&route("fetch"), get(fetch))
            .route(&route("put"), put(put_object))
            .route(&route("send"), put(send))
            .route(&route("delete"), delete(delete_object))
            .route(&route("rename"), post(rename_object));
    }

    vaults
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authenticate,
        ))
        .with_state(state)
        .route("/health", get(health))
}

async fn health() -> &'static str {
    "ok"
}

async fn index(State(state): State<AppState>) -> Json<Vec<String>> {
    Json(state.vault_names())
}

async fn list(
    State(state): State<AppState>,
    UrlPath(params): UrlPath<HashMap<String, String>>,
) -> Result<Json<Vec<Object>>, ApiError> {
    let (vault, id) = target(&state, &params)?;
    Ok(Json(vault.get_origin().list(&id).await?))
}

async fn get_object(
    State(state): State<AppState>,
    UrlPath(params): UrlPath<HashMap<String, String>>,
) -> Result<Json<Object>, ApiError> {
    let (vault, id) = target(&state, &params)?;
    Ok(Json(vault.get_origin().get(&id).await?))
}

async fn fetch(
    State(state): State<AppState>,
    UrlPath(params): UrlPath<HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiError> {
    let (vault, id) = target(&state, &params)?;
    let stream = vault.get_origin().fetch(&id).await?;
    Ok((
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from_stream(stream),
    ))
}

async fn put_object(
    State(state): State<AppState>,
    UrlPath(params): UrlPath<HashMap<String, String>>,
    Json(mut object): Json<Object>,
) -> Result<Json<Object>, ApiError> {
    state.ensure_writable()?;
    let (vault, destination) = target(&state, &params)?;
    // `target` vets the id in the URL; the name in the body is the other half of where the
    // object lands, and a `../x` there would put it outside the vault.
    nimbus_vault::object::validate_name(&object.get_name())
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(
        vault.get_origin().put(&mut object, &destination).await?,
    ))
}

async fn send(
    State(state): State<AppState>,
    UrlPath(params): UrlPath<HashMap<String, String>>,
    headers: HeaderMap,
    body: Body,
) -> Result<(), ApiError> {
    state.ensure_writable()?;
    let (vault, id) = target(&state, &params)?;
    let origin = vault.get_origin();

    // The stored object takes the time the client sent (its source's), or the current time
    // when there is none. Leaving the metadata `get` returned would re-stamp the file with
    // its own old time.
    let modified = modified_header(&headers)?;
    let mut object = origin.get(&id).await?;
    if let Some(mut meta) = object.get_meta() {
        meta.modified = modified;
        object.set_meta(meta);
    }
    let payload = body
        .into_data_stream()
        .map(|chunk| chunk.map_err(|e| VaultError::OriginError(e.to_string())))
        .boxed();

    origin.send(&object, payload).await?;
    Ok(())
}

/// The optional `x-nimbus-modified` header of a `send`: the source's modification time, which
/// the stored copy takes. `Ok(None)` when absent; a value that isn't RFC 3339 is a
/// [`ApiError::BadRequest`].
pub fn modified_header(
    headers: &HeaderMap,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApiError> {
    let Some(value) = headers.get(nimbus_vault::MODIFIED_HEADER) else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
        .map(|time| Some(time.with_timezone(&chrono::Utc)))
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "{} must be an RFC 3339 timestamp",
                nimbus_vault::MODIFIED_HEADER
            ))
        })
}

/// Body of `POST .../rename/{id}`: where to move the object, and under what name.
#[derive(serde::Deserialize)]
struct RenameRequest {
    /// The new parent's id; empty or `/` for the vault root.
    destination: String,
    /// The object's new name: a single path component.
    name: String,
}

/// Moves an object in one step through the origin's native rename (a single `rename(2)` for
/// an fs vault). Answers `501` when the origin can't, so a client can fall back to
/// copy-then-delete.
async fn rename_object(
    State(state): State<AppState>,
    UrlPath(params): UrlPath<HashMap<String, String>>,
    Json(request): Json<RenameRequest>,
) -> Result<Json<Object>, ApiError> {
    state.ensure_writable()?;
    let (vault, id) = target(&state, &params)?;
    let destination = match request.destination.as_str() {
        "" | "/" => vault.get_id(),
        raw => object_id(raw)?,
    };
    nimbus_vault::object::validate_name(&request.name)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(
        vault
            .get_origin()
            .rename(&id, &destination, &request.name)
            .await?,
    ))
}

async fn delete_object(
    State(state): State<AppState>,
    UrlPath(params): UrlPath<HashMap<String, String>>,
) -> Result<(), ApiError> {
    state.ensure_writable()?;
    let (vault, id) = target(&state, &params)?;
    vault.get_origin().delete(&id).await?;
    Ok(())
}

fn target<'a>(
    state: &'a AppState,
    params: &HashMap<String, String>,
) -> Result<(&'a Vault, ObjectId), ApiError> {
    let name = params
        .get("vault")
        .ok_or_else(|| ApiError::InvalidId("no vault in the request path".to_string()))?;
    let vault = state.vault(name)?;

    let id = match params.get("id").map(String::as_str) {
        None | Some("") | Some("/") => vault.get_id(),
        Some(raw) => object_id(raw)?,
    };
    Ok((vault, id))
}

pub fn object_id(raw: &str) -> Result<ObjectId, ApiError> {
    let relative = raw.trim_start_matches('/');

    if relative.contains('\0') {
        return Err(ApiError::InvalidId("id contains a null byte".to_string()));
    }
    if relative.split(['/', '\\']).any(|segment| segment == "..") {
        return Err(ApiError::InvalidId(format!("id '{raw}' escapes the vault")));
    }
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir => {
                return Err(ApiError::InvalidId(format!("id '{raw}' is absolute")));
            }
            Component::ParentDir => {
                return Err(ApiError::InvalidId(format!("id '{raw}' escapes the vault")));
            }
        }
    }

    Ok(match raw.starts_with('/') {
        true => ObjectId::from(format!("/{relative}")),
        false => ObjectId::from(relative),
    })
}

pub async fn serve(listener: tokio::net::TcpListener, router: Router) -> std::io::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}

#[cfg(test)]
#[path = "tests/api.rs"]
mod tests;
