#![cfg(all(windows, feature = "process-fixture"))]
#[path = "../src/windows_process.rs"]
mod windows_process;
use std::{
    ffi::OsString,
    path::Path,
    time::{Duration, Instant},
};
use windows_process::{run, Error, Limits};

fn limits() -> Limits {
    Limits {
        timeout: Duration::from_secs(5),
        stdout: Some(1024 * 1024),
        stderr: Some(65536),
    }
}
fn fixture() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_process-fixture"))
}
fn arguments(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

#[test]
fn cleanup_deadline_is_not_reported_as_business_timeout() {
    use windows_process::{run_with_ops, Operation, WinOps};
    struct SlowCleanup;
    impl WinOps for SlowCleanup {
        fn before(&self, point: Operation) -> Result<(), Error> {
            if point == Operation::Query {
                std::thread::sleep(Duration::from_millis(1050));
            }
            Ok(())
        }
    }
    assert!(matches!(
        run_with_ops(
            fixture(),
            &arguments(&["echo", "done"]),
            None,
            limits(),
            &SlowCleanup
        ),
        Err(Error::Cleanup { cause: None })
    ));
}

#[test]
fn injected_api_failures_release_handles_and_allow_next_call() {
    if std::env::var_os("DASHBOARD_LEAK_CHECK_CHILD").is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "injected_api_failures_release_handles_and_allow_next_call",
                "--test-threads=1",
            ])
            .env("DASHBOARD_LEAK_CHECK_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    use windows_process::{run_with_ops, Operation, Operation::*, WinOps};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    struct FailOnce {
        point: Operation,
        fired: std::cell::Cell<bool>,
    }
    impl WinOps for FailOnce {
        fn before(&self, point: Operation) -> Result<(), Error> {
            if point == self.point && !self.fired.get() {
                self.fired.set(true);
                return Err(Error::Io);
            }
            Ok(())
        }
    }
    fn handles() -> u32 {
        let mut count = 0;
        assert_ne!(
            unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut count) },
            0
        );
        count
    }
    // 隔离进程中先完成首次 CreateProcess 的系统懒初始化，再逐一比较故障路径。
    // 诊断实测首次成功调用增加13个常驻句柄，之后16个注入点计数不再增长。
    assert!(
        run(fixture(), &arguments(&["echo", "warmup"]), None, limits())
            .unwrap()
            .status
            .success()
    );
    let before = handles();
    for point in [
        JobCreate,
        JobConfigure,
        PipeCreate,
        PipeInheritance,
        Input,
        AttributesSize,
        AttributesInit,
        HandleList,
        JobList,
        Spawn,
        Peek,
        Read,
        Wait,
        ExitCode,
        Terminate,
        Query,
    ] {
        let ops = FailOnce {
            point,
            fired: std::cell::Cell::new(false),
        };
        assert!(
            run_with_ops(
                fixture(),
                &arguments(&["echo", "fault"]),
                None,
                limits(),
                &ops
            )
            .is_err(),
            "{point:?}"
        );
        assert!(ops.fired.get(), "未实际覆盖注入点 {point:?}");
        assert!(
            run(fixture(), &arguments(&["echo", "next"]), None, limits())
                .unwrap()
                .status
                .success()
        );
        assert!(handles() <= before, "故障点句柄泄漏: {point:?}");
    }
    let after = handles();
    assert!(after <= before + 2, "句柄泄漏: {before} -> {after}");
}

#[test]
fn preserves_argv_unicode_empty_quotes_and_trailing_slashes() {
    let values = ["", "a b", "尾部\\", "& | $(x)", "a\"b", "\\\\\"", "中文🙂"];
    let mut args = vec![OsString::from("echo")];
    args.extend(values.iter().map(OsString::from));
    let output = run(fixture(), &args, None, limits()).unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Vec<String>>(&output.stdout).unwrap(),
        values
    );
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "错误输出✓");
}

