mod auth;
mod book;
mod telemetry;

use crate::{
    crypto,
    db::Store,
    error::{self, ApiError},
    limit::{IpConcurrency, Limiter, client_ip},
};
use anyhow::Result;
use axum::{
    Json, Router,
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use ipnet::IpNet;
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct App {
    pub store: Store,
    limiter: Arc<Limiter>,
    password_workers: Arc<Semaphore>,
    requests: Arc<Semaphore>,
    requests_per_ip: Arc<IpConcurrency>,
    trusted: Arc<Vec<IpNet>>,
    dummy_hash: Arc<String>,
}
#[derive(Clone, Copy)]
struct ClientIp(IpAddr);
#[derive(Debug)]
struct Principal {
    id: String,
    username: String,
    session: String,
}
type ApiResult<T> = std::result::Result<T, ApiError>;

impl App {
    pub fn new(store: Store) -> Result<Self> {
        let config = &store.config;
        Ok(Self {
            limiter: Arc::new(Limiter::new(config.limiter_capacity)),
            password_workers: Arc::new(Semaphore::new(config.password_workers)),
            requests: Arc::new(Semaphore::new(config.max_concurrent_requests)),
            requests_per_ip: Arc::new(IpConcurrency::new(config.max_concurrent_requests_per_ip)),
            trusted: Arc::new(
                config
                    .trusted_proxies
                    .iter()
                    .map(|x| x.parse())
                    .collect::<std::result::Result<_, _>>()?,
            ),
            dummy_hash: Arc::new(crypto::hash_password(&crypto::random_token())?),
            store,
        })
    }
    pub fn router(self) -> Router {
        let max_body = self.store.config.max_request_bytes;
        Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .route("/api/login-options", get(|| async { Json(json!([])) }))
            .route("/api/login", post(auth::login))
            .route("/api/logout", post(auth::logout))
            .route("/api/currentUser", post(auth::current_user))
            .route("/api/users", get(auth::users))
            .route("/api/peers", get(auth::devices))
            .route("/api/device-group/accessible", get(auth::groups))
            .route("/api/ab/personal", post(book::personal))
            .route("/api/ab/settings", post(book::settings))
            .route("/api/ab/shared/profiles", post(book::profiles))
            .route("/api/ab/peers", post(book::peers))
            .route("/api/ab/tags/{book}", post(book::tags))
            .route("/api/ab/peer/add/{book}", post(book::add_peer))
            .route("/api/ab/peer/update/{book}", put(book::update_peer))
            .route("/api/ab/peer/{book}", delete(book::delete_peers))
            .route("/api/ab/tag/add/{book}", post(book::add_tag))
            .route("/api/ab/tag/rename/{book}", put(book::rename_tag))
            .route("/api/ab/tag/update/{book}", put(book::update_tag))
            .route("/api/ab/tag/{book}", delete(book::delete_tags))
            .route("/api/sysinfo", post(telemetry::sysinfo))
            .route("/api/heartbeat", post(telemetry::heartbeat))
            .route("/api/audit/{kind}", post(telemetry::report))
            .fallback(|| async { (StatusCode::NOT_FOUND, Json(json!({"error":"Not found"}))) })
            .layer(DefaultBodyLimit::max(max_body))
            .layer(middleware::from_fn_with_state(self.clone(), guard))
            .with_state(self)
    }
}

async fn guard(State(app): State<App>, mut request: Request, next: Next) -> Response {
    let Ok(_permit) = app.requests.clone().try_acquire_owned() else {
        return ApiError(error::busy()).into_response();
    };
    let remote = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let ip = client_ip(remote, request.headers(), &app.trusted);
    let Some(_ip_permit) = app.requests_per_ip.acquire(ip) else {
        return ApiError(error::busy()).into_response();
    };
    if !app.limiter.allow(&[(
        format!("requests/{ip}"),
        app.store.config.requests_per_ip_per_minute,
    )]) {
        return ApiError(error::busy()).into_response();
    }
    request.extensions_mut().insert(ClientIp(ip));
    let (parts, body) = request.into_parts();
    let bytes = match tokio::time::timeout(
        Duration::from_secs(10),
        axum::body::to_bytes(body, app.store.config.max_request_bytes),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({"error":"Request body too large"})),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::REQUEST_TIMEOUT,
                Json(json!({"error":"Request timed out"})),
            )
                .into_response();
        }
    };
    let request = Request::from_parts(parts, axum::body::Body::from(bytes));
    let mut response = match tokio::time::timeout(Duration::from_secs(20), next.run(request)).await
    {
        Ok(response) => response,
        Err(_) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(json!({"error":"Request timed out"})),
        )
            .into_response(),
    };
    if response.status().is_client_error()
        && !response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|s| s.starts_with("application/json"))
    {
        response = (response.status(), Json(json!({"error":"Invalid request"}))).into_response();
    }
    response.headers_mut().insert(
        "cache-control",
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        axum::http::HeaderValue::from_static("nosniff"),
    );
    response
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &Bytes) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| error::bad("Invalid JSON request"))
}
fn token(headers: &HeaderMap) -> Result<Vec<u8>> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| s.len() == 43)
        .ok_or_else(error::unauthorized)?;
    Ok(crypto::token_hash(token))
}
fn authorize(c: &Connection, token: &[u8]) -> Result<Principal> {
    c.query_row("SELECT u.id,u.username,s.id FROM sessions s JOIN users u ON u.id=s.user_id
        LEFT JOIN devices d ON d.id=s.device_id WHERE s.token_hash=?1 AND s.revoked IS NULL AND u.disabled=0
        AND (s.device_id IS NULL OR (d.owner=s.user_id AND d.uuid=s.machine_uuid))", [token],
        |r| Ok(Principal { id:r.get(0)?, username:r.get(1)?, session:r.get(2)? }))
        .optional()?.ok_or_else(error::unauthorized)
}
fn user_payload(user: &Principal) -> Value {
    json!({"id":user.id,"name":user.username,"display_name":user.username,"email":"","note":"","avatar":"","status":1,"is_admin":false})
}

#[derive(Deserialize)]
#[serde(default)]
struct Page {
    current: usize,
    #[serde(rename = "pageSize")]
    size: usize,
    ab: String,
}
impl Default for Page {
    fn default() -> Self {
        Self {
            current: 1,
            size: 100,
            ab: String::new(),
        }
    }
}
impl Page {
    fn bounds(&self) -> Result<(i64, i64)> {
        if self.current == 0 || self.current > 10000 || self.size == 0 || self.size > 100 {
            return Err(error::bad(
                "Invalid pagination (pageSize 1-100, current 1-10000)",
            ));
        }
        Ok((self.size as i64, ((self.current - 1) * self.size) as i64))
    }
}
