#![allow(clippy::result_large_err)] // 早期リターン用の Response をそのまま Err に載せる (短命な値)
//! ルートごとのハンドラ。HTML はすべて askama (自動エスケープ) で生成する。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use askama::Template;
use axum::extract::{ConnectInfo, FromRequestParts, Path, State};
use axum::http::request::Parts;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;
use watchtower_core::effective::{MAX_INTERVAL_SECS, MIN_INTERVAL_SECS};
use watchtower_core::Watch;
use watchtower_process::diff;
use watchtower_scheduler::Command;

use crate::auth::{constant_time_eq, verify_password};
use crate::AppState;

const SESSION_COOKIE: &str = "wt_session";
pub(crate) const MAX_URL_CHARS: usize = 2048;
pub(crate) const MAX_TITLE_CHARS: usize = 200;
const STYLESHEET: &str = include_str!("../static/app.css");

// ---- テンプレート -----------------------------------------------------------

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    error: Option<&'static str>,
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    message: String,
}

struct Row {
    id: String,
    title: String,
    url: String,
    paused: bool,
    last_checked: String,
    last_changed: String,
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexPage {
    csrf: String,
    rows: Vec<Row>,
}

struct DiffLine {
    added: bool,
    text: String,
}

#[derive(Template)]
#[template(path = "watch.html")]
struct WatchPage {
    csrf: String,
    row: Row,
    snapshots: usize,
    diff: Vec<DiffLine>,
    events: Vec<crate::events::EventRow>,
}

pub(crate) fn render<T: Template>(status: StatusCode, t: &T) -> Response {
    match t.render() {
        Ok(html) => (status, Html(html)).into_response(),
        Err(e) => {
            tracing::error!("template error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

pub(crate) fn error_page(status: StatusCode, message: impl Into<String>) -> Response {
    render(
        status,
        &ErrorPage {
            message: message.into(),
        },
    )
}

const SCRIPT: &str = include_str!("../static/app.js");

pub async fn script() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        SCRIPT,
    )
        .into_response()
}

pub async fn stylesheet() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        STYLESHEET,
    )
        .into_response()
}

// ---- 認証 -------------------------------------------------------------------

pub(crate) struct Authed {
    pub(crate) sid: String,
    pub(crate) csrf: String,
}

fn unconfigured() -> Response {
    error_page(
        StatusCode::SERVICE_UNAVAILABLE,
        "No password is configured. Run `watchtower set-password` first.",
    )
}

pub(crate) async fn require(state: &AppState, headers: &HeaderMap) -> Result<Authed, Response> {
    let Some(hash) = state.password_hash().await else {
        return Err(unconfigured());
    };
    let jar = CookieJar::from_headers(headers);
    jar.get(SESSION_COOKIE)
        .and_then(|c| {
            let sid = c.value().to_string();
            state
                .0
                .sessions
                .get(&sid)
                // パスワードが変更されたら、それ以前のセッションは無効
                .filter(|s| s.pw_fingerprint == hash)
                .map(|s| Authed { sid, csrf: s.csrf })
        })
        .ok_or_else(|| Redirect::to("/login").into_response())
}

pub(crate) fn check_csrf(auth: &Authed, supplied: &str) -> Result<(), Response> {
    if constant_time_eq(&auth.csrf, supplied) {
        Ok(())
    } else {
        Err(error_page(StatusCode::FORBIDDEN, "Invalid CSRF token."))
    }
}

/// `Origin` があるなら `Host` と同じホストでなければ拒否する (ログイン CSRF 対策)。
fn same_origin(headers: &HeaderMap) -> bool {
    // Fetch Metadata: 同一オリジン、またはユーザー自身の操作 (none) 以外は拒否する
    if let Some(site) = headers.get("sec-fetch-site") {
        if !matches!(site.to_str(), Ok("same-origin" | "none")) {
            return false;
        }
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok());
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|o| o.split_once("://"))
        .map(|(_, rest)| rest.trim_end_matches('/'));
    matches!((origin_host, host), (Some(o), Some(h)) if o.eq_ignore_ascii_case(h))
}

