"""Stub/runtime docstring sync guard.

The API is documented twice — in Rust doc comments (compiled into the
extension as runtime ``__doc__``, what ``help()`` shows) and in
``_core.pyi`` (what mkdocstrings renders into the docs site). This
check keeps both sources byte-identical after whitespace/punctuation
normalization, and asserts every public item carries BOTH Chinese and
English text.

Usage: ``.venv/Scripts/python.exe tests/check_stubs.py``
(exit 0 = in sync).
"""

from __future__ import annotations

import ast
import re
import sys
from pathlib import Path

import eve_proxy_ng
import eve_proxy_ng._core as core

FULLWIDTH = {"（": "(", "）": ")", "，": ",", "；": ";", "：": ":", "？": "?", "！": "!"}


def norm(text: str | None) -> str:
    if text is None:
        return ""
    for fw, hw in FULLWIDTH.items():
        text = text.replace(fw, hw)
    return re.sub(r"\s+", " ", text).strip()


def is_bilingual(text: str | None) -> bool:
    if not text:
        return False
    has_cjk = bool(re.search(r"[\u4e00-\u9fff]", text))
    has_en = bool(re.search(r"[A-Za-z]{4,}", text))
    return has_cjk and has_en


def stub_items(path: Path) -> dict[str, str]:
    """Flatten ``name -> docstring`` from the stub module (funcs,
    classes, methods, attribute docstrings)."""
    tree = ast.parse(path.read_text(encoding="utf-8"))
    items: dict[str, str] = {}
    module_doc = ast.get_docstring(tree)
    if module_doc:
        items["_module_"] = module_doc
    for node in tree.body:
        if isinstance(node, ast.FunctionDef):
            doc = ast.get_docstring(node)
            if doc:
                items[node.name] = doc
        elif isinstance(node, ast.ClassDef):
            doc = ast.get_docstring(node)
            if doc:
                items[node.name] = doc
            last_assigned: str | None = None
            for sub in node.body:
                if isinstance(sub, ast.FunctionDef):
                    last_assigned = None
                    doc = ast.get_docstring(sub)
                    if doc:
                        items[f"{node.name}.{sub.name}"] = doc
                elif isinstance(sub, ast.AnnAssign) and isinstance(
                    sub.target, ast.Name
                ):
                    last_assigned = sub.target.id
                elif (
                    isinstance(sub, ast.Expr)
                    and isinstance(sub.value, ast.Constant)
                    and isinstance(sub.value.value, str)
                    and last_assigned
                ):
                    # Attribute docstring: `x: int` followed by a bare
                    # string literal (standard stub-file convention).
                    items[f"{node.name}.{last_assigned}"] = sub.value.value
                    last_assigned = None
    return items


def runtime_items() -> dict[str, str]:
    """Same flattening from the live extension module."""
    items: dict[str, str] = {"_module_": core.__doc__ or ""}
    for fn in (
        "discover_clients",
        "get_resource_image",
        "resources_info",
        "type_name",
        "module_name_from_icon",
    ):
        items[fn] = getattr(core, fn).__doc__ or ""
    object_init_doc = getattr(object.__init__, "__doc__") or ""
    for cls_name in ("GameClient", "UiReader"):
        cls = getattr(core, cls_name)
        items[cls_name] = cls.__doc__ or ""
        for attr in dir(cls):
            if attr.startswith("_"):
                continue
            obj = getattr(cls, attr, None)
            doc = getattr(obj, "__doc__", None)
            if isinstance(doc, str) and doc.strip():
                items[f"{cls_name}.{attr}"] = doc
        init_doc = getattr(cls.__init__, "__doc__", None)
        # Skip the default object.__init__ docstring noise on classes
        # without an explicit constructor.
        if isinstance(init_doc, str) and init_doc.strip() and init_doc != object_init_doc:
            items[f"{cls_name}.__init__"] = init_doc
    return items


def first_diff(a: str, b: str) -> str:
    for i, (ca, cb) in enumerate(zip(a, b)):
        if ca != cb:
            return f"at {i}: runtime {a[max(0,i-20):i+20]!r} vs stub {b[max(0,i-20):i+20]!r}"
    return f"length {len(a)} vs {len(b)}: tail {a[len(b):]!r}" if len(a) != len(b) else "?"


def main() -> int:
    stub_path = (
        Path(__file__).resolve().parents[1]
        / "crates" / "eve-bindings" / "python" / "eve_proxy_ng" / "_core.pyi"
    )
    stub = stub_items(stub_path)
    runtime = runtime_items()

    failures: list[str] = []
    for name, stub_doc in sorted(stub.items()):
        got = norm(runtime.get(name))
        want = norm(stub_doc)
        if got != want:
            failures.append(f"{name}: stub/runtime docstring mismatch")
        if not is_bilingual(stub_doc):
            failures.append(f"{name}: missing Chinese or English text")
    # Runtime-only public items must not appear without stubs.
    for name in runtime:
        if name not in stub and not name.startswith("_"):
            failures.append(f"{name}: present at runtime but missing from _core.pyi")

    if failures:
        print("STUB SYNC FAILURES:")
        for f in failures:
            print(" -", f)
        for name, stub_doc in sorted(stub.items()):
            got, want = norm(runtime.get(name)), norm(stub_doc)
            if got != want:
                print(f"   [{name}] diff {first_diff(got, want)}")
        return 1
    print(f"stub/runtime docstrings in sync ({len(stub)} items, all bilingual)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
