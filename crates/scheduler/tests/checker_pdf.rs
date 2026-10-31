//! チェッカーが PDF (生のバイト列) を文字列と同じように扱い、変更を検知すること。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use url::Url;
use watchtower_core::Watch;
use watchtower_fetch::{FetchError, FetchRequest, FetchResponse, Fetcher};
use watchtower_scheduler::{CheckOutcome, CheckRunner, CheckerConfig, StoreCheckRunner};
use watchtower_store::Store;

/// 呼ばれるたびに、用意した PDF を順に返す。
struct PdfFetcher(Mutex<VecDeque<Vec<u8>>>);

#[async_trait]
impl Fetcher for PdfFetcher {
    async fn fetch(
        &self,
        req: &FetchRequest,
        _c: CancellationToken,
    ) -> Result<FetchResponse, FetchError> {
        let raw = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .expect("script exhausted");
        Ok(FetchResponse {
            status: 200,
            headers: http::HeaderMap::new(),
            content_type: Some("application/pdf".into()),
            body: String::new(),
            raw: Some(Bytes::from(raw)),
            screenshot: None,
            final_url: req.url.clone(),
        })
    }
}

fn pdf(lines: &[&str]) -> Vec<u8> {
    let mut content = String::from("BT /F1 12 Tf 72 720 Td 14 TL\n");
    for l in lines {
        content.push_str(&format!("({l}) Tj T*\n"));
    }
    content.push_str("ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R \
         /Resources << /Font << /F1 4 0 R >> >> >>"
            .to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
    }
    let xref_at = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for o in &offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    out
}

struct Fixture {
    runner: StoreCheckRunner,
    rx: mpsc::Receiver<CheckOutcome>,
    store: Store,
    id: uuid::Uuid,
    _dir: tempfile::TempDir,
}

async fn fixture(docs: Vec<Vec<u8>>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("t.db"), dir.path().join("snaps"))
        .await
        .unwrap();
    let w = Watch::new(Url::parse("https://example.com/catalog.pdf").unwrap());
    store.add_watch(&w).await.unwrap();
    let (tx, rx) = mpsc::channel(16);
    let cfg = CheckerConfig {
        jitter_ratio: 0.0,
        ..Default::default()
    };
    let fetcher = Arc::new(PdfFetcher(Mutex::new(docs.into())));
    Fixture {
        runner: StoreCheckRunner::new(store.clone(), fetcher, cfg, Some(tx)),
        rx,
        store,
        id: w.id,
        _dir: dir,
    }
}

async fn check(f: &mut Fixture) -> CheckOutcome {
    f.runner.run(f.id, CancellationToken::new()).await;
    f.rx.recv().await.unwrap()
}

#[tokio::test]
async fn a_pdf_gets_a_baseline_then_a_change_with_a_diff() {
    let mut f = fixture(vec![pdf(&["price 100"]), pdf(&["price 80"])]).await;
    let first = check(&mut f).await;
    assert!(first.first_snapshot && first.error.is_none(), "{first:?}");

    let second = check(&mut f).await;
    assert!(second.changed, "{second:?}");
    let diff = second.diff.unwrap();
    assert!(
        diff.added.iter().any(|l| l.contains("price 80")),
        "{diff:?}"
    );
    assert!(
        diff.removed.iter().any(|l| l.contains("price 100")),
        "{diff:?}"
    );
}

#[tokio::test]
async fn an_unchanged_pdf_is_not_a_change() {
    let mut f = fixture(vec![pdf(&["same"]), pdf(&["same"])]).await;
    check(&mut f).await;
    let again = check(&mut f).await;
    assert!(
        !again.changed && !again.first_snapshot && again.error.is_none(),
        "{again:?}"
    );
}

#[tokio::test]
async fn the_stored_snapshot_is_the_extracted_text_not_the_binary() {
    let mut f = fixture(vec![pdf(&["readable text"])]).await;
    check(&mut f).await;
    let text = f.store.latest_snapshot_text(f.id).await.unwrap().unwrap();
    assert!(text.contains("readable text"), "{text:?}");
    assert!(!text.contains("%PDF"), "{text:?}");
}

#[tokio::test]
async fn an_unreadable_pdf_is_reported_as_a_check_error() {
    let mut f = fixture(vec![b"%PDF-1.4 truncated".to_vec()]).await;
    let out = check(&mut f).await;
    let err = out.error.expect("error expected");
    assert!(err.contains("pdf"), "{err}");
    let w = f.store.get_watch(f.id).await.unwrap().unwrap();
    assert_eq!(w.state.consecutive_errors, 1);
}
