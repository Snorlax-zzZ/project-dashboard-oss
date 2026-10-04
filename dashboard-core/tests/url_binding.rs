// Q1-C：add_project URL 绑定校验四路集成测试 —— 读本地真实 origin 决定 primary/alias/拒绝。
// 覆盖 AC-1（公司 Bitbucket 可加入）/ AC-2（双形态跨机配对）/ AC-6（origin 无关拒绝）。

use dashboard_core::commands::{add_project, AddProjectRequest, CommandError};
use dashboard_core::config::Registry;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-url-binding-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
}

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git must be on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// 建一个带 origin remote 的 git 仓（不 fetch，避免网络依赖）。
fn init_repo_with_origin(cwd: &Path, origin: &str) {
    git(cwd, &["init", "-q", "-b", "main"]);
    git(cwd, &["remote", "add", "origin", origin]);
}

/// 把 dashboard-state 布成"就绪"态（有 .git + origin==配置 remote）+ 写好配同一 remote 的
/// local.yaml，返回 (dashboard_state_root, local_path)。Q2-B 门禁要求 sync:true 操作前存储仓
/// 就绪（未配 remote 且无本地仓 → Unconfigured 拦截），故 URL 绑定四路测试统一先备好就绪存储仓。
fn ready_ds_lp(tmp: &Path) -> (PathBuf, PathBuf) {
    const REMOTE: &str = "git@ds-host:team/dashboard-state.git";
    let ds = tmp.join("dashboard-state");
    std::fs::create_dir_all(&ds).unwrap();
    git(&ds, &["init", "-q", "-b", "main"]);
    git(&ds, &["remote", "add", "origin", REMOTE]);
    // 决策 3：补合法结构 registry/projects，否则 add sync:true 的 gate 判 InvalidStructure
    std::fs::create_dir_all(ds.join("registry").join("projects")).unwrap();
    let lp = tmp.join(".project-dashboard").join("local.yaml");
    std::fs::create_dir_all(lp.parent().unwrap()).unwrap();
    let yaml = format!(
        "schema_version: 1\nmachine_id: test-machine\nwork_dirs: []\nprivate_projects: []\ndiagnostic_mode: false\nsync_tick_minutes: 3\ndashboard_state_remote: {REMOTE}\n"
    );
    std::fs::write(&lp, yaml).unwrap();
    (ds, lp)
}

/// 造一个 sync:true 项目请求：project_path 指向本机真实 git 仓（供读 origin）。
fn sync_req(name: &str, git_url: &str, project_path: &Path, confirm: bool) -> AddProjectRequest {
    let mut machines = BTreeMap::new();
    machines.insert(
        "test-machine".to_string(),
        project_path.to_string_lossy().into_owned(),
    );
    AddProjectRequest {
        name: name.to_string(),
        git_url: git_url.to_string(),
        path: project_path.to_string_lossy().into_owned(),
        machines,
        sync: true,
        confirm_dual_form: confirm,
    }
}

/// 一致：origin 归一化与手填一致 → primary = origin，aliases 空。
#[test]
fn add_sync_consistent_origin() {
    let tmp = make_tempdir("consistent");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    init_repo_with_origin(&proj, "https://github.com/team/repo.git");
    let (ds, lp) = ready_ds_lp(&tmp);

    let req = sync_req("repo", "https://github.com/team/repo", &proj, false);
    let r = add_project(&ds, &lp, req).expect("一致 origin 应成功");
    let uuid = r.project_id.unwrap();
    let reg = Registry::load_at(&ds, &lp).unwrap();
    let p = reg.get_shared(&uuid).unwrap();
    assert_eq!(p.git_url, "https://github.com/team/repo");
    assert!(p.git_url_aliases.is_empty(), "一致路径无 alias");
}

