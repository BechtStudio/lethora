//! The HTTP side. Every `/v1/projects` request needs `Authorization: Bearer
//! <token>` with a token from a welcome for that project, valid while that
//! WebSocket connection is open (401 without):
//!
//! - `GET /v1/ws`: the WebSocket (see [`conn`](crate::conn)); 429 when the
//!   address has too many open.
//! - `GET /v1/projects/{project}/{document}/{device}/snapshot`: a stream's
//!   snapshot, for any member; header `x-lethora-generation`.
//! - `PUT /v1/projects/{project}/{document}/{device}/snapshot?covers=<n>`:
//!   compaction by that device; answers `{"generation": g, "length": l}`.
//! - `GET|PUT /v1/projects/{project}/assets/{blob}`: GET for any member;
//!   PUT for devices that write `main`, 201 when stored, 200 if it existed.
//! - `PUT /v1/projects/{project}/invites/{invite}`: the wrapped project key,
//!   by the invite's creator while the invite is open.
//! - `GET /v1/projects/{project}/invites/{invite}`: the wrapped key, with an
//!   invitee token for that invite.
//! - `DELETE /v1/projects/{project}`: by the owner's device, the body signed
//!   as `lethora_protocol::wire::deletion_body` says, its time within five
//!   minutes of the server's clock; deletes the project's files (local
//!   copies stay) and closes its connections.
//! - `GET /healthz`: `ok`, then on the same line the server's version, the
//!   protocol versions it serves and its feature level, e.g. `ok
//!   (lethora-server 0.1.0, protocol 2-2, level 1)`.
//!
//! Refusals map to statuses: 400 bad request, 401 auth, 403 not a member or
//! forbidden, 404 unknown, 409 exists or stale, 410 invite closed, 413 too
//! large, 429 rate limited, 507 quota, 500 internal.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use lethora_protocol::check_id;
use lethora_protocol::wire::{Code, verify_deletion};
use serde::Deserialize;

use crate::conn::{self, WS_SLACK};
use crate::project::{DOCUMENTS, Project, Refusal, Who, is_blob_name};
use crate::{App, now};

/// How far a deletion's time may be from the server's clock, in seconds.
const DELETION_SKEW: u64 = 5 * 60;

/// The server's routes.
pub fn router(app: Arc<App>) -> Router {
    let body_limit = usize::try_from(app.config.limits.asset_bytes).unwrap_or(usize::MAX);
    Router::new()
        .route(
            "/healthz",
            get(|| async { format!("ok ({})", crate::version_line()) }),
        )
        .route("/v1/ws", get(websocket))
        .route(
            "/v1/projects/{project}/{document}/{device}/snapshot",
            get(get_snapshot).put(put_snapshot),
        )
        .route(
            "/v1/projects/{project}/assets/{blob}",
            get(get_asset).put(put_asset),
        )
        .route(
            "/v1/projects/{project}/invites/{invite}",
            get(get_invite).put(put_invite),
        )
        .route(
            "/v1/projects/{project}",
            axum::routing::delete(delete_project),
        )
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(app)
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let status = match self.code {
            Code::Version | Code::BadRequest => StatusCode::BAD_REQUEST,
            Code::Auth => StatusCode::UNAUTHORIZED,
            Code::NotAMember | Code::Forbidden => StatusCode::FORBIDDEN,
            Code::UnknownProject => StatusCode::NOT_FOUND,
            Code::Exists | Code::Stale => StatusCode::CONFLICT,
            Code::InviteClosed => StatusCode::GONE,
            Code::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Code::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Code::Quota => StatusCode::INSUFFICIENT_STORAGE,
            Code::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.message).into_response()
    }
}

type Result<T> = std::result::Result<T, Refusal>;

fn not_found(what: &str) -> Refusal {
    Refusal::new(Code::UnknownProject, format!("no such {what}"))
}

async fn websocket(
    State(app): State<Arc<App>>,
    ConnectInfo(socket): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Result<Response> {
    let forwarded = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
    let ip = app.client_ip(socket, forwarded);
    let slot = app.connection_slot(ip)?;
    let max = usize::try_from(app.config.limits.message_bytes)
        .unwrap_or(usize::MAX)
        .saturating_add(WS_SLACK);
    Ok(ws
        .max_message_size(max)
        .max_frame_size(max)
        .on_upgrade(move |socket| conn::serve(app, socket, ip, slot)))
}

/// Runs `f` off the async threads.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(Refusal::internal(e)))
}

