use dashboard_core::commands;
use dashboard_core::sessions::{
    codex_detail, codex_session_by_id_at, scan_codex_at, CodexSessionSource, SessionSource,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn make_tempdir(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "project-dashboard-codex-sessions-{label}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_rollout(root: &Path, sid: &str, lines: &[String]) -> PathBuf {
    let dir = root.join("2026").join("07").join("29");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("rollout-2026-07-29T10-00-00-{sid}.jsonl"));
    let mut body = lines.join("\n");
    body.push('\n');
    fs::write(&path, body).unwrap();
    path
}

fn meta_line(sid: &str, cwd: &str, branch: &str) -> String {
    serde_json::json!({
        "timestamp": "2026-07-29T10:00:00.000Z",
        "type": "session_meta",
        "payload": {
            "id": sid,
            "session_id": sid,
            "cwd": cwd,
            "git": { "branch": branch }
        }
    })
    .to_string()
}

fn event_message(kind: &str, message: &str) -> String {
    serde_json::json!({
        "timestamp": "2026-07-29T10:00:01.000Z",
        "type": "event_msg",
        "payload": {
            "type": kind,
            "message": message
        }
    })
    .to_string()
}

fn response_message(role: &str, block_type: &str, text: &str) -> String {
    serde_json::json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": role,
            "content": [{
                "type": block_type,
                "text": text
            }]
        }
    })
    .to_string()
}

#[test]
fn codex_titles_skip_ui_wrappers_but_preserve_detail_content() {
    for wrapper in [
        "<external_codex_apps_open_page>内部上下文</external_codex_apps_open_page>",
        "<send_user_message_question_reply>用户答复</send_user_message_question_reply>",
    ] {
        for response_only in [false, true] {
            let tmp = make_tempdir("ui-wrapper-title");
            let root = tmp.join("sessions");
            let cache = tmp.join("cache");
            let sid = "019c0de0-0000-7000-8000-00000000000d";
            let message = |text: &str| {
                if response_only {
                    response_message("user", "input_text", text)
                } else {
                    event_message("user_message", text)
                }
            };
            write_rollout(
                &root,
                sid,
                &[meta_line(sid, "/tmp/title", "main"), message(wrapper)],
            );
            assert_eq!(
                scan_codex_at(&root, &cache)[0].title,
                "",
                "包装消息不能充当标题"
            );

            let path = write_rollout(
                &root,
                sid,
                &[
                    meta_line(sid, "/tmp/title", "main"),
                    message(wrapper),
                    message("修复真实用户的问题"),
                ],
            );
            let original = fs::read(&path).unwrap();
            let metas = scan_codex_at(&root, &cache);
            assert_eq!(metas[0].title, "修复真实用户的问题");
            assert!(
                codex_detail(&metas[0], 5)
                    .preview
                    .join("\n")
                    .contains(wrapper),
                "标题筛选不得删除详情里的用户答复"
            );
            assert_eq!(fs::read(path).unwrap(), original);
            let _ = fs::remove_dir_all(tmp);
        }
    }
}

#[test]
fn codex_v2_wrapped_title_cache_recovers_middle_title_without_source_change() {
    for wrapper in [
        "<external_codex_apps_open_page>",
        "<send_user_message_question_reply>",
    ] {
        let tmp = make_tempdir("v2-wrapper-cache");
        let root = tmp.join("sessions");
        let cache = tmp.join("cache");
        let sid = "019c0de0-0000-7000-8000-00000000000e";
        let padding =
            |size| serde_json::json!({"type":"diagnostic", "padding":"x".repeat(size)}).to_string();
        let path = write_rollout(
            &root,
            sid,
            &[
                meta_line(sid, "/tmp/title", "main"),
                response_message("user", "input_text", wrapper),
                padding(96 * 1024),
                response_message("user", "input_text", "被旧缓存遮住的真实标题"),
                padding(640 * 1024),
            ],
        );
        scan_codex_at(&root, &cache);
        let cache_file = cache.join(format!("{sid}.json"));
        let mut stale: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&cache_file).unwrap()).unwrap();
        stale["version"] = serde_json::json!(2);
        stale["meta"]["title"] = serde_json::json!(wrapper.chars().take(30).collect::<String>());
        fs::write(&cache_file, stale.to_string()).unwrap();
        let original = fs::read(&path).unwrap();

        let found = codex_session_by_id_at(&root, &cache, sid).unwrap();
        assert_eq!(found.title, "被旧缓存遮住的真实标题");
        assert_eq!(scan_codex_at(&root, &cache)[0].title, found.title);
        assert!(
            codex_detail(&found, 5).preview.is_empty(),
            "保持原有512KiB详情读取上限"
        );
        assert_eq!(fs::read(path).unwrap(), original);
        let _ = fs::remove_dir_all(tmp);
    }
}

