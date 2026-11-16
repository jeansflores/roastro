//! The HTTP API and the live WebSocket ("Contract between the backend and the
//! web UI"). Every handler asks the engine through its queue; nothing here
//! touches the database or the PLC. There is no route that writes to the PLC.

use std::fmt::Display;
use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequest, Path, Query, Request as HttpRequest, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use store::RoastId;
use tokio::sync::{broadcast, mpsc, oneshot};
use uuid::Uuid;

use crate::engine::{Edit, Reply, Request};
use crate::wire::{
    CoffeeIn, CoffeeOut, CoffeePatch, Columns, EventIn, NextRoastIn, NextRoastOut, NoteIn, NoteOut,
    NotePatch, RoastDoc, RoastPage, RoastPatchIn,
};

/// An API error: an HTTP status and `{ "error": { "code", "message" } }`.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    /// The stable error code, e.g. `roast_not_found`.
    #[cfg(test)]
    #[must_use]
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// 404.
    pub fn not_found(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
            message: message.into(),
        }
    }

    /// 409: the request does not fit the roast's current state.
    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            message: message.into(),
        }
    }

    /// 400: the request itself is malformed.
    pub fn invalid(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    /// 422: well formed, but it breaks a roast rule.
    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code,
            message: message.into(),
        }
    }

    /// 403.
    fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code,
            message: message.into(),
        }
    }

    /// 500. The detail goes to the log, not to the client.
    pub fn internal(err: impl Display) -> Self {
        tracing::error!(%err, "internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: "something went wrong; see the log".into(),
        }
    }

    fn unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "shutting_down",
            message: "Roastro is stopping".into(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: &'a str,
    message: &'a str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code,
                message: &self.message,
            },
        };
        (self.status, Json(body)).into_response()
    }
}

/// What every handler needs.
#[derive(Clone)]
pub struct AppState {
    engine: mpsc::Sender<Request>,
    /// Origins allowed to open the live stream and change things.
    origins: Arc<Vec<String>>,
}

impl AppState {
    /// `origins` are full origins such as `http://127.0.0.1:8000`.
    #[must_use]
    pub fn new(engine: mpsc::Sender<Request>, origins: Vec<String>) -> Self {
        Self {
            engine,
            origins: Arc::new(origins),
        }
    }

    /// Sends a request to the engine and waits for its reply.
    async fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Request) -> Result<T, ApiError> {
        let (tx, rx) = oneshot::channel();
        self.engine
            .send(make(tx))
            .await
            .map_err(|_| ApiError::unavailable())?;
        rx.await.map_err(|_| ApiError::unavailable())?
    }
}

/// The API routes.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/live", get(live))
        .route("/api/charge", post(charge))
        .route("/api/drop", post(drop_now))
        .route("/api/roasts", get(roasts))
        .route("/api/roasts/{id}", get(roast).patch(patch_roast))
        .route("/api/roasts/{id}/samples", get(samples))
        .route(
            "/api/roasts/{id}/events/{kind}",
            put(set_event).delete(delete_event),
        )
        .route("/api/roasts/{id}/notes", post(add_note))
        .route(
            "/api/roasts/{id}/notes/{note}",
            patch(patch_note).delete(delete_note),
        )
        .route("/api/roasts/{id}/discard", post(discard))
        .route("/api/roasts/{id}/restore", post(restore))
        .route("/api/roasts/{id}/close", post(close))
        .route("/api/next-roast", get(next_roast).put(put_next_roast))
        .route("/api/coffees", get(coffees).post(add_coffee))
        .route("/api/coffees/{id}", patch(patch_coffee))
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), check_origin))
        .with_state(state)
}

/// Browsers do not apply same-origin rules to WebSocket handshakes or to
/// simple POSTs, so any page open on the roasting PC could otherwise talk to
/// Roastro. Requests that change things, and the live stream, must come from
/// an allowed origin; tools such as curl send none and are let through.
async fn check_origin(State(state): State<AppState>, req: HttpRequest, next: Next) -> Response {
    let guarded = req.method() != Method::GET || req.uri().path() == "/api/live";
    if guarded && !origin_allowed(req.headers(), &state.origins) {
        return ApiError::forbidden(
            "forbidden_origin",
            "requests from this page are not allowed",
        )
        .into_response();
    }
    next.run(req).await
}

