use dashboard_core::commands::{self, SettingsUpdate};
use dashboard_core::config::{load_dashboard_state_root_from_read_only, LocalConfig};
use std::fs;
use std::path::{Path, PathBuf};

fn make_tempdir(tag: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!(
        "repowiki-config-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&base).unwrap();
    base
}

fn seed_local(root: &Path) -> (PathBuf, PathBuf) {
    let dashboard_state = root.join("dashboard-state");
    let local_path = root.join("local.yaml");
    fs::write(
        &local_path,
        serde_yaml::to_string(&LocalConfig::new("test-machine")).unwrap(),
    )
    .unwrap();
    (dashboard_state, local_path)
}

#[test]
fn old_local_config_defaults_repowiki_paths_to_absent() {
    let yaml = r#"
schema_version: 1
machine_id: test-machine
work_dirs: []
private_projects: []
diagnostic_mode: false
sync_tick_minutes: 3
"#;

    let config: LocalConfig = serde_yaml::from_str(yaml).expect("old local config loads");

    assert_eq!(config.repowiki_cli_path, None);
    assert_eq!(config.repowiki_data_root, None);
}

#[test]
fn read_only_dashboard_root_lookup_never_runs_legacy_backup_preflight() {
    let temp = make_tempdir("read-only-root");
    let local_path = temp.join("local.yaml");
    let dashboard_state = temp.join("custom-state");
    fs::write(
        &local_path,
        format!(
            "schema_version: 1\nmachine_id: test-machine\nwork_dirs: []\nprivate_projects: []\ndiagnostic_mode: false\nsync_tick_minutes: 3\ndashboard_state_root: {}\npending_sync_projects:\n  - name: legacy\n",
            dashboard_state.display()
        ),
    )
    .unwrap();
    let before = fs::read(&local_path).unwrap();

    let resolved = load_dashboard_state_root_from_read_only(&local_path);

    assert_eq!(resolved, dashboard_state);
    assert_eq!(fs::read(&local_path).unwrap(), before);
    assert!(!temp.join("backups").exists());
}

#[test]
fn repowiki_paths_are_local_config_fields() {
    let mut config = LocalConfig::new("test-machine");
    config.repowiki_cli_path = Some("/opt/demo-tools/repowiki".to_string());
    config.repowiki_data_root = Some("/data/repowiki".to_string());

    let serialized = serde_yaml::to_string(&config).expect("serialize local config");
    let restored: LocalConfig = serde_yaml::from_str(&serialized).expect("restore local config");

    assert_eq!(
        restored.repowiki_cli_path.as_deref(),
        Some("/opt/demo-tools/repowiki")
    );
    assert_eq!(
        restored.repowiki_data_root.as_deref(),
        Some("/data/repowiki")
    );
}

#[test]
fn settings_update_accepts_absolute_repowiki_paths_and_preserves_tri_state() {
    let temp = make_tempdir("absolute");
    let (dashboard_state, local_path) = seed_local(&temp);
    let cli = temp.join("bin").join("repowiki");
    let data_root = temp.join("repowiki-data");
    fs::create_dir_all(cli.parent().unwrap()).unwrap();
    fs::create_dir_all(&data_root).unwrap();
    fs::write(&cli, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
    }

    let updated = commands::update_settings(
        &dashboard_state,
        &local_path,
        SettingsUpdate {
            sync_tick_minutes: None,
            diagnostic_mode: None,
            dashboard_state_remote: None,
            dashboard_state_root: None,
            repowiki_cli_path: Some(Some(format!("  {}  ", cli.display()))),
            repowiki_data_root: Some(Some(format!("  {}  ", data_root.display()))),
        },
    )
    .unwrap();
    assert_eq!(updated.repowiki_cli_path.as_deref(), cli.to_str());
    assert_eq!(updated.repowiki_data_root.as_deref(), data_root.to_str());

    let preserved = commands::update_settings(
        &dashboard_state,
        &local_path,
        SettingsUpdate {
            sync_tick_minutes: Some(9),
            diagnostic_mode: None,
            dashboard_state_remote: None,
            dashboard_state_root: None,
            repowiki_cli_path: None,
            repowiki_data_root: None,
        },
    )
    .unwrap();
    assert_eq!(preserved.repowiki_cli_path.as_deref(), cli.to_str());
    assert_eq!(preserved.repowiki_data_root.as_deref(), data_root.to_str());
}

#[cfg(unix)]
#[test]
fn settings_update_atomically_replaces_local_yaml() {
    use std::os::unix::fs::MetadataExt;

    let temp = make_tempdir("atomic-local-settings");
    let (dashboard_state, local_path) = seed_local(&temp);
    let before_inode = fs::metadata(&local_path).unwrap().ino();

    commands::update_settings(
        &dashboard_state,
        &local_path,
        SettingsUpdate {
            sync_tick_minutes: Some(9),
            diagnostic_mode: None,
            dashboard_state_remote: None,
            dashboard_state_root: None,
            repowiki_cli_path: None,
            repowiki_data_root: None,
        },
    )
    .unwrap();

    let after_inode = fs::metadata(&local_path).unwrap().ino();
    assert_ne!(before_inode, after_inode, "设置保存必须原子替换 local.yaml");
    let restored: LocalConfig =
        serde_yaml::from_str(&fs::read_to_string(&local_path).unwrap()).unwrap();
    assert_eq!(restored.sync_tick_minutes, 9);
}

#[test]
fn settings_update_rejects_relative_repowiki_paths_without_writing() {
    for (cli, data_root) in [
        (Some(Some("bin/repowiki".to_string())), None),
        (None, Some(Some("repowiki-data".to_string()))),
    ] {
        let temp = make_tempdir("relative");
        let (dashboard_state, local_path) = seed_local(&temp);
        let before = fs::read(&local_path).unwrap();

        let result = commands::update_settings(
            &dashboard_state,
            &local_path,
            SettingsUpdate {
                sync_tick_minutes: None,
                diagnostic_mode: None,
                dashboard_state_remote: None,
                dashboard_state_root: None,
                repowiki_cli_path: cli,
                repowiki_data_root: data_root,
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read(&local_path).unwrap(), before);
    }
}

#[test]
fn repowiki_only_settings_update_does_not_touch_dashboard_state() {
    let temp = make_tempdir("repowiki-only");
    let (dashboard_state, local_path) = seed_local(&temp);
    let cli = temp.join("bin").join("repowiki");
    let data_root = temp.join("repowiki-data");
    fs::create_dir_all(cli.parent().unwrap()).unwrap();
    fs::create_dir_all(&data_root).unwrap();
    fs::create_dir_all(&dashboard_state).unwrap();
    fs::write(dashboard_state.join("sentinel"), b"UNCHANGED").unwrap();
    fs::write(&cli, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o700)).unwrap();
    }

    let updated = commands::update_repowiki_settings(
        &dashboard_state,
        &local_path,
        Some(cli.display().to_string()),
        Some(data_root.display().to_string()),
    )
    .unwrap();

    assert_eq!(updated.repowiki_cli_path.as_deref(), cli.to_str());
    assert_eq!(updated.repowiki_data_root.as_deref(), data_root.to_str());
    assert_eq!(
        fs::read(dashboard_state.join("sentinel")).unwrap(),
        b"UNCHANGED"
    );
    assert_eq!(fs::read_dir(&dashboard_state).unwrap().count(), 1);
}
