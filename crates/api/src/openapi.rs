//! OpenAPI 定義と API ドキュメント。操作の表を 1 つだけ持ち、JSON と HTML の両方をそこから作る。

use std::sync::OnceLock;

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde_json::{json, Map, Value};
use utoipa::openapi::OpenApi;

struct Param {
    name: &'static str,
    location: &'static str,
    description: &'static str,
}

struct Operation {
    method: &'static str,
    path: &'static str,
    summary: &'static str,
    params: &'static [Param],
    body: Option<&'static str>,
    responses: &'static [(&'static str, &'static str)],
}

const UUID: Param = Param {
    name: "uuid",
    location: "path",
    description: "Watch UUID",
};

const OPERATIONS: &[Operation] = &[
    Operation {
        method: "get",
        path: "/api/v1/watch",
        summary: "List watches",
        params: &[Param {
            name: "tag",
            location: "query",
            description: "Only watches with this tag (title or UUID)",
        }],
        body: None,
        responses: &[("200", "Object keyed by watch UUID")],
    },
    Operation {
        method: "post",
        path: "/api/v1/watch",
        summary: "Create a watch",
        params: &[],
        body: Some("JSON: url (required), title, paused, tags, time_between_check, include_filters, subtractive_selectors, ignore_text, trigger_text, notification_urls, notification_muted"),
        responses: &[("201", "Created; returns the uuid"), ("400", "Invalid input")],
    },
    Operation {
        method: "get",
        path: "/api/v1/watch/{uuid}",
        summary: "Get a watch; ?recheck=1 checks it now, ?paused=paused|unpaused pauses or resumes it",
        params: &[
            UUID,
            Param {
                name: "recheck",
                location: "query",
                description: "1 or true: queue an immediate check",
            },
            Param {
                name: "paused",
                location: "query",
                description: "paused or unpaused",
            },
        ],
        body: None,
        responses: &[("200", "The watch, or OK for recheck/paused"), ("404", "Unknown watch")],
    },
    Operation {
        method: "put",
        path: "/api/v1/watch/{uuid}",
        summary: "Update a watch (only the fields present are changed)",
        params: &[UUID],
        body: Some("JSON: any of the fields accepted when creating a watch"),
        responses: &[("200", "Updated"), ("400", "Invalid input"), ("404", "Unknown watch")],
    },
    Operation {
        method: "delete",
        path: "/api/v1/watch/{uuid}",
        summary: "Delete a watch",
        params: &[UUID],
        body: None,
        responses: &[("204", "Deleted"), ("404", "Unknown watch")],
    },
    Operation {
        method: "get",
        path: "/api/v1/watch/{uuid}/history",
        summary: "List snapshot times of a watch",
        params: &[UUID],
        body: None,
        responses: &[("200", "Object: UNIX time to snapshot URL"), ("404", "Unknown watch")],
    },
    Operation {
        method: "get",
        path: "/api/v1/watch/{uuid}/history/{timestamp}",
        summary: "Get a snapshot body as plain text (timestamp may be latest)",
        params: &[
            UUID,
            Param {
                name: "timestamp",
                location: "path",
                description: "UNIX time of the snapshot, or latest",
            },
        ],
        body: None,
        responses: &[("200", "Snapshot text"), ("404", "Unknown watch or snapshot")],
    },
    Operation {
        method: "get",
        path: "/api/v1/tags",
        summary: "List tags",
        params: &[],
        body: None,
        responses: &[("200", "Object keyed by tag UUID")],
    },
    Operation {
        method: "post",
        path: "/api/v1/tags",
        summary: "Create a tag",
        params: &[],
        body: Some("JSON: title (1 to 100 characters)"),
        responses: &[("201", "Created; returns the uuid"), ("400", "Invalid input")],
    },
    Operation {
        method: "post",
        path: "/api/v1/import",
        summary: "Import URLs in bulk (text/plain, one URL per line)",
        params: &[
            Param {
                name: "tag",
                location: "query",
                description: "Tag title to attach (created if missing)",
            },
            Param {
                name: "paused",
                location: "query",
                description: "true to create the watches paused",
            },
        ],
        body: Some("text/plain: one http(s) URL per line, at most 5000"),
        responses: &[("200", "Array of the new watch UUIDs"), ("400", "A line is invalid; nothing is imported")],
    },
    Operation {
        method: "get",
        path: "/api/v1/systeminfo",
        summary: "Queue length, watch count and version",
        params: &[],
        body: None,
        responses: &[("200", "System information")],
    },
];