/// 接続元の IP。接続情報が無い場合 (テストなど) は None。
pub struct Peer(pub(crate) Option<IpAddr>);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0.ip()),
        ))
    }
}

pub async fn login_page(State(state): State<AppState>) -> Response {
    if state.password_hash().await.is_none() {
        return unconfigured();
    }
    render(StatusCode::OK, &LoginPage { error: None })
}

#[derive(Deserialize)]
pub struct LoginForm {
    #[serde(default)]
    password: String,
}

pub async fn login(
    State(state): State<AppState>,
    Peer(peer): Peer,
    headers: HeaderMap,
    jar: CookieJar,
    Form(form): Form<LoginForm>,
) -> Response {
    let Some(hash) = state.password_hash().await else {
        return unconfigured();
    };
    if !same_origin(&headers) {
        return error_page(StatusCode::FORBIDDEN, "Cross-origin request refused.");
    }
    match verify_with_budget(&state, peer, form.password, hash.clone()).await {
        Verdict::Ok => (start_session(&state, jar, &hash), Redirect::to("/")).into_response(),
        Verdict::Busy => too_many(),
        Verdict::Wrong => render(
            StatusCode::UNAUTHORIZED,
            &LoginPage {
                error: Some("Incorrect password."),
            },
        ),
    }
}

pub(crate) enum Verdict {
    Ok,
    Wrong,
    /// 試行の予算または同時検証の枠が尽きている。
    Busy,
}

pub(crate) fn too_many() -> Response {
    error_page(
        StatusCode::TOO_MANY_REQUESTS,
        "Too many login attempts. Try again later.",
    )
}

/// パスワードを検証する。接続元ごとに試行を予約してから検証するので、並列に投げても
/// 予算を超えられない。成功した試行は予算に数えない。ログインとパスワード変更で共有する。
pub(crate) async fn verify_with_budget(
    state: &AppState,
    peer: Option<IpAddr>,
    password: String,
    hash: String,
) -> Verdict {
    let client = peer.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    if !state.0.limiter.reserve(client) {
        return Verdict::Busy;
    }
    // 同時に走る Argon2 検証の数を絞る (メモリ・CPU の枯渇対策)
    let Ok(_slot) = state.0.verify_slots.try_acquire() else {
        state.0.limiter.release(client);
        return Verdict::Busy;
    };
    let ok = tokio::task::spawn_blocking(move || verify_password(&password, &hash))
        .await
        .unwrap_or(false);
    if ok {
        state.0.limiter.release(client);
        Verdict::Ok
    } else {
        Verdict::Wrong
    }
}

/// 新しいセッションを作り、そのセッション Cookie を載せた `jar` を返す。
pub(crate) fn start_session(state: &AppState, jar: CookieJar, hash: &str) -> CookieJar {
    let ttl = state.0.cfg.session_ttl;
    let (sid, _csrf) = state.0.sessions.create(ttl, hash);
    let cookie = Cookie::build((SESSION_COOKIE, sid))
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(state.0.cfg.secure_cookies)
        .path("/")
        .max_age(time::Duration::try_from(ttl).unwrap_or(time::Duration::HOUR))
        .build();
    jar.add(cookie)
}

#[derive(Deserialize)]
pub struct CsrfForm {
    #[serde(default)]
    csrf: String,
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Form(form): Form<CsrfForm>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = check_csrf(&auth, &form.csrf) {
        return r;
    }
    state.0.sessions.remove(&auth.sid);
    (
        jar.remove(Cookie::build(SESSION_COOKIE).path("/")),
        Redirect::to("/login"),
    )
        .into_response()
}

// ---- 画面 -------------------------------------------------------------------

fn fmt_time(t: Option<OffsetDateTime>) -> String {
    t.and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_else(|| "never".into())
}

