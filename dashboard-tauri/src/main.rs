// dashboard-tauri backend：Tauri IPC commands 是 dashboard-core::commands 的薄壳。
// 每个 `#[tauri::command]` 只负责参数透传 + Result 转 String，业务逻辑全在 dashboard-core。
//
// ⚠️ M2 §5.2 起手：跑 `tauri init` 前先执行 `mv src/main.rs src/main.rs.bak`，
// tauri init 会生成默认 main.rs 骨架；再把本文件的 commands 注册段合入。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use dashboard_core::commands::{
    self, AddProjectRequest, EditProjectRequest, SettingsUpdate,
};
use dashboard_core::config::default_local_config_path;
use dashboard_core::identity_migration;
use dashboard_core::repowiki::{RepoWikiEnvelope, RepoWikiProjectSelector};
use dashboard_core::sync::Trigger;
use std::path::PathBuf;
use uuid::Uuid;

fn default_dashboard_state_root() -> PathBuf {
    // 每次调用重读固定 local.yaml 拿数据目录根（不缓存，设置页改路径保存即生效）；
    // 未配 / 老配置 → fallback ~/.project-dashboard/dashboard-state-clone。
    dashboard_core::config::load_dashboard_state_root()
}

fn repowiki_dashboard_state_root_read_only() -> PathBuf {
    dashboard_core::config::load_dashboard_state_root_read_only()
}

fn err<T: std::fmt::Display>(e: T) -> String {
    e.to_string()
}

async fn run_repowiki_read<T: Send + 'static>(
    read: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    // 配置、Git 绑定校验和 CLI 都可能阻塞；整个读取链离开 macOS UI 线程。
    tauri::async_runtime::spawn_blocking(read)
        .await
        .map_err(|e| format!("代码知识读取任务执行失败：{e}"))?
}

#[tauri::command]
async fn list_sessions(
    source: String,
    query: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
    visibility: Option<String>,
) -> Result<dashboard_core::sessions::SessionPage, String> {
    // Codex 单会话可能超过 100MB，冷扫虽只读头尾块，也必须离开 macOS UI 主线程。
    tauri::async_runtime::spawn_blocking(move || {
        commands::list_sessions_for_view(
            &source,
            query.as_deref(),
            page.unwrap_or(1),
            page_size.unwrap_or(20),
            visibility.as_deref(),
        )
        .map_err(err)
    })
    .await
    .map_err(|e| format!("读取会话列表任务失败：{e}"))?
}

#[tauri::command]
async fn session_detail(
    source: String,
    session_id: String,
) -> Result<Option<dashboard_core::sessions::SessionDetail>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        commands::session_detail_by_source(&source, &session_id).map_err(err)
    })
    .await
    .map_err(|e| format!("读取会话详情任务失败：{e}"))?
}

#[tauri::command]
fn copy_resume_command(source: String, session_id: String) -> Result<String, String> {
    commands::copy_resume_command_for_source(&source, &session_id).map_err(err)
}

#[tauri::command]
async fn toggle_favorite(source: String, session_id: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        dashboard_core::session_storage::default_storage()
            .toggle_favorite_for_source(&source, &session_id)
            .map_err(err)
    })
    .await
    .map_err(|e| format!("保存收藏任务失败：{e}"))?
}

#[tauri::command]
async fn set_session_hidden(
    source: String,
    session_id: String,
    hidden: bool,
) -> Result<dashboard_core::session_visibility::HiddenSessionUpdate, String> {
    tauri::async_runtime::spawn_blocking(move || {
        commands::set_session_hidden(&source, &session_id, hidden).map_err(err)
    })
    .await
    .map_err(|e| format!("保存隐藏偏好任务失败：{e}"))?
}

#[tauri::command]
fn list_projects() -> Result<commands::ProjectListResponse, String> {
    commands::list_projects(&default_dashboard_state_root()).map_err(err)
}

#[tauri::command]
async fn add_project(req: AddProjectRequest) -> Result<commands::AddProjectResult, String> {
    // 决策 6 修复（2026-07-22）：sync:true 项目 NeedsInit 时锁内 clone 走网络，阻塞主线程会冻结
    // webview（同 manual_sync 症状）。挪进 spawn_blocking 离开主线程（= UI 线程）。
    tauri::async_runtime::spawn_blocking(move || {
        commands::add_project(&default_dashboard_state_root(), &default_local_config_path(), req)
            .map_err(err)
    })
    .await
    .map_err(|e| format!("新增项目任务执行失败：{e}"))?
}

