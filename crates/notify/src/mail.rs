//! メール宛先 (`mailto://` / `mailtos://`) の解釈。
//!
//! 形式: `mailto://[user[:password]@]host[:port]/?to=a@x,b@y[&from=w@x][&security=starttls|tls|none]`
//! ユーザー名・パスワードは URL エンコードする (`@` は `%40`)。

use std::str::FromStr;

use lettre::Address;
use percent_encoding::percent_decode_str;
use url::Url;

use crate::NotifyError;

const MAX_RECIPIENTS: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Security {
    None,
    StartTls,
    Tls,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct MailTarget {
    pub host: String,
    pub port: u16,
    pub security: Security,
    pub credentials: Option<(String, String)>,
    pub from: Address,
    pub to: Vec<Address>,
}

fn invalid(why: &'static str) -> NotifyError {
    NotifyError::InvalidUrl(why)
}

fn decode(s: &str) -> Result<String, NotifyError> {
    percent_decode_str(s)
        .decode_utf8()
        .map(|c| c.into_owned())
        .map_err(|_| invalid("invalid percent-encoding"))
}

fn address(s: &str) -> Result<Address, NotifyError> {
    Address::from_str(s.trim()).map_err(|_| invalid("invalid email address"))
}

impl MailTarget {
    /// `implicit_tls` は `mailtos://` のとき true。
    pub(crate) fn parse(rest: &str, implicit_tls: bool) -> Result<Self, NotifyError> {
        if rest.is_empty() {
            return Err(invalid("missing host"));
        }
        let url = Url::parse(&format!("smtp://{rest}")).map_err(|_| invalid("malformed URL"))?;
        let host = url
            .host_str()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| invalid("missing host"))?
            .trim_matches(['[', ']'])
            .to_string();

        let mut to_raw = None;
        let mut from_raw = None;
        let mut security_raw = None;
        for (k, v) in url.query_pairs() {
            match k.as_ref() {
                "to" => to_raw = Some(v.into_owned()),
                "from" => from_raw = Some(v.into_owned()),
                "security" => security_raw = Some(v.into_owned()),
                _ => {}
            }
        }

        let security = match security_raw.as_deref() {
            None if implicit_tls => Security::Tls,
            None => Security::StartTls,
            Some("none") => Security::None,
            Some("starttls") => Security::StartTls,
            Some("tls") => Security::Tls,
            Some(_) => return Err(invalid("unknown security mode")),
        };
        let port = url.port().unwrap_or(match security {
            Security::None => 25,
            Security::StartTls => 587,
            Security::Tls => 465,
        });

        let user = decode(url.username())?;
        let credentials = (!user.is_empty()).then(|| {
            let pass = url.password().map(decode).transpose();
            pass.map(|p| (user.clone(), p.unwrap_or_default()))
        });
        let credentials = credentials.transpose()?;
        if credentials.is_some() && security == Security::None {
            // 平文の接続で認証情報を送らない
            return Err(invalid("credentials require TLS"));
        }

        let to: Vec<Address> = to_raw
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(address)
            .collect::<Result<_, _>>()?;
        if to.is_empty() {
            return Err(invalid("missing recipient"));
        }
        if to.len() > MAX_RECIPIENTS {
            return Err(invalid("too many recipients"));
        }

        // 差出人は from=、なければメールアドレス形式のユーザー名
        let from = match (&from_raw, &credentials) {
            (Some(f), _) => address(f)?,
            (None, Some((u, _))) if u.contains('@') => address(u)?,
            _ => return Err(invalid("missing sender (from=)")),
        };

        Ok(Self {
            host,
            port,
            security,
            credentials,
            from,
            to,
        })
    }

    pub(crate) fn scheme_is_tls(&self) -> bool {
        self.security == Security::Tls
    }
}
