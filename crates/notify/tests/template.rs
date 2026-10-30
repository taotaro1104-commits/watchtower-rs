//! 通知テンプレート (minijinja、サンドボックス)。

use watchtower_notify::{NotificationContext, NotifyError, TemplateSet};

fn ctx() -> NotificationContext {
    NotificationContext {
        watch_url: "https://example.com/p".into(),
        watch_title: "Example".into(),
        diff: "- 10\n+ 20".into(),
        diff_added: "20".into(),
        diff_removed: "10".into(),
        current_snapshot: "price: 20".into(),
        preview_url: "http://localhost:5000/preview/1".into(),
    }
}

#[test]
fn default_templates_mention_title_url_and_diff() {
    let m = TemplateSet::default().render(&ctx()).unwrap();
    assert!(m.title.contains("Example"), "{}", m.title);
    assert!(m.body.contains("https://example.com/p"));
    assert!(m.body.contains("+ 20"));
}

#[test]
fn custom_templates_use_every_documented_variable() {
    let t = TemplateSet::new(
        "{{ watch_title }}",
        "{{ watch_url }}|{{ diff_added }}|{{ diff_removed }}|{{ current_snapshot }}|{{ preview_url }}",
    )
    .unwrap();
    let m = t.render(&ctx()).unwrap();
    assert_eq!(m.title, "Example");
    assert_eq!(
        m.body,
        "https://example.com/p|20|10|price: 20|http://localhost:5000/preview/1"
    );
}

#[test]
fn syntax_errors_are_caught_when_the_template_is_saved() {
    assert!(matches!(
        TemplateSet::new("{{ watch_title", "ok"),
        Err(NotifyError::Template(_))
    ));
    assert!(matches!(
        TemplateSet::new("ok", "{% for %}"),
        Err(NotifyError::Template(_))
    ));
}

#[test]
fn unknown_variables_are_rejected_at_save_time() {
    assert!(matches!(
        TemplateSet::new("{{ no_such_variable }}", "ok"),
        Err(NotifyError::Template(_))
    ));
}

#[test]
fn diff_content_is_not_interpreted_as_template_code() {
    let mut c = ctx();
    c.diff = "{{ 7 * 7 }} {% raw %}".into();
    let m = TemplateSet::new("t", "{{ diff }}")
        .unwrap()
        .render(&c)
        .unwrap();
    assert_eq!(m.body, "{{ 7 * 7 }} {% raw %}");
}

#[test]
fn runaway_templates_are_stopped() {
    let t = TemplateSet::new(
        "t",
        "{% for a in range(10000) %}{% for b in range(10000) %}x{% endfor %}{% endfor %}",
    );
    // 保存時の検証か描画時のどちらかで必ず止まる (ハングしない)。
    let stopped = match t {
        Err(_) => true,
        Ok(t) => t.render(&ctx()).is_err(),
    };
    assert!(stopped);
}

#[test]
fn huge_variables_are_truncated_so_notifications_still_go_out() {
    let mut c = ctx();
    c.diff = "x".repeat(5 * 1024 * 1024);
    let m = TemplateSet::default().render(&c).unwrap();
    assert!(m.body.len() < 400_000, "{}", m.body.len());
    assert!(m.body.ends_with('…'));
}

#[test]
fn output_amplification_is_rejected() {
    let mut c = ctx();
    c.current_snapshot = "x".repeat(100_000);
    let t = TemplateSet::new(
        "t",
        "{% for i in range(50) %}{{ current_snapshot }}{% endfor %}",
    );
    // 保存時 (サンプル値) は通り得るが、実データでの描画は上限エラーになる。
    assert!(t.unwrap().render(&c).is_err());
}
