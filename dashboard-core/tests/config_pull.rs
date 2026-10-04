// Q2-B 子块 1 集成测试：observe 观测采集 + git_clone 成功/失败清理端到端。
// 纯函数四态判定在 sync.rs 单元测试（resolve_status_states）；本文件验证真跑 git 的 IO 层。

use dashboard_core::commands::{
    add_project, delete_project, edit_project, trigger_config_pull, AddProjectRequest, CommandError,
    EditProjectRequest,
};
use dashboard_core::config::Registry;
use dashboard_core::sync::{
    resolve_status, ConfigPullErrorKind, DashboardStateRepo, StateRepoStatus,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-config-pull-{}-{}-{}",
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
    let ok = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?} 应成功");
}

/// 建一个带一次提交的本地裸仓当 remote，返回其绝对路径字符串。
fn make_local_bare(tmp: &Path) -> String {
    let work = tmp.join("origin-work");
    std::fs::create_dir_all(&work).unwrap();
    Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&work)
        .output()
        .unwrap();
    git(&work, &["config", "user.email", "t@project-dashboard"]);
    git(&work, &["config", "user.name", "pd-test"]);
    git(&work, &["config", "commit.gpgsign", "false"]);
    std::fs::write(work.join("README"), "x").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "init"]);
    let bare = tmp.join("origin.git");
    Command::new("git")
        .args([
            "clone",
            "-q",
            "--bare",
            &work.to_string_lossy(),
            &bare.to_string_lossy(),
        ])
        .output()
        .unwrap();
    bare.to_string_lossy().to_string()
}

#[test]
fn observe_reports_git_and_origin() {
    let tmp = make_tempdir("observe");
    let root = tmp.join("dashboard-state-clone");
    std::fs::create_dir_all(&root).unwrap();
    let repo = DashboardStateRepo::new(&root, "m");

    // 空目录（无 .git）：has_git=false，origin=None，root_empty=true
    let o = repo.observe();
    assert!(o.root_exists && o.root_empty && !o.has_git && o.origin.is_none());

    // git init + 配 origin：has_git=true，读到 origin，非空
    Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&root)
        .output()
        .unwrap();
    git(&root, &["remote", "add", "origin", "git@host:team/ds.git"]);
    // 决策 3：补合法结构，否则 resolve_status 判 InvalidStructure 而非 Ready/RemoteMismatch
    std::fs::create_dir_all(root.join("registry").join("projects")).unwrap();
    let o = repo.observe();
    assert!(o.has_git, "有 .git");
    assert_eq!(o.origin.as_deref(), Some("git@host:team/ds.git"));
    assert!(!o.root_empty, ".git 后目录非空");

    // 配置 remote 与 origin 一致 → Ready
    assert_eq!(
        resolve_status(&o, Some("git@host:team/ds.git"), false),
        StateRepoStatus::Ready
    );
    // 配置 remote 与 origin 不一致 → RemoteMismatch（不静默 set-url）
    assert!(matches!(
        resolve_status(&o, Some("git@host:team/other.git"), false),
        StateRepoStatus::Conflict(_)
    ));
}

#[test]
fn git_clone_from_local_bare_succeeds_and_ready() {
    let tmp = make_tempdir("clone-ok");
    let remote = make_local_bare_with_registry(&tmp);
    let root = tmp.join("dashboard-state-clone");
    let repo = DashboardStateRepo::new(&root, "m");

    repo.git_clone(&remote).expect("从本地裸仓 clone 应成功");

    let o = repo.observe();
    assert!(o.has_git, "clone 后有 .git");
    assert_eq!(
        o.origin.as_deref(),
        Some(remote.as_str()),
        "origin 应等于 clone 时用的 remote"
    );
    assert_eq!(resolve_status(&o, Some(&remote), false), StateRepoStatus::Ready);
}

#[test]
fn clone_failure_no_half_init() {
    let tmp = make_tempdir("clone-fail");
    let root = tmp.join("dashboard-state-clone"); // 事前不存在
    let repo = DashboardStateRepo::new(&root, "m");

    // 指向不存在的本地裸仓 → clone 必失败
    let bogus = tmp.join("nonexistent-remote.git");
    let r = repo.git_clone(&bogus.to_string_lossy());
    assert!(r.is_err(), "clone 不存在的 remote 应失败");
    assert!(
        !root.exists(),
        "clone 失败后残留的半初始化 root 应被清理，以便配置即拉可重试"
    );
}

// -------- Q2-B CRUD 门禁 --------

