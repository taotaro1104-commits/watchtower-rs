//! 信頼できない入力を扱うパーサ (PDF) や、利用者が書いたプログラム (jq) を安全に走らせる。
//!
//! 専用スレッドで走らせ、パニックは捕まえ、時間切れなら待つのをやめる。
//! 止められないスレッドは放置されるので、放置が溜まったら新しく始めない。

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// 時間切れで放置されたまま、まだ動いている評価スレッドの数。
static ABANDONED: AtomicUsize = AtomicUsize::new(0);
/// 放置を許す数。これに達したら新しい実行を断る。
const MAX_ABANDONED: usize = 2;
/// 再帰の深いパーサでもあふれにくいよう、スタックを広めに取る。
const STACK_BYTES: usize = 16 * 1024 * 1024;

const RUNNING: u8 = 0;
const GAVE_UP: u8 = 1;
const FINISHED: u8 = 2;

#[derive(Debug, PartialEq, Eq)]
pub enum SandboxError {
    TimedOut,
    Panicked,
    /// 放置されたスレッドが多すぎて、新しく始められない。
    Busy,
}

/// `f` を別スレッドで実行し、`timeout` まで待つ。
pub fn run<T, F>(timeout: Duration, f: F) -> Result<T, SandboxError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    if ABANDONED.load(Ordering::SeqCst) >= MAX_ABANDONED {
        return Err(SandboxError::Busy);
    }
    let (tx, rx) = mpsc::channel();
    let state = Arc::new(AtomicU8::new(RUNNING));
    let thread_state = Arc::clone(&state);
    std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(f)).map_err(|_| SandboxError::Panicked);
            // 見放されたあとに終わった場合だけ、放置数を戻す
            if thread_state.swap(FINISHED, Ordering::SeqCst) == GAVE_UP {
                ABANDONED.fetch_sub(1, Ordering::SeqCst);
            }
            let _ = tx.send(result);
        })
        .map_err(|_| SandboxError::Busy)?;

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            let gave_up = state
                .compare_exchange(RUNNING, GAVE_UP, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok();
            if gave_up {
                ABANDONED.fetch_add(1, Ordering::SeqCst);
                Err(SandboxError::TimedOut)
            } else {
                // 時間切れの直前に終わっていた
                rx.recv().unwrap_or(Err(SandboxError::Panicked))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_normal_result_is_returned() {
        assert_eq!(run(Duration::from_secs(5), || 6 * 7), Ok(42));
    }

    #[test]
    fn a_panic_becomes_an_error_and_the_caller_survives() {
        let r: Result<(), _> = run(Duration::from_secs(5), || panic!("boom"));
        assert_eq!(r, Err(SandboxError::Panicked));
    }

    #[test]
    fn a_slow_job_times_out_and_too_many_of_them_are_refused() {
        // 放置数はプロセス全体で共有されるので、同じテストの中でまとめて確かめる
        let sleepy = || std::thread::sleep(Duration::from_millis(1500));
        let a = run(Duration::from_millis(20), sleepy);
        let b = run(Duration::from_millis(20), sleepy);
        assert!(matches!(
            a,
            Err(SandboxError::TimedOut) | Err(SandboxError::Busy)
        ));
        assert!(matches!(
            b,
            Err(SandboxError::TimedOut) | Err(SandboxError::Busy)
        ));
        let c = run(Duration::from_millis(20), sleepy);
        assert!(matches!(
            c,
            Err(SandboxError::Busy) | Err(SandboxError::TimedOut)
        ));
        // 放置されたスレッドが終われば、また使える
        std::thread::sleep(Duration::from_millis(2000));
        assert_eq!(run(Duration::from_secs(5), || 1), Ok(1));
    }
}