/// v4.2 §16.4 pull-first：新增 / 关联 / 更新 URL 之前必须先调此 IPC 拉最新 registry。
#[tauri::command]
async fn pull_registry_before_write() -> Result<(), String> {
    // 决策 6 修复（2026-07-22）：git_pull/clone 走网络，挪出主线程避免冻结 UI。
    tauri::async_runtime::spawn_blocking(|| {
        commands::pull_registry(&default_dashboard_state_root(), &default_local_config_path())
            .map_err(err)
    })
    .await
    .map_err(|e| format!("拉取 registry 任务执行失败：{e}"))?
}

/// v4.2 §16.4：按 git_url 反查 registry，判断"关联已有 UUID"还是"新建 UUID"。
#[tauri::command]
fn find_project_by_git_url(git_url: String) -> Result<commands::FindProjectResult, String> {
    commands::find_project_by_git_url(
        &default_dashboard_state_root(),
        &default_local_config_path(),
        &git_url,
    )
    .map_err(err)
}

#[tauri::command]
fn edit_project(req: EditProjectRequest) -> Result<(), String> {
    commands::edit_project(&default_dashboard_state_root(), &default_local_config_path(), req)
        .map_err(err)
}

#[tauri::command]
fn delete_project(project_id: Uuid) -> Result<(), String> {
    commands::delete_project(&default_dashboard_state_root(), &default_local_config_path(), project_id)
        .map_err(err)
}

#[tauri::command]
async fn manual_sync() -> Result<commands::ManualSyncResult, String> {
    // 决策 6 修复（2026-07-22）：pull+push 走网络，原同步 command 跑在 macOS 主线程（= UI 线程），
    // 慢 push 时窗口冻结 20s，JS 的 15s watchdog 红条 /「重启 App」按钮无法渲染（真人验实测 FAIL）。
    // 改 async + spawn_blocking 把阻塞 git 挪到专用阻塞线程池，主线程不冻，watchdog / 重启按钮恢复。
    tauri::async_runtime::spawn_blocking(|| {
        let local_path = default_local_config_path();
        // 身份取不到就让同步失败：绝不兜底成假 id 往云端写快照（会留孤儿 machines/ 目录）。
        let settings = commands::get_settings(&local_path).ok();
        let machine_id = commands::resolve_machine_id(settings.as_ref()).map_err(|e| {
            err(commands::CommandError::MachineIdentityUnavailable(e))
        })?;
        commands::manual_sync(
            &default_dashboard_state_root(),
            &local_path,
            &machine_id,
            Trigger::Manual,
            true, // 手动同步一条龙走 push
        )
        .map_err(err)
    })
    .await
    .map_err(|e| format!("同步任务执行失败：{e}"))?
}

#[tauri::command]
async fn get_sync_status() -> Result<dashboard_core::sync_status::SyncStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let local_path = default_local_config_path();
        let settings = dashboard_core::config::load_local_config_read_only(&local_path).map_err(err)?;
        let machine_id = commands::resolve_machine_id(settings.as_ref()).map_err(err)?;
        let root = dashboard_core::config::resolve_dashboard_state_root(
            settings.as_ref().and_then(|s| s.dashboard_state_root.as_deref()),
        );
        commands::get_sync_status(&root, &local_path, &machine_id)
    }).await.map_err(|e| format!("读取同步记录任务失败：{e}"))?
}

#[tauri::command]
fn aggregate_fleet_board() -> Result<Vec<dashboard_core::aggregate::ProjectAggregate>, String> {
    commands::aggregate_fleet_board(&default_dashboard_state_root(), &default_local_config_path())
        .map_err(err)
}

#[tauri::command]
fn get_settings() -> Result<dashboard_core::config::LocalConfig, String> {
    commands::get_settings(&default_local_config_path()).map_err(err)
}

#[tauri::command]
async fn repowiki_status(selector: RepoWikiProjectSelector) -> Result<RepoWikiEnvelope, String> {
    run_repowiki_read(move || {
        dashboard_core::repowiki::status(
            &repowiki_dashboard_state_root_read_only(),
            &default_local_config_path(),
            &selector,
        )
        .map_err(err)
    })
    .await
}

