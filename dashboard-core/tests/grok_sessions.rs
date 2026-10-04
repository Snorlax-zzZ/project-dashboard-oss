// grok 会话来源的 fixture 测试。形态取自 2026-09-09 实测的真实会话：
// `~/.grok/sessions/<URL 编码 cwd>/<uuid>/`，元数据在 summary.json、正文在 chat_history.jsonl，
// user 行把真实提问包在 <user_query> 里（块内其余是注入的环境/规则），assistant 行 content 是纯字符串。
use dashboard_core::commands;
use dashboard_core::sessions::{grok_detail, scan_grok_at, GrokSessionSource, SessionSource};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SID: &str = "00000000-0000-4000-8000-000000000101";
const CWD_SLUG: &str = "%2FUsers%2Ftest%2FDocuments";

fn make_tempdir(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "project-dashboard-grok-sessions-{label}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_session(
    root: &Path,
    sid: &str,
    cwd: &str,
    title: &str,
    updated_at: &str,
    history: &[String],
) -> PathBuf {
    let dir = root.join(CWD_SLUG).join(sid);
    fs::create_dir_all(&dir).unwrap();
    let summary = serde_json::json!({
        "info": { "id": sid, "cwd": cwd },
        "session_summary": title,
        "generated_title": title,
        "created_at": "2026-09-07T03:22:42.864490Z",
        "updated_at": updated_at,
        "num_chat_messages": history.len(),
        "current_model_id": "grok-4.6",
    });
    fs::write(dir.join("summary.json"), summary.to_string()).unwrap();
    let mut body = history.join("\n");
    body.push('\n');
    fs::write(dir.join("chat_history.jsonl"), body).unwrap();
    dir
}

fn user_line(injected: &str, query: &str) -> String {
    serde_json::json!({
        "type": "user",
        "content": [{
            "type": "text",
            "text": format!("{injected}\n\n<user_query>\n{query}\n</user_query>"),
        }],
    })
    .to_string()
}

fn assistant_line(text: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "content": text,
        "model_id": "grok-4.6",
        "model_fingerprint": "fp_test",
    })
    .to_string()
}

