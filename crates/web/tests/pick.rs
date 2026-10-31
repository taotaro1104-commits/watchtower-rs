//! ビジュアルセレクタ: 描画したページを見せ、クリックした要素のセレクタをウォッチの設定に加える。

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::http::StatusCode;
use common::*;
use tokio::sync::Notify;
use url::Url;
use watchtower_core::pick::{Inspection, PickElement};
use watchtower_core::{BrowserStep, Watch};
use watchtower_store::Store;
use watchtower_web::{router, AppState, PagePicker, WebConfig};

struct FakePicker {
    result: Result<Inspection, String>,
    seen: Mutex<Vec<(String, usize)>>,
    /// 設定すると、これが通知されるまで検査を終えない。
    gate: Option<Arc<Notify>>,
}

#[async_trait]
impl PagePicker for FakePicker {
    async fn inspect(&self, url: Url, steps: Vec<BrowserStep>) -> Result<Inspection, String> {
        self.seen
            .lock()
            .unwrap()
            .push((url.to_string(), steps.len()));
        if let Some(g) = &self.gate {
            g.notified().await;
        }
        self.result.clone()
    }
}

fn element(selector: &str, x: u32, y: u32, w: u32, h: u32, text: &str) -> PickElement {
    PickElement {
        selector: selector.into(),
        x,
        y,
        width: w,
        height: h,
        text: text.into(),
    }
}

fn inspection() -> Inspection {
    Inspection {
        width: 1280,
        height: 900,
        screenshot: b"PNG-fake".to_vec(),
        elements: vec![
            element("#price", 100, 200, 80, 30, "¥1,980"),
            element(
                "body:nth-of-type(1) > div:nth-of-type(1)",
                0,
                0,
                1280,
                900,
                "page",
            ),
            element("#evil\"><script>x</script>", 10, 10, 50, 50, "<b>hi</b>"),
        ],
    }
}

async fn app_with_picker(picker: Option<Arc<FakePicker>>) -> App {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let mut state = AppState::new(store.clone(), WebConfig::default());
    if let Some(p) = picker {
        state = state.with_picker(p);
    }
    state.set_password(PASSWORD).await.unwrap();
    App {
        router: router(state.clone()),
        state,
        store,
        _dir: dir,
    }
}

fn fake(result: Result<Inspection, String>) -> Arc<FakePicker> {
    Arc::new(FakePicker {
        result,
        seen: Mutex::new(Vec::new()),
        gate: None,
    })
}

async fn add_watch(a: &App) -> Watch {
    let mut w = Watch::new(Url::parse("https://example.com/item").unwrap());
    w.config.browser_steps = vec![BrowserStep::Wait { ms: 10 }];
    a.store.add_watch(&w).await.unwrap();
    w
}

