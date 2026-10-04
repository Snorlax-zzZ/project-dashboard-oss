// pi 会话来源的 fixture 测试。形态取自 2026-09-09 实测的真实会话：
// `~/.pi/agent/sessions/<cwd slug>/<时间戳>_<uuid>.jsonl`，首行 type=session 带 cwd，
// 其余是 type=message，role 藏在 message.role（不是顶层 type）。
use dashboard_core::commands;
use dashboard_core::sessions::{pi_detail, scan_pi_at, PiSessionSource, SessionSource};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SID: &str = "00000000-0000-4000-8000-000000000201";

fn make_tempdir(label: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "project-dashboard-pi-sessions-{label}-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_session(root: &Path, slug: &str, sid: &str, lines: &[String]) -> PathBuf {
    let dir = root.join(slug);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("2026-09-08T11-46-24-618Z_{sid}.jsonl"));
    let mut body = lines.join("\n");
    body.push('\n');
    fs::write(&path, body).unwrap();
    path
}

fn session_line(sid: &str, cwd: &str) -> String {
    serde_json::json!({
        "type": "session",
        "version": 3,
        "id": sid,
        "timestamp": "2026-09-08T11:46:24.618Z",
        "cwd": cwd,
    })
    .to_string()
}

fn message_line(role: &str, text: &str) -> String {
    serde_json::json!({
        "type": "message",
        "id": "00000001",
        "timestamp": "2026-09-08T11:47:06.461Z",
        "message": {
            "role": role,
            "content": [{ "type": "text", "text": text }],
        }
    })
    .to_string()
}

fn tool_call_line(name: &str, path: &str) -> String {
    serde_json::json!({
        "type": "message",
        "id": "00000002",
        "timestamp": "2026-09-08T11:47:08.060Z",
        "message": {
            "role": "assistant",
            "content": [{
                "type": "toolCall",
                "id": "call_00_abc",
                "name": name,
                "arguments": { "path": path },
            }],
        }
    })
    .to_string()
}

