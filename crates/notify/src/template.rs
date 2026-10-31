use minijinja::{Environment, UndefinedBehavior, Value};

use crate::{Message, NotifyError};

/// 1 回の描画で許す命令数。無限ループ / 巨大な range を止める。
const FUEL: u64 = 200_000;
/// 描画結果の上限。
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// テンプレートへ渡す 1 変数あたりの上限 (文字数)。巨大ページの差分でも通知が失敗しないようにする。
const MAX_VARIABLE_CHARS: usize = 100_000;

const DEFAULT_TITLE: &str = "Change detected: {{ watch_title }}";
const DEFAULT_BODY: &str = "{{ watch_url }}\n\n{{ diff }}";

/// テンプレートから参照できる変数 (設計書の一覧と同じ)。
#[derive(Debug, Clone)]
pub struct NotificationContext {
    pub watch_url: String,
    pub watch_title: String,
    pub diff: String,
    pub diff_added: String,
    pub diff_removed: String,
    pub current_snapshot: String,
    pub preview_url: String,
}

impl NotificationContext {
    fn to_value(&self) -> Value {
        let cap = |v: &str| -> String {
            match v.char_indices().nth(MAX_VARIABLE_CHARS) {
                Some((end, _)) => format!("{}…", &v[..end]),
                None => v.to_string(),
            }
        };
        Value::from_pairs([
            ("watch_url", cap(&self.watch_url)),
            ("watch_title", cap(&self.watch_title)),
            ("diff", cap(&self.diff)),
            ("diff_added", cap(&self.diff_added)),
            ("diff_removed", cap(&self.diff_removed)),
            ("current_snapshot", cap(&self.current_snapshot)),
            ("preview_url", cap(&self.preview_url)),
        ])
    }

    fn sample() -> Self {
        let s = |v: &str| v.to_string();
        Self {
            watch_url: s("https://example.com/"),
            watch_title: s("Example"),
            diff: s("- old\n+ new"),
            diff_added: s("new"),
            diff_removed: s("old"),
            current_snapshot: s("new"),
            preview_url: s("http://localhost/preview/0"),
        }
    }
}

/// 検証済みのテンプレート (件名 + 本文)。
#[derive(Debug, Clone)]
pub struct TemplateSet {
    title: String,
    body: String,
}

impl Default for TemplateSet {
    fn default() -> Self {
        Self {
            title: DEFAULT_TITLE.into(),
            body: DEFAULT_BODY.into(),
        }
    }
}

impl TemplateSet {
    /// 保存時の検証: 構文と未定義変数をサンプル値で描画して確かめる。
    pub fn new(title: &str, body: &str) -> Result<Self, NotifyError> {
        let set = Self {
            title: title.into(),
            body: body.into(),
        };
        set.render(&NotificationContext::sample())?;
        Ok(set)
    }

    /// 件名と本文の片方 (または両方) だけを差し替える。未指定の側は既定の文面。保存時の検証にも使う。
    pub fn with_overrides(title: Option<&str>, body: Option<&str>) -> Result<Self, NotifyError> {
        Self::new(title.unwrap_or(DEFAULT_TITLE), body.unwrap_or(DEFAULT_BODY))
    }

    pub fn render(&self, ctx: &NotificationContext) -> Result<Message, NotifyError> {
        Ok(Message {
            title: render_one(&self.title, ctx)?,
            body: render_one(&self.body, ctx)?,
        })
    }
}

fn render_one(source: &str, ctx: &NotificationContext) -> Result<String, NotifyError> {
    // 関数・フィルタは minijinja 標準のみ (ファイルや外部への手段は無い)。
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_fuel(Some(FUEL));
    let out = env
        .render_str(source, ctx.to_value())
        .map_err(|e| NotifyError::Template(e.to_string()))?;
    if out.len() > MAX_OUTPUT_BYTES {
        return Err(NotifyError::Template("rendered output too large".into()));
    }
    Ok(out)
}
