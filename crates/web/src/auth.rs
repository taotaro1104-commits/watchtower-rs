//! パスワードハッシュ、セッション、ログイン試行の制限。

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::{Argon2, PasswordHasher, PasswordVerifier};

/// セッションを同時に保持する上限 (メモリ消費の上限)。
const MAX_SESSIONS: usize = 1000;
pub const MIN_PASSWORD_CHARS: usize = 8;

pub fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// ハッシュが壊れていても false を返す (ログインできないだけで落とさない)。
pub fn verify_password(password: &str, hash: &str) -> bool {
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

/// 推測されない 256 bit のトークン (16 進)。
pub fn random_token() -> String {
    rand::random::<[u8; 32]>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 長さに依存しない比較ではないが、トークン長は固定なので漏れるのは長さだけ。
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Debug, Clone)]
pub struct Session {
    pub csrf: String,
    /// 作成時のパスワードハッシュ。変更されたら無効にする。
    pub pw_fingerprint: String,
    expires: Instant,
}

#[derive(Default)]
pub struct Sessions(Mutex<HashMap<String, Session>>);

impl Sessions {
    /// 新しいセッションを作り、(セッション ID, CSRF トークン) を返す。
    pub fn create(&self, ttl: Duration, pw_fingerprint: &str) -> (String, String) {
        let (id, csrf) = (random_token(), random_token());
        let now = Instant::now();
        let mut map = self.0.lock().expect("sessions lock");
        map.retain(|_, s| s.expires > now);
        if map.len() >= MAX_SESSIONS {
            // 上限に達したら期限の近いものから捨てる。
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, s)| s.expires)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(
            id.clone(),
            Session {
                csrf: csrf.clone(),
                pw_fingerprint: pw_fingerprint.to_string(),
                expires: now + ttl,
            },
        );
        (id, csrf)
    }

    pub fn get(&self, id: &str) -> Option<Session> {
        let mut map = self.0.lock().expect("sessions lock");
        match map.get(id) {
            Some(s) if s.expires > Instant::now() => Some(s.clone()),
            Some(_) => {
                map.remove(id);
                None
            }
            None => None,
        }
    }

    pub fn remove(&self, id: &str) {
        self.0.lock().expect("sessions lock").remove(id);
    }
}

/// 追跡するクライアント数の上限 (メモリ消費の上限)。
const MAX_TRACKED_CLIENTS: usize = 10_000;

/// クライアント (IP) ごとに、一定時間内のログイン試行回数を数える。
/// 検証の前に試行を予約し、成功したら取り消す。並列に投げても予算を超えて検証できない。
pub struct LoginLimiter {
    attempts: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
    max: u32,
    window: Duration,
}

impl LoginLimiter {
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
            max,
            window,
        }
    }

    /// 試行を 1 回予約する。予算を使い切っていれば false (何も記録しない)。
    pub fn reserve(&self, key: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = self.attempts.lock().expect("limiter lock");
        map.retain(|_, q| {
            while q
                .front()
                .is_some_and(|t| now.duration_since(*t) >= self.window)
            {
                q.pop_front();
            }
            !q.is_empty()
        });
        if !map.contains_key(&key) && map.len() >= MAX_TRACKED_CLIENTS {
            return false;
        }
        let q = map.entry(key).or_default();
        if q.len() >= self.max as usize {
            return false;
        }
        q.push_back(now);
        true
    }

    /// ログインに成功したので、予約した試行を戻す。
    pub fn release(&self, key: IpAddr) {
        let mut map = self.attempts.lock().expect("limiter lock");
        if let Some(q) = map.get_mut(&key) {
            q.pop_back();
            if q.is_empty() {
                map.remove(&key);
            }
        }
    }
}
