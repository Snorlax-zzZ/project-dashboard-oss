//! 手动性能探针：真实四来源扫描、偏好读取、筛选与分页，不设机器相关的耗时门槛。
use dashboard_core::{
    session_storage::SessionStorage,
    session_visibility::{list_visible_paged, Visibility},
    sessions::{
        ClaudeCliSource, CodexSessionSource, GrokSessionSource, PiSessionSource, SessionMeta,
        SessionSource,
    },
};
use std::{cell::Cell, fs, path::Path, time::Instant};

struct Counted {
    inner: Box<dyn SessionSource>,
    calls: Cell<usize>,
}
impl SessionSource for Counted {
    fn source_id(&self) -> &'static str {
        self.inner.source_id()
    }
    fn scan(&self) -> Vec<SessionMeta> {
        self.calls.set(self.calls.get() + 1);
        self.inner.scan()
    }
}
fn fixture(root: &Path, source: &str, id: &str) {
    let dir = root.join("project");
    fs::create_dir_all(&dir).unwrap();
    let record = match source {
        "claude-cli" => serde_json::json!({"type":"ai-title","aiTitle":"synthetic topic"}),
        "codex" => {
            serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":"/synthetic/project"}})
        }
        "pi" => {
            serde_json::json!({"type":"session","version":3,"id":id,"cwd":"/synthetic/project"})
        }
        "grok" => {
            let dir = dir.join(id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("summary.json"), serde_json::json!({
                "info":{"id":id,"cwd":"/synthetic/project"}, "generated_title":"synthetic topic",
                "created_at":"2026-10-01T00:00:00Z", "updated_at":"2026-10-01T00:00:00Z"
            }).to_string()).unwrap();
            fs::write(
                dir.join("chat_history.jsonl"),
                "{\"type\":\"user\",\"content\":\"synthetic topic\"}\n",
            )
            .unwrap();
            return;
        }
        _ => unreachable!(),
    };
    fs::write(dir.join(format!("{id}.jsonl")), format!("{record}\n")).unwrap();
}

#[test]
#[ignore = "手动运行端到端性能探针，所有文件仅在临时目录"]
fn real_sources_visibility_query_cost() {
    let base = std::env::temp_dir().join(format!("dashboard-performance-{}", uuid::Uuid::new_v4()));
    let ids: Vec<_> = (0..1000)
        .map(|i| format!("00000000-0000-4000-8000-{i:012x}"))
        .collect();
    for source in ["claude-cli", "codex", "pi", "grok"] {
        let root = base.join(source);
        let s = SessionStorage::new(root.join("sessions"), root.join("legacy"));
        let source_root = root.join("harness");
        for id in &ids {
            fixture(&source_root, source, id);
        }
        let cache = s.cache_dir(source);
        let input = Counted {
            inner: match source {
                "claude-cli" => Box::new(ClaudeCliSource {
                    root: source_root,
                    cache: cache.clone(),
                }),
                "codex" => Box::new(CodexSessionSource {
                    root: source_root,
                    cache: cache.clone(),
                }),
                "pi" => Box::new(PiSessionSource {
                    root: source_root,
                    cache: cache.clone(),
                }),
                "grok" => Box::new(GrokSessionSource {
                    root: source_root,
                    cache: cache.clone(),
                }),
                _ => unreachable!(),
            },
            calls: Cell::new(0),
        };
        let start = Instant::now();
        s.ensure_initialized().unwrap();
        println!(
            "{source} fresh_init_ms={:.3}",
            start.elapsed().as_secs_f64() * 1000.0
        );
        for (name, residual) in [
            ("missing", None),
            ("half", Some(0)),
            ("residual100k", Some(100_000)),
        ] {
            if let Some(residual) = residual {
                let hidden: Vec<_> = ids[..500]
                    .iter()
                    .cloned()
                    .chain((0..residual).map(|i| format!("gone-{i}")))
                    .collect();
                fs::write(
                    s.root().join("hidden.json"),
                    serde_json::json!({"version":1,"hidden":{source:hidden}}).to_string(),
                )
                .unwrap();
            }
            for visibility in [Visibility::Visible, Visibility::Hidden] {
                // 冷指解析缓存为空；不声称清除了操作系统文件缓存。
                fs::remove_dir_all(&cache).unwrap();
                let mut samples = Vec::new();
                let expected = if name == "missing" {
                    if visibility == Visibility::Visible {
                        1000
                    } else {
                        0
                    }
                } else {
                    500
                };
                for run in 0..12 {
                    let before = input.calls.get();
                    let start = Instant::now();
                    let page = list_visible_paged(
                        &input,
                        &s,
                        None,
                        run + 1,
                        [10, 20, 50][run % 3],
                        visibility,
                    )
                    .unwrap();
                    samples.push(start.elapsed().as_secs_f64() * 1000.0);
                    assert_eq!(input.calls.get(), before + 1);
                    assert_eq!(page.total, expected);
                }
                let cold = samples.remove(0);
                samples.sort_by(f64::total_cmp);
                println!("{source} {name} {visibility:?} cold_ms={cold:.3} warm_median_ms={:.3} warm_max_ms={:.3} scans_per_query=1", samples[samples.len()/2], samples.last().unwrap());
            }
        }
        // 独立测首次导入 1000 条实际解析缓存，旧目录保持原样。
        let legacy = root.join("legacy");
        let origin = if source == "claude-cli" {
            legacy.clone()
        } else {
            legacy.join(source)
        };
        fs::create_dir_all(&origin).unwrap();
        for entry in fs::read_dir(&cache).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), origin.join(entry.file_name())).unwrap();
        }
        let migrated = SessionStorage::new(root.join("migrated"), legacy);
        let start = Instant::now();
        assert!(migrated.ensure_initialized().unwrap().warnings.is_empty());
        println!(
            "{source} import_1000_ms={:.3}",
            start.elapsed().as_secs_f64() * 1000.0
        );
        assert_eq!(
            fs::read_dir(migrated.cache_dir(source)).unwrap().count(),
            1000
        );
    }
    fs::remove_dir_all(base).unwrap();
}
