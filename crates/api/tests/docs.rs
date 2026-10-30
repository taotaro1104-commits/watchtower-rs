//! OpenAPI 定義 (`/api/openapi.json`) と、API ドキュメント (`/api/docs`)。

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::Value;

const OPERATIONS: &[(&str, &str)] = &[
    ("get", "/api/v1/watch"),
    ("post", "/api/v1/watch"),
    ("get", "/api/v1/watch/{uuid}"),
    ("put", "/api/v1/watch/{uuid}"),
    ("delete", "/api/v1/watch/{uuid}"),
    ("get", "/api/v1/watch/{uuid}/history"),
    ("get", "/api/v1/watch/{uuid}/history/{timestamp}"),
    ("get", "/api/v1/tags"),
    ("post", "/api/v1/tags"),
    ("post", "/api/v1/import"),
    ("get", "/api/v1/systeminfo"),
];

async fn spec(a: &Api) -> Value {
    let r = send(a, get("/api/openapi.json", None)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    r.json()
}

#[tokio::test]
async fn the_spec_is_public_valid_openapi_3_and_declares_the_api_key() {
    let a = api().await;
    let s = spec(&a).await;
    assert!(s["openapi"].as_str().unwrap().starts_with("3."), "{s}");
    assert_eq!(s["info"]["version"], env!("CARGO_PKG_VERSION"));
    let scheme = &s["components"]["securitySchemes"]["apiKey"];
    assert_eq!(scheme["type"], "apiKey");
    assert_eq!(scheme["in"], "header");
    assert_eq!(scheme["name"], "x-api-key");
    assert!(
        s["security"].to_string().contains("apiKey"),
        "global security requirement"
    );
}

#[tokio::test]
async fn every_operation_of_the_design_is_documented() {
    let a = api().await;
    let s = spec(&a).await;
    for (method, path) in OPERATIONS {
        let op = &s["paths"][path][method];
        assert!(op.is_object(), "{method} {path} is missing");
        assert!(
            op["summary"].as_str().is_some_and(|t| !t.is_empty()),
            "{method} {path}"
        );
        assert!(op["responses"].is_object(), "{method} {path}");
    }
}

#[tokio::test]
async fn path_parameters_are_declared_for_templated_paths() {
    let a = api().await;
    let s = spec(&a).await;
    let params =
        s["paths"]["/api/v1/watch/{uuid}/history/{timestamp}"]["get"]["parameters"].to_string();
    assert!(
        params.contains("\"uuid\"") && params.contains("\"timestamp\""),
        "{params}"
    );
    let query = s["paths"]["/api/v1/watch/{uuid}"]["get"]["parameters"].to_string();
    assert!(
        query.contains("recheck") && query.contains("paused"),
        "{query}"
    );
}

#[tokio::test]
async fn every_documented_operation_is_really_routed() {
    let a = api().await;
    let s = spec(&a).await;
    let zero = "00000000-0000-0000-0000-000000000000";
    for (method, path) in OPERATIONS {
        let concrete = path
            .replace("{uuid}", zero)
            .replace("{timestamp}", "latest");
        let req = match *method {
            "get" => get(&concrete, Some(&a.key)),
            "delete" => delete(&concrete, Some(&a.key)),
            "post" if concrete.ends_with("/import") => text_post(&concrete, Some(&a.key), ""),
            m => json_req(
                m.to_uppercase().parse().unwrap(),
                &concrete,
                Some(&a.key),
                &serde_json::json!({}),
            ),
        };
        let r = send(&a, req).await;
        // 未登録のパスは空の 404 / 405。登録済みなら API 自身の JSON か成功応答が返る。
        let routed = r.status != StatusCode::METHOD_NOT_ALLOWED
            && !(r.status == StatusCode::NOT_FOUND && r.body.is_empty());
        assert!(routed, "{method} {path} -> {} {:?}", r.status, r.body);
    }
    assert_eq!(s["paths"].as_object().unwrap().len(), 7);
}

#[tokio::test]
async fn the_docs_page_lists_the_endpoints_without_needing_a_key() {
    let a = api().await;
    let r = send(&a, get("/api/docs", None)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    for (_, path) in OPERATIONS {
        let escaped = path.replace('{', "&#123;").replace('}', "&#125;");
        assert!(
            r.body.contains(path) || r.body.contains(&escaped),
            "{path} missing from the docs page"
        );
    }
    assert!(r.body.contains("x-api-key"));
    assert!(r.body.contains("/api/openapi.json"));
}

#[tokio::test]
async fn the_docs_page_cannot_run_scripts() {
    let a = api().await;
    let r = send(&a, get("/api/docs", None)).await;
    let csp = r.headers["content-security-policy"].to_str().unwrap();
    assert!(csp.contains("default-src 'none'"), "{csp}");
    assert!(!csp.contains("script-src"), "{csp}");
    assert!(
        !r.body.to_lowercase().contains("<script"),
        "no scripts in the page"
    );
}

#[tokio::test]
async fn documentation_does_not_open_the_api_itself() {
    let a = api().await;
    assert_eq!(
        send(&a, get("/api/v1/watch", None)).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn docs_are_available_even_before_a_key_exists() {
    let a = api_without_key().await;
    assert_eq!(
        send(&a, get("/api/docs", None)).await.status,
        StatusCode::OK
    );
    assert_eq!(
        send(&a, get("/api/openapi.json", None)).await.status,
        StatusCode::OK
    );
}
