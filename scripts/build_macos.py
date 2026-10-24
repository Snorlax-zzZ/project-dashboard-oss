"""构建 macOS ARM64 dogfooding 包，附依赖声明并脱敏编译路径。"""

import os
import platform
import shlex
import subprocess
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]


def main():
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        raise SystemExit("此发布脚本仅验证 macOS Apple Silicon。")
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
    subprocess.run(["python3", "scripts/make_notices.py", "--target", "aarch64-apple-darwin"], cwd=ROOT, env=env, check=True)
    command = [
        "cargo", "tauri", "build", "--bundles", "app,dmg",
        "--config", '{"bundle":{"resources":["resources/THIRD-PARTY-NOTICES.txt"]}}',
        "--", "--locked",
    ]
    if env.get("DASHBOARD_OFFLINE") == "1":
        command.append("--offline")
    subprocess.run(command, cwd=ROOT / "dashboard-tauri", env=env, check=True)


if __name__ == "__main__":
    main()