fn origin_allowed(headers: &HeaderMap, allowed: &[String]) -> bool {
    match headers.get(header::ORIGIN) {
        None => true,
        Some(origin) => origin
            .to_str()
            .is_ok_and(|o| allowed.iter().any(|a| a == o)),
    }
}

async fn charge(State(app): State<AppState>) -> Result<Json<RoastDoc>, ApiError> {
    app.ask(Request::Charge).await.map(Json)
}

async fn drop_now(State(app): State<AppState>) -> Result<Json<RoastDoc>, ApiError> {
    app.ask(Request::Drop).await.map(Json)
}

/// A JSON request body whose rejections use the contract's error format.
struct Body<T>(T);

impl<S, T> FromRequest<S> for Body<T>
where
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: HttpRequest, state: &S) -> Result<Self, ApiError> {
        Json::<T>::from_request(req, state)
            .await
            .map(|Json(v)| Body(v))
            .map_err(|e| ApiError::invalid("invalid_body", e.body_text()))
    }
}

fn parse_id(s: &str, what: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(s)
        .map_err(|_| ApiError::invalid("invalid_id", format!("{s:?} is not a {what} id")))
}

fn roast_id(s: &str) -> Result<RoastId, ApiError> {
    parse_id(s, "roast").map(RoastId)
}

async fn not_found() -> ApiError {
    ApiError::not_found("no_such_route", "no such API route")
}