#[test]
fn codex_scan_reads_nested_active_rollout_metadata_and_skips_bad_lines() {
    let tmp = make_tempdir("scan");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let sid = "019c0de0-1111-7222-8333-444455556666";
    write_rollout(
        &root,
        sid,
        &[
            meta_line(
                sid,
                "/Users/test/Documents/project-dashboard",
                "feature/tabs",
            ),
            event_message("user_message", "给 Session Board 增加 Codex 来源页签"),
            "this is not json".to_string(),
            event_message("agent_message", "我会先补测试再实现。"),
            "{\"type\":\"event_msg\"".to_string(),
        ],
    );
    // 非 jsonl 与目录外兄弟文件必须忽略。
    fs::write(root.join("ignore.txt"), "not a session").unwrap();

    let metas = scan_codex_at(&root, &cache);
    assert_eq!(metas.len(), 1);
    let meta = &metas[0];
    assert_eq!(meta.session_id, sid);
    assert_eq!(meta.source, "codex");
    assert_eq!(meta.cwd, "/Users/test/Documents/project-dashboard");
    assert_eq!(meta.git_branch, "feature/tabs");
    assert_eq!(meta.title, "给 Session Board 增加 Codex 来源页签");
    assert!(meta.haystack.contains("我会先补测试再实现"));
    assert_eq!(meta.projects, "project-dashboard");
    assert!(cache.join(format!("{sid}.json")).is_file());

    let source = CodexSessionSource {
        root: root.clone(),
        cache: cache.clone(),
    };
    assert_eq!(source.source_id(), "codex");
    assert_eq!(source.scan().len(), 1);

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn codex_scan_falls_back_to_filename_uuid_and_empty_title() {
    let tmp = make_tempdir("fallback");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let sid = "019c0de0-aaaa-7bbb-8ccc-ddddeeeeffff";
    write_rollout(
        &root,
        sid,
        &[
            serde_json::json!({
                "type": "turn_context",
                "payload": { "cwd": "/tmp/fallback-project" }
            })
            .to_string(),
            event_message("agent_message", "没有用户消息时不能伪造标题"),
        ],
    );

    let metas = scan_codex_at(&root, &cache);
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].session_id, sid);
    assert_eq!(metas[0].cwd, "/tmp/fallback-project");
    assert_eq!(metas[0].title, "");

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn codex_scan_excludes_internal_subagent_rollouts() {
    let tmp = make_tempdir("subagent");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let root_sid = "019c0de0-0000-7000-8000-000000000001";
    let child_sid = "019c0de0-0000-7000-8000-000000000002";
    let string_child_sid = "019c0de0-0000-7000-8000-00000000000a";
    write_rollout(
        &root,
        root_sid,
        &[
            meta_line(root_sid, "/tmp/root-session", "main"),
            event_message("user_message", "顶层会话"),
        ],
    );
    write_rollout(
        &root,
        child_sid,
        &[
            serde_json::json!({
                "type": "session_meta",
                "payload": {
                    "id": child_sid,
                    "cwd": "/tmp/root-session",
                    "source": {
                        "subagent": {
                            "thread_spawn": { "parent_thread_id": root_sid }
                        }
                    }
                }
            })
            .to_string(),
            response_message("user", "input_text", "内部子会话"),
        ],
    );
    write_rollout(
        &root,
        string_child_sid,
        &[
            serde_json::json!({
                "type": "session_meta",
                "payload": {
                    "id": string_child_sid,
                    "cwd": "/tmp/root-session",
                    "source": "subagent"
                }
            })
            .to_string(),
            response_message("user", "input_text", "字符串标记的内部子会话"),
        ],
    );

    let metas = scan_codex_at(&root, &cache);
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].session_id, root_sid);
    assert!(!cache.join(format!("{child_sid}.json")).exists());
    assert!(
        !cache.join(format!("{string_child_sid}.json")).exists(),
        "字符串 source=subagent 的 rollout 也必须被过滤且不得落缓存"
    );

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn codex_response_item_is_fallback_only_and_never_duplicates_event_messages() {
    let tmp = make_tempdir("response-fallback");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let fallback_sid = "019c0de0-0000-7000-8000-000000000003";
    write_rollout(
        &root,
        fallback_sid,
        &[
            meta_line(fallback_sid, "/tmp/fallback", "main"),
            response_message("user", "input_text", "只在 response_item 的问题"),
            response_message("assistant", "output_text", "只在 response_item 的回答"),
        ],
    );

    let duplicate_sid = "019c0de0-0000-7000-8000-000000000004";
    write_rollout(
        &root,
        duplicate_sid,
        &[
            meta_line(duplicate_sid, "/tmp/duplicate", "main"),
            response_message("user", "input_text", "重复问题"),
            response_message("assistant", "output_text", "重复回答"),
            event_message("user_message", "重复问题"),
            event_message("agent_message", "重复回答"),
        ],
    );

    let metas = scan_codex_at(&root, &cache);
    let fallback = metas
        .iter()
        .find(|meta| meta.session_id == fallback_sid)
        .unwrap();
    assert_eq!(fallback.title, "只在 response_item 的问题");
    assert_eq!(
        codex_detail(fallback, 5).preview,
        vec![
            "🧑 只在 response_item 的问题".to_string(),
            "🤖 只在 response_item 的回答".to_string(),
        ]
    );

    let duplicate = metas
        .iter()
        .find(|meta| meta.session_id == duplicate_sid)
        .unwrap();
    assert_eq!(
        codex_detail(duplicate, 5).preview,
        vec!["🧑 重复问题".to_string(), "🤖 重复回答".to_string()],
        "event_msg 存在时不得再拼 response_item 造成双份对话"
    );

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn codex_session_by_id_uses_validated_cache_and_rejects_unknown_id() {
    let tmp = make_tempdir("by-id");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let sid = "019c0de0-0000-7000-8000-000000000005";
    write_rollout(
        &root,
        sid,
        &[
            meta_line(sid, "/tmp/by-id", "main"),
            event_message("user_message", "按 SID 读取详情"),
        ],
    );
    let scanned = scan_codex_at(&root, &cache);
    assert_eq!(scanned.len(), 1);

    let found = codex_session_by_id_at(&root, &cache, sid).expect("已缓存 SID 应直取成功");
    assert_eq!(found.session_id, sid);
    assert_eq!(found.source, "codex");
    assert_eq!(found.title, "按 SID 读取详情");

    // 篡改缓存 path 指向 root 外的另一份合法 JSONL；即使 stat 字段也匹配，读取入口仍须
    // 拒绝该缓存并从真实 sessions root 重建，不能被引导去读任意本机文件。
    let outside = tmp.join("outside.jsonl");
    fs::write(
        &outside,
        [
            meta_line(sid, "/tmp/outside", "main"),
            event_message("user_message", "不应从越界缓存读取"),
        ]
        .join("\n"),
    )
    .unwrap();
    let outside_stat = outside.metadata().unwrap();
    let outside_mtime = outside_stat
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let cache_file = cache.join(format!("{sid}.json"));
    let mut cached: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cache_file).unwrap()).unwrap();
    cached["meta"]["path"] = serde_json::json!(outside.to_string_lossy());
    cached["meta"]["size"] = serde_json::json!(outside_stat.len());
    cached["meta"]["mtime"] = serde_json::json!(outside_mtime);
    fs::write(&cache_file, cached.to_string()).unwrap();
    let rebuilt = codex_session_by_id_at(&root, &cache, sid).expect("越界缓存应回退真实 root 重建");
    assert_ne!(rebuilt.path, outside.to_string_lossy());
    assert_eq!(rebuilt.title, "按 SID 读取详情");

    assert!(
        codex_session_by_id_at(&root, &cache, "019c0de0-0000-7000-8000-000000000099").is_none()
    );
    assert!(
        codex_session_by_id_at(&root, &cache, "../../outside").is_none(),
        "非 UUID 不得参与缓存路径拼接"
    );

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn codex_cache_version_mismatch_forces_reparse() {
    let tmp = make_tempdir("cache-version");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let sid = "019c0de0-0000-7000-8000-00000000000b";
    write_rollout(
        &root,
        sid,
        &[
            meta_line(sid, "/tmp/cache-version", "main"),
            event_message("user_message", "源文件中的新标题"),
        ],
    );

    let first = scan_codex_at(&root, &cache);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].title, "源文件中的新标题");

    let cache_file = cache.join(format!("{sid}.json"));
    let mut stale: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cache_file).unwrap()).unwrap();
    stale["version"] = serde_json::json!(0);
    stale["meta"]["title"] = serde_json::json!("不应命中的旧版本缓存");
    fs::write(&cache_file, stale.to_string()).unwrap();

    let reparsed = scan_codex_at(&root, &cache);
    assert_eq!(reparsed.len(), 1);
    assert_eq!(
        reparsed[0].title, "源文件中的新标题",
        "缓存版本不匹配时必须重读源 JSONL，不能返回旧缓存内容"
    );
    let rewritten: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cache_file).unwrap()).unwrap();
    assert_ne!(
        rewritten["version"].as_u64(),
        Some(0),
        "重建后必须写回当前缓存版本"
    );

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn codex_scan_recovers_middle_title_from_v1_empty_cache() {
    let tmp = make_tempdir("adaptive-title");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let sid = "019c0de0-0000-7000-8000-00000000000c";
    let expected_title = "大会话中段的首条用户标题";
    let path = write_rollout(
        &root,
        sid,
        &[
            meta_line(sid, "/tmp/adaptive-title", "main"),
            serde_json::json!({
                "type": "diagnostic",
                "payload": { "padding": "x".repeat(96 * 1024) }
            })
            .to_string(),
            event_message("user_message", expected_title),
            serde_json::json!({
                "type": "diagnostic",
                "payload": { "padding": "y".repeat(320 * 1024) }
            })
            .to_string(),
        ],
    );

    let stat = path.metadata().unwrap();
    let mtime = stat
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    fs::create_dir_all(&cache).unwrap();
    fs::write(
        cache.join(format!("{sid}.json")),
        serde_json::json!({
            "version": 1,
            "meta": {
                "session_id": sid,
                "path": path.to_string_lossy(),
                "mtime": mtime,
                "size": stat.len(),
                "cwd": "/tmp/adaptive-title",
                "title": "",
                "git_branch": "main",
                "haystack": "",
                "projects": "adaptive-title",
                "source": "codex"
            }
        })
        .to_string(),
    )
    .unwrap();

    let metas = scan_codex_at(&root, &cache);
    assert_eq!(metas.len(), 1);
    assert_eq!(
        metas[0].title, expected_title,
        "旧版空标题缓存必须失效，并在标准头尾窗口都未命中时有界补扫 JSONL 前缀"
    );
    let rewritten: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(cache.join(format!("{sid}.json"))).unwrap())
            .unwrap();
    assert_eq!(
        rewritten["version"].as_u64(),
        Some(3),
        "标题解析策略变化必须提升缓存版本"
    );

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn codex_detail_keeps_last_five_rounds_and_patch_paths() {
    let tmp = make_tempdir("detail");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let sid = "019c0de0-1234-7abc-8def-1234567890ab";
    let mut lines = vec![meta_line(
        sid,
        "/Users/test/Documents/project-dashboard",
        "main",
    )];
    for i in 1..=6 {
        lines.push(event_message("user_message", &format!("Q{i}")));
        lines.push(event_message("agent_message", &format!("A{i}")));
    }
    lines.push(
        serde_json::json!({
            "type": "event_msg",
            "payload": {
                "type": "patch_apply_end",
                "changes": {
                    "/Users/test/Documents/project-dashboard/src/old.rs": {
                        "type": "update",
                        "move_path": "/Users/test/Documents/project-dashboard/src/new.rs"
                    },
                    "/Users/test/Documents/project-dashboard/src/lib.rs": {
                        "type": "update"
                    }
                }
            }
        })
        .to_string(),
    );
    let path = write_rollout(&root, sid, &lines);
    let meta = scan_codex_at(&root, &cache)
        .into_iter()
        .find(|m| m.path == path.to_string_lossy())
        .unwrap();
    assert!(
        meta.haystack.contains("src/lib.rs"),
        "全文搜索索引必须包含改动文件路径"
    );

    let detail = codex_detail(&meta, 5);
    let joined = detail.preview.join("|");
    assert!(!joined.contains("Q1"), "只保留最后五轮：{joined}");
    assert!(joined.contains("Q2"), "最后五轮应从 Q2 开始：{joined}");
    assert!(
        joined.contains("A6"),
        "最后一条 agent 回复必须保留：{joined}"
    );
    assert_eq!(
        detail
            .preview
            .iter()
            .filter(|line| line.starts_with("🧑"))
            .count(),
        5
    );
    assert_eq!(
        detail.files,
        vec![
            "/Users/test/Documents/project-dashboard/src/old.rs",
            "/Users/test/Documents/project-dashboard/src/new.rs",
            "/Users/test/Documents/project-dashboard/src/lib.rs",
        ]
    );

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn source_aware_resume_commands_and_unknown_source_are_fail_closed() {
    assert_eq!(
        commands::copy_resume_command_for_source("claude-cli", "claude-sid").unwrap(),
        "claude --resume claude-sid"
    );
    assert_eq!(
        commands::copy_resume_command_for_source("codex", "codex-sid").unwrap(),
        "codex resume codex-sid"
    );

    let copy_error = commands::copy_resume_command_for_source("unexpected", "sid").unwrap_err();
    assert!(copy_error.to_string().contains("unexpected"));

    let list_error =
        commands::list_sessions_paged_for_source("unexpected", None, 1, 20).unwrap_err();
    assert!(list_error.to_string().contains("unexpected"));

    let detail_error = commands::session_detail_by_source("unexpected", "sid").unwrap_err();
    assert!(detail_error.to_string().contains("unexpected"));

    for unsafe_id in ["", "sid with space", "sid;open /tmp", "../session"] {
        let error = commands::copy_resume_command_for_source("codex", unsafe_id).unwrap_err();
        assert!(
            error.to_string().contains("INVALID_SESSION_ID"),
            "危险 SID 必须 fail-closed：{unsafe_id:?} -> {error}"
        );
    }
}
