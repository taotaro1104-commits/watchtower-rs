#![allow(clippy::result_large_err)] // 早期リターン用の Response をそのまま Err に載せる (短命な値)

//! 設定画面: 共通の通知先、パスワード変更、本家データのインポート。

use askama::Template;
use axum::extract::{Multipart, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use axum_extra::extract::cookie::CookieJar;
use serde::Deserialize;
use watchtower_notify::NotifyTarget;
use watchtower_scheduler::Command;

use crate::handlers::{
    bad_request, check_csrf, error_page, internal, render, require, start_session, too_many,
    verify_with_budget, Peer, Verdict,
};
use crate::{auth, AppState};

const MAX_URLS: usize = 50;
const MAX_URL_CHARS: usize = 500;
/// インポートできるファイルの大きさ。
pub(crate) const MAX_IMPORT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage {
    csrf: String,
    notify_urls: String,
    min_password: usize,
}

#[derive(Template)]
#[template(path = "import_result.html")]
struct ImportResultPage {
    imported: usize,
    skipped: Vec<String>,
    warnings: Vec<String>,
}

pub async fn page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    match state.0.store.global_notify_urls().await {
        Ok(urls) => render(
            StatusCode::OK,
            &SettingsPage {
                csrf: auth.csrf,
                notify_urls: urls.join("\n"),
                min_password: auth::MIN_PASSWORD_CHARS,
            },
        ),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct NotifyForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    notify_urls: String,
}

pub async fn save_notify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<NotifyForm>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = check_csrf(&auth, &form.csrf) {
        return r;
    }
    let urls: Vec<String> = form
        .notify_urls
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    if urls.len() > MAX_URLS || urls.iter().any(|u| u.chars().count() > MAX_URL_CHARS) {
        return bad_request("Too many or too long notification URLs.");
    }
    for u in &urls {
        // 資格情報を含み得るので、URL そのものは応答に含めず理由だけ返す
        if let Err(e) = NotifyTarget::parse(u) {
            return bad_request(&format!("Invalid notification target: {e}"));
        }
    }
    match state.0.store.set_global_notify_urls(&urls).await {
        Ok(()) => Redirect::to("/settings").into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Deserialize)]
pub struct PasswordForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    current: String,
    #[serde(default)]
    new: String,
    #[serde(default)]
    confirm: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    Peer(peer): Peer,
    headers: HeaderMap,
    jar: CookieJar,
    Form(form): Form<PasswordForm>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = check_csrf(&auth, &form.csrf) {
        return r;
    }
    if form.new != form.confirm {
        return bad_request("The new passwords do not match.");
    }
    if form.new.chars().count() < auth::MIN_PASSWORD_CHARS {
        return bad_request(&format!(
            "The new password must be at least {} characters.",
            auth::MIN_PASSWORD_CHARS
        ));
    }
    let Some(hash) = state.password_hash().await else {
        return error_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "No password is configured.",
        );
    };
    // 現在のパスワードの確認もログインと同じ試行予算を使う (盗まれたセッションからの総当たり対策)
    match verify_with_budget(&state, peer, form.current, hash).await {
        Verdict::Ok => {}
        Verdict::Busy => return too_many(),
        Verdict::Wrong => {
            return error_page(
                StatusCode::UNAUTHORIZED,
                "The current password is incorrect.",
            );
        }
    }
    if let Err(e) = state.set_password(&form.new).await {
        return internal(e);
    }
    // 旧パスワードのセッションは (この画面のものも含め) 無効になるので、新しいセッションを発行する
    state.0.sessions.remove(&auth.sid);
    let Some(new_hash) = state.password_hash().await else {
        return internal("password hash missing after update");
    };
    (
        start_session(&state, jar, &new_hash),
        Redirect::to("/settings"),
    )
        .into_response()
}

pub async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let (csrf, file) = match read_form(multipart).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(r) = check_csrf(&auth, &csrf) {
        return r;
    }
    let Some(file) = file else {
        return bad_request("Choose a url-watches.json file to import.");
    };
    let Ok(json) = String::from_utf8(file) else {
        return bad_request("The file is not valid UTF-8 text.");
    };
    match state.0.store.import_changedetection(&json).await {
        Ok(r) => {
            // 取り込んだウォッチをスケジューラへ伝える (再起動を待たずにチェックが始まる)
            for id in &r.ids {
                state.notify_scheduler(Command::Upsert(*id)).await;
            }
            render(
                StatusCode::OK,
                &ImportResultPage {
                    imported: r.imported,
                    skipped: r.skipped,
                    warnings: r.warnings,
                },
            )
        }
        Err(watchtower_store::StoreError::LimitExceeded(msg)) => bad_request(&msg),
        Err(watchtower_store::StoreError::Json(_) | watchtower_store::StoreError::Corrupt(_)) => {
            bad_request("This does not look like a changedetection.io url-watches.json file.")
        }
        Err(e) => internal(e),
    }
}

/// `csrf` と `file` フィールドを読む。大きすぎる場合は 413。
async fn read_form(mut mp: Multipart) -> Result<(String, Option<Vec<u8>>), Response> {
    let mut csrf = String::new();
    let mut file = None;
    loop {
        let field = match mp.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return Err((e.status(), "Invalid upload.").into_response()),
        };
        let name = field.name().unwrap_or_default().to_string();
        let bytes = field
            .bytes()
            .await
            .map_err(|e| (e.status(), "Invalid upload.").into_response())?;
        match name.as_str() {
            "csrf" => csrf = String::from_utf8_lossy(&bytes).into_owned(),
            "file" => file = Some(bytes.to_vec()),
            _ => {}
        }
    }
    Ok((csrf, file))
}

// ---- バックアップのダウンロード ---------------------------------------------

#[derive(Deserialize)]
pub struct BackupForm {
    #[serde(default)]
    csrf: String,
}

/// 配信が終わる (または途中で切れる) と一時ディレクトリを消す。
struct Cleanup {
    stream: tokio_util::io::ReaderStream<tokio::fs::File>,
    // 宣言順に破棄されるので、ファイルを閉じてから消える
    _dir: tempfile::TempDir,
}

impl futures_util::Stream for Cleanup {
    type Item = std::io::Result<axum::body::Bytes>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.stream).poll_next(cx)
    }
}

pub async fn backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<BackupForm>,
) -> Response {
    let auth = match require(&state, &headers).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = check_csrf(&auth, &form.csrf) {
        return r;
    }
    // 同時に何本も作らせない (ディスクと CPU を使うため)
    let Ok(_slot) = state.0.backup_slot.try_acquire() else {
        return error_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "A backup is already running.",
        );
    };
    let dir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(e) => return internal(e),
    };
    let path = dir.path().join("backup.zip");
    if let Err(e) = state.0.store.backup_to(&path).await {
        return internal(e);
    }
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => return internal(e),
    };
    let len = file.metadata().await.map(|m| m.len()).ok();

    let stamp =
        time::format_description::parse_borrowed::<2>("[year][month][day]-[hour][minute][second]")
            .ok()
            .and_then(|f| time::OffsetDateTime::now_utc().format(&f).ok())
            .unwrap_or_else(|| "export".into());
    let body = axum::body::Body::from_stream(Cleanup {
        stream: tokio_util::io::ReaderStream::new(file),
        _dir: dir,
    });
    let mut res = Response::new(body);
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!(
        "attachment; filename=\"watchtower-backup-{stamp}.zip\""
    )) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    if let Some(len) = len {
        h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    }
    res
}