#[tauri::command]
async fn repowiki_read_project(selector: RepoWikiProjectSelector) -> Result<RepoWikiEnvelope, String> {
    run_repowiki_read(move || {
        dashboard_core::repowiki::read_project(
            &repowiki_dashboard_state_root_read_only(),
            &default_local_config_path(),
            &selector,
        )
        .map_err(err)
    })
    .await
}

#[tauri::command]
async fn repowiki_read_page(
    selector: RepoWikiProjectSelector,
    page_id: String,
) -> Result<RepoWikiEnvelope, String> {
    run_repowiki_read(move || {
        dashboard_core::repowiki::read_page(
            &repowiki_dashboard_state_root_read_only(),
            &default_local_config_path(),
            &selector,
            &page_id,
        )
        .map_err(err)
    })
    .await
}

#[tauri::command]
async fn repowiki_read_generations(
    selector: RepoWikiProjectSelector,
) -> Result<RepoWikiEnvelope, String> {
    run_repowiki_read(move || {
        dashboard_core::repowiki::read_generations(
            &repowiki_dashboard_state_root_read_only(),
            &default_local_config_path(),
            &selector,
        )
        .map_err(err)
    })
    .await
}

#[tauri::command]
async fn repowiki_graph_search(
    selector: RepoWikiProjectSelector,
    text: String,
    limit: u32,
) -> Result<RepoWikiEnvelope, String> {
    run_repowiki_read(move || {
        dashboard_core::repowiki::graph_search(
            &repowiki_dashboard_state_root_read_only(),
            &default_local_config_path(),
            &selector,
            &text,
            limit,
        )
        .map_err(err)
    })
    .await
}

#[tauri::command]
async fn repowiki_graph_show(
    selector: RepoWikiProjectSelector,
    node_id: String,
) -> Result<RepoWikiEnvelope, String> {
    run_repowiki_read(move || {
        dashboard_core::repowiki::graph_show(
            &repowiki_dashboard_state_root_read_only(),
            &default_local_config_path(),
            &selector,
            &node_id,
        )
        .map_err(err)
    })
    .await
}

#[tauri::command]
async fn repowiki_graph_callers(
    selector: RepoWikiProjectSelector,
    node_id: String,
    depth: u32,
    limit: u32,
) -> Result<RepoWikiEnvelope, String> {
    run_repowiki_read(move || {
        dashboard_core::repowiki::graph_callers(
            &repowiki_dashboard_state_root_read_only(),
            &default_local_config_path(),
            &selector,
            &node_id,
            depth,
            limit,
        )
        .map_err(err)
    })
    .await
}

#[tauri::command]
async fn repowiki_graph_callees(
    selector: RepoWikiProjectSelector,
    node_id: String,
    depth: u32,
    limit: u32,
) -> Result<RepoWikiEnvelope, String> {
    run_repowiki_read(move || {
        dashboard_core::repowiki::graph_callees(
            &repowiki_dashboard_state_root_read_only(),
            &default_local_config_path(),
            &selector,
            &node_id,
            depth,
            limit,
        )
        .map_err(err)
    })
    .await
}

#[tauri::command]
async fn repowiki_graph_impact(
    selector: RepoWikiProjectSelector,
    node_id: String,
    depth: u32,
    limit: u32,
) -> Result<RepoWikiEnvelope, String> {
    run_repowiki_read(move || {
        dashboard_core::repowiki::graph_impact(
            &repowiki_dashboard_state_root_read_only(),
            &default_local_config_path(),
            &selector,
            &node_id,
            depth,
            limit,
        )
        .map_err(err)
    })
    .await
}

#[tauri::command]
fn update_repowiki_settings(
    repowiki_cli_path: Option<String>,
    repowiki_data_root: Option<String>,
) -> Result<dashboard_core::config::LocalConfig, String> {
    commands::update_repowiki_settings(
        &repowiki_dashboard_state_root_read_only(),
        &default_local_config_path(),
        repowiki_cli_path,
        repowiki_data_root,
    )
    .map_err(err)
}

