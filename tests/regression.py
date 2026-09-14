"""Sample regression: replay the recorded 曙光 sample and pin key fields.

Run:  .venv/Scripts/python.exe tests/regression.py [path-to-sample.zip]

Skips (exit 0) when the sample archive is not present — samples are not
committed. When a new sample is recorded, update the expected values in
one place: EXPECTED.
"""

import sys
from pathlib import Path

import eve_proxy_ng

EXPECTED = {
    "flavor": "infinity",
    "node_count": 3041,
    "ui_root": "0x24446985588",
    # ship (docked-at-POS rookie ship, everything idle)
    "ship_modules": (2, 1, 0),
    "ship_capacitor": 100,
    "ship_hitpoints": (100, 100, 100),
    "ship_speed_text": "0.0 m/s",
    # overview (Chinese client, 通用 tab)
    "overview_caption": "总览 (通用: 通用)",
    "overview_tabs": ["通用", "目标", "采矿", "跃迁到", "全部"],
    "overview_entry_count": 25,
    "first_entry_name": "X4UV-Z",
    "first_entry_distance": 1_225_000,
    "info_panel_types": {"InfoPanelLocationInfo", "InfoPanelRoute"},
    # interaction (dump state: promo/gacha windows cover the overview rows)
    "first_entry_interactable": False,
    "first_entry_occluder": "activitypic",
    "module_mid_interactable": True,
    "neocom_interactable_count": (16, 16),
}


def main(sample: Path) -> int:
    reader = eve_proxy_ng.UiReader(sample=str(sample))
    failures: list[str] = []

    def check(label, actual, expected):
        if actual != expected:
            failures.append(f"{label}: expected {expected!r}, got {actual!r}")
        else:
            print(f"ok  {label} = {actual!r}")

    check("ui root", reader.find_ui_root(), EXPECTED["ui_root"])

    snapshot = reader.read_snapshot()
    check("flavor", snapshot["flavor"], EXPECTED["flavor"])
    check("node count", snapshot["node_count"], EXPECTED["node_count"])

    ship = snapshot["ship_ui"]
    if ship is None:
        failures.append("ship_ui: missing")
    else:
        check(
            "ship modules (hi/mid/lo)",
            (
                len(ship["module_buttons_high"]),
                len(ship["module_buttons_mid"]),
                len(ship["module_buttons_low"]),
            ),
            EXPECTED["ship_modules"],
        )
        check("ship capacitor", ship["capacitor_percent"], EXPECTED["ship_capacitor"])
        hp = ship["hitpoints"]
        check(
            "ship hitpoints",
            None if hp is None else (hp["shield_percent"], hp["armor_percent"], hp["structure_percent"]),
            EXPECTED["ship_hitpoints"],
        )
        check("ship speed text", ship["speed_text"], EXPECTED["ship_speed_text"])

    windows = snapshot["overview_windows"]
    if not windows:
        failures.append("overview windows: none")
    else:
        overview = windows[0]
        check("overview caption", overview["caption"], EXPECTED["overview_caption"])
        check("overview tabs", [t["name"] for t in overview["tabs"]], EXPECTED["overview_tabs"])
        check("overview entry count", len(overview["entries"]), EXPECTED["overview_entry_count"])
        first = overview["entries"][0]
        check("first entry name", first["object_name"], EXPECTED["first_entry_name"])
        check("first entry distance", first["distance_meters"], EXPECTED["first_entry_distance"])
        check(
            "first entry interactable",
            first["is_interactable"],
            EXPECTED["first_entry_interactable"],
        )
        occluders = {
            name
            for o in first.get("occluded_by", [])
            for name in (o["type_name"], o.get("name"))
            if name
        }
        if EXPECTED["first_entry_occluder"] in occluders:
            print(f"ok  first entry occluded by {EXPECTED['first_entry_occluder']!r}")
        else:
            failures.append(
                f"first entry occluders: expected {EXPECTED['first_entry_occluder']!r} "
                f"among {sorted(occluders)}"
            )

    ship = snapshot["ship_ui"]
    if ship:
        mid = ship["module_buttons_mid"]
        mid_ok = all(b["is_interactable"] for b in mid) if mid else True
        check("mid modules interactable", mid_ok, EXPECTED["module_mid_interactable"])

    neocom = snapshot["neocom"]
    if neocom:
        check(
            "neocom interactable count",
            (sum(1 for b in neocom["buttons"] if b["is_interactable"]), len(neocom["buttons"])),
            EXPECTED["neocom_interactable_count"],
        )

    panels = snapshot["info_panels"]
    if panels is None:
        failures.append("info panels: missing")
    else:
        present = {p["type_name"] for p in panels["panels"]}
        missing = EXPECTED["info_panel_types"] - present
        if missing:
            failures.append(f"info panels missing: {sorted(missing)}")
        else:
            print(f"ok  info panels contain {sorted(EXPECTED['info_panel_types'])}")

    if failures:
        print("\nFAILURES:")
        for failure in failures:
            print("  -", failure)
        return 1
    print("\nregression OK")
    return 0


if __name__ == "__main__":
    default = Path(__file__).parent.parent / "samples" / "process-sample-a8b4bd7529.zip"
    sample = Path(sys.argv[1]) if len(sys.argv) > 1 else default
    if not sample.exists():
        print(f"sample not found at {sample}; skipping regression")
        sys.exit(0)
    sys.exit(main(sample))