/// Bitbucket 双形态 + 已确认：primary = 真实 origin（clone 形式），手填（web）进 aliases（覆盖 AC-2）。
#[test]
fn add_sync_bitbucket_dualform_confirmed() {
    let tmp = make_tempdir("dualform-ok");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    // 真实 origin = clone 形式
    init_repo_with_origin(&proj, "https://bb.example/scm/team/exampleapp.git");
    let (ds, lp) = ready_ds_lp(&tmp);

    // 用户手填 web 形式，confirm=true
    let req = sync_req(
        "project-alpha",
        "https://bb.example/projects/TEAM/repos/ExampleApp",
        &proj,
        true,
    );
    let r = add_project(&ds, &lp, req).expect("双形态确认后应成功");
    let uuid = r.project_id.unwrap();
    let reg = Registry::load_at(&ds, &lp).unwrap();
    let p = reg.get_shared(&uuid).unwrap();
    assert_eq!(
        p.git_url, "https://bb.example/scm/team/exampleapp",
        "primary 服从真实 origin（clone 形式）"
    );
    assert_eq!(
        p.git_url_aliases,
        vec!["https://bb.example/projects/team/repos/exampleapp".to_string()],
        "用户手填 web 归一化进 aliases"
    );
}

/// Bitbucket 双形态但未确认 → NeedDualFormConfirm，不写 registry。
#[test]
fn add_sync_bitbucket_dualform_needs_confirm() {
    let tmp = make_tempdir("dualform-noconfirm");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    init_repo_with_origin(&proj, "https://bb.example/scm/team/exampleapp.git");
    let (ds, lp) = ready_ds_lp(&tmp);

    let req = sync_req(
        "project-alpha",
        "https://bb.example/projects/TEAM/repos/ExampleApp",
        &proj,
        false, // 未确认
    );
    let err = add_project(&ds, &lp, req).unwrap_err();
    assert!(
        matches!(err, CommandError::NeedDualFormConfirm { .. }),
        "未确认双形态应返回 NeedDualFormConfirm，got {err:?}"
    );
    let reg = Registry::load_at(&ds, &lp).unwrap();
    assert!(reg.list_shared().is_empty(), "需确认时不应写 registry");
}

/// 完全无关 → OriginUnrelated 拒绝，不写 registry（覆盖 AC-6）。
#[test]
fn add_sync_unrelated_rejected() {
    let tmp = make_tempdir("unrelated");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    init_repo_with_origin(&proj, "https://github.com/realowner/realrepo.git");
    let (ds, lp) = ready_ds_lp(&tmp);

    // 手填一个与 origin 完全无关的地址（选错了项目目录）
    let req = sync_req("wrong", "https://github.com/wrong/project", &proj, false);
    let err = add_project(&ds, &lp, req).unwrap_err();
    assert!(
        matches!(err, CommandError::OriginUnrelated { .. }),
        "无关 origin 应拒绝保存，got {err:?}"
    );
    let reg = Registry::load_at(&ds, &lp).unwrap();
    assert!(reg.list_shared().is_empty(), "拒绝后不应写 registry");
}

/// 本地无 origin（新 git init 未加 remote）→ 回退用手填作 primary。
#[test]
fn add_sync_no_origin_fallback() {
    let tmp = make_tempdir("no-origin");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    git(&proj, &["init", "-q", "-b", "main"]); // 无 origin remote
    let (ds, lp) = ready_ds_lp(&tmp);

    let req = sync_req("solo", "https://github.com/manual/entry", &proj, false);
    let r = add_project(&ds, &lp, req).expect("无 origin 应回退手填成功");
    let uuid = r.project_id.unwrap();
    let reg = Registry::load_at(&ds, &lp).unwrap();
    let p = reg.get_shared(&uuid).unwrap();
    assert_eq!(p.git_url, "https://github.com/manual/entry", "无 origin 回退用手填");
    assert!(p.git_url_aliases.is_empty());
}

