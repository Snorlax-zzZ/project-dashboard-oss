// scan_at 粘性回归：会话文件增长、尾块 TAIL_CHUNK 窗口向后滚过原 file_path 后，
// 项目归属（projects）不应丢失——对应用户反馈的“同步之后项目没了”这个 bug。
//
// 机制：项目名从会话尾块（最后 TAIL_CHUNK 字节）里的 tool_use.file_path 反推。会话越写
// 越长后，原来的文件操作被挤出尾块窗口，重扫时尾块已无 file_path。修复前 projects 会被
// 重扫结果（空）覆盖；修复后靠缓存里上次已识别的项目做粘性合并，只增不丢。

use dashboard_core::sessions::{scan_at, TAIL_CHUNK};
use std::fs;
use std::path::PathBuf;

fn tmp_base(tag: &str) -> PathBuf {
    let mut b = std::env::temp_dir();
    b.push(format!(
        "fleet-sess-sticky-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    b
}

// file_path 必须落在真实 $HOME/Documents/ 下，pick_project 才认（extra_project_roots 运行时算）
fn home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .expect("测试需要 HOME/USERPROFILE")
        .replace('\\', "/")
}

#[test]
fn scan_at_keeps_project_after_file_grows_past_tail_window() {
    let base = tmp_base("grow");
    // scan_at 结构：projects_dir/<proj_slug>/<sid>.jsonl
    let projects_dir = base.join("projects");
    let session_dir = projects_dir.join("-Users-x-Documents");
    fs::create_dir_all(&session_dir).unwrap();
    let cache = base.join("cache");
    fs::create_dir_all(&cache).unwrap();
    let jsonl = session_dir.join("sess-1.jsonl");

    let fp = format!("{}/Documents/project-dashboard/src/main.rs", home());
    let head_line = r#"{"cwd":"/whatever","gitBranch":"main"}"#;
    let tool_line = format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","input":{{"file_path":"{}"}}}}]}}}}"#,
        fp
    );

    // ① 首次扫描：尾块含 project-dashboard 的 file_path → 应识别
    fs::write(&jsonl, format!("{head_line}\n{tool_line}\n")).unwrap();
    let metas = scan_at(&projects_dir, &cache);
    let m = metas
        .iter()
        .find(|m| m.session_id == "sess-1")
        .expect("应扫到 sess-1");
    assert!(
        m.projects.contains("project-dashboard"),
        "首次应识别 project-dashboard，实得 {:?}",
        m.projects
    );

    // ② 会话增长：追加 > TAIL_CHUNK 的无 file_path 内容，把原 tool_use 挤出尾块窗口，
    //    同时 size 变化触发缓存 miss、强制重扫
    let filler = format!(
        r#"{{"type":"user","message":{{"content":"{}"}}}}"#,
        "x".repeat(2000)
    );
    let mut content = fs::read_to_string(&jsonl).unwrap();
    while content.len() < TAIL_CHUNK + 64 * 1024 {
        content.push_str(&filler);
        content.push('\n');
    }
    fs::write(&jsonl, content).unwrap();

    // ③ 重扫：尾块已无 project-dashboard file_path，粘性应保留（修复前此处会变空）
    let metas2 = scan_at(&projects_dir, &cache);
    let m2 = metas2
        .iter()
        .find(|m| m.session_id == "sess-1")
        .expect("应扫到 sess-1");
    assert!(
        m2.projects.contains("project-dashboard"),
        "文件增长、file_path 滚出尾块后 projects 应粘性保留，实得 {:?}",
        m2.projects
    );

    let _ = fs::remove_dir_all(&base);
}