#[test]
fn pi_scan_reads_metadata_title_and_skips_bad_lines() {
    let tmp = make_tempdir("scan");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    write_session(
        &root,
        "--Users-test-Documents-project--dashboard--",
        SID,
        &[
            session_line(SID, "/Users/test/Documents/project-dashboard"),
            serde_json::json!({"type": "model_change", "provider": "x", "modelId": "y"}).to_string(),
            message_line("user", "给 Session Board 增加 pi 来源页签"),
            "this is not json".to_string(),
            message_line("assistant", "先补 fixture 测试再实现。"),
            // toolResult 是工具回显，不是对话，不能进 haystack
            message_line("toolResult", "compaction.md\ncontainerization.md"),
            "{\"type\":\"message\"".to_string(),
        ],
    );
    // 非 jsonl 的兄弟文件必须忽略
    fs::create_dir_all(root.join("--Users-test-Documents--")).unwrap();
    fs::write(root.join("--Users-test-Documents--/notes.txt"), "not a session").unwrap();

    let metas = scan_pi_at(&root, &cache);
    assert_eq!(metas.len(), 1);
    let meta = &metas[0];
    assert_eq!(meta.session_id, SID);
    assert_eq!(meta.source, "pi");
    assert_eq!(meta.cwd, "/Users/test/Documents/project-dashboard");
    assert_eq!(meta.title, "给 Session Board 增加 pi 来源页签");
    assert!(meta.haystack.contains("先补 fixture 测试再实现"));
    assert!(
        !meta.haystack.contains("containerization"),
        "toolResult 角色不是对话，不得进 haystack"
    );
    assert_eq!(meta.projects, "project-dashboard");
    assert!(cache.join(format!("{SID}.json")).is_file());

    let source = PiSessionSource {
        root: root.clone(),
        cache: cache.clone(),
    };
    assert_eq!(source.source_id(), "pi");
    assert_eq!(source.scan().len(), 1);
    assert_eq!(source.find_by_id(SID).unwrap().session_id, SID);
    assert!(source.find_by_id("not-a-uuid").is_none());

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn pi_scan_falls_back_to_first_line_id_when_filename_has_no_uuid() {
    let tmp = make_tempdir("fallback-id");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let dir = root.join("--Users-test-Documents--");
    fs::create_dir_all(&dir).unwrap();
    let mut body = [
        session_line(SID, "/Users/test/Documents"),
        message_line("user", "hi pi"),
    ]
    .join("\n");
    body.push('\n');
    fs::write(dir.join("session-without-uuid.jsonl"), body).unwrap();

    let metas = scan_pi_at(&root, &cache);
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].session_id, SID, "文件名无 UUID 时回落到首行 id");
    // cwd 不在 PROJECT_ROOTS 覆盖范围时用目录名兜底
    assert_eq!(metas[0].projects, "Documents");

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn pi_scan_skips_files_without_any_resolvable_id() {
    let tmp = make_tempdir("no-id");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let dir = root.join("--Users-test-Documents--");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("broken.jsonl"),
        format!("{}\n", message_line("user", "没有 session 行也没有 UUID 文件名")),
    )
    .unwrap();

    assert!(
        scan_pi_at(&root, &cache).is_empty(),
        "拿不到 session id 的文件必须跳过，不能生成一条无法 resume 的记录"
    );

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn pi_cache_hit_reuses_entry_and_version_mismatch_forces_reparse() {
    let tmp = make_tempdir("cache");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let path = write_session(
        &root,
        "--Users-test-Documents--",
        SID,
        &[
            session_line(SID, "/Users/test/Documents/project-dashboard"),
            message_line("user", "第一版标题"),
        ],
    );
    assert_eq!(scan_pi_at(&root, &cache)[0].title, "第一版标题");

    // 缓存命中（mtime + size 未变）：即使把缓存里的标题改掉也应原样复用
    let cache_file = cache.join(format!("{SID}.json"));
    let raw = fs::read_to_string(&cache_file).unwrap();
    let mut entry: serde_json::Value = serde_json::from_str(&raw).unwrap();
    entry["meta"]["title"] = serde_json::Value::String("缓存里的标题".into());
    fs::write(&cache_file, entry.to_string()).unwrap();
    assert_eq!(
        scan_pi_at(&root, &cache)[0].title,
        "缓存里的标题",
        "mtime + size 一致时必须走缓存，不重读文件"
    );

    // 版本号不认识 → 必须重新解析文件，不能沿用旧格式缓存
    entry["version"] = serde_json::Value::from(u32::MAX);
    fs::write(&cache_file, entry.to_string()).unwrap();
    assert_eq!(
        scan_pi_at(&root, &cache)[0].title,
        "第一版标题",
        "缓存版本不匹配时必须重解析"
    );

    // 文件增长 → 缓存失效，标题跟着新内容走
    let mut body = fs::read_to_string(&path).unwrap();
    body.push_str(&message_line("assistant", "追加一轮"));
    body.push('\n');
    fs::write(&path, body).unwrap();
    let metas = scan_pi_at(&root, &cache);
    assert!(metas[0].haystack.contains("追加一轮"));

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn pi_detail_keeps_last_rounds_and_edit_write_paths() {
    let tmp = make_tempdir("detail");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let mut lines = vec![session_line(SID, "/Users/test/Documents/project-dashboard")];
    for round in 1..=7 {
        lines.push(message_line("user", &format!("问题 {round}")));
        lines.push(message_line("assistant", &format!("回答 {round}")));
    }
    lines.push(tool_call_line("write", "/Users/test/Documents/a.rs"));
    lines.push(tool_call_line("edit", "/Users/test/Documents/b.rs"));
    // bash / read 不改文件，不能进改动清单
    lines.push(tool_call_line("read", "/Users/test/Documents/c.rs"));
    write_session(&root, "--Users-test-Documents--", SID, &lines);

    let meta = scan_pi_at(&root, &cache).remove(0);
    let detail = pi_detail(&meta, 5);
    assert_eq!(
        detail.preview.iter().filter(|line| line.starts_with('🧑')).count(),
        5,
        "rounds=5 只保留最后 5 个 user 轮次"
    );
    assert!(detail.preview.first().unwrap().contains("问题 3"));
    assert!(detail.preview.last().unwrap().contains("回答 7"));
    assert_eq!(
        detail.files,
        vec![
            "/Users/test/Documents/a.rs".to_string(),
            "/Users/test/Documents/b.rs".to_string(),
        ],
        "只有 edit / write 会改文件；read 的 path 不算改动"
    );

    let _ = fs::remove_dir_all(tmp);
}

/// 列表拿到的 session_id 必须能原样喂回 find_by_id。
/// 文件名尾缀是大写 hex 的 UUID 时，取 id 与查 id 两条路径的大小写归一化必须对称——
/// 否则列表正常显示、点开详情却静默空白（Ok(None) 不报错，最难查的一种）。
#[test]
fn pi_session_id_is_normalized_so_detail_lookup_never_misses() {
    let tmp = make_tempdir("uuid-case");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let upper = SID.to_uppercase();
    write_session(
        &root,
        "--Users-test-Documents--",
        &upper,
        &[
            session_line(&upper, "/Users/test/Documents"),
            message_line("user", "大写 UUID 文件名"),
        ],
    );

    let metas = scan_pi_at(&root, &cache);
    assert_eq!(metas.len(), 1);
    let listed = metas[0].session_id.clone();
    let source = PiSessionSource {
        root: root.clone(),
        cache: cache.clone(),
    };
    assert!(
        source.find_by_id(&listed).is_some(),
        "列表给出的 session_id ({listed}) 必须能查回详情"
    );
    // 大小写两种写法都要能命中，避免只是把不对称换了个方向
    assert!(source.find_by_id(&upper).is_some(), "大写写法必须命中");
    assert!(
        source.find_by_id(&SID.to_lowercase()).is_some(),
        "小写写法必须命中"
    );

    let _ = fs::remove_dir_all(tmp);
}

/// 缓存是本机可写文件：被改过 `meta.path` 的缓存不能把详情读取引向来源根目录之外。
/// 实测过：加围栏之前，篡改后的缓存能让 pi_detail 读出根目录外文件的内容。
#[test]
fn pi_tampered_cache_path_cannot_escape_source_root() {
    let tmp = make_tempdir("path-escape");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    write_session(
        &root,
        "--Users-test-Documents--",
        SID,
        &[
            session_line(SID, "/Users/test/Documents"),
            message_line("user", "正常内容"),
        ],
    );
    assert_eq!(scan_pi_at(&root, &cache).len(), 1);

    let outside = tmp.join("outside.jsonl");
    fs::write(&outside, format!("{}\n", message_line("user", "OUTSIDE_SENTINEL"))).unwrap();

    // 只改 path，version / mtime / size 全部保持一致
    let cache_file = cache.join(format!("{SID}.json"));
    let mut entry: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cache_file).unwrap()).unwrap();
    entry["meta"]["path"] = serde_json::Value::String(outside.to_string_lossy().to_string());
    fs::write(&cache_file, entry.to_string()).unwrap();

    let retarget = |target: &Path| {
        let mut entry: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&cache_file).unwrap()).unwrap();
        entry["meta"]["path"] = serde_json::Value::String(target.to_string_lossy().to_string());
        fs::write(&cache_file, entry.to_string()).unwrap();
    };
    let assert_fenced = |label: &str| {
        let source = PiSessionSource {
            root: root.clone(),
            cache: cache.clone(),
        };
        let meta = source.find_by_id(SID).expect("会话仍应被扫描到");
        assert!(
            !meta.path.contains("outside.jsonl") && !meta.path.contains("root-evil"),
            "[{label}] 缓存里越界的 path 必须被丢弃并重新扫描，实际拿到 {}",
            meta.path
        );
        assert!(
            !pi_detail(&meta, 5).preview.join(" ").contains("OUTSIDE_SENTINEL"),
            "[{label}] 详情不得读取来源根目录之外的文件"
        );
    };

    // ① 直接指向根目录外的绝对路径
    assert_fenced("绝对路径越界");

    // ② 兄弟目录前缀相同：`<root>-evil` 不得被当成在 `<root>` 内
    //    （依赖 Path::starts_with 是按路径组件比较，不是字符串前缀——实测确认过）
    let sibling = tmp.join("sessions-evil").join("--Users-test-Documents--");
    fs::create_dir_all(&sibling).unwrap();
    let sibling_file = sibling.join(format!("2026-09-08T11-46-24-618Z_{SID}.jsonl"));
    fs::write(
        &sibling_file,
        format!("{}\n", message_line("user", "OUTSIDE_SENTINEL")),
    )
    .unwrap();
    retarget(&sibling_file);
    assert_fenced("兄弟目录前缀相同");

    // ③ 根目录内的 symlink 指向根目录外
    let link = root
        .join("--Users-test-Documents--")
        .join("linked.jsonl");
    #[cfg(unix)]
    {
        let _ = std::os::unix::fs::symlink(&outside, &link);
        retarget(&link);
        assert_fenced("root 内 symlink 指向 root 外");
    }

    // ④ `../` 路径遍历
    retarget(&root.join("..").join("outside.jsonl"));
    assert_fenced("../ 路径遍历");

    let _ = fs::remove_dir_all(tmp);
}

