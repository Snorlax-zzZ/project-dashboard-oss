"""构建 Windows x64 dogfooding 包，附依赖声明并脱敏编译路径。"""

import os
import platform
import shlex
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def main():
    if platform.system() != "Windows" or platform.machine().lower() not in {"amd64", "x86_64"}:
        raise SystemExit("此发布脚本要求 Windows x64、MSVC 构建工具与 WebView2。")
    env = os.environ.copy()
    env["CI"] = "true"
    flags = env.get("CARGO_ENCODED_RUSTFLAGS", "").split("\x1f")
    if flags == [""]:
        flags = shlex.split(env.get("RUSTFLAGS", ""))
    flags.extend([
        f"--remap-path-prefix={Path.home()}=builder-home",
        f"--remap-path-prefix={ROOT}=.",
        f"--remap-path-prefix={Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))}=cargo-home",
    ])
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    subprocess.run([
        "cargo", "tauri", "icon", "dashboard-tauri/icons/app-icon.svg",
        "--output", "dashboard-tauri/icons",
    ], cwd=ROOT, env=env, check=True)
    subprocess.run([
        sys.executable, "scripts/make_notices.py", "--target", "x86_64-pc-windows-msvc",
    ], cwd=ROOT, env=env, check=True)
    command = [
        "cargo", "tauri", "build", "--bundles", "nsis",
        "--config", '{"bundle":{"resources":["resources/THIRD-PARTY-NOTICES.txt"]}}',
        "--", "--locked",
    ]
    if env.get("DASHBOARD_OFFLINE") == "1":
        command.append("--offline")
    subprocess.run(command, cwd=ROOT / "dashboard-tauri", env=env, check=True)


if __name__ == "__main__":
    main()