fn shared_req(name: &str, path: &Path) -> AddProjectRequest {
    let mut machines = BTreeMap::new();
    machines.insert(
        "test-machine".to_string(),
        path.to_string_lossy().into_owned(),
    );
    AddProjectRequest {
        name: name.to_string(),
        git_url: format!("https://github.com/team/{name}"),
        path: path.to_string_lossy().into_owned(),
        machines,
        sync: true,
        confirm_dual_form: false,
    }
}

fn priv_req(name: &str) -> AddProjectRequest {
    AddProjectRequest {
        name: name.to_string(),
        git_url: format!("https://github.com/team/{name}"),
        path: format!("/tmp/{name}"),
        machines: BTreeMap::new(),
        sync: false,
        confirm_dual_form: false,
    }
}

/// 建就绪存储仓（有 .git + origin==配置 remote）+ 配同 remote 的 local.yaml，返回 (ds, lp)。
fn ready_ds_lp(tmp: &Path) -> (PathBuf, PathBuf) {
    const REMOTE: &str = "git@ds-host:team/dashboard-state.git";
    let ds = tmp.join("dashboard-state");
    std::fs::create_dir_all(&ds).unwrap();
    git(&ds, &["init", "-q", "-b", "main"]);
    git(&ds, &["remote", "add", "origin", REMOTE]);
    // 决策 3：补合法 dashboard-state 结构（registry/projects），否则 gate 判 InvalidStructure
    std::fs::create_dir_all(ds.join("registry").join("projects")).unwrap();
    let lp = tmp.join(".project-dashboard").join("local.yaml");
    std::fs::create_dir_all(lp.parent().unwrap()).unwrap();
    std::fs::write(
        &lp,
        format!("schema_version: 1\nmachine_id: test-machine\nwork_dirs: []\nprivate_projects: []\ndiagnostic_mode: false\nsync_tick_minutes: 3\ndashboard_state_remote: {REMOTE}\n"),
    )
    .unwrap();
    (ds, lp)
}

/// 未配存储仓地址（Unconfigured）：sync:true 的 add/edit/delete 都被拦；sync:false 私有放行。
#[test]
fn crud_gate_blocks_shared_but_allows_private_when_unconfigured() {
    let tmp = make_tempdir("gate-unconfigured");
    let ds = tmp.join("dashboard-state"); // 无 .git、未配 remote
    let lp = tmp.join(".project-dashboard").join("local.yaml");

    // add sync:true → 拦
    let err = add_project(&ds, &lp, shared_req("shared", &tmp.join("noproj"))).unwrap_err();
    assert!(
        matches!(err, CommandError::InvalidSettings(_)),
        "未配 remote 时 add sync:true 应被拦，got {err:?}"
    );
    assert!(!ds.join("registry").exists(), "拦截后不写 registry");

    // add sync:false → 放行（私有不碰存储仓）
    add_project(&ds, &lp, priv_req("private")).expect("sync:false 私有项目应放行");
    assert_eq!(
        Registry::load_at(&ds, &lp).unwrap().list_private().len(),
        1,
        "私有项目应写入 local.yaml"
    );

    // edit / delete（都是共享操作）→ 拦
    let e = edit_project(
        &ds,
        &lp,
        EditProjectRequest {
            project_id: Uuid::new_v4(),
            name: Some("x".into()),
            git_url: None,
            machines: None,
        },
    )
    .unwrap_err();
    assert!(
        matches!(e, CommandError::InvalidSettings(_)),
        "未配 remote 时 edit 共享应被拦"
    );
    let d = delete_project(&ds, &lp, Uuid::new_v4()).unwrap_err();
    assert!(
        matches!(d, CommandError::InvalidSettings(_)),
        "未配 remote 时 delete 共享应被拦"
    );
}

/// 存储仓就绪（Ready）：门禁放行，add sync:true 正常走 URL 绑定 + 写 registry。
#[test]
fn crud_gate_allows_shared_when_ready() {
    let tmp = make_tempdir("gate-ready");
    let (ds, lp) = ready_ds_lp(&tmp);
    // proj 无 origin → add sync:true 走 NoOrigin 回退手填；关键验证 gate 放行（存储仓 Ready）
    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    git(&proj, &["init", "-q", "-b", "main"]);

    let r = add_project(&ds, &lp, shared_req("shared", &proj))
        .expect("存储仓就绪时 add sync:true 应放行");
    assert!(r.project_id.is_some(), "gate 放行后正常写 registry");
}

// -------- Q2-D2 配置即拉错误分类 + 结构校验接线 --------

