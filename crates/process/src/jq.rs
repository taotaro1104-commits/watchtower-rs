//! jq (jaq) による JSON の抽出。
//!
//! 設定した人が書くプログラムだが、無限ループや環境変数の読み出しは拒否し、
//! 出力数・出力量・実行時間に上限を設ける。

use std::time::Duration;

use jaq_core::load::{Arena, File, Loader};
use jaq_core::{data, unwrap_valr, Compiler, Ctx, Vars};
use jaq_json::{read, Val};

use crate::sandbox::{self, SandboxError};
use crate::ProcessError;

/// 1 つのプログラムから取り出す出力の上限。
const MAX_OUTPUTS: usize = 10_000;
/// 出力全体の大きさの上限 (バイト)。
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(5);

/// 止まらない・環境や入力を読む可能性のある語。文字列リテラルの中は見ない。
const FORBIDDEN: &[&str] = &[
    "until",
    "while",
    "repeat",
    "env",
    "$ENV",
    "input",
    "inputs",
    "halt",
    "halt_error",
];

fn invalid(msg: impl Into<String>) -> ProcessError {
    ProcessError::InvalidJq(msg.into())
}

/// 文字列リテラルを除いた語を調べる。
fn reject_forbidden(program: &str) -> Result<(), ProcessError> {
    let mut word = String::new();
    let mut in_string = false;
    let mut escaped = false;
    let check = |w: &mut String| -> Result<(), ProcessError> {
        if FORBIDDEN.contains(&w.as_str()) {
            return Err(invalid(format!("`{w}` is not allowed")));
        }
        w.clear();
        Ok(())
    };
    for c in program.chars() {
        if in_string {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        if c == '"' {
            check(&mut word)?;
            in_string = true;
        } else if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
            word.push(c);
        } else {
            check(&mut word)?;
        }
    }
    check(&mut word)
}

/// 出力 1 件を文字列にする。文字列はそのまま、それ以外は jq の出力形式。
fn render(v: &Val) -> String {
    let s = v.to_string();
    serde_json::from_str::<String>(&s).unwrap_or(s)
}

fn evaluate(program: &str, body: &str) -> Result<Vec<String>, ProcessError> {
    let input = read::parse_single(body.as_bytes())
        .map_err(|e| ProcessError::InvalidJson(format!("{e:?}")))?;
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs());
    let funs = jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs());
    let arena = Arena::default();
    let modules = Loader::new(defs)
        .load(
            &arena,
            File {
                code: program,
                path: (),
            },
        )
        .map_err(|e| invalid(format!("{e:?}")))?;
    let filter = Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|e| invalid(format!("{e:?}")))?;
    let ctx = Ctx::<data::JustLut<Val>>::new(&filter.lut, Vars::new([]));

    let mut out = Vec::new();
    let mut bytes = 0;
    for item in filter
        .id
        .run((ctx, input))
        .map(unwrap_valr)
        .take(MAX_OUTPUTS)
    {
        let line = render(&item.map_err(|e| invalid(format!("{e}")))?);
        bytes += line.len() + 1;
        if bytes > MAX_OUTPUT_BYTES {
            return Err(invalid("output is too large"));
        }
        out.push(line);
    }
    Ok(out)
}

/// `program` を `body` (JSON 文字列) に適用し、出力を 1 件 1 要素で返す。
pub fn run(program: &str, body: &str) -> Result<Vec<String>, ProcessError> {
    run_with_timeout(program, body, TIMEOUT)
}

pub(crate) fn run_with_timeout(
    program: &str,
    body: &str,
    timeout: Duration,
) -> Result<Vec<String>, ProcessError> {
    if program.trim().is_empty() {
        return Err(invalid("empty program"));
    }
    reject_forbidden(program)?;
    let (program, body) = (program.to_string(), body.to_string());
    match sandbox::run(timeout, move || evaluate(&program, &body)) {
        Ok(result) => result,
        Err(SandboxError::TimedOut) => Err(invalid("evaluation timed out")),
        Err(SandboxError::Busy) => Err(invalid("earlier jq evaluations are still running")),
        Err(SandboxError::Panicked) => Err(invalid("evaluation failed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_program_times_out_instead_of_blocking() {
        let started = std::time::Instant::now();
        let r = run_with_timeout(
            "reduce range(0; 400000000) as $i (0; . + $i)",
            "null",
            Duration::from_millis(100),
        );
        assert!(
            matches!(r, Err(ProcessError::InvalidJq(ref m)) if m.contains("timed out")),
            "{r:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn forbidden_words_inside_strings_are_fine() {
        assert!(reject_forbidden(r#"select(.a == "while \"until\" env")"#).is_ok());
        assert!(reject_forbidden("until(.; .)").is_err());
        assert!(reject_forbidden(".a|env").is_err());
        assert!(reject_forbidden(".envelope | .environment").is_ok());
    }
}