#[test]
fn grok_scan_reads_summary_metadata_and_strips_injected_context() {
    let tmp = make_tempdir("scan");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    write_session(
        &root,
        SID,
        "/Users/test/Documents/project-dashboard",
        "会话记录加 Grok 来源页签",
        "2026-09-08T03:23:48.982261Z",
        &[
            serde_json::json!({"type": "system", "content": "You are Grok 4.5 released by xAI."})
                .to_string(),
            user_line(
                "<user_info>\nOS Version: macos\n</user_info>\n\n<rules>注入的用户规则正文</rules>",
                "帮我加两个来源页签",
            ),
            "this is not json".to_string(),
            assistant_line("用户，我先看一遍现有实现。"),
            serde_json::json!({"type": "tool_result", "tool_call_id": "call-1", "content": "工具回显不是对话"})
                .to_string(),
            serde_json::json!({"type": "reasoning", "content": serde_json::Value::Null}).to_string(),
        ],
    );
    // 顶层的 session_search.sqlite 与会话目录同级的散文件都必须忽略
    fs::write(root.join("session_search.sqlite"), "not a session").unwrap();
    fs::write(root.join(CWD_SLUG).join("prompt_history.jsonl"), "{}\n").unwrap();

    let metas = scan_grok_at(&root, &cache);
    assert_eq!(metas.len(), 1);
    let meta = &metas[0];
    assert_eq!(meta.session_id, SID);
    assert_eq!(meta.source, "grok");
    assert_eq!(meta.cwd, "/Users/test/Documents/project-dashboard");
    assert_eq!(meta.title, "会话记录加 Grok 来源页签", "标题直接用 session_summary");
    assert_eq!(meta.projects, "project-dashboard");
    assert!(meta.haystack.contains("帮我加两个来源页签"));
    assert!(meta.haystack.contains("用户，我先看一遍现有实现"));
    assert!(
        !meta.haystack.contains("注入的用户规则正文"),
        "<user_query> 之外的注入上下文不得进 haystack"
    );
    assert!(
        !meta.haystack.contains("工具回显不是对话"),
        "tool_result 不是对话"
    );
    assert!(
        !meta.haystack.contains("You are Grok"),
        "system 提示不是对话"
    );

    // mtime 取自 summary.updated_at，不是文件 mtime（目录里的 .lock 旁路文件会扰动后者）
    let expected = chrono::DateTime::parse_from_rfc3339("2026-09-08T03:23:48.982261Z")
        .unwrap()
        .timestamp() as f64;
    assert!((meta.mtime - expected).abs() < 1.0);

    let source = GrokSessionSource {
        root: root.clone(),
        cache: cache.clone(),
    };
    assert_eq!(source.source_id(), "grok");
    assert_eq!(source.scan().len(), 1);
    assert_eq!(source.find_by_id(SID).unwrap().session_id, SID);
    assert!(source.find_by_id("not-a-uuid").is_none());

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn grok_scan_skips_dirs_without_summary_and_bad_ids() {
    let tmp = make_tempdir("skip");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    // 只有 chat_history 没有 summary.json：拿不到 id / 标题，跳过
    let orphan = root.join(CWD_SLUG).join("00000000-0000-4000-8000-000000000103");
    fs::create_dir_all(&orphan).unwrap();
    fs::write(orphan.join("chat_history.jsonl"), "{}\n").unwrap();
    // summary.json 里的 id 不是 UUID：跳过（避免拼出一条无法 resume 的记录）
    let bad = root.join(CWD_SLUG).join("bad-id");
    fs::create_dir_all(&bad).unwrap();
    fs::write(
        bad.join("summary.json"),
        serde_json::json!({"info": {"id": "not-a-uuid", "cwd": "/Users/test"}}).to_string(),
    )
    .unwrap();

    assert!(scan_grok_at(&root, &cache).is_empty());

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn grok_cache_hit_reuses_entry_until_updated_at_moves() {
    let tmp = make_tempdir("cache");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let history = vec![user_line("<user_info>x</user_info>", "第一轮提问")];
    write_session(
        &root,
        SID,
        "/Users/test/Documents/project-dashboard",
        "初始标题",
        "2026-09-08T03:23:48.982261Z",
        &history,
    );
    assert_eq!(scan_grok_at(&root, &cache)[0].title, "初始标题");

    // 缓存命中（updated_at + chat_history 大小都没变）：改缓存内容应被原样复用
    let cache_file = cache.join(format!("{SID}.json"));
    let mut entry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cache_file).unwrap()).unwrap();
    entry["meta"]["title"] = serde_json::Value::String("缓存里的标题".into());
    fs::write(&cache_file, entry.to_string()).unwrap();
    assert_eq!(scan_grok_at(&root, &cache)[0].title, "缓存里的标题");

    // updated_at 前进 → 缓存失效，重读 summary.json
    write_session(
        &root,
        SID,
        "/Users/test/Documents/project-dashboard",
        "改过的标题",
        "2026-09-09T10:00:00.000000Z",
        &history,
    );
    assert_eq!(
        scan_grok_at(&root, &cache)[0].title,
        "改过的标题",
        "updated_at 变了必须重解析"
    );

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn grok_detail_keeps_last_rounds_and_reports_no_files() {
    let tmp = make_tempdir("detail");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let mut history = Vec::new();
    for round in 1..=7 {
        history.push(user_line("<user_info>x</user_info>", &format!("问题 {round}")));
        history.push(assistant_line(&format!("回答 {round}")));
    }
    write_session(
        &root,
        SID,
        "/Users/test/Documents/project-dashboard",
        "多轮会话",
        "2026-09-08T03:23:48.982261Z",
        &history,
    );

    let meta = scan_grok_at(&root, &cache).remove(0);
    let detail = grok_detail(&meta, 5);
    assert_eq!(
        detail.preview.iter().filter(|line| line.starts_with('🧑')).count(),
        5,
        "rounds=5 只保留最后 5 个 user 轮次"
    );
    assert!(detail.preview.first().unwrap().contains("问题 3"));
    assert!(detail.preview.last().unwrap().contains("回答 7"));
    assert!(
        detail.files.is_empty(),
        "chat_history 不含工具入参，改动文件恒为空——不能凭空造路径"
    );

    let _ = fs::remove_dir_all(tmp);
}

/// 同 pi：被篡改 `meta.path` 的缓存不得把详情读取引向来源根目录之外。
#[test]
fn grok_tampered_cache_path_cannot_escape_source_root() {
    let tmp = make_tempdir("path-escape");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    write_session(
        &root,
        SID,
        "/Users/test/Documents/project-dashboard",
        "正常会话",
        "2026-09-08T03:23:48.982261Z",
        &[user_line("<user_info>x</user_info>", "正常内容")],
    );
    assert_eq!(scan_grok_at(&root, &cache).len(), 1);

    let outside = tmp.join("outside.jsonl");
    fs::write(
        &outside,
        format!("{}\n", user_line("<user_info>x</user_info>", "OUTSIDE_SENTINEL")),
    )
    .unwrap();

    let cache_file = cache.join(format!("{SID}.json"));
    let mut entry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cache_file).unwrap()).unwrap();
    entry["meta"]["path"] = serde_json::Value::String(outside.to_string_lossy().to_string());
    fs::write(&cache_file, entry.to_string()).unwrap();

    let source = GrokSessionSource {
        root: root.clone(),
        cache: cache.clone(),
    };
    let meta = source.find_by_id(SID).expect("会话仍应被扫描到");
    assert!(
        !meta.path.contains("outside.jsonl"),
        "缓存里越界的 path 必须被丢弃并重新扫描，实际拿到 {}",
        meta.path
    );
    let preview = grok_detail(&meta, 5).preview.join(" ");
    assert!(
        !preview.contains("OUTSIDE_SENTINEL"),
        "详情不得读取来源根目录之外的文件"
    );

    let _ = fs::remove_dir_all(tmp);
}