/// 建含 dashboard-state 结构（registry/）的本地裸仓当 remote，返回其绝对路径。
fn make_local_bare_with_registry(tmp: &Path) -> String {
    let work = tmp.join("origin-work-reg");
    std::fs::create_dir_all(work.join("registry").join("projects")).unwrap();
    Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&work)
        .output()
        .unwrap();
    git(&work, &["config", "user.email", "t@project-dashboard"]);
    git(&work, &["config", "user.name", "pd-test"]);
    git(&work, &["config", "commit.gpgsign", "false"]);
    std::fs::write(work.join("registry").join("projects").join(".keep"), "").unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "init"]);
    let bare = tmp.join("origin-reg.git");
    Command::new("git")
        .args([
            "clone",
            "-q",
            "--bare",
            &work.to_string_lossy(),
            &bare.to_string_lossy(),
        ])
        .output()
        .unwrap();
    bare.to_string_lossy().to_string()
}

/// 合法结构：clone 到含 registry/ 的仓 → 结构校验通过 → Ok。
/// trigger 返回 clone **前**判定的 NeedsInit（=本次触发了初始化），而非 clone 后的 Ready。
#[test]
fn trigger_pull_clone_valid_structure_ok() {
    let tmp = make_tempdir("trig-valid");
    let remote = make_local_bare_with_registry(&tmp);
    let root = tmp.join("dashboard-state-clone"); // 不存在 → NeedsInit
    let status = trigger_config_pull(&root, Some(&remote), false, true).expect("合法结构 clone 应成功");
    assert_eq!(
        status,
        StateRepoStatus::NeedsInit,
        "trigger 返回 clone 前判定的 NeedsInit（本次触发了初始化）"
    );
    assert!(root.join(".git").is_dir(), "clone 后有 .git");
    assert!(root.join("registry").is_dir(), "clone 后 root 含 registry/（结构校验通过）");
}

/// LEAK-05：git_add_all 只 stage `registry/` + `machines/`，数据根下的无关文件
/// （local.yaml / .env / 密钥）**不进 index**，不会被 commit/push 到跨机共享仓。
#[test]
fn leak05_git_add_all_excludes_untracked_root_files() {
    let tmp = make_tempdir("leak05");
    let remote = make_local_bare_with_registry(&tmp);
    let root = tmp.join("dashboard-state-clone");
    // clone 成 Ready 仓（含 registry/）
    trigger_config_pull(&root, Some(&remote), false, true).expect("clone 合法结构应成功");
    git(&root, &["config", "user.email", "t@project-dashboard"]);
    git(&root, &["config", "user.name", "pd-test"]);

    // 往数据根放"不该出机"的敏感文件 + 合法 machines 快照 + events
    std::fs::write(root.join("local.yaml"), "dashboard_state_remote: secret\n").unwrap();
    std::fs::write(root.join(".env"), "TOKEN=should-not-leak\n").unwrap();
    // 模拟历史 `add -A` 遗留 / 用户手动 stage：.env 在 git_add_all 之前已进 index。
    // 只靠白名单 add 挡不住它，reset 必须把它清出去（堵死 commit 侧的泄露通道）。
    git(&root, &["add", "--", ".env"]);
    std::fs::create_dir_all(root.join("machines").join("m")).unwrap();
    std::fs::write(root.join("machines").join("m").join("snapshot.json"), "{}\n").unwrap();
    std::fs::write(root.join("machines").join("m").join("events.ndjson"), "{}\n").unwrap();

    let repo = DashboardStateRepo::new(&root, "m");
    repo.git_add_all().expect("git_add_all 白名单应成功（registry/ machines/ 均存在）");

    // 读 index：只应含 machines 受管文件，绝不含 local.yaml / 预先 stage 的 .env
    let out = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(&root)
        .output()
        .unwrap();
    let staged = String::from_utf8_lossy(&out.stdout);
    assert!(!staged.contains("local.yaml"), "local.yaml 不得进 index：\n{staged}");
    assert!(
        !staged.contains(".env"),
        "预先 stage 的 .env 也须被 reset 清出 index：\n{staged}"
    );
    assert!(
        staged.contains("machines/m/snapshot.json"),
        "machines 快照应正常进 index（白名单不能漏合法数据）：\n{staged}"
    );
    assert!(
        staged.contains("machines/m/events.ndjson"),
        "machines events 也应正常进 index：\n{staged}"
    );

    // 最终 commit 内容也不得含 .env（reset 已保证 index 只剩白名单，commit 自然干净）。
    repo.git_commit("test whitelist commit").expect("commit 白名单数据");
    let show = Command::new("git")
        .args(["show", "--pretty=", "--name-only", "HEAD"])
        .current_dir(&root)
        .output()
        .unwrap();
    let committed = String::from_utf8_lossy(&show.stdout);
    assert!(
        !committed.contains(".env"),
        "最终 commit 不得含预暂存的 .env：\n{committed}"
    );
    assert!(
        committed.contains("machines/m/snapshot.json"),
        "commit 应含 machines 快照：\n{committed}"
    );
}

