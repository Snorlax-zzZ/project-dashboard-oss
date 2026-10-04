// 组 1 首启自举（决策 1）：新用户无 local.yaml 时，设置页读返内存默认（不落盘）、首次保存原子建档。
//
// 修复前 get_settings 裸 read_to_string，NotFound → Err(Io)，设置页读、存都失败，无法自举
// 首份配置（2026-07-18 全面审计 P0）。本测试锁定：读返默认且不落盘、首存建档且可回读。

use dashboard_core::commands::{
    get_settings, is_legacy_hostname_id, resolve_machine_id, update_settings, SettingsUpdate,
};
use std::path::PathBuf;

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "fleet-first-run-{}-{}-{}",
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

/// SettingsUpdate 无 Default derive，全字段显式构造：只改 sync_tick_minutes，其余未传（保持原值）。
fn only_tick(minutes: u32) -> SettingsUpdate {
    SettingsUpdate {
        sync_tick_minutes: Some(minutes),
        diagnostic_mode: None,
        dashboard_state_remote: None,
        dashboard_state_root: None,
        repowiki_cli_path: None,
        repowiki_data_root: None,
    }
}

#[test]
fn get_settings_returns_default_when_file_missing() {
    let tmp = make_tempdir("get-default");
    let local_path = tmp.join("local.yaml"); // 故意不创建
    assert!(!local_path.exists());

    let cfg = get_settings(&local_path).expect("首启无 local.yaml 应返内存默认而非 Err");

    // 内存默认：machine_id **故意留空**、remote 为 None（read_configured_remote 零回归的前提）。
    // 留空的原因：若在此计算硬件身份并 `?`，取不到标识时连设置页都打不开，用户就没法进去设
    // DASHBOARD_MACHINE_ID 自救。空值只活在内存，写侧 update_settings 落盘前必定补成真身份。
    assert_eq!(cfg.machine_id, "");
    assert_eq!(cfg.dashboard_state_remote, None);

    // 决策 1 承重不变量：只读不落盘（get_settings 在锁内外被反复调，首读落盘会引入锁内不可控写）
    assert!(
        !local_path.exists(),
        "get_settings 遇缺失文件不得落盘（首份文件只由 update_settings 建）"
    );
}

#[test]
fn update_settings_bootstraps_first_config_on_missing_file() {
    let tmp = make_tempdir("bootstrap");
    let local_path = tmp.join("local.yaml"); // 无 local.yaml
    let state_root = tmp.join("state");
    std::fs::create_dir_all(&state_root).unwrap(); // 空 root（存在但未 clone、无 .git）
    assert!(!local_path.exists());

    // 首次保存任一字段 → 自举建档（写侧 write_local_config_atomic 建父目录 + 原子首份文件）
    let saved = update_settings(&state_root, &local_path, only_tick(7))
        .expect("首启首次保存应自举建档成功");
    assert_eq!(saved.sync_tick_minutes, 7);
    // 落盘的身份必须是真身份（硬件式 `<前缀>-<8位hex>`），绝不能把空值或旧式主机名写进去。
    assert_eq!(saved.machine_id, resolve_machine_id(None).unwrap());
    assert!(!saved.machine_id.is_empty(), "空身份不得落盘");
    assert!(
        !is_legacy_hostname_id(&saved.machine_id),
        "落盘身份不得是旧式主机名：{}",
        saved.machine_id
    );
    // 主机名作为展示字段一并留痕，供日后诊断漂移。
    assert!(!saved.hostname.is_empty(), "应记录当次主机名");
    assert_eq!(saved.hostname_history, vec![saved.hostname.clone()]);

    // 原子建档：文件已存在
    assert!(local_path.exists(), "首次保存应原子建 local.yaml");

    // 重启后可回填该配置
    let reread = get_settings(&local_path).expect("回读应成功");
    assert_eq!(reread.sync_tick_minutes, 7);
    assert_eq!(reread.machine_id, saved.machine_id);
    assert_eq!(reread.dashboard_state_remote, None);
}
