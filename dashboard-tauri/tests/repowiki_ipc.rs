#[test]
fn repowiki_ipc_surface_is_explicit_and_registered() {
    let source = include_str!("../src/main.rs");
    for command in [
        "repowiki_status",
        "repowiki_read_project",
        "repowiki_read_page",
        "repowiki_read_generations",
        "repowiki_graph_search",
        "repowiki_graph_show",
        "repowiki_graph_callers",
        "repowiki_graph_callees",
        "repowiki_graph_impact",
    ] {
        assert!(
            source.contains(&format!("async fn {command}(")),
            "只读 IPC 必须异步执行，不能占用 UI 线程：{}",
            command
        );
        let wrapper = source
            .split(&format!("async fn {command}("))
            .nth(1)
            .unwrap()
            .split("#[tauri::command]")
            .next()
            .unwrap();
        assert!(
            wrapper.contains("run_repowiki_read(move ||"),
            "文件校验与 CLI 读取必须一起移到后台阻塞线程：{}",
            command
        );
        assert!(
            source.matches(command).count() >= 2,
            "IPC wrapper 未注册到 generate_handler：{}",
            command
        );
    }
    assert!(!source.contains("repowiki_execute"));
    assert!(!source.contains("repowiki_run_command"));
}
