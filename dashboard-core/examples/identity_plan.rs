//! 身份迁移干跑：只打印计划，一个字节都不写。
//!
//! 跑法：`cargo run -p dashboard-core --example identity_plan`
//! 用途：迁移前核对「哪几行改成什么」，确认无误再走设置页的迁移按钮。

use dashboard_core::{commands, config, hardware_id, identity_migration};

fn main() {
    let state_root = config::load_dashboard_state_root_read_only();
    let local_path = config::default_local_config_path();
    let hostname = hardware_id::current_hostname();
    let target = match commands::compute_machine_id() {
        Ok(id) => id,
        Err(error) => {
            println!("取不到硬件身份：{error}");
            return;
        }
    };
    match identity_migration::plan(&state_root, &local_path, &target, &hostname) {
        Ok(plan) => println!("{}", serde_json::to_string_pretty(&plan).unwrap()),
        Err(error) => println!("干跑失败：{error}"),
    }
}