#[tauri::command]
async fn update_settings(update: SettingsUpdate) -> Result<commands::UpdateSettingsResponse, String> {
    // 决策 6 修复（2026-07-22）：配置即拉（trigger_config_pull）clone/pull 走网络，原同步 command
    // 阻塞主线程冻结 UI。整段（保存 + 配置即拉）挪进 spawn_blocking 离开主线程；部分成功语义不变。
    tauri::async_runtime::spawn_blocking(move || {
        // 1) 保存设置：core 内部持锁写 local.yaml，返回时锁已释放（保持纯保存，不在其中 clone）。
        let cfg = commands::update_settings(
            &default_dashboard_state_root(),
            &default_local_config_path(),
            update,
        )
        .map_err(err)?;
        // 2) 保存后配置即拉：用更新后的数据目录根（default_* 每次重读 local.yaml 拿新值）+ 新 remote。
        //    trigger_config_pull 自拿宽松锁，**必须在 core 保存释放锁后调**（此处已释放，不重入自锁）。
        //    换数据目录 → 新 root 走 NeedsInit clone。
        //    **P1-4 部分成功语义**：local.yaml 此刻已落盘，同步失败不应掩盖"保存成功"。故命令仍返回
        //    Ok（保存成功），把同步子状态放进 UpdateSettingsResponse.sync 交前端消费（D3 toast）——
        //    避免"报保存失败但设置已生效"的认知割裂（换数据目录时尤其危险）。
        let trigger_result = commands::trigger_config_pull(
            &default_dashboard_state_root(),
            cfg.dashboard_state_remote.as_deref(),
            cfg.dashboard_state_remote_cleared,
            true,
        );
        let sync = commands::SyncOutcome::from_trigger_result(&trigger_result);
        // 诊断日志保留：同步失败时留一行 stderr（带 D1 tag），前端 toast 之外便于排障。
        if let commands::SyncOutcome::Failed { tag, hint } = &sync {
            eprintln!("[update_settings] 设置已保存，但配置即拉失败：[{tag}] {hint}");
        }
        Ok(commands::UpdateSettingsResponse { config: cfg, sync })
    })
    .await
    .map_err(|e| format!("更新设置任务执行失败：{e}"))?
}

/// 在系统文件管理器中打开数据目录（dashboard_state_root）。跨平台：macOS `open` / Windows
/// `explorer` / 其他 unix `xdg-open`。目录不存在或被错配成文件（尚未配存储仓 / 手改 local.yaml）
/// 时返回明确提示。launcher 秒退后后台 reap，避免多次点按钮堆积 zombie。
#[tauri::command]
fn open_data_dir() -> Result<(), String> {
    open_directory(&default_dashboard_state_root())
}

#[tauri::command]
async fn open_session_data_dir() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        let storage = dashboard_core::session_storage::default_storage();
        storage.ensure_initialized().map_err(err)?;
        open_directory(storage.root())
    })
    .await
    .map_err(|e| format!("打开本机会话目录任务失败：{e}"))?
}

fn open_directory(dir: &std::path::Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!(
            "数据目录不存在或不是目录：{}（配置存储仓地址后会自动创建）",
            dir.display()
        ));
    }
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    let child = std::process::Command::new(program)
        .arg(&dir)
        .spawn()
        .map_err(|e| format!("打开数据目录失败：{e}"))?;
    // open / xdg-open 是 launcher：fork-exec 真正的文件管理器后自身秒退。后台线程 reap 回收，
    // 避免每次点"打开数据目录"堆积 zombie。Windows explorer.exe 退出码语义特殊、不宜 wait，
    // spawn 后直接 detach（Child drop 不杀进程，文件管理器照常运行）。
    #[cfg(not(target_os = "windows"))]
    std::thread::spawn(move || {
        let mut child = child;
        let _ = child.wait();
    });
    #[cfg(target_os = "windows")]
    let _ = child;
    Ok(())
}

#[tauri::command]
fn emergency_purge_precheck() -> commands::EmergencyPrecheck {
    commands::emergency_purge_precheck()
}

#[tauri::command]
fn get_local_context() -> commands::MachineContext {
    commands::get_local_context(&default_local_config_path())
}

/// 项目登记页的机器下拉数据源。身份改硬件哈希后用户没法凭记忆手填对方机 id，故改选。
#[tauri::command]
fn list_known_machines() -> Vec<commands::KnownMachine> {
    commands::list_known_machines(
        &default_dashboard_state_root(),
        &default_local_config_path(),
    )
}

/// 身份迁移**干跑**：只算不写，前端拿它填确认框（哪几行改成什么）。
#[tauri::command]
fn plan_identity_migration() -> Result<identity_migration::IdentityMigrationPlan, String> {
    let local_path = default_local_config_path();
    let settings = commands::get_settings(&local_path).ok();
    let target = commands::resolve_machine_id(settings.as_ref()).map_err(err)?;
    identity_migration::plan(
        &default_dashboard_state_root(),
        &local_path,
        &target,
        &dashboard_core::hardware_id::current_hostname(),
    )
    .map_err(err)
}

