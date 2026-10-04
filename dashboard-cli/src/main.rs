// dashboard-cli：M1.5 起为主二进制，供 hook / 排障 / 隐私事故用。日常走 Tauri App。
//
// 用法：
//   fleet                              默认列会话（对拍 M1 Python 版）
//   fleet <query>                      全文搜索
//   fleet --json                       会话列表 JSON
//   fleet sessions --details N         会话详情 JSON
//   fleet collect --project <uuid> [--push]      采集单个项目并推送
//   fleet collect --all [--push]                 采集所有已注册项目
//   fleet emergency-purge --project <name>       隐私事故备胎（前置检测 + 步骤指引）
//   fleet --version

use dashboard_core::commands;
use dashboard_core::sessions;
use dashboard_core::sync::Trigger;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("dashboard-cli {VERSION}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return ExitCode::SUCCESS;
    }

    match args.first().map(String::as_str) {
        Some("collect") => cmd_collect(&args[1..]),
        Some("emergency-purge") => cmd_emergency_purge(&args[1..]),
        Some("sessions") => cmd_sessions(&args[1..]),
        _ => cmd_sessions(&args),
    }
}

fn print_help() {
    let bin = "fleet";
    println!(
        "project-dashboard CLI {VERSION}\n\n\
         用法（日常走 Tauri App，本 CLI 供 hook / 排障 / 事故用）：\n  \
           {bin}                              列会话\n  \
           {bin} <query>                      全文搜索会话\n  \
           {bin} --json                       会话列表 JSON\n  \
           {bin} sessions --details N         会话详情 JSON\n  \
           {bin} collect --project <uuid> [--push]     采集单个项目\n  \
           {bin} collect --all [--push]                采集所有\n  \
           {bin} emergency-purge --project <name>      隐私事故备胎\n"
    );
}