/// The project and who the request's token belongs to; the token must be
/// for `project`.
async fn authorize(
    app: &Arc<App>,
    headers: &HeaderMap,
    project: &str,
) -> Result<(Arc<Project>, Who)> {
    let unauthorized = || Refusal::new(Code::Auth, "a valid bearer token is needed");
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|t| app.token(t.trim()))
        .filter(|t| t.project == project)
        .ok_or_else(unauthorized)?;
    let (app, id) = (app.clone(), project.to_owned());
    let project = blocking(move || app.project(&id)?.ok_or_else(|| not_found("project"))).await?;
    if project.is_deleted() {
        return Err(not_found("project"));
    }
    Ok((project, token.who))
}

/// The device of a device token.
fn device(who: Who) -> Result<String> {
    match who {
        Who::Device(device) => Ok(device),
        Who::Invitee(_) => Err(Refusal::new(Code::Forbidden, "only for devices")),
    }
}

fn check_stream(document: &str, device: &str) -> Result<()> {
    if !DOCUMENTS.contains(&document) || check_id(device, "device").is_err() {
        return Err(not_found("stream"));
    }
    Ok(())
}

async fn get_snapshot(
    State(app): State<Arc<App>>,
    Path((project, document, of)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Result<Response> {
    check_stream(&document, &of)?;
    let (project, who) = authorize(&app, &headers, &project).await?;
    let device = device(who)?;
    let snapshot = blocking(move || project.snapshot(&device, &document, &of)).await?;
    let (generation, bytes) = snapshot.ok_or_else(|| not_found("snapshot"))?;
    Ok(([("x-lethora-generation", generation.to_string())], bytes).into_response())
}

#[derive(Deserialize)]
struct Covers {
    covers: u64,
}

async fn put_snapshot(
    State(app): State<Arc<App>>,
    Path((project, document, of)): Path<(String, String, String)>,
    Query(Covers { covers }): Query<Covers>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    check_stream(&document, &of)?;
    let (project, who) = authorize(&app, &headers, &project).await?;
    let device = device(who)?;
    if device != of {
        return Err(Refusal::new(
            Code::Forbidden,
            "a device compacts only its own streams",
        ));
    }
    let (generation, length) =
        blocking(move || project.compact(&device, &document, covers, &body)).await?;
    let json = serde_json::json!({ "generation": generation, "length": length });
    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        json.to_string(),
    )
        .into_response())
}

async fn get_asset(
    State(app): State<Arc<App>>,
    Path((project, blob)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response> {
    if !is_blob_name(&blob) {
        return Err(not_found("asset"));
    }
    let (project, who) = authorize(&app, &headers, &project).await?;
    let device = device(who)?;
    let bytes = blocking(move || project.asset(&device, &blob)).await?;
    Ok(bytes.ok_or_else(|| not_found("asset"))?.into_response())
}

async fn put_asset(
    State(app): State<Arc<App>>,
    Path((project, blob)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    if !is_blob_name(&blob) {
        return Err(Refusal::new(
            Code::BadRequest,
            "a blob name is 64 lowercase hex",
        ));
    }
    let (project, who) = authorize(&app, &headers, &project).await?;
    let device = device(who)?;
    let created = blocking(move || project.put_asset(&device, &blob, &body)).await?;
    Ok(if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    }
    .into_response())
}

async fn put_invite(
    State(app): State<Arc<App>>,
    Path((project, invite)): Path<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    if check_id(&invite, "invite").is_err() {
        return Err(not_found("invite"));
    }
    let (project, who) = authorize(&app, &headers, &project).await?;
    let device = device(who)?;
    blocking(move || project.put_invite_key(&device, &invite, &body, now())).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn get_invite(
    State(app): State<Arc<App>>,
    Path((project, invite)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response> {
    let (project, who) = authorize(&app, &headers, &project).await?;
    if who != Who::Invitee(invite.clone()) {
        return Err(Refusal::new(
            Code::Forbidden,
            "only for the holder of this invite",
        ));
    }
    let key = blocking(move || project.invite_key(&invite, now())).await?;
    Ok(key.ok_or_else(|| not_found("invite key"))?.into_response())
}

async fn delete_project(
    State(app): State<Arc<App>>,
    Path(project): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response> {
    let (project, who) = authorize(&app, &headers, &project).await?;
    let device = device(who)?;
    let app = app.clone();
    blocking(move || {
        if project.member(&device)? != lethora_protocol::members::Role::Owner {
            return Err(Refusal::new(
                Code::Forbidden,
                "only the owner deletes the project",
            ));
        }
        let public = project
            .with_members(|m| m.device(&device).map(|d| d.public.sign))
            .ok_or_else(|| not_found("device"))?;
        let time = verify_deletion(&public, &project.id, &body)
            .map_err(|_| Refusal::new(Code::Auth, "the deletion's signature doesn't match"))?;
        if time.abs_diff(now()) > DELETION_SKEW {
            return Err(Refusal::new(
                Code::Auth,
                "the deletion's time is too far from the server's clock",
            ));
        }
        app.delete_project(&project)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
