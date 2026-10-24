//! 有上限的真实进程测试载荷；正式构建不启用 process-fixture。
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
#[cfg(windows)]
#[path = "../../src/windows_process.rs"]
mod windows_process;

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    // 伪 git 模式（仅当测试进程显式设置 DASHBOARD_FAKE_GIT 时启用，正式行为不受影响）：
    // `remote get-url origin` 这类只读探测按 FAKE_GIT_ORIGIN 回显，保持出机门禁判 Ready；
    // 其余一切命令（reset/add/pull/push...）长睡越过长超时预算，等同步层的超时收割。
    if std::env::var_os("DASHBOARD_FAKE_GIT").is_some() {
        let values: Vec<_> = args
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        if values.iter().any(|s| s == "get-url") {
            if let Some(origin) = std::env::var_os("FAKE_GIT_ORIGIN") {
                println!("{}", origin.to_string_lossy());
            }
            return;
        }
        std::thread::sleep(Duration::from_secs(180));
        std::process::exit(1);
    }
    match args.first().and_then(|s| s.to_str()).unwrap_or("") {
        #[cfg(windows)]
        "host" | "nested" | "blocked-host" => {
            use windows_sys::Win32::{
                Foundation::*,
                System::{JobObjects::*, Threading::*},
            };
            let mut blocking_job = std::ptr::null_mut();
            if args[0] == "blocked-host" {
                blocking_job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
                assert!(!blocking_job.is_null());
                let mut value: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
                value.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                value.BasicLimitInformation.ActiveProcessLimit = 1;
                assert_ne!(
                    unsafe {
                        SetInformationJobObject(
                            blocking_job,
                            JobObjectExtendedLimitInformation,
                            (&value as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                            std::mem::size_of_val(&value) as u32,
                        )
                    },
                    0
                );
                assert_ne!(
                    unsafe { AssignProcessToJobObject(blocking_job, GetCurrentProcess()) },
                    0
                );
            }
            let child_args = if args[0] == "nested" {
                vec!["echo".into(), "nested-ok".into()]
            } else {
                vec!["tree".into(), args[1].clone()]
            };
            let result = windows_process::run(
                &std::env::current_exe().unwrap(),
                &child_args,
                None,
                windows_process::Limits {
                    timeout: Duration::from_secs(20),
                    stdout: Some(65536),
                    stderr: Some(65536),
                },
            );
            if args[0] == "blocked-host" {
                assert!(result.is_err());
                unsafe {
                    CloseHandle(blocking_job);
                }
                println!("blocked-before-entry");
            } else {
                let result = result.unwrap();
                io::stdout().write_all(&result.stdout).unwrap();
            }
        }
        #[cfg(windows)]
        "inherited-event" => {
            use windows_sys::Win32::System::Threading::SetEvent;
            let handle =
                args[1].to_str().unwrap().parse::<usize>().unwrap() as *mut std::ffi::c_void;
            assert_eq!(unsafe { SetEvent(handle) }, 0, "非stdio句柄不应被继承");
        }
        "--repo" => {
            let values: Vec<_> = args.iter().map(|s| s.to_string_lossy()).collect();
            let start = values.iter().position(|s| s == "--json").unwrap() + 1;
            let command = if values[start] == "graph" {
                format!("graph {}", values[start + 1])
            } else {
                values[start].to_string()
            };
            // 经 --text 注入的定向场景：让一次真实 API 调用命中协议错误 / 超限 / 超时路径。
            let text = values
                .iter()
                .position(|s| s == "--text")
                .map(|position| values[position + 1].clone());
            match text.as_deref() {
                // 规范错误 envelope + 非零退出：ok=false 必须与退出码一致，error 三字段齐全。
                Some("repowiki-fail") => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "protocol_version":1,
                            "command":command,
                            "ok":false,
                            "result":{},
                            "warnings":[],
                            "error":{"code":"forced_failure","message":"模拟远端失败","details":{"exit":1}}
                        })
                    );
                    std::process::exit(1);
                }
                // 非零退出且 stdout 非协议 JSON：应判协议违规而非成功。
                Some("repowiki-crash") => {
                    print!("not-json");
                    io::stdout().flush().unwrap();
                    std::process::exit(2);
                }
                // stdout 超过 20MiB 预算（21MiB 定量输出后退出）。
                Some("repowiki-spew-out") => {
                    let mut out = io::stdout();
                    let chunk = [b'x'; 8192];
                    for _ in 0..(21 * 1024 * 1024 / chunk.len()) {
                        out.write_all(&chunk).unwrap();
                    }
                    out.flush().unwrap();
                    std::process::exit(0);
                }
                // stderr 超过 64KiB 预算（1MiB 定量输出后退出）。
                Some("repowiki-spew-err") => {
                    let mut err = io::stderr();
                    let chunk = [b'e'; 8192];
                    for _ in 0..128 {
                        err.write_all(&chunk).unwrap();
                    }
                    err.flush().unwrap();
                    std::process::exit(0);
                }
                // 睡过 30 秒期限，等执行器超时收割。
                Some("repowiki-stall") => {
                    std::thread::sleep(Duration::from_secs(180));
                }
                _ => {}
            }
            println!(
                "{}",
                serde_json::json!({"protocol_version":1,"command":command,"ok":true,"result":{"argv":values},"warnings":[]})
            );
        }
        "echo" => {
            let values: Vec<_> = args.iter().skip(1).map(|s| s.to_string_lossy()).collect();
            println!("{}", serde_json::to_string(&values).unwrap());
            eprint!("错误输出✓");
        }
        "exit" => std::process::exit(23),
        "sleep" => std::thread::sleep(Duration::from_secs(20)),
        "flood" => {
            let err = std::thread::spawn(|| flood(io::stderr()));
            flood(io::stdout());
            let _ = err.join();
        }
        "tree" | "orphan" => {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .arg("leaf")
                .arg(&args[1])
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !std::path::Path::new(&args[1]).exists() {
                assert!(Instant::now() < deadline, "后代握手超时");
                std::thread::sleep(Duration::from_millis(5));
            }
            println!("ready");
            io::stdout().flush().unwrap();
            if args[0] == "tree" {
                let _ = child.wait();
            }
        }
        "leaf" => {
            std::fs::write(&args[1], std::process::id().to_string()).unwrap();
            std::thread::sleep(Duration::from_secs(20));
        }
        _ => panic!("未知测试场景"),
    }
}

fn flood(mut stream: impl Write) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if stream.write_all(&[b'x'; 8192]).is_err() {
            break;
        }
    }
}