/// `updated_at` 只差 1 微秒时缓存也必须失效。
/// mtime 是 f64 秒，在 1.7e9 量级单个 ULP 已≈2.4e-7 秒，1μs 的差在 f64 里只剩 9.5e-7，
/// 会被 1e-6 容差整个吃掉——所以判据必须落在 updated_at 原文上，不能只看换算后的 mtime。
#[test]
fn grok_cache_invalidates_on_sub_microsecond_updated_at_change() {
    let tmp = make_tempdir("microsecond");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    // 两版正文字节数必须完全相同，把 size 这条信号也堵死，只剩 updated_at 能救
    let before = vec![user_line("<user_info>x</user_info>", "AAA")];
    let after = vec![user_line("<user_info>x</user_info>", "BBB")];
    assert_eq!(before[0].len(), after[0].len(), "前置条件：两版正文必须等长");

    write_session(
        &root,
        SID,
        "/Users/test/Documents",
        "标题",
        "2026-09-08T03:23:48.000000Z",
        &before,
    );
    assert!(scan_grok_at(&root, &cache)[0].haystack.contains("AAA"));

    write_session(
        &root,
        SID,
        "/Users/test/Documents",
        "标题",
        "2026-09-08T03:23:48.000001Z",
        &after,
    );
    assert!(
        scan_grok_at(&root, &cache)[0].haystack.contains("BBB"),
        "updated_at 只差 1 微秒也必须让缓存失效"
    );

    let _ = fs::remove_dir_all(tmp);
}

/// `session_summary` 是 null / 空串时必须回退到 `generated_title`。
/// 在 Value 层写 `.get(a).or_else(.get(b))` 会踩坑：null 也是 Some，b 永远轮不到。
#[test]
fn grok_null_session_summary_falls_back_to_generated_title() {
    let tmp = make_tempdir("null-title");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    for (index, (label, summary_value)) in [
        ("null", serde_json::Value::Null),
        ("empty", serde_json::Value::String(String::new())),
        ("blank", serde_json::Value::String("   ".into())),
        ("非字符串", serde_json::Value::from(42)),
    ]
    .into_iter()
    .enumerate()
    {
        let dir = root.join(CWD_SLUG).join(SID);
        fs::create_dir_all(&dir).unwrap();
        // updated_at 必须逐轮唯一：早先版本用 label.len() 生成，而 "empty"/"blank" 长度都是 5，
        // 第二轮直接命中上一轮缓存、根本没解析到 summary，是个假阳性。
        fs::write(
            dir.join("summary.json"),
            serde_json::json!({
                "info": { "id": SID, "cwd": "/Users/test/Documents" },
                "session_summary": summary_value,
                // 每轮换值：命中上一轮缓存的话拿到的会是上一轮的标题，断言随即失败
                "generated_title": format!("GENERATED-{index}"),
                "updated_at": format!("2026-09-08T03:23:48.00000{index}Z"),
            })
            .to_string(),
        )
        .unwrap();
        // 正文里也没有可用的 user 行，逼标题必须走 generated_title
        fs::write(dir.join("chat_history.jsonl"), "{\"type\":\"system\",\"content\":\"x\"}\n")
            .unwrap();

        assert_eq!(
            scan_grok_at(&root, &cache)[0].title,
            format!("GENERATED-{index}"),
            "session_summary 为 {label} 时必须回退到本轮的 generated_title（拿到旧值 = 误命中缓存）"
        );
    }

    let _ = fs::remove_dir_all(tmp);
}

