"""Scene-builder unit tests for the semantic view (sample-gated)."""

from __future__ import annotations

import sys
from pathlib import Path

import eve_proxy_ng
from eve_proxy_ng import view

SAMPLE = Path(__file__).resolve().parents[1] / "samples" / "process-sample-a8b4bd7529.zip"


def main() -> int:
    if not SAMPLE.is_file():
        print("skipping: sample not present")
        return 0
    reader = eve_proxy_ng.UiReader(sample=str(SAMPLE))
    reader.find_ui_root()
    scene = view.build_scene(reader.read_snapshot())

    assert scene["size"][0] >= 1280 and scene["size"][1] >= 720, scene["size"]
    kinds = {n["k"] for n in scene["nodes"]}
    assert "overview" in kinds, f"overview rows missing: {kinds}"
    assert "module" in kinds, f"module buttons missing: {kinds}"
    assert "element" in kinds, f"generic elements missing: {kinds}"
    # Overview rows carry icon + name + distance sub-label.
    row = next(n for n in scene["nodes"] if n["k"] == "overview")
    assert row["l"], row
    # Every node is positioned and drawable.
    for n in scene["nodes"]:
        assert n["w"] > 0 and n["h"] > 0, n
    stats = scene["stats"]
    print(f"ok  scene nodes={stats['nodes']} kinds={sorted(kinds)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