async fn roast(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RoastDoc>, ApiError> {
    let id = roast_id(&id)?;
    app.ask(|r| Request::Roast(id, r)).await.map(Json)
}

async fn samples(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Columns>, ApiError> {
    let id = roast_id(&id)?;
    app.ask(|r| Request::Samples(id, r)).await.map(Json)
}

#[derive(Deserialize)]
struct ListQuery {
    before: Option<String>,
    include: Option<String>,
}

async fn roasts(
    State(app): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<RoastPage>, ApiError> {
    let before = q
        .before
        .as_deref()
        .map(|b| parse_id(b, "roast"))
        .transpose()?;
    let discarded = q.include.as_deref() == Some("discarded");
    app.ask(|r| Request::Edit(Edit::Roasts(before, discarded, r)))
        .await
        .map(Json)
}

async fn patch_roast(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Body(body): Body<RoastPatchIn>,
) -> Result<Json<RoastDoc>, ApiError> {
    let id = roast_id(&id)?;
    app.ask(|r| Request::Edit(Edit::PatchRoast(id, body, r)))
        .await
        .map(Json)
}

async fn set_event(
    State(app): State<AppState>,
    Path((id, kind)): Path<(String, String)>,
    Body(body): Body<EventIn>,
) -> Result<Json<RoastDoc>, ApiError> {
    let id = roast_id(&id)?;
    app.ask(|r| Request::Edit(Edit::SetEvent(id, kind, body, r)))
        .await
        .map(Json)
}

async fn delete_event(
    State(app): State<AppState>,
    Path((id, kind)): Path<(String, String)>,
) -> Result<Json<RoastDoc>, ApiError> {
    let id = roast_id(&id)?;
    app.ask(|r| Request::Edit(Edit::DeleteEvent(id, kind, r)))
        .await
        .map(Json)
}

async fn add_note(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Body(body): Body<NoteIn>,
) -> Result<(StatusCode, Json<NoteOut>), ApiError> {
    let id = roast_id(&id)?;
    app.ask(|r| Request::Edit(Edit::AddNote(id, body, r)))
        .await
        .map(|n| (StatusCode::CREATED, Json(n)))
}

async fn patch_note(
    State(app): State<AppState>,
    Path((id, note)): Path<(String, String)>,
    Body(body): Body<NotePatch>,
) -> Result<Json<NoteOut>, ApiError> {
    let (id, note) = (roast_id(&id)?, parse_id(&note, "note")?);
    app.ask(|r| Request::Edit(Edit::PatchNote(id, note, body, r)))
        .await
        .map(Json)
}

async fn delete_note(
    State(app): State<AppState>,
    Path((id, note)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let (id, note) = (roast_id(&id)?, parse_id(&note, "note")?);
    app.ask(|r| Request::Edit(Edit::DeleteNote(id, note, r)))
        .await
        .map(|()| StatusCode::NO_CONTENT)
}

async fn discard(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RoastDoc>, ApiError> {
    let id = roast_id(&id)?;
    app.ask(|r| Request::Edit(Edit::Discard(id, r)))
        .await
        .map(Json)
}

async fn restore(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RoastDoc>, ApiError> {
    let id = roast_id(&id)?;
    app.ask(|r| Request::Edit(Edit::Restore(id, r)))
        .await
        .map(Json)
}

async fn close(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RoastDoc>, ApiError> {
    let id = roast_id(&id)?;
    app.ask(|r| Request::Edit(Edit::Close(id, r)))
        .await
        .map(Json)
}

async fn next_roast(State(app): State<AppState>) -> Result<Json<NextRoastOut>, ApiError> {
    app.ask(|r| Request::Edit(Edit::GetNextRoast(r)))
        .await
        .map(Json)
}

async fn put_next_roast(
    State(app): State<AppState>,
    Body(body): Body<NextRoastIn>,
) -> Result<Json<NextRoastOut>, ApiError> {
    app.ask(|r| Request::Edit(Edit::PutNextRoast(body, r)))
        .await
        .map(Json)
}

#[derive(Deserialize)]
struct CoffeeQuery {
    #[serde(default)]
    archived: bool,
}

async fn coffees(
    State(app): State<AppState>,
    Query(q): Query<CoffeeQuery>,
) -> Result<Json<Vec<CoffeeOut>>, ApiError> {
    app.ask(|r| Request::Edit(Edit::Coffees(q.archived, r)))
        .await
        .map(Json)
}

async fn add_coffee(
    State(app): State<AppState>,
    Body(body): Body<CoffeeIn>,
) -> Result<(StatusCode, Json<CoffeeOut>), ApiError> {
    app.ask(|r| Request::Edit(Edit::AddCoffee(body, r)))
        .await
        .map(|c| (StatusCode::CREATED, Json(c)))
}

async fn patch_coffee(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Body(body): Body<CoffeePatch>,
) -> Result<Json<CoffeeOut>, ApiError> {
    let id = parse_id(&id, "coffee")?;
    app.ask(|r| Request::Edit(Edit::PatchCoffee(id, body, r)))
        .await
        .map(Json)
}

async fn live(ws: WebSocketUpgrade, State(app): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| stream_live(socket, app))
}

/// Sends a snapshot, then every live message. A client that falls too far
/// behind gets a fresh snapshot instead of the messages it missed.
async fn stream_live(mut socket: WebSocket, app: AppState) {
    loop {
        let Ok((snapshot, mut rx)) = app.ask(Request::Subscribe).await else {
            return;
        };
        if socket
            .send(Message::Text(snapshot.to_json().into()))
            .await
            .is_err()
        {
            return;
        }
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Ok(msg) => {
                        if socket.send(Message::Text(msg.to_json().into())).await.is_err() {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => break, // resnapshot
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                incoming = socket.recv() => match incoming {
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return,
                    Some(Ok(_)) => {} // the stream is one-way; ignore what the client says
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn origin_check_lets_tools_and_our_page_through() {
        let allowed = vec!["http://127.0.0.1:8000".to_owned()];
        let with = |o: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::ORIGIN, HeaderValue::from_str(o).unwrap());
            h
        };
        assert!(
            origin_allowed(&HeaderMap::new(), &allowed),
            "no Origin: curl and friends"
        );
        assert!(origin_allowed(&with("http://127.0.0.1:8000"), &allowed));
        assert!(!origin_allowed(&with("https://evil.example"), &allowed));
        assert!(
            !origin_allowed(&with("http://127.0.0.1:9999"), &allowed),
            "another local app"
        );
    }
}
