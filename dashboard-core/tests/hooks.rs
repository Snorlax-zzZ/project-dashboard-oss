// §11 hooks 集成测试 —— 幂等安装 / 卸载 / 冲突检测 / strip 保留用户其他 hook 内容

use dashboard_core::hooks::{
    detect_hook_framework, hook_block, install_hook, mac_launchagent_plist, strip_fleet_block,
    uninstall_hook, windows_scheduled_task_xml, HookFramework, HOOK_BEGIN, HOOK_END,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-hooks-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&base).unwrap();
    base
}

fn init_repo(cwd: &Path) {
    let _ = Command::new("git").args(["init", "-q"]).current_dir(cwd).output();
}

#[test]
fn install_creates_hook_with_begin_end_markers() {
    let tmp = make_tempdir("basic-install");
    init_repo(&tmp);
    let fleet_bin = PathBuf::from("/opt/fleet/fleet");
    let log = PathBuf::from("/tmp/hook.log");
    let installed = install_hook(&tmp, &fleet_bin, "proj-1", &log).unwrap();
    assert!(installed.content.contains(HOOK_BEGIN));
    assert!(installed.content.contains(HOOK_END));
    assert!(installed.content.contains("collect --push --project proj-1"));
    assert!(installed.content.contains("/opt/fleet/fleet"));
    assert!(installed.content.trim_end().ends_with(HOOK_END));
}

#[test]
fn install_is_idempotent() {
    let tmp = make_tempdir("idempotent");
    init_repo(&tmp);
    let fleet_bin = PathBuf::from("/opt/fleet/fleet");
    let log = PathBuf::from("/tmp/hook.log");
    let a = install_hook(&tmp, &fleet_bin, "proj-1", &log).unwrap();
    let b = install_hook(&tmp, &fleet_bin, "proj-1", &log).unwrap();
    assert_eq!(a.content, b.content);
    // 只有一个 BEGIN 块
    assert_eq!(b.content.matches(HOOK_BEGIN).count(), 1);
    assert_eq!(b.content.matches(HOOK_END).count(), 1);
}

#[test]
fn install_preserves_existing_user_hook_content() {
    let tmp = make_tempdir("preserve");
    init_repo(&tmp);
    let hook_path = tmp.join(".git/hooks/post-commit");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    let user_hook = "#!/bin/bash\n# my custom hook\nnpm test\n";
    fs::write(&hook_path, user_hook).unwrap();
    let fleet_bin = PathBuf::from("/opt/fleet/fleet");
    let log = PathBuf::from("/tmp/hook.log");
    let installed = install_hook(&tmp, &fleet_bin, "proj-1", &log).unwrap();
    assert!(installed.content.contains("# my custom hook"));
    assert!(installed.content.contains("npm test"));
    assert!(installed.content.contains("collect --push --project proj-1"));
}

#[test]
fn uninstall_removes_fleet_block_only() {
    let tmp = make_tempdir("uninstall");
    init_repo(&tmp);
    let hook_path = tmp.join(".git/hooks/post-commit");
    fs::create_dir_all(hook_path.parent().unwrap()).unwrap();
    fs::write(&hook_path, "#!/bin/bash\nnpm test\n").unwrap();
    let fleet_bin = PathBuf::from("/opt/fleet/fleet");
    install_hook(&tmp, &fleet_bin, "proj-1", &PathBuf::from("/tmp/l.log")).unwrap();
    let removed = uninstall_hook(&tmp).unwrap();
    assert!(removed);
    let remaining = fs::read_to_string(&hook_path).unwrap();
    assert!(remaining.contains("npm test"));
    assert!(!remaining.contains(HOOK_BEGIN));
    assert!(!remaining.contains(HOOK_END));
}

#[test]
fn uninstall_deletes_hook_if_only_fleet_block_remains() {
    let tmp = make_tempdir("uninstall-only");
    init_repo(&tmp);
    let fleet_bin = PathBuf::from("/opt/fleet/fleet");
    install_hook(&tmp, &fleet_bin, "proj-1", &PathBuf::from("/tmp/l.log")).unwrap();
    let removed = uninstall_hook(&tmp).unwrap();
    assert!(removed);
    // 只有 shebang 时应该被删掉
    assert!(!tmp.join(".git/hooks/post-commit").exists());
}

#[test]
fn uninstall_returns_false_when_no_hook() {
    let tmp = make_tempdir("uninstall-none");
    init_repo(&tmp);
    assert!(!uninstall_hook(&tmp).unwrap());
}

#[test]
fn detect_husky_via_dot_husky_dir() {
    let tmp = make_tempdir("husky");
    fs::create_dir_all(tmp.join(".husky")).unwrap();
    assert_eq!(detect_hook_framework(&tmp), Some(HookFramework::Husky));
}

#[test]
fn detect_husky_via_package_json() {
    let tmp = make_tempdir("husky-pkg");
    fs::write(tmp.join("package.json"), r#"{"husky":{"hooks":{}}}"#).unwrap();
    assert_eq!(detect_hook_framework(&tmp), Some(HookFramework::Husky));
}

#[test]
fn detect_lefthook_via_yml() {
    let tmp = make_tempdir("lefthook");
    fs::write(tmp.join("lefthook.yml"), "pre-commit:\n  commands: {}\n").unwrap();
    assert_eq!(detect_hook_framework(&tmp), Some(HookFramework::Lefthook));
}

#[test]
fn detect_pre_commit_framework() {
    let tmp = make_tempdir("pre-commit");
    fs::write(tmp.join(".pre-commit-config.yaml"), "repos: []\n").unwrap();
    assert_eq!(detect_hook_framework(&tmp), Some(HookFramework::PreCommit));
}

#[test]
fn hook_block_format_matches_spec() {
    let block = hook_block(Path::new("/x/fleet"), "proj-abc", Path::new("/y/hook.log"));
    assert!(block.contains(&format!("\"/x/fleet\" collect --push --project proj-abc")));
    assert!(block.contains("&\n")); // fork 后台
    assert!(block.contains("2>>\"/y/hook.log\""));
}

#[test]
fn strip_removes_only_fleet_block() {
    let content = format!(
        "#!/bin/bash\nnpm test\n{HOOK_BEGIN}\nfleet stuff\n{HOOK_END}\necho done\n"
    );
    let stripped = strip_fleet_block(&content);
    assert!(stripped.contains("npm test"));
    assert!(stripped.contains("echo done"));
    assert!(!stripped.contains("fleet stuff"));
    assert!(!stripped.contains(HOOK_BEGIN));
    assert!(!stripped.contains(HOOK_END));
}

#[test]
fn win_task_xml_contains_binary_and_triggers() {
    let xml = windows_scheduled_task_xml("fleet-cron", Path::new(r"C:\fleet\fleet.exe"));
    assert!(xml.contains("<LogonTrigger>"));
    assert!(xml.contains("PT6H"));
    assert!(xml.contains("collect --push --all") || xml.contains("Arguments>collect"));
    assert!(xml.contains(r"C:\\fleet\\fleet.exe"));
}

#[test]
fn mac_plist_contains_binary_and_interval() {
    let plist = mac_launchagent_plist("fleet.cron", Path::new("/usr/local/bin/fleet"));
    assert!(plist.contains("<key>Label</key>"));
    assert!(plist.contains("fleet.cron"));
    assert!(plist.contains("21600"));
    assert!(plist.contains("/usr/local/bin/fleet"));
}