/// 扫描阶段不得跟随 symlink。
///
/// 这条跟上面那个缓存围栏是两条独立路径：把一个**文件名带合法 UUID** 的 symlink 放进
/// 会话目录，扫描器压根不查缓存就能把根目录外的内容当成正常会话收进来。
/// 实测过：加围栏之前 `scan_pi_at` 会返回 1 条、cwd 和正文都来自外部文件。
#[cfg(unix)]
#[test]
fn pi_scan_does_not_follow_symlinks_out_of_root() {
    let tmp = make_tempdir("symlink-scan");
    let root = tmp.join("sessions");
    let dir = root.join("--Users-test-Documents--");
    fs::create_dir_all(&dir).unwrap();

    let outside = tmp.join("outside.jsonl");
    fs::write(
        &outside,
        format!(
            "{}\n{}\n",
            session_line(SID, "/secret"),
            message_line("user", "OUTSIDE_SENTINEL")
        ),
    )
    .unwrap();
    // 文件名刻意做成合法的 pi 会话名，让扫描器无法靠文件名过滤掉
    std::os::unix::fs::symlink(
        &outside,
        dir.join(format!("2026-09-08T11-46-24-618Z_{SID}.jsonl")),
    )
    .unwrap();

    let metas = scan_pi_at(&root, &tmp.join("cache"));
    assert!(
        metas.is_empty(),
        "symlink 不得被当成会话收进来，实际收到 {:?}",
        metas.iter().map(|m| &m.cwd).collect::<Vec<_>>()
    );

    // 目录级 symlink 同样不认
    let outside_dir = tmp.join("outside-dir");
    fs::create_dir_all(&outside_dir).unwrap();
    fs::write(
        outside_dir.join(format!("2026-09-08T11-46-24-618Z_{SID}.jsonl")),
        format!("{}\n", session_line(SID, "/secret")),
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside_dir, root.join("--linked--")).unwrap();
    assert!(
        scan_pi_at(&root, &tmp.join("cache2")).is_empty(),
        "指向 root 外的目录 symlink 也不得被收进来"
    );

    let _ = fs::remove_dir_all(tmp);
}