fn operation_json(op: &Operation) -> Value {
    let params: Vec<Value> = op
        .params
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "in": p.location,
                "required": p.location == "path",
                "description": p.description,
                "schema": { "type": "string" },
            })
        })
        .collect();
    let responses: Map<String, Value> = op
        .responses
        .iter()
        .map(|(code, desc)| ((*code).to_string(), json!({ "description": desc })))
        .collect();
    let mut v = json!({
        "summary": op.summary,
        "parameters": params,
        "responses": responses,
    });
    if let Some(body) = op.body {
        let (media, schema) = if body.starts_with("text/plain") {
            ("text/plain", json!({ "type": "string" }))
        } else {
            ("application/json", json!({ "type": "object" }))
        };
        v["requestBody"] = json!({
            "description": body,
            "required": true,
            "content": { media: { "schema": schema } },
        });
    }
    v
}

fn spec_value() -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for op in OPERATIONS {
        let item = paths
            .entry(op.path.to_string())
            .or_insert_with(|| json!({}));
        item[op.method] = operation_json(op);
    }
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "watchtower API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "changedetection.io compatible REST API v1. Send the key in the x-api-key header.",
        },
        "paths": paths,
        "components": {
            "securitySchemes": {
                "apiKey": { "type": "apiKey", "in": "header", "name": "x-api-key" }
            }
        },
        "security": [{ "apiKey": [] }],
    })
}

/// utoipa の型に通して検証・整形した OpenAPI 文書 (JSON)。
fn spec_json() -> &'static Result<String, String> {
    static SPEC: OnceLock<Result<String, String>> = OnceLock::new();
    SPEC.get_or_init(|| {
        serde_json::from_value::<OpenApi>(spec_value())
            .map_err(|e| e.to_string())
            .and_then(|doc| doc.to_pretty_json().map_err(|e| e.to_string()))
    })
}

pub async fn openapi_json() -> Response {
    match spec_json() {
        Ok(json) => (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            json.clone(),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("OpenAPI document is invalid: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn docs_html() -> String {
    let mut h = String::from(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>watchtower API</title><style>\
         body{font:16px/1.5 system-ui,sans-serif;max-width:60rem;margin:0 auto;padding:16px;color:#1b1f24;background:#fff}\
         code{font-family:ui-monospace,Consolas,monospace}h3{margin:1.5rem 0 .25rem}\
         .m{display:inline-block;min-width:4rem;font-weight:700;text-transform:uppercase}\
         .muted{color:#667085}@media(prefers-color-scheme:dark){body{background:#14171c;color:#e6e9ee}.muted{color:#98a2b3}}\
         </style></head><body><h1>watchtower API</h1>",
    );
    h.push_str(&format!(
        "<p>Version {}. Every request needs the key in the <code>x-api-key</code> header \
         (create one with <code>watchtower api-key</code>). Machine-readable definition: \
         <a href=\"/api/openapi.json\">/api/openapi.json</a>.</p>",
        esc(env!("CARGO_PKG_VERSION"))
    ));
    for op in OPERATIONS {
        h.push_str(&format!(
            "<h3><span class=\"m\">{}</span> <code>{}</code></h3><p>{}</p>",
            esc(op.method),
            esc(op.path),
            esc(op.summary)
        ));
        if !op.params.is_empty() {
            h.push_str("<ul>");
            for p in op.params {
                h.push_str(&format!(
                    "<li><code>{}</code> <span class=\"muted\">({})</span> {}</li>",
                    esc(p.name),
                    esc(p.location),
                    esc(p.description)
                ));
            }
            h.push_str("</ul>");
        }
        if let Some(b) = op.body {
            h.push_str(&format!("<p class=\"muted\">Body: {}</p>", esc(b)));
        }
        h.push_str("<ul>");
        for (code, desc) in op.responses {
            h.push_str(&format!(
                "<li><code>{}</code> {}</li>",
                esc(code),
                esc(desc)
            ));
        }
        h.push_str("</ul>");
    }
    h.push_str("</body></html>");
    h
}

pub async fn docs_page() -> Response {
    let mut res = Html(docs_html()).into_response();
    // スクリプトは一切動かさない。スタイルだけ埋め込みで許す。
    res.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_document_is_accepted_by_utoipa() {
        if let Err(e) = spec_json() {
            panic!("OpenAPI document rejected: {e}");
        }
    }

    #[test]
    fn every_operation_has_a_unique_method_and_path() {
        let mut seen = std::collections::HashSet::new();
        for op in OPERATIONS {
            assert!(
                seen.insert((op.method, op.path)),
                "{} {}",
                op.method,
                op.path
            );
        }
    }
}
