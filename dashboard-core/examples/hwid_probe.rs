//! 硬件标识诊断：打印本机算出的 machine_id 及其各组成部分。
//!
//! 用途：
//! 1. 跨平台核实 —— 在 Windows / Linux 上跑一次，确认取标识的路径真能命中
//!    （Windows 走 `reg.exe query` 读 MachineGuid，该分支未在 mac 上验证过）。
//! 2. 排查漂移 —— 主机名变了但硬件哈希不变，即可确认是同一台机改了名。
//!
//! 跑法：`cargo run -p dashboard-core --example hwid_probe`
//!
//! 注意：只打印哈希，不打印硬件标识原文 —— 原文是设备指纹，不该落进终端记录或日志。

use dashboard_core::hardware_id;

fn main() {
    let hostname = hardware_id::current_hostname();
    println!("当前主机名      : {hostname}");
    match hardware_id::hardware_hash() {
        Ok(hash) => {
            let prefix = hardware_id::derive_prefix(&hostname);
            println!("硬件哈希(前8位) : {hash}");
            println!("推导短前缀      : {prefix}");
            println!(
                "组装 machine_id : {}",
                hardware_id::compose_machine_id(&prefix, &hash)
            );
            println!();
            println!("再取一次校验稳定性：");
            match hardware_id::hardware_hash() {
                Ok(again) if again == hash => println!("  两次一致，标识稳定 ✅"),
                Ok(again) => {
                    println!("  两次不一致：{hash} vs {again} —— 该标识不可用作 machine_id ❌")
                }
                Err(error) => println!("  第二次取失败：{error} ❌"),
            }
        }
        Err(error) => {
            println!("硬件哈希(前8位) : 取不到 —— {error}");
            println!();
            println!("这台机器无法用硬件标识做 machine_id，需显式设 DASHBOARD_MACHINE_ID 兜底。");
        }
    }
}