/// 同 mtime 的多条记录顺序必须稳定。
/// 去重用的 HashMap 遍历顺序不确定，只按 mtime 倒排时实测 8 轮能扫出 8 种顺序，
/// 表现为列表每次刷新行乱跳；必须有 session_id 兜底的二级排序。
#[test]
fn pi_listing_order_is_stable_when_mtimes_tie() {
    let tmp = make_tempdir("order");
    let root = tmp.join("sessions");
    let dir = root.join("--Users-test-Documents--");
    fs::create_dir_all(&dir).unwrap();
    let mut paths = Vec::new();
    for index in 0..6u8 {
        let sid = format!("00000000-0000-4000-8000-0000000002{index:02x}");
        let path = dir.join(format!("2026-09-08T11-46-24-618Z_{sid}.jsonl"));
        fs::write(&path, format!("{}\n", session_line(&sid, "/Users/test/Documents"))).unwrap();
        paths.push(path);
    }
    // 把所有文件的 mtime 抹成同一个值，制造并列
    let shared = paths[0].metadata().unwrap().modified().unwrap();
    let times = fs::FileTimes::new().set_modified(shared).set_accessed(shared);
    for path in &paths {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(times)
            .unwrap();
    }

    let mut orders = std::collections::HashSet::new();
    for round in 0..8 {
        // 每轮换新 cache，排除缓存把顺序固化的干扰
        let order: Vec<String> = scan_pi_at(&root, &tmp.join(format!("cache-{round}")))
            .into_iter()
            .map(|meta| meta.session_id)
            .collect();
        assert_eq!(order.len(), 6);
        orders.insert(order.join(","));
    }
    assert_eq!(
        orders.len(),
        1,
        "mtime 并列时列表顺序必须稳定，实测出现了 {} 种顺序",
        orders.len()
    );

    let _ = fs::remove_dir_all(tmp);
}

