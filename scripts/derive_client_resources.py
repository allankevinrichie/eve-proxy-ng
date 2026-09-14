# -*- coding: utf-8 -*-
"""Derive game resource tables (typeID -> name/icon) from the LOCAL
client only.

Pipeline (no network for game data — the client is the source):

1. Resolve the client's staticdata FSD binaries (types / iconids /
   groups / categories) and the localization pickle through
   ``resfileindex.txt`` from the shared cache (content-addressed
   ``ResFiles``).
2. Bootstrap a stock CPython 2.7 (amd64) tool — the FSD decoders the
   client ships (``bin64\\<name>Loader.pyd``) are official Python 2
   extension modules and are the only decoder guaranteed to track the
   client's format. The 2.7 MSI is downloaded from python.org once and
   admin-extracted locally (no install, no registry, no admin).
3. Run the py2 payload: import the client's own loaders, decode the
   tables, join with the localization pickle (zh by default) and write
   the derived JSON.

Output: ``data/types.<flavor>.json`` — ``{"meta": {...}, "types":
{"<typeID>": {"name": ..., "icon": ...}}}``.

Usage::

    .venv/Scripts/python.exe scripts/derive_client_resources.py \
        [--flavor infinity] [--lang zh] [--client-root C:\\EVE\\SharedCache] \
        [--out data/types.infinity.json] [--tools tools] [--force]

The legacy fuzzwork SDE path is the documented fallback for machines
without a client install; this script replaces it as the default.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PY27_MSI = "https://www.python.org/ftp/python/2.7.18/python-2.7.18.amd64.msi"
PAYLOAD = Path(__file__).parent / "py2" / "derive_payload.py"

FSD_TABLES = ["types", "iconids", "groups", "categories"]
# resfileindex names differ from loader names in case only for iconids.
RES_NAME = {"types": "types", "iconids": "iconids", "groups": "groups", "categories": "categories"}
PICKLE_LANGS = {"zh": "zh", "en": "en-us"}


def resolve_res(client_root: Path, flavor: str) -> dict[str, Path]:
    """res path -> ResFiles path via the flavor's resfileindex.txt."""
    index = client_root / flavor / "resfileindex.txt"
    if not index.is_file():
        raise SystemExit(f"resource index not found: {index}")
    want = {f"res:/staticdata/{n}.fsdbinary": n for n in RES_NAME.values()}
    lang = f"res:/localizationfsd/localization_fsd_{{lang}}.pickle"
    found: dict[str, Path] = {}
    resfiles = client_root / "ResFiles"
    for line in index.read_text(encoding="utf-8", errors="replace").splitlines():
        parts = line.split(",")
        if len(parts) < 2:
            continue
        res_path, blob = parts[0], parts[1]
        if res_path in want:
            found[want[res_path]] = resfiles / blob
    missing = [n for n in FSD_TABLES if n not in found]
    if missing:
        raise SystemExit(f"staticdata tables missing from index: {missing}")
    for name, path in found.items():
        if not path.is_file():
            raise SystemExit(f"indexed file missing on disk: {path}")
    return found


