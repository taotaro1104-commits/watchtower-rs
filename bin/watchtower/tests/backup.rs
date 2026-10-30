//! 実バイナリでバックアップと復元を確かめる。

use std::path::Path;
use std::process::Output;

use tokio::process::Command;

fn cmd(data: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_watchtower"));
    c.env("WATCHTOWER_DATA_DIR", data)
        .env("RUST_LOG", "warn")
        .kill_on_drop(true);
    c
}

async fn wt(data: &Path, args: &[&str]) -> Output {
    cmd(data).args(args).output().await.unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[tokio::test]
async fn backup_then_restore_into_a_fresh_data_dir_brings_the_watches_back() {
    let a = tempfile::tempdir().unwrap();
    let add = wt(
        a.path(),
        &[
            "watch",
            "add",
            "https://example.com/p",
            "--title",
            "Backed up",
        ],
    )
    .await;
    assert!(add.status.success(), "{}", err(&add));

    let zip = a.path().join("out.zip");
    let b = wt(a.path(), &["backup", zip.to_str().unwrap()]).await;
    assert!(b.status.success(), "{}", err(&b));
    assert!(out(&b).contains("backup"), "{}", out(&b));
    assert!(zip.exists());

    let fresh = tempfile::tempdir().unwrap();
    let r = wt(fresh.path(), &["restore", zip.to_str().unwrap()]).await;
    assert!(r.status.success(), "{}", err(&r));
    let list = wt(fresh.path(), &["watch", "list"]).await;
    assert!(out(&list).contains("Backed up"), "{}", out(&list));
}

#[tokio::test]
async fn backup_never_overwrites_an_existing_file() {
    let a = tempfile::tempdir().unwrap();
    let zip = a.path().join("out.zip");
    std::fs::write(&zip, b"keep me").unwrap();
    let b = wt(a.path(), &["backup", zip.to_str().unwrap()]).await;
    assert!(!b.status.success());
    assert_eq!(std::fs::read(&zip).unwrap(), b"keep me");
}

#[tokio::test]
async fn restore_refuses_to_replace_existing_data_without_force() {
    let a = tempfile::tempdir().unwrap();
    wt(
        a.path(),
        &["watch", "add", "https://example.com/from-backup"],
    )
    .await;
    let zip = a.path().join("out.zip");
    assert!(wt(a.path(), &["backup", zip.to_str().unwrap()])
        .await
        .status
        .success());

    let target = tempfile::tempdir().unwrap();
    wt(
        target.path(),
        &["watch", "add", "https://example.com/existing"],
    )
    .await;

    let refused = wt(target.path(), &["restore", zip.to_str().unwrap()]).await;
    assert!(!refused.status.success());
    assert!(err(&refused).contains("--force"), "{}", err(&refused));
    assert!(out(&wt(target.path(), &["watch", "list"]).await).contains("existing"));

    let forced = wt(
        target.path(),
        &["restore", zip.to_str().unwrap(), "--force"],
    )
    .await;
    assert!(forced.status.success(), "{}", err(&forced));
    let list = out(&wt(target.path(), &["watch", "list"]).await);
    assert!(
        list.contains("from-backup") && !list.contains("existing"),
        "{list}"
    );
}

#[tokio::test]
async fn restoring_a_missing_or_bogus_archive_fails_cleanly() {
    let d = tempfile::tempdir().unwrap();
    let missing = wt(d.path(), &["restore", "no-such.zip"]).await;
    assert!(!missing.status.success());
    let junk = d.path().join("junk.zip");
    std::fs::write(&junk, b"not a zip").unwrap();
    let bogus = wt(d.path(), &["restore", junk.to_str().unwrap()]).await;
    assert!(!bogus.status.success());
}
