"""Build the distributable eve-proxy-ng wheel.

Assembles everything the wheel must carry on top of the compiled
extension, then invokes maturin:

1. ``cargo build --release`` — fresh ``eve-cli.exe``;
2. ``mkdocs build`` — the docs site, staged into ``eve_proxy_ng/docs``;
3. stage ``eve-cli.exe`` + its ffmpeg DLL closure into ``eve_proxy_ng/bin``
   (closure verified in an isolated directory: avcodec/avformat/
   avutil/swscale/swresample are all eve-cli's import table needs);
4. ``maturin build --release``.

Usage::

    .venv/Scripts/python.exe scripts/package.py [--skip-build] [--skip-docs] [--clean]

``--clean`` removes the staged ``bin``/``docs`` directories afterwards
(they are build artifacts, gitignored).
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BINDINGS = ROOT / "crates" / "eve-bindings"
PKG = BINDINGS / "python" / "eve_proxy_ng"
BIN_DIR = PKG / "bin"
DOCS_DIR = PKG / "docs"
SITE_DIR = ROOT / "site"
FFMPEG_BIN = ROOT / "third_party" / "ffmpeg-7.1-win64-gpl-shared" / "bin"

# eve-cli.exe 的 DLL 导入闭包（隔离目录实测；avfilter/avdevice/postproc 未链接）。
DLL_CLOSURE = [
    "avcodec-61.dll",
    "avformat-61.dll",
    "avutil-59.dll",
    "swscale-8.dll",
    "swresample-5.dll",
]

# py2.7 extractor runtime ships in the wheel so `icons update
# --source client` works offline (the client's FSD loaders are py2
# extensions). Trimmed to interpreter + Lib + DLLs + license.


def stage_py27(dst: Path) -> bool:
    src = ROOT / "tools" / "py27"
    if not (src / "python.exe").is_file():
        return False
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(
        src, dst,
        ignore=shutil.ignore_patterns("Doc", "Tools", "include", "libs", "*.msi"),
    )
    return True


def run(cmd: list[str], **kw) -> None:
    print("+", " ".join(str(c) for c in cmd), flush=True)
    subprocess.run(cmd, check=True, **kw)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--skip-build", action="store_true", help="skip cargo build")
    ap.add_argument("--skip-docs", action="store_true", help="skip mkdocs build")
    ap.add_argument("--clean", action="store_true", help="remove staged bin/docs after build")
    args = ap.parse_args()

    exe = ROOT / "target" / "release" / "eve-cli.exe"
    if not args.skip_build:
        env = os.environ.copy()
        env.setdefault("FFMPEG_DIR", str(FFMPEG_BIN.parent))
        env.setdefault("LIBCLANG_PATH", r"C:\Program Files\LLVM\bin")
        run(["cargo", "build", "--release", "-p", "eve-cli"], cwd=ROOT, env=env)
    if not exe.is_file():
        print(f"eve-cli.exe missing: {exe}", file=sys.stderr)
        return 1

    # --- stage binaries ---
    BIN_DIR.mkdir(parents=True, exist_ok=True)
    shutil.copy2(exe, BIN_DIR / "eve-cli.exe")
    for dll in DLL_CLOSURE:
        src = FFMPEG_BIN / dll
        if not src.is_file():
            print(f"ffmpeg DLL missing: {src}", file=sys.stderr)
            return 1
        shutil.copy2(src, BIN_DIR / dll)

    # --- stage the py2.7 extractor runtime (client-source FSD decode) ---
    tools27 = PKG / "tools" / "py27"
    if not stage_py27(tools27):
        print(
            "tools/py27 missing — client-source icons update will "
            "bootstrap it on demand (online) instead of using a bundled copy",
            file=sys.stderr,
        )

    # --- stage docs ---
    if not args.skip_docs:
        run([str(ROOT / ".venv" / "Scripts" / "mkdocs.exe"), "build", "--strict"], cwd=ROOT)
    if (SITE_DIR / "index.html").is_file():
        if DOCS_DIR.exists():
            shutil.rmtree(DOCS_DIR)
        shutil.copytree(SITE_DIR, DOCS_DIR)
    else:
        print("docs site missing (run without --skip-docs)", file=sys.stderr)
        return 1

    # --- wheel ---
    run(
        [
            "maturin", "build", "--release",
            "-m", str(BINDINGS / "Cargo.toml"),
            "-i", str(ROOT / ".venv" / "Scripts" / "python.exe"),
        ],
        cwd=ROOT,
    )

    if args.clean:
        shutil.rmtree(BIN_DIR, ignore_errors=True)
        shutil.rmtree(DOCS_DIR, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