#[test]
fn preserves_nonzero_exit() {
    let output = run(fixture(), &arguments(&["exit"]), None, limits()).unwrap();
    assert_eq!(output.status.code(), Some(23));
}

#[test]
fn deadline_and_following_invocation() {
    let start = Instant::now();
    let mut l = limits();
    l.timeout = Duration::from_millis(100);
    assert!(matches!(
        run(fixture(), &arguments(&["sleep"]), None, l),
        Err(Error::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(
        run(fixture(), &arguments(&["echo", "after"]), None, limits())
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn bounds_both_output_streams() {
    for stderr in [false, true] {
        let mut l = limits();
        l.stdout = if stderr { None } else { Some(1024) };
        l.stderr = if stderr { Some(1024) } else { None };
        let start = Instant::now();
        assert!(matches!(
            run(fixture(), &arguments(&["flood"]), None, l),
            Err(Error::OutputTooLarge)
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn root_exit_and_timeout_do_not_wait_for_descendant_eof() {
    for mode in ["tree", "orphan"] {
        let marker = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("tmp")
            .join(format!("dashboard-job-{}", uuid::Uuid::new_v4()));
        let args = vec![OsString::from(mode), marker.as_os_str().to_owned()];
        let mut l = limits();
        l.timeout = Duration::from_secs(1);
        let start = Instant::now();
        let result = run(fixture(), &args, None, l);
        assert!(marker.exists(), "后代没有进入测试");
        assert!(start.elapsed() < Duration::from_secs(3));
        if mode == "tree" {
            assert!(matches!(result, Err(Error::Timeout)));
        } else {
            assert!(result.unwrap().status.success());
        }
        std::fs::remove_file(marker).unwrap();
    }
}

#[test]
fn nested_jobs_and_failed_creation_do_not_start_uncontained_work() {
    let output = run(fixture(), &arguments(&["nested"]), None, limits()).unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("nested-ok"));
    let marker = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tmp")
        .join(format!("never-start-{}", uuid::Uuid::new_v4()));
    let output = run(
        fixture(),
        &vec!["blocked-host".into(), marker.as_os_str().to_owned()],
        None,
        limits(),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!marker.exists());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "blocked-before-entry"
    );
}

#[test]
fn abrupt_host_exit_closes_job_and_kills_descendant() {
    use windows_sys::Win32::{Foundation::*, System::Threading::*};
    let marker = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tmp")
        .join(format!("host-crash-{}", uuid::Uuid::new_v4()));
    let mut host = std::process::Command::new(fixture())
        .arg("host")
        .arg(&marker)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if !marker.exists() {
        let _ = host.kill();
        let _ = host.wait();
        panic!("后代未握手");
    }
    let pid = std::fs::read_to_string(&marker).unwrap().parse().unwrap();
    let child = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!child.is_null());
    assert_eq!(unsafe { WaitForSingleObject(child, 0) }, WAIT_TIMEOUT);
    host.kill().unwrap();
    host.wait().unwrap();
    assert_eq!(unsafe { WaitForSingleObject(child, 2000) }, WAIT_OBJECT_0);
    unsafe {
        CloseHandle(child);
    }
    std::fs::remove_file(marker).unwrap();
}

#[test]
fn only_explicit_stdio_handles_are_inherited() {
    use windows_sys::Win32::{Foundation::*, Security::SECURITY_ATTRIBUTES, System::Threading::*};
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let event = unsafe { CreateEventW(&sa, 1, 0, std::ptr::null()) };
    assert!(!event.is_null());
    let result = run(
        fixture(),
        &arguments(&["inherited-event", &(event as usize).to_string()]),
        None,
        limits(),
    );
    assert_eq!(unsafe { WaitForSingleObject(event, 0) }, WAIT_TIMEOUT);
    unsafe {
        CloseHandle(event);
    }
    assert!(result.unwrap().status.success());
}
