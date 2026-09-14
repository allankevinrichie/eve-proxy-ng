"""eve-cli 命令行包装器（console script 入口）。

Wrapper that runs the ``eve-cli.exe`` bundled inside the package,
with its directory (the ffmpeg shared libraries) prepended to the DLL
search path. After ``pip install eve-proxy-ng`` the command ``eve-cli`` is
available from any environment — 无需单独安装 Rust 工具链或 ffmpeg。

用法 / usage::

    eve-cli clients                       # 列出运行中的客户端
    eve-cli read --pid 31336              # 读取 UI 树
    eve-cli record --duration-sec 60      # 三路会话录制
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


def _bin_dir() -> Path:
    return Path(__file__).parent / "bin"


def main(argv: list[str] | None = None) -> int:
    """运行随包分发的 eve-cli.exe，透传全部命令行参数。

    Run the bundled ``eve-cli.exe`` with the given arguments (defaults
    to ``sys.argv[1:]``), making the package's ``bin`` directory —
    which contains the ffmpeg DLLs — visible to the Windows DLL
    loader first.

    Args:
        argv: 传给 eve-cli 的参数列表。Arguments forwarded to
            eve-cli; ``sys.argv[1:]`` when omitted.

    Returns:
        ``int``：子进程退出码。The child process exit code.
    """
    exe = _bin_dir() / "eve-cli.exe"
    if not exe.is_file():
        print(
            "eve-cli.exe not found in package; reinstall the eve-proxy-ng wheel",
            file=sys.stderr,
        )
        return 127
    bin_dir = str(_bin_dir())
    env = os.environ.copy()
    env["PATH"] = bin_dir + os.pathsep + env.get("PATH", "")
    try:
        os.add_dll_directory(bin_dir)
    except (OSError, ValueError):
        # 非 Windows 或目录失效；PATH 前缀已是兜底。
        # Non-Windows or invalid dir; the PATH prefix still applies.
        pass
    args = [str(exe), *(argv if argv is not None else sys.argv[1:])]
    return subprocess.call(args, env=env)


if __name__ == "__main__":
    raise SystemExit(main())
