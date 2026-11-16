//! The HTTP API and the live WebSocket ("Contract between the backend and the
//! web UI"). Every handler asks the engine through its queue; nothing here
//! touches the database or the PLC. There is no route that writes to the PLC.

use std::fmt::Display;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Request as HttpRequest, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use store::RoastId;
use tokio::sync::{broadcast, mpsc, oneshot};
use uuid::Uuid;

use crate::engine::{Reply, Request};
use crate::wire::{Columns, RoastDoc};

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
        .route("/api/roasts/{id}", get(roast))
        .route("/api/roasts/{id}/samples", get(samples))
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

async fn roast(
    State(app): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<RoastDoc>, ApiError> {
    app.ask(|r| Request::Roast(RoastId(id), r)).await.map(Json)
}

async fn samples(
    State(app): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Columns>, ApiError> {
    app.ask(|r| Request::Samples(RoastId(id), r))
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
        if socket.send(Message::Text(snapshot.into())).await.is_err() {
            return;
        }
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Ok(json) => {
                        if socket.send(Message::Text(json.as_ref().into())).await.is_err() {
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