fn cmd_sessions(args: &[String]) -> ExitCode {
    if let Err(error) = dashboard_core::session_storage::default_storage().ensure_initialized() {
        eprintln!("会话目录初始化失败：{error}");
        return ExitCode::FAILURE;
    }
    let json_mode = args.iter().any(|a| a == "--json");
    if let Some(idx) = args.iter().position(|a| a == "--details") {
        let n: usize = args.get(idx + 1).and_then(|s| s.parse().ok()).unwrap_or(5);
        let metas = sessions::scan();
        let mut out: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for m in metas.iter().take(n) {
            let d = sessions::detail(m, 10);
            out.insert(
                m.session_id.clone(),
                serde_json::json!({ "preview": d.preview, "files": d.files }),
            );
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return ExitCode::SUCCESS;
    }
    let query: String = args
        .iter()
        .filter(|a| a.as_str() != "--json")
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let metas = sessions::scan();
    let filtered = sessions::search(&metas, &query);
    if json_mode {
        let owned: Vec<sessions::SessionMeta> = filtered.into_iter().cloned().collect();
        println!("{}", serde_json::to_string_pretty(&owned).unwrap_or_default());
        return ExitCode::SUCCESS;
    }
    if filtered.is_empty() {
        eprintln!("(no sessions matched)");
        return ExitCode::SUCCESS;
    }
    for m in filtered {
        println!(
            "[{}] {:<20} {}",
            m.age(),
            m.projects,
            if m.title.is_empty() { &m.session_id } else { &m.title }
        );
    }
    ExitCode::SUCCESS
}

fn cmd_collect(args: &[String]) -> ExitCode {
    let push = args.iter().any(|a| a == "--push");
    let all = args.iter().any(|a| a == "--all");
    let project_uuid = arg_value(args, "--project");
    if !all && project_uuid.is_none() {
        eprintln!("collect: 需要 --project <uuid> 或 --all");
        return ExitCode::from(2);
    }
    let dashboard_state_root = default_dashboard_state_root();
    let local_path = dashboard_core::config::default_local_config_path();
    // 身份解析放在采集之前：取不到就直接失败退出，绝不带着假身份往下走。
    let machine_id = match resolve_machine_id_for_collect(&dashboard_state_root, &local_path) {
        Ok(id) => id,
        Err(message) => {
            eprintln!("collect failed: {message}");
            return ExitCode::FAILURE;
        }
    };
    // 决策 4：trigger 固定 Manual（不再用 Hook/Manual 反转语义）；push 通过显式参数控制
    let trigger = Trigger::Manual;

    // 审计对账 P0-G（2026-07-06）：--project 走单项目路径，只有 --all 才走全库 manual_sync。
    let result = if let Some(uuid_s) = project_uuid.as_deref() {
        match uuid::Uuid::parse_str(uuid_s) {
            Ok(pid) => {
                // v4.2 §16.5：单项目采集默认开防伪校验（hook / --project 都是精确定位，值得跑一次 shell out）
                let verify_remote = true;
                commands::collect_single(
                    &dashboard_state_root,
                    &local_path,
                    &machine_id,
                    pid,
                    trigger,
                    verify_remote,
                    push,
                )
            }
            Err(_) => {
                eprintln!("collect: --project 必须是合法 UUID，收到: {uuid_s}");
                return ExitCode::from(2);
            }
        }
    } else {
        commands::manual_sync(&dashboard_state_root, &local_path, &machine_id, trigger, push)
    };

    match result {
        Ok(r) => {
            println!(
                "collected {} project(s) at {}, pushed={}",
                r.project_count,
                r.collected_at.to_rfc3339(),
                r.pushed
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("collect failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_emergency_purge(args: &[String]) -> ExitCode {
    let name = match arg_value(args, "--project") {
        Some(n) => n,
        None => {
            eprintln!("emergency-purge: 需要 --project <name>");
            return ExitCode::from(2);
        }
    };
    let precheck = commands::emergency_purge_precheck();
    if !precheck.git_filter_repo_installed {
        eprintln!(
            "❌ 未检测到 git-filter-repo。\n\
             安装：Win → `pip install git-filter-repo`；Mac → `brew install git-filter-repo`\n\
             安装完后再跑本命令。"
        );
        return ExitCode::from(3);
    }
    // 只输出步骤指引，不真跑 filter-repo（防止误 force push）
    let dashboard_state_root = default_dashboard_state_root();
    println!(
        "⚠️  emergency-purge 步骤指引（本命令**不自动执行 force push**，避免误伤）：\n\
         \n\
         1. 确认要清除的项目：`{name}`\n\
         2. cd 到本机 dashboard-state clone：{}\n\
         3. 执行：`git filter-repo --path registry/projects/<uuid-of-{name}>.yaml --invert-paths`\n\
         4. `git push --force`（**这一步会重写远端历史**）\n\
         5. 本机 clone 重新拉：`rm -rf {} && git clone <remote> {}`\n\
         6. 手工 push `.emergency-purge-<timestamp>` 空文件到 dashboard-state 仓根\n\
         7. 打开另一台机 Tauri App → 检测到标记 → 一键清理老 clone\n\
         \n\
         或者：走 Tauri App 内的 Emergency Purge 向导（M2.5 补）。\n",
        dashboard_state_root.display(),
        dashboard_state_root.display(),
        dashboard_state_root.display(),
    );
    ExitCode::SUCCESS
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    let idx = args.iter().position(|a| a == flag)?;
    args.get(idx + 1).cloned()
}

/// 解析本机身份，**与 App 同源**：local.yaml 冻结值优先，缺失才算硬件标识
/// （见 `commands::resolve_machine_id`）。冻结值优先是关键 —— CLI 与 App 必须落到同一个
/// `machines/<id>/`，否则 hook 采集会另开一个目录，云端再次裂成两个身份。
///
/// **取不到身份返回 Err，绝不兜底**。旧实现在这里兜底成 `unknown-machine` 照样往云端写快照，
/// 结果 `machines/` 下永久多出一个谁都不认的孤儿目录（实测已存在，本次改造正是为清它）。
/// hook 触发的采集宁可当次失败，也不能污染跨机数据。
fn resolve_machine_id_for_collect(
    dashboard_state_root: &Path,
    local_path: &Path,
) -> Result<String, String> {
    let local = dashboard_core::config::Registry::load_at_read_only(dashboard_state_root, local_path)
        .ok()
        .and_then(|registry| registry.local);
    commands::resolve_machine_id(local.as_ref()).map_err(|e| {
        format!(
            "[MACHINE_IDENTITY_UNAVAILABLE] 无法确定本机身份：{e}。\
             请设置环境变量 DASHBOARD_MACHINE_ID 后重试（诊断：cargo run -p dashboard-core --example hwid_probe）"
        )
    })
}

fn default_dashboard_state_root() -> PathBuf {
    // 委托 core 统一实现（每次重读固定 local.yaml），与 dashboard-tauri 侧对齐；
    // 消除 CLI 独立硬编码导致 App 改数据目录后 hook 采集仍走老路径的回归。
    dashboard_core::config::load_dashboard_state_root()
}