/// 身份迁移**真改**：先整仓备份，再改注册表键 + 合并快照。旧目录一个都不删。
/// 只由设置页按钮显式触发 —— 不做启动自动迁移（两台机同时启动会抢改同一份 yaml）。
#[tauri::command]
async fn apply_identity_migration(
) -> Result<identity_migration::IdentityMigrationReport, String> {
    // 走 spawn_blocking：迁移含整仓拷贝 + 多文件写，别卡 macOS 主线程（= UI 线程）。
    tauri::async_runtime::spawn_blocking(|| {
        let local_path = default_local_config_path();
        let settings = commands::get_settings(&local_path).ok();
        let target = commands::resolve_machine_id(settings.as_ref()).map_err(err)?;
        let state_root = default_dashboard_state_root();
        // 备份落数据目录的兄弟目录 backups/，不放数据目录内（否则被 dashboard-state 仓 git 跟踪）。
        let backups_root = dashboard_core::backup::default_backups_root(&state_root);
        identity_migration::apply(
            &state_root,
            &local_path,
            &backups_root,
            &target,
            &dashboard_core::hardware_id::current_hostname(),
        )
        .map_err(err)
    })
    .await
    .map_err(|e| format!("迁移任务执行失败：{e}"))?
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            session_detail,
            copy_resume_command,
            toggle_favorite,
            set_session_hidden,
            list_projects,
            add_project,
            pull_registry_before_write,
            find_project_by_git_url,
            edit_project,
            delete_project,
            manual_sync,
            get_sync_status,
            aggregate_fleet_board,
            get_settings,
            repowiki_status,
            repowiki_read_project,
            repowiki_read_page,
            repowiki_read_generations,
            repowiki_graph_search,
            repowiki_graph_show,
            repowiki_graph_callers,
            repowiki_graph_callees,
            repowiki_graph_impact,
            update_repowiki_settings,
            update_settings,
            open_data_dir,
            open_session_data_dir,
            emergency_purge_precheck,
            get_local_context,
            list_known_machines,
            plan_identity_migration,
            apply_identity_migration,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod ui_thread_tests {
    use super::run_repowiki_read;

    #[test]
    fn repowiki_reads_run_off_ui_thread() {
        tauri::async_runtime::block_on(async {
            let caller = std::thread::current().id();
            let worker = run_repowiki_read(|| Ok(std::thread::current().id()))
                .await
                .unwrap();
            assert_ne!(caller, worker, "文件与 CLI 读取不得占用调用命令的 UI 线程");
        });
    }

    #[test]
    fn repowiki_read_errors_are_preserved() {
        let result = tauri::async_runtime::block_on(run_repowiki_read(|| {
            Err::<(), _>("[repowiki_cli_not_configured] 未配置 CLI".to_string())
        }));
        assert_eq!(result.unwrap_err(), "[repowiki_cli_not_configured] 未配置 CLI");
    }
}

#[cfg(test)]
mod session_visibility_ipc_tests {
    use super::*;
    #[test]
    fn invalid_session_arguments_return_tagged_errors_without_storage_access() {
        tauri::async_runtime::block_on(async {
            let error = list_sessions("unsupported".into(), None, None, None, None)
                .await
                .unwrap_err();
            assert!(error.contains("UNSUPPORTED_SESSION_SOURCE"));
            let error = list_sessions("pi".into(), None, None, None, Some("wrong".into()))
                .await
                .unwrap_err();
            assert!(error.contains("INVALID_SESSION_VISIBILITY"));
            let error = set_session_hidden("pi".into(), "bad/id".into(), true)
                .await
                .unwrap_err();
            assert!(error.contains("INVALID_SESSION_ID"));
        });
    }
    #[test]
    fn hidden_update_wire_response_round_trips() {
        use dashboard_core::session_visibility::HiddenSessionUpdate;
        let response = HiddenSessionUpdate {
            source: "pi".into(),
            session_id: "same".into(),
            hidden: true,
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"source":"pi","session_id":"same","hidden":true})
        );
        assert_eq!(
            serde_json::from_value::<HiddenSessionUpdate>(value).unwrap(),
            response
        );
    }
}