def ensure_py27(tools: Path) -> Path:
    """Stock CPython 2.7 amd64, admin-extracted under tools/py27."""
    python = tools / "py27" / "python.exe"
    if python.is_file():
        return python
    msi = tools / "python-2.7.18.amd64.msi"
    if not msi.is_file():
        print(f"downloading {PY27_MSI} …")
        tools.mkdir(parents=True, exist_ok=True)
        urllib.request.urlretrieve(PY27_MSI, msi)
    target = tools / "py27"
    print(f"admin-extracting {msi.name} -> {target} (no install, no admin)…")
    subprocess.run(
        ["msiexec", "/a", str(msi), "/qn", f"TARGETDIR={target}"],
        check=True,
        creationflags=subprocess.CREATE_NO_WINDOW,
    )
    if not python.is_file():
        raise SystemExit("py2.7 extract failed: python.exe not found")
    return python


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--flavor", default="infinity", help="client flavor dir (infinity/serenity/tranquility)")
    ap.add_argument("--lang", default="zh", choices=sorted(PICKLE_LANGS), help="localization language")
    ap.add_argument("--client-root", default=r"C:\EVE\SharedCache", help="shared cache root")
    ap.add_argument("--out", default=None, help="output JSON (default data/types.<flavor>.json)")
    ap.add_argument("--tools", default=str(ROOT / "tools"), help="dir for the py2.7 bootstrap")
    ap.add_argument("--force", action="store_true", help="re-derive even if output exists")
    ap.add_argument(
        "--bootstrap-py27",
        action="store_true",
        help="only ensure the py2.7 extractor runtime exists (CI stage step), then exit",
    )
    args = ap.parse_args()

    if args.bootstrap_py27:
        py27 = ensure_py27(Path(args.tools))
        print(f"py2.7 ready: {py27}")
        return 0

    out = Path(args.out) if args.out else ROOT / "data" / f"types.{args.flavor}.json"
    if out.is_file() and not args.force:
        print(f"{out} exists (use --force to re-derive)")
        return 0

    client_root = Path(args.client_root)
    tables = resolve_res(client_root, args.flavor)

    lang_tag = PICKLE_LANGS[args.lang]
    pickle_path = client_root / "ResFiles"
    # localization pickle lives in the shared resfileindex too
    index = client_root / args.flavor / "resfileindex.txt"
    loc_key = f"res:/localizationfsd/localization_fsd_{lang_tag}.pickle"
    loc_path = None
    for line in index.read_text(encoding="utf-8", errors="replace").splitlines():
        parts = line.split(",")
        if parts and parts[0] == loc_key:
            loc_path = pickle_path / parts[1]
            break
    if loc_path is None or not loc_path.is_file():
        raise SystemExit(f"localization pickle not found: {loc_key}")

    bin64 = client_root / args.flavor / "bin64"
    loaders = bin64 / "typesLoader.pyd"
    if not loaders.is_file():
        raise SystemExit(f"client loaders not found under {bin64}")

    py27 = ensure_py27(Path(args.tools))

    with tempfile.TemporaryDirectory(prefix="eve-ng-derive-") as tmp:
        tmp = Path(tmp)
        staged = {}
        for name, src in tables.items():
            dst = tmp / f"{name}.fsdbinary"
            shutil.copy2(src, dst)
            staged[name] = str(dst)
        loc_dst = tmp / "loc.pickle"
        shutil.copy2(loc_path, loc_dst)
        result = tmp / "derived.json"

        cmd = [
            str(py27), str(PAYLOAD),
            "--bin64", str(bin64),
            "--types", staged["types"],
            "--iconids", staged["iconids"],
            "--groups", staged["groups"],
            "--categories", staged["categories"],
            "--localization", str(loc_dst),
            "--out", str(result),
        ]
        env = {
            "PATH": str(bin64) + ";" + str(py27.parent) + r";C:\Windows\System32;C:\Windows",
            "PYTHONIOENCODING": "utf-8",
        }
        print("+ running client FSD loaders (py2)…")
        proc = subprocess.run(cmd, env=env, capture_output=True, text=True, encoding="utf-8")
        sys.stdout.write(proc.stdout or "")
        if proc.returncode != 0:
            sys.stderr.write(proc.stderr or "")
            raise SystemExit("py2 derive payload failed")

        payload = json.loads(result.read_text(encoding="utf-8"))

    meta = {
        "flavor": args.flavor,
        "lang": args.lang,
        "derived_from": "local client (official FSD loaders)",
        "client_root": str(client_root),
        **payload["meta"],
    }
    doc = {"meta": meta, "types": payload["types"]}
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, ensure_ascii=False), encoding="utf-8")
    print(f"written {out} ({len(payload['types'])} types)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