#[tokio::test]
async fn the_page_shows_the_screenshot_with_a_clickable_box_per_element() {
    let p = fake(Ok(inspection()));
    let a = app_with_picker(Some(Arc::clone(&p))).await;
    let w = add_watch(&a).await;
    let cookie = login(&a).await;
    let r = send(&a, get(&format!("/watches/{}/pick", w.id), Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(
        r.body.contains("data:image/png;base64,UE5HLWZha2U="),
        "{}",
        r.body
    );
    assert!(r.body.contains("viewBox=\"0 0 1280 900\""), "{}", r.body);
    assert!(
        r.body.contains("/pick/choose?selector=%23price"),
        "{}",
        r.body
    );
    // 大きい要素が先 (下)、小さい要素が後 (上) に描かれ、クリックで小さいほうが選べる
    let big = r.body.find("body%3Anth-of-type").unwrap();
    let small = r.body.find("selector=%23price").unwrap();
    assert!(big < small, "{}", r.body);
    assert_eq!(
        *p.seen.lock().unwrap(),
        vec![("https://example.com/item".to_string(), 1)],
        "the watch's URL and steps are used"
    );
}

#[tokio::test]
async fn text_and_selectors_from_the_page_are_escaped() {
    let a = app_with_picker(Some(fake(Ok(inspection())))).await;
    let w = add_watch(&a).await;
    let cookie = login(&a).await;
    let r = send(&a, get(&format!("/watches/{}/pick", w.id), Some(&cookie))).await;
    assert!(!r.body.contains("<script>x</script>"), "{}", r.body);
    assert!(!r.body.contains("<b>hi</b>"), "{}", r.body);
}

#[tokio::test]
async fn without_a_browser_the_feature_explains_itself() {
    let a = app_with_picker(None).await;
    let w = add_watch(&a).await;
    let cookie = login(&a).await;
    let r = send(&a, get(&format!("/watches/{}/pick", w.id), Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.body.contains("browser"), "{}", r.body);
}

#[tokio::test]
async fn it_requires_login_and_a_known_watch() {
    let a = app_with_picker(Some(fake(Ok(inspection())))).await;
    let w = add_watch(&a).await;
    let r = send(&a, get(&format!("/watches/{}/pick", w.id), None)).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let cookie = login(&a).await;
    let r = send(
        &a,
        get(
            &format!("/watches/{}/pick", uuid::Uuid::new_v4()),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = send(&a, get("/watches/not-a-uuid/pick", Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_failed_render_is_reported_without_crashing() {
    let a = app_with_picker(Some(fake(Err("the browser is not reachable".into())))).await;
    let w = add_watch(&a).await;
    let cookie = login(&a).await;
    let r = send(&a, get(&format!("/watches/{}/pick", w.id), Some(&cookie))).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert!(r.body.contains("not reachable"), "{}", r.body);
}

#[tokio::test]
async fn only_one_render_runs_at_a_time() {
    let gate = Arc::new(Notify::new());
    let p = Arc::new(FakePicker {
        result: Ok(inspection()),
        seen: Mutex::new(Vec::new()),
        gate: Some(Arc::clone(&gate)),
    });
    let a = Arc::new(app_with_picker(Some(p)).await);
    let w = add_watch(&a).await;
    let cookie = login(&a).await;
    let path = format!("/watches/{}/pick", w.id);

    let (a2, c2, p2) = (Arc::clone(&a), cookie.clone(), path.clone());
    let first = tokio::spawn(async move { send(&a2, get(&p2, Some(&c2))).await });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let second = send(&a, get(&path, Some(&cookie))).await;
    assert_eq!(second.status, StatusCode::TOO_MANY_REQUESTS);
    gate.notify_one();
    assert_eq!(first.await.unwrap().status, StatusCode::OK);
    // 終わったあとはまた使える
    gate.notify_one();
    assert_eq!(
        send(&a, get(&path, Some(&cookie))).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn choosing_shows_the_selector_and_asks_for_include_or_exclude() {
    let a = app_with_picker(Some(fake(Ok(inspection())))).await;
    let w = add_watch(&a).await;
    let cookie = login(&a).await;
    let sel = "div#a > p:nth-of-type(2)\"><script>";
    let r = send(
        &a,
        get(
            &format!("/watches/{}/pick/choose?selector={}", w.id, enc(sel)),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(
        r.body.contains("div#a &#62; p:nth-of-type(2)"),
        "{}",
        r.body
    );
    assert!(!r.body.contains("<script>"), "{}", r.body);
    assert!(r.body.contains("name=\"csrf\""));
    assert!(r.body.contains("value=\"include\"") && r.body.contains("value=\"exclude\""));
}

async fn choose(a: &App, cookie: &str, w: &Watch, selector: &str, mode: &str) -> StatusCode {
    let t = enc(&csrf(a, cookie).await);
    send(
        a,
        post(
            &format!("/watches/{}/pick/choose", w.id),
            Some(cookie),
            &format!("csrf={t}&selector={}&mode={mode}", enc(selector)),
        ),
    )
    .await
    .status
}

async fn config_of(a: &App, w: &Watch) -> watchtower_core::ConfigLayer {
    a.store.get_watch(w.id).await.unwrap().unwrap().config
}

#[tokio::test]
async fn a_chosen_selector_is_added_once_to_include_or_exclude() {
    let a = app_with_picker(Some(fake(Ok(inspection())))).await;
    let w = add_watch(&a).await;
    let cookie = login(&a).await;

    assert_eq!(
        choose(&a, &cookie, &w, "#price", "include").await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        choose(&a, &cookie, &w, "#price", "include").await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        choose(&a, &cookie, &w, ".ad", "exclude").await,
        StatusCode::SEE_OTHER
    );
    let c = config_of(&a, &w).await;
    assert_eq!(c.include_filters, vec!["#price"]);
    assert_eq!(c.exclude_filters, vec![".ad"]);
    assert_eq!(c.browser_steps.len(), 1, "other settings are untouched");
}

#[tokio::test]
async fn bad_choices_are_rejected_and_change_nothing() {
    let a = app_with_picker(Some(fake(Ok(inspection())))).await;
    let w = add_watch(&a).await;
    let cookie = login(&a).await;
    for (sel, mode) in [
        ("", "include"),
        ("   ", "include"),
        ("a\nb", "include"),
        (&"x".repeat(501), "include"),
        ("#ok", "sideways"),
        ("#ok", ""),
    ] {
        assert_eq!(
            choose(&a, &cookie, &w, sel, mode).await,
            StatusCode::BAD_REQUEST,
            "{sel:?} {mode}"
        );
    }
    // CSRF トークンが違う
    let r = send(
        &a,
        post(
            &format!("/watches/{}/pick/choose", w.id),
            Some(&cookie),
            "csrf=wrong&selector=%23ok&mode=include",
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let c = config_of(&a, &w).await;
    assert!(c.include_filters.is_empty() && c.exclude_filters.is_empty());
}

#[tokio::test]
async fn the_edit_page_links_to_the_picker_only_when_a_browser_exists() {
    let a = app_with_picker(Some(fake(Ok(inspection())))).await;
    let w = add_watch(&a).await;
    let cookie = login(&a).await;
    let r = send(&a, get(&format!("/watches/{}/edit", w.id), Some(&cookie))).await;
    assert!(
        r.body.contains(&format!("/watches/{}/pick", w.id)),
        "{}",
        r.body
    );

    let a = app_with_picker(None).await;
    let w = add_watch(&a).await;
    let cookie = login(&a).await;
    let r = send(&a, get(&format!("/watches/{}/edit", w.id), Some(&cookie))).await;
    assert!(!r.body.contains("/pick"), "{}", r.body);
}