fn row_of(w: &Watch) -> Row {
    Row {
        id: w.id.to_string(),
        title: w.title.clone().unwrap_or_default(),
        url: w.url.to_string(),
        paused: w.paused,
        last_checked: fmt_time(w.state.last_checked_at),
        last_changed: fmt_time(w.state.last_changed_at),
        error: w.state.last_error.clone(),
    }
}

pub(crate) fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!("request failed: {e}");
    error_page(StatusCode::INTERNAL_SERVER_ERROR, "Internal error.")
}

pub async fn index(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    match state.0.store.list_watches().await {
        Ok(ws) => render(
            StatusCode::OK,
            &IndexPage {
                csrf: auth.csrf,
                rows: ws.iter().map(row_of).collect(),
            },
        ),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct AddForm {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    interval: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) fn bad_request(msg: &str) -> Response {
    error_page(StatusCode::BAD_REQUEST, msg)
}

pub(crate) fn parse_interval(raw: &str) -> Result<Option<u64>, Response> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    match raw.parse::<u64>() {
        Ok(n) if (MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(&n) => Ok(Some(n)),
        _ => Err(bad_request(
            "Interval must be a number of seconds within the allowed range.",
        )),
    }
}

pub async fn add_watch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<AddForm>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = check_csrf(&auth, &form.csrf) {
        return r;
    }
    let raw = form.url.trim();
    let url = match Url::parse(raw) {
        Ok(u) if raw.chars().count() <= MAX_URL_CHARS && matches!(u.scheme(), "http" | "https") => {
            u
        }
        _ => return bad_request("A valid http(s) URL is required."),
    };
    let interval = match parse_interval(&form.interval) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let title = form.title.trim();
    if title.chars().count() > MAX_TITLE_CHARS {
        return bad_request("Title is too long.");
    }
    let mut w = Watch::new(url);
    w.title = (!title.is_empty()).then(|| title.to_string());
    w.config.check_interval_secs = interval;
    if let Err(e) = state.0.store.add_watch(&w).await {
        return internal(e);
    }
    state.notify_scheduler(Command::Upsert(w.id)).await;
    Redirect::to("/").into_response()
}

pub(crate) async fn find_watch(state: &AppState, id: &str) -> Result<Watch, Response> {
    let not_found = || error_page(StatusCode::NOT_FOUND, "Watch not found.");
    let id = Uuid::parse_str(id).map_err(|_| not_found())?;
    match state.0.store.get_watch(id).await {
        Ok(Some(w)) => Ok(w),
        Ok(None) => Err(not_found()),
        Err(e) => Err(internal(e)),
    }
}

pub async fn watch_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let w = match find_watch(&state, &id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let snaps = state.0.store.list_snapshots(w.id).await.unwrap_or_default();
    let mut lines = Vec::new();
    if let [.., prev, last] = snaps.as_slice() {
        let old = state.0.store.read_snapshot(prev).await;
        let new = state.0.store.read_snapshot(last).await;
        if let (Ok(old), Ok(new)) = (old, new) {
            let d = diff(&old, &new);
            lines.extend(
                d.removed
                    .into_iter()
                    .map(|text| DiffLine { added: false, text }),
            );
            lines.extend(
                d.added
                    .into_iter()
                    .map(|text| DiffLine { added: true, text }),
            );
        }
    }
    render(
        StatusCode::OK,
        &WatchPage {
            csrf: auth.csrf,
            row: row_of(&w),
            snapshots: snaps.len(),
            diff: lines,
            events: crate::events::watch_rows(&state, w.id)
                .await
                .unwrap_or_default(),
        },
    )
}

pub async fn delete_watch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = check_csrf(&auth, &form.csrf) {
        return r;
    }
    let w = match find_watch(&state, &id).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    if let Err(e) = state.0.store.delete_watch(w.id).await {
        return internal(e);
    }
    state.notify_scheduler(Command::Remove(w.id)).await;
    Redirect::to("/").into_response()
}
