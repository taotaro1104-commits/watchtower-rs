//! PDF のテキスト化 (pdf-extract)。
//!
//! 信頼できない入力なので、大きさを制限し、別スレッドで時間とパニックを守る。

use std::time::Duration;

use crate::sandbox::{self, SandboxError};
use crate::ProcessError;

/// 受け付ける PDF の大きさ。取得側の上限 (10MB) より大きめにしてある。
const MAX_PDF_BYTES: usize = 20 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);
/// 取り出すテキストの最大文字数。
const MAX_TEXT_CHARS: usize = 2_000_000;
/// `%PDF-` が現れるべき先頭からの範囲 (仕様上、先頭に少しゴミがあってもよい)。
const HEADER_WINDOW: usize = 1024;

fn fail(msg: &str) -> ProcessError {
    ProcessError::Pdf(msg.to_string())
}

pub fn extract_text(raw: &[u8]) -> Result<String, ProcessError> {
    if raw.len() > MAX_PDF_BYTES {
        return Err(fail("file is too large"));
    }
    let head = &raw[..raw.len().min(HEADER_WINDOW)];
    if !head.windows(5).any(|w| w == b"%PDF-") {
        return Err(fail("not a PDF file"));
    }
    let data = raw.to_vec();
    match sandbox::run(TIMEOUT, move || pdf_extract::extract_text_from_mem(&data)) {
        Ok(Ok(text)) => Ok(match text.char_indices().nth(MAX_TEXT_CHARS) {
            Some((end, _)) => text[..end].to_string(),
            None => text,
        }),
        Ok(Err(_)) => Err(fail("could not read the document")),
        Err(SandboxError::TimedOut) => Err(fail("reading the document timed out")),
        Err(SandboxError::Panicked) => Err(fail("the document could not be parsed")),
        Err(SandboxError::Busy) => Err(fail("earlier PDF reads are still running")),
    }
}
