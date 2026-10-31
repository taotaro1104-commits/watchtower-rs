//! ブラウザ取得で、ページを読み込んだ後に行う操作 (クリック・入力・待機)。
//!
//! 入力した文字列にはパスワードが入りうるので、`Debug` とエラーメッセージには出さない。

use std::fmt;

use serde::{Deserialize, Serialize};

pub const MAX_STEPS: usize = 20;
pub const MAX_SELECTOR_CHARS: usize = 500;
pub const MAX_TEXT_CHARS: usize = 1000;
pub const MAX_WAIT_MS: u64 = 30_000;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrowserStep {
    Click {
        selector: String,
    },
    Type {
        selector: String,
        text: String,
    },
    /// 要素が現れるまで待つ。`timeout_ms` が無ければ既定 (5 秒)。
    WaitFor {
        selector: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
    },
    Wait {
        ms: u64,
    },
}

impl fmt::Debug for BrowserStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Click { selector } => write!(f, "Click({selector:?})"),
            Self::Type { selector, .. } => write!(f, "Type({selector:?}, ***)"),
            Self::WaitFor { selector, .. } => write!(f, "WaitFor({selector:?})"),
            Self::Wait { ms } => write!(f, "Wait({ms})"),
        }
    }
}

impl BrowserStep {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Click { .. } => "click",
            Self::Type { .. } => "type",
            Self::WaitFor { .. } => "wait_for",
            Self::Wait { .. } => "wait",
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Click { selector } => check_selector(selector),
            Self::Type { selector, text } => {
                check_selector(selector)?;
                if text.chars().count() > MAX_TEXT_CHARS {
                    return Err("typed text is too long".into());
                }
                Ok(())
            }
            Self::WaitFor {
                selector,
                timeout_ms,
            } => {
                check_selector(selector)?;
                match timeout_ms {
                    Some(0) => Err("timeout must be positive".into()),
                    Some(t) if *t > MAX_WAIT_MS => Err("timeout is too long".into()),
                    _ => Ok(()),
                }
            }
            Self::Wait { ms } if *ms > MAX_WAIT_MS => Err("wait is too long".into()),
            Self::Wait { .. } => Ok(()),
        }
    }
}

fn check_selector(s: &str) -> Result<(), String> {
    if s.trim().is_empty() {
        return Err("selector is empty".into());
    }
    if s.chars().count() > MAX_SELECTOR_CHARS {
        return Err("selector is too long".into());
    }
    if s.chars().any(char::is_control) {
        return Err("selector has control characters".into());
    }
    Ok(())
}

pub fn validate_steps(steps: &[BrowserStep]) -> Result<(), String> {
    if steps.len() > MAX_STEPS {
        return Err(format!("too many steps (at most {MAX_STEPS})"));
    }
    for (i, s) in steps.iter().enumerate() {
        s.validate().map_err(|e| format!("step {}: {e}", i + 1))?;
    }
    Ok(())
}

/// 1 行 1 ステップのテキスト表現:
/// `click SEL` / `type SEL = TEXT` / `wait_for SEL [@ミリ秒]` / `wait ミリ秒`。
pub fn parse_steps_text(text: &str) -> Result<Vec<BrowserStep>, String> {
    let mut steps = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let step = parse_line(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        step.validate()
            .map_err(|e| format!("line {}: {e}", i + 1))?;
        steps.push(step);
    }
    validate_steps(&steps)?;
    Ok(steps)
}

fn parse_line(line: &str) -> Result<BrowserStep, String> {
    let (op, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let rest = rest.trim();
    match op {
        "click" if !rest.is_empty() => Ok(BrowserStep::Click {
            selector: rest.into(),
        }),
        "type" => {
            let (selector, text) = rest
                .split_once(" = ")
                .ok_or("type needs `SELECTOR = TEXT`")?;
            Ok(BrowserStep::Type {
                selector: selector.trim().into(),
                text: text.into(),
            })
        }
        "wait_for" if !rest.is_empty() => match rest.rsplit_once(" @") {
            Some((selector, ms)) => Ok(BrowserStep::WaitFor {
                selector: selector.trim().into(),
                timeout_ms: Some(ms.trim().parse().map_err(|_| "bad timeout")?),
            }),
            None => Ok(BrowserStep::WaitFor {
                selector: rest.into(),
                timeout_ms: None,
            }),
        },
        "wait" => Ok(BrowserStep::Wait {
            ms: rest.parse().map_err(|_| "wait needs milliseconds")?,
        }),
        "click" | "wait_for" => Err(format!("{op} needs a selector")),
        _ => Err("unknown step".into()),
    }
}

pub fn steps_to_text(steps: &[BrowserStep]) -> String {
    steps
        .iter()
        .map(|s| match s {
            BrowserStep::Click { selector } => format!("click {selector}"),
            BrowserStep::Type { selector, text } => format!("type {selector} = {text}"),
            BrowserStep::WaitFor {
                selector,
                timeout_ms: None,
            } => format!("wait_for {selector}"),
            BrowserStep::WaitFor {
                selector,
                timeout_ms: Some(t),
            } => format!("wait_for {selector} @{t}"),
            BrowserStep::Wait { ms } => format!("wait {ms}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
