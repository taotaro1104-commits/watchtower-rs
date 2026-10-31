use std::fmt;

use url::Url;

use crate::mail::MailTarget;
use crate::NotifyError;

/// 検証済みの通知先。`parse` を通ったものだけが存在する。
#[derive(Clone, PartialEq, Eq)]
pub struct NotifyTarget {
    pub(crate) kind: Kind,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    Json {
        url: Url,
    },
    Discord {
        id: String,
        token: String,
    },
    Slack {
        parts: [String; 3],
    },
    Telegram {
        bot_token: String,
        chat_id: String,
    },
    Ntfy {
        endpoint: Url,
        topic: String,
    },
    Gotify {
        endpoint: Url,
        token: String,
    },
    Mattermost {
        endpoint: Url,
        token: String,
    },
    Pushover {
        user: String,
        token: String,
    },
    Mail(MailTarget),
    /// 未実装のサービス。Apprise API サーバへそのまま転送する。
    Foreign {
        scheme: String,
        url: String,
    },
}

const MAX_FOREIGN_CHARS: usize = 2048;
const NTFY_DEFAULT_ENDPOINT: &str = "https://ntfy.sh/";

impl NotifyTarget {
    pub fn parse(input: &str) -> Result<Self, NotifyError> {
        let (scheme, rest) = input
            .trim()
            .split_once("://")
            .ok_or(NotifyError::InvalidUrl("missing scheme"))?;
        if scheme.is_empty() || !scheme.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(NotifyError::InvalidUrl("malformed scheme"));
        }
        let scheme = scheme.to_ascii_lowercase();
        let kind = match scheme.as_str() {
            "json" => Kind::Json {
                url: http_url("http", rest)?,
            },
            "jsons" => Kind::Json {
                url: http_url("https", rest)?,
            },
            "discord" => {
                let [id, token] = segments::<2>(rest)?;
                Kind::Discord { id, token }
            }
            "slack" => Kind::Slack {
                parts: segments::<3>(rest)?,
            },
            "tgram" => {
                let [bot_token, chat_id] = segments::<2>(rest)?;
                Kind::Telegram { bot_token, chat_id }
            }
            "mmost" | "mmosts" => {
                let (host, token) = host_and_one(rest)?;
                Kind::Mattermost {
                    endpoint: http_url(http_scheme(scheme == "mmosts"), &format!("{host}/"))?,
                    token,
                }
            }
            "pover" => pushover(rest)?,
            "mailto" | "mailtos" => Kind::Mail(MailTarget::parse(rest, scheme == "mailtos")?),
            "ntfy" | "ntfys" => ntfy(rest, scheme == "ntfys")?,
            "gotify" | "gotifys" => {
                let (host, token) = host_and_one(rest)?;
                Kind::Gotify {
                    endpoint: http_url(http_scheme(scheme == "gotifys"), &format!("{host}/"))?,
                    token,
                }
            }
            _ if crate::foreign::foreign_allowed() => foreign(&scheme, input.trim(), rest)?,
            _ => return Err(NotifyError::UnsupportedScheme(scheme)),
        };
        Ok(Self { kind })
    }

    pub fn scheme(&self) -> &'static str {
        match self.kind {
            Kind::Json { .. } => "json",
            Kind::Discord { .. } => "discord",
            Kind::Slack { .. } => "slack",
            Kind::Telegram { .. } => "tgram",
            Kind::Ntfy { .. } => "ntfy",
            Kind::Gotify { .. } => "gotify",
            Kind::Mattermost { .. } => "mmost",
            Kind::Pushover { .. } => "pover",
            Kind::Mail(_) => "mailto",
            Kind::Foreign { .. } => "foreign",
        }
    }

    pub fn is_foreign(&self) -> bool {
        matches!(self.kind, Kind::Foreign { .. })
    }

    /// Apprise API サーバへ渡す元の URL (トークンを含む)。
    pub(crate) fn foreign_url(&self) -> Option<&str> {
        match &self.kind {
            Kind::Foreign { url, .. } => Some(url),
            _ => None,
        }
    }

    pub fn is_mail(&self) -> bool {
        matches!(self.kind, Kind::Mail(_))
    }

    pub(crate) fn mail(&self) -> Option<&MailTarget> {
        match &self.kind {
            Kind::Mail(m) => Some(m),
            _ => None,
        }
    }
}

