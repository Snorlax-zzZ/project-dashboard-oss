"""从锁定依赖的许可证文件生成随 App 分发的声明；不安装依赖。"""

import json
import os
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
THIRD_PARTY = ROOT / "third-party"


def main():
    command = [
        "cargo", "metadata", "--locked", "--format-version", "1",
        "--filter-platform", "aarch64-apple-darwin",
    ]
    if os.environ.get("DASHBOARD_OFFLINE") == "1":
        command.append("--offline")
    metadata = json.loads(subprocess.check_output(command, cwd=ROOT, text=True))
    fallbacks = json.loads((THIRD_PARTY / "rust-license-fallbacks.json").read_text())
    sections = [
        "project-dashboard 第三方声明\n",
        (ROOT / "LICENSE").read_text(),
        "Marked 15.0.12\nhttps://github.com/markedjs/marked/tree/v15.0.12\n"
        + (THIRD_PARTY / "marked-LICENSE.txt").read_text(),
        "Lucide / Feather 内联图标（路径及笔画有调整）\n"
        "https://github.com/lucide-icons/lucide/tree/500620a2e8123f8d1db191538886dc0c223f69a9\n"
        + (THIRD_PARTY / "lucide-LICENSE.txt").read_text(),
        "Feather 图标\n"
        "https://github.com/feathericons/feather/tree/3dc050d97405062eba78aa57115c0a15c63abdaa\n"
        + (THIRD_PARTY / "feather-LICENSE.txt").read_text(),
    ]
    count = 0
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["source"] is None:
            continue
        name, version = package["name"], package["version"]
        directory = Path(package["manifest_path"]).parent
        licenses = sorted(
            path for path in directory.iterdir()
            if path.is_file() and path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE"))
        )
        if package.get("license_file"):
            declared = directory / package["license_file"]
            if declared.is_file() and declared not in licenses:
                licenses.append(declared)
        if not licenses:
            licenses = [THIRD_PARTY / file for file in fallbacks.get(f"{name}@{version}", [])]
        if not licenses or any(not path.is_file() for path in licenses):
            raise SystemExit(f"缺少 {name}@{version} 的许可证正文，停止打包。")
        texts = []
        for license_path in licenses:
            text = license_path.read_text(encoding="utf-8")
            if not text.strip():
                raise SystemExit(f"{name}@{version} 的许可证为空，停止打包。")
            texts.append(text)
        sections.append(
            f"{name} {version}\n许可证：{package.get('license') or '见正文'}\n"
            f"源码：https://crates.io/crates/{name}/{version}\n\n" + "\n\n".join(texts)
        )
        count += 1
    output = ROOT / "dashboard-tauri/resources/THIRD-PARTY-NOTICES.txt"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n\n" + ("\n\n" + "=" * 72 + "\n\n").join(sections), encoding="utf-8")
    print(f"已为 {count} 个 Rust 依赖、Marked 和 Lucide / Feather 图标生成第三方声明。")


if __name__ == "__main__":
    main()