/// pathspec 边界：全新仓只有 `registry/`、`machines/` 尚不存在时，`git_add_all` 不应因
/// pathspec 不匹配 fatal（公开方法的边界回归防护）。
#[test]
fn git_add_all_ok_when_machines_dir_absent() {
    let tmp = make_tempdir("leak05-no-machines");
    let remote = make_local_bare_with_registry(&tmp);
    let root = tmp.join("dashboard-state-clone");
    trigger_config_pull(&root, Some(&remote), false, true).expect("clone 合法结构应成功");
    git(&root, &["config", "user.email", "t@project-dashboard"]);
    git(&root, &["config", "user.name", "pd-test"]);
    assert!(!root.join("machines").exists(), "前置：machines/ 尚不存在");

    let repo = DashboardStateRepo::new(&root, "m");
    // 不 panic / 不 fatal 即通过（registry 存在、machines 不存在被跳过）。
    repo.git_add_all()
        .expect("machines/ 不存在时 git_add_all 不应 pathspec fatal");
}

/// 非法结构：clone 成功但拉到的不是 dashboard-state 仓（无 registry/）→ InvalidStructure。
#[test]
fn trigger_pull_clone_invalid_structure_rejected() {
    let tmp = make_tempdir("trig-invalid");
    let remote = make_local_bare(&tmp); // 只有 README，无 registry/
    let root = tmp.join("dashboard-state-clone");
    let err = trigger_config_pull(&root, Some(&remote), false, true).unwrap_err();
    assert!(
        matches!(err, CommandError::ConfigPullFailed { kind } if kind == ConfigPullErrorKind::InvalidStructure),
        "clone 到非 dashboard-state 仓应归 InvalidStructure，got {err:?}"
    );
}

/// clone 失败（远端不存在）→ ConfigPullFailed 带分类，而非裸 Sync。
#[test]
fn trigger_pull_clone_failure_classified() {
    let tmp = make_tempdir("trig-fail");
    let bogus = tmp.join("nonexistent-remote.git");
    let root = tmp.join("dashboard-state-clone");
    let err = trigger_config_pull(&root, Some(&bogus.to_string_lossy()), false, true).unwrap_err();
    assert!(
        matches!(err, CommandError::ConfigPullFailed { .. }),
        "clone 失败应归 ConfigPullFailed（带分类），got {err:?}"
    );
}

/// 前端靠 tag 前缀识别 → ConfigPullFailed 的 Display 必须带 [TAG]。
#[test]
fn config_pull_failed_has_tag_prefix() {
    let e = CommandError::ConfigPullFailed {
        kind: ConfigPullErrorKind::HttpsAuth,
    };
    assert!(e.to_string().starts_with("[HTTPS_AUTH]"), "实际: {e}");
}

/// P1-2 门禁对称性：gate_shared_crud 的 NeedsInit 分支 clone 到非 dashboard-state 仓
/// （只有 README、无 registry/）时，应与 trigger_config_pull 一致归 InvalidStructure，
/// 而非放行让后续 CRUD 把错仓写成半个 dashboard-state（用户填错 remote 场景）。
#[test]
fn crud_gate_needsinit_clone_invalid_structure_rejected() {
    let tmp = make_tempdir("gate-invalid-structure");
    let remote = make_local_bare(&tmp); // 只有 README，无 registry/
    let ds = tmp.join("dashboard-state-clone"); // 不存在 → NeedsInit
    let lp = tmp.join(".project-dashboard").join("local.yaml");
    std::fs::create_dir_all(lp.parent().unwrap()).unwrap();
    std::fs::write(
        &lp,
        format!("schema_version: 1\nmachine_id: test-machine\nwork_dirs: []\nprivate_projects: []\ndiagnostic_mode: false\nsync_tick_minutes: 3\ndashboard_state_remote: {remote}\n"),
    )
    .unwrap();

    let proj = tmp.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    git(&proj, &["init", "-q", "-b", "main"]);

    let err = add_project(&ds, &lp, shared_req("shared", &proj)).unwrap_err();
    assert!(
        matches!(err, CommandError::ConfigPullFailed { kind } if kind == ConfigPullErrorKind::InvalidStructure),
        "gate NeedsInit clone 非法结构应归 InvalidStructure，got {err:?}"
    );
    assert!(!ds.join("registry").exists(), "非法结构不应写 registry");
}