/// 自建 Bitbucket 一致路径（AC-1）：origin 与手填都是 web 形式且一致 → 成功归一化为小写。
#[test]
fn add_company_bitbucket_consistent() {
    let tmp = make_tempdir("company-bb");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    init_repo_with_origin(
        &proj,
        "http://git.example.org/projects/TEAM/repos/ExampleApp",
    );
    let (ds, lp) = ready_ds_lp(&tmp);

    let req = sync_req(
        "project-alpha",
        "http://git.example.org/projects/TEAM/repos/ExampleApp",
        &proj,
        false,
    );
    let r = add_project(&ds, &lp, req).expect("公司 Bitbucket 一致应成功");
    let uuid = r.project_id.unwrap();
    let reg = Registry::load_at(&ds, &lp).unwrap();
    let p = reg.get_shared(&uuid).unwrap();
    assert_eq!(
        p.git_url,
        "https://git.example.org/projects/team/repos/exampleapp"
    );
}

/// Q1-C 收紧：origin 存在但归一化失败（格式坏）→ 拒绝，不 fail-open 回退手填。
#[test]
fn add_sync_origin_unnormalizable_rejected() {
    let tmp = make_tempdir("bad-origin");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    // origin 是单段 URL，normalize 会拒（段数 < 2）
    init_repo_with_origin(&proj, "https://weird.example/singleseg");
    let (ds, lp) = ready_ds_lp(&tmp);

    let req = sync_req("x", "https://github.com/some/repo", &proj, false);
    let err = add_project(&ds, &lp, req).unwrap_err();
    assert!(
        matches!(err, CommandError::OriginUnrelated { .. }),
        "有 origin 但归一化失败应拒绝（不回退），got {err:?}"
    );
    let reg = Registry::load_at(&ds, &lp).unwrap();
    assert!(reg.list_shared().is_empty(), "拒绝后不应写 registry");
}

/// Bitbucket 双形态反向（origin=web、手填=clone）确认后：primary=真实origin(web)，手填(clone)进 aliases。
#[test]
fn add_sync_bitbucket_dualform_reverse_confirmed() {
    let tmp = make_tempdir("dualform-reverse");
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    // 真实 origin = web 形式
    init_repo_with_origin(&proj, "https://bb.example/projects/TEAM/repos/ExampleApp");
    let (ds, lp) = ready_ds_lp(&tmp);

    let req = sync_req(
        "project-alpha",
        "https://bb.example/scm/team/exampleapp.git",
        &proj,
        true,
    );
    let r = add_project(&ds, &lp, req).expect("反向双形态确认应成功");
    let uuid = r.project_id.unwrap();
    let reg = Registry::load_at(&ds, &lp).unwrap();
    let p = reg.get_shared(&uuid).unwrap();
    assert_eq!(
        p.git_url, "https://bb.example/projects/team/repos/exampleapp",
        "primary 服从真实 origin（web 形式）"
    );
    assert_eq!(
        p.git_url_aliases,
        vec!["https://bb.example/scm/team/exampleapp".to_string()],
        "手填 clone 归一化进 aliases"
    );
}

/// Q1-C：锁定 CommandError Display 的 tag 前缀契约（前端靠 tag 识别，重构 Display 不能破坏）。
#[test]
fn command_error_tags_are_stable() {
    let e1 = CommandError::NeedDualFormConfirm {
        origin: "o".into(),
        input: "i".into(),
    };
    assert!(
        e1.to_string().starts_with("[NEED_DUAL_FORM_CONFIRM]"),
        "NeedDualFormConfirm 必须以 tag 前缀开头，实际: {e1}"
    );
    let e2 = CommandError::OriginUnrelated {
        actual_origin: "o".into(),
        input: "i".into(),
    };
    assert!(
        e2.to_string().starts_with("[ORIGIN_UNRELATED]"),
        "OriginUnrelated 必须以 tag 前缀开头，实际: {e2}"
    );
}