/// 提问正文里出现字面 `</user_query>` 时（比如问「这个标签干嘛用的」），
/// 必须按最外层闭合标签收尾，否则后半句整段丢失。
#[test]
fn grok_user_query_with_literal_tag_inside_is_not_truncated() {
    let tmp = make_tempdir("literal-tag");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    write_session(
        &root,
        SID,
        "/Users/test/Documents",
        "",
        "2026-09-08T03:23:48.982261Z",
        &[user_line(
            "<user_info>x</user_info>",
            "REAL-BEFORE <user_query>SAMPLE</user_query> REAL-AFTER",
        )],
    );

    let haystack = scan_grok_at(&root, &cache).remove(0).haystack;
    assert!(haystack.contains("REAL-BEFORE"), "前半句必须在");
    assert!(
        haystack.contains("REAL-AFTER"),
        "内嵌字面闭合标签之后的正文不得丢失，实际 haystack={haystack}"
    );

    let _ = fs::remove_dir_all(tmp);
}

/// 同 pi：grok 的会话目录本身是 symlink 时不得被收进来。
/// grok 是「一个会话一个目录」，所以逃逸面是目录级的——整份 summary.json + 正文都可以
/// 来自根目录之外。实测过：加围栏之前扫描会返回 1 条，标题和正文都是外部文件的内容。
#[cfg(unix)]
#[test]
fn grok_scan_does_not_follow_symlinked_session_dirs() {
    let tmp = make_tempdir("symlink-scan");
    let root = tmp.join("sessions");
    let slug = root.join(CWD_SLUG);
    fs::create_dir_all(&slug).unwrap();

    let outside = tmp.join("outside-session");
    fs::create_dir_all(&outside).unwrap();
    fs::write(
        outside.join("summary.json"),
        serde_json::json!({
            "info": { "id": SID, "cwd": "/secret" },
            "session_summary": "OUTSIDE_SENTINEL",
            "updated_at": "2026-09-08T03:23:48.982261Z",
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        outside.join("chat_history.jsonl"),
        "{\"type\":\"assistant\",\"content\":\"OUTSIDE_BODY\"}\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, slug.join(SID)).unwrap();

    let metas = scan_grok_at(&root, &tmp.join("cache"));
    assert!(
        metas.is_empty(),
        "symlink 会话目录不得被收进来，实际收到标题 {:?}",
        metas.iter().map(|m| &m.title).collect::<Vec<_>>()
    );

    // cwd 层的目录 symlink 同样不认
    let outer = tmp.join("outer-slug");
    fs::create_dir_all(outer.join(SID)).unwrap();
    fs::write(
        outer.join(SID).join("summary.json"),
        serde_json::json!({
            "info": { "id": SID, "cwd": "/secret" },
            "session_summary": "OUTSIDE_SENTINEL",
            "updated_at": "2026-09-08T03:23:48.982261Z",
        })
        .to_string(),
    )
    .unwrap();
    std::os::unix::fs::symlink(&outer, root.join("%2Flinked")).unwrap();
    assert!(
        scan_grok_at(&root, &tmp.join("cache2")).is_empty(),
        "cwd 层的目录 symlink 也不得被收进来"
    );

    let _ = fs::remove_dir_all(tmp);
}

/// `updated_at` / `created_at` 都缺失时不得命中缓存。
/// 空指纹会跟 serde 的默认空串撞上，让坏 summary + 等长正文的会话永远返回旧标题。
#[test]
fn grok_missing_timestamp_never_hits_cache() {
    let tmp = make_tempdir("no-timestamp");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let dir = root.join(CWD_SLUG).join(SID);
    fs::create_dir_all(&dir).unwrap();
    // 正文固定不变，把 size 这条信号也钉死，只剩缓存判据本身能救
    fs::write(dir.join("chat_history.jsonl"), "{\"type\":\"system\",\"content\":\"x\"}\n").unwrap();

    let write_summary = |title: &str| {
        fs::write(
            dir.join("summary.json"),
            serde_json::json!({
                "info": { "id": SID, "cwd": "/Users/test/Documents" },
                "session_summary": title,
                // updated_at / created_at 都没有
            })
            .to_string(),
        )
        .unwrap();
    };

    write_summary("OLD");
    assert_eq!(scan_grok_at(&root, &cache)[0].title, "OLD");
    write_summary("NEW");
    assert_eq!(
        scan_grok_at(&root, &cache)[0].title,
        "NEW",
        "拿不出时间戳指纹就不能复用缓存，否则标题永远停在旧值"
    );

    let _ = fs::remove_dir_all(tmp);
}

/// 同一个 SID 出现在两个会话目录里（手工 cp 备份）时只保留 updated_at 最新的一条。
/// pi 侧上一轮已经补了去重，grok 侧不能留这个不对称：否则列表重复行 + 两条记录互踩同一个缓存文件。
#[test]
fn grok_same_session_id_in_two_dirs_is_deduped_to_newest() {
    let tmp = make_tempdir("dedup");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let write_at = |slug: &str, title: &str, updated_at: &str| {
        let dir = root.join(slug).join(SID);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("summary.json"),
            serde_json::json!({
                "info": { "id": SID, "cwd": "/Users/test/Documents" },
                "session_summary": title,
                "updated_at": updated_at,
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            dir.join("chat_history.jsonl"),
            "{\"type\":\"system\",\"content\":\"x\"}\n",
        )
        .unwrap();
    };
    write_at(CWD_SLUG, "旧备份", "2026-09-07T03:00:00.000000Z");
    write_at("%2FUsers%2Ftest%2FOther", "最新一份", "2026-09-08T03:00:00.000000Z");

    let metas = scan_grok_at(&root, &cache);
    assert_eq!(metas.len(), 1, "同一个 SID 只能出现一条");
    assert_eq!(metas[0].title, "最新一份", "应保留 updated_at 更新的那份");

    let _ = fs::remove_dir_all(tmp);
}

/// 0.2.6 之前写的缓存没有 fingerprint 字段，serde 会补成空串。
/// 这类缓存必须一律 miss 重扫一次自愈，不能因为「空 == 空」被误判成命中。
#[test]
fn grok_legacy_cache_without_fingerprint_is_invalidated() {
    let tmp = make_tempdir("legacy-cache");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    write_session(
        &root,
        SID,
        "/Users/test/Documents",
        "真实标题",
        "2026-09-08T03:23:48.982261Z",
        &[user_line("<user_info>x</user_info>", "正文")],
    );
    assert_eq!(scan_grok_at(&root, &cache)[0].title, "真实标题");

    // 伪造一份「老格式」缓存：完全去掉 fingerprint 字段，标题写成陈旧值
    let cache_file = cache.join(format!("{SID}.json"));
    let mut entry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cache_file).unwrap()).unwrap();
    entry.as_object_mut().unwrap().remove("fingerprint");
    entry["meta"]["title"] = serde_json::Value::String("陈旧标题".into());
    fs::write(&cache_file, entry.to_string()).unwrap();
    assert!(
        !fs::read_to_string(&cache_file).unwrap().contains("fingerprint"),
        "前置条件：缓存里确实没有 fingerprint 字段"
    );

    assert_eq!(
        scan_grok_at(&root, &cache)[0].title,
        "真实标题",
        "缺 fingerprint 的老缓存必须失效重扫，不能返回陈旧标题"
    );

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn grok_resume_command_uses_resume_flag() {
    assert_eq!(
        commands::copy_resume_command_for_source("grok", SID).unwrap(),
        format!("grok --resume {SID}")
    );
    assert!(commands::copy_resume_command_for_source("grok", "bad id!").is_err());
}

#[test]
fn unknown_session_source_stays_fail_closed() {
    // 新增两个来源后，未登记的来源仍必须被拒（不能因为 allowlist 放宽而漏过）
    for source in ["grokbuild", "gemini", "opencode", ""] {
        assert!(
            commands::copy_resume_command_for_source(source, SID).is_err(),
            "未登记来源 {source} 必须 fail-closed"
        );
        assert!(commands::list_sessions_paged_for_source(source, None, 1, 20).is_err());
        assert!(commands::session_detail_by_source(source, SID).is_err());
    }
}