/// 転送先で解釈される宛先。ここでは形だけを確かめる (空白・制御文字・極端な長さは拒否)。
fn foreign(scheme: &str, url: &str, rest: &str) -> Result<Kind, NotifyError> {
    if rest.is_empty() {
        return Err(NotifyError::InvalidUrl("missing target"));
    }
    if url.chars().count() > MAX_FOREIGN_CHARS
        || url.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(NotifyError::InvalidUrl("malformed URL"));
    }
    Ok(Kind::Foreign {
        scheme: scheme.to_string(),
        url: url.to_string(),
    })
}

fn http_scheme(tls: bool) -> &'static str {
    if tls {
        "https"
    } else {
        "http"
    }
}

fn ntfy(rest: &str, tls: bool) -> Result<Kind, NotifyError> {
    let rest = rest.trim_end_matches('/');
    if !rest.contains('/') {
        return Ok(Kind::Ntfy {
            endpoint: Url::parse(NTFY_DEFAULT_ENDPOINT).expect("constant URL"),
            topic: token(rest)?,
        });
    }
    let (host, topic) = host_and_one(rest)?;
    Ok(Kind::Ntfy {
        endpoint: http_url(http_scheme(tls), &format!("{host}/"))?,
        topic,
    })
}

/// `user@token` (どちらも英数字のみ)。
fn pushover(rest: &str) -> Result<Kind, NotifyError> {
    let alnum = |s: &str| {
        if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric()) {
            Ok(s.to_string())
        } else {
            Err(NotifyError::InvalidUrl("invalid token characters"))
        }
    };
    let (user, token) = rest
        .split_once('@')
        .ok_or(NotifyError::InvalidUrl("missing token"))?;
    Ok(Kind::Pushover {
        user: alnum(user)?,
        token: alnum(token)?,
    })
}

/// `host[:port]/token` を分ける。
fn host_and_one(rest: &str) -> Result<(String, String), NotifyError> {
    let (host, tok) = rest
        .trim_end_matches('/')
        .split_once('/')
        .ok_or(NotifyError::InvalidUrl("missing path"))?;
    Ok((host.to_string(), token(tok)?))
}

fn http_url(scheme: &str, rest: &str) -> Result<Url, NotifyError> {
    if rest.is_empty() {
        return Err(NotifyError::InvalidUrl("missing host"));
    }
    let url = Url::parse(&format!("{scheme}://{rest}"))
        .map_err(|_| NotifyError::InvalidUrl("malformed URL"))?;
    if url.host_str().is_none_or(str::is_empty) {
        return Err(NotifyError::InvalidUrl("missing host"));
    }
    Ok(url)
}

/// `/` 区切りでちょうど N 個のトークンを取り出す。
fn segments<const N: usize>(rest: &str) -> Result<[String; N], NotifyError> {
    let parts: Vec<String> = rest
        .trim_end_matches('/')
        .split('/')
        .map(token)
        .collect::<Result<_, _>>()?;
    parts
        .try_into()
        .map_err(|_| NotifyError::InvalidUrl("wrong number of path parts"))
}

/// パス・ヘッダに埋め込む値。URL 構造を変えられる文字 (`/ . ? # 空白` など) は拒否する。
fn token(s: &str) -> Result<String, NotifyError> {
    let ok = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'@'));
    if ok {
        Ok(s.to_string())
    } else {
        Err(NotifyError::InvalidUrl("invalid token characters"))
    }
}

/// 資格情報を出さない表示 (ログ・UI 用)。
impl fmt::Display for NotifyTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            Kind::Json { url } => {
                let tls = if url.scheme() == "https" { "s" } else { "" };
                write!(f, "json{tls}://{}", url.host_str().unwrap_or_default())
            }
            Kind::Ntfy { endpoint, .. }
            | Kind::Gotify { endpoint, .. }
            | Kind::Mattermost { endpoint, .. } => write!(
                f,
                "{}://{}",
                self.scheme(),
                endpoint.host_str().unwrap_or_default()
            ),
            Kind::Mail(m) => write!(
                f,
                "mailto{}://{}",
                if m.scheme_is_tls() { "s" } else { "" },
                m.host
            ),
            Kind::Foreign { scheme, .. } => write!(f, "{scheme}://***"),
            _ => write!(f, "{}://***", self.scheme()),
        }
    }
}

impl fmt::Debug for NotifyTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NotifyTarget({self})")
    }
}