/// 同一个 SID 出现在多个文件里（例如手工 cp 备份）时只保留 mtime 最新的一条，
/// 否则列表出现重复行、两条记录还会互相覆盖同一个缓存文件。
#[test]
fn pi_same_session_id_in_two_files_is_deduped_to_newest() {
    let tmp = make_tempdir("dedup");
    let root = tmp.join("sessions");
    let cache = tmp.join("cache");
    let dir = root.join("--Users-test-Documents--");
    fs::create_dir_all(&dir).unwrap();
    let older = dir.join(format!("2026-09-07T10-00-00-000Z_{SID}.jsonl"));
    let newer = dir.join(format!("2026-09-08T10-00-00-000Z_{SID}.jsonl"));
    fs::write(
        &older,
        format!(
            "{}\n{}\n",
            session_line(SID, "/Users/test/Documents"),
            message_line("user", "旧备份")
        ),
    )
    .unwrap();
    // 靠写入间隔拉开 mtime（不引 filetime 依赖）；20ms 远大于 APFS 的 mtime 分辨率
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(
        &newer,
        format!(
            "{}\n{}\n",
            session_line(SID, "/Users/test/Documents"),
            message_line("user", "最新一份")
        ),
    )
    .unwrap();
    assert!(
        newer.metadata().unwrap().modified().unwrap() > older.metadata().unwrap().modified().unwrap(),
        "前置条件：两个文件的 mtime 必须真的分出先后"
    );

    let metas = scan_pi_at(&root, &cache);
    assert_eq!(metas.len(), 1, "同一个 SID 只能出现一条");
    assert!(
        metas[0].path.contains("2026-09-08"),
        "应保留 mtime 更新的那份，实际 {}",
        metas[0].path
    );

    let _ = fs::remove_dir_all(tmp);
}

#[test]
fn pi_resume_command_uses_session_flag() {
    // pi 的 --resume/-r 是交互式选择器不接 id，必须用 --session
    assert_eq!(
        commands::copy_resume_command_for_source("pi", SID).unwrap(),
        format!("pi --session {SID}")
    );
    assert!(commands::copy_resume_command_for_source("pi", "bad id!").is_err());
}
