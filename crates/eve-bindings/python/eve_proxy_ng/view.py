"""语义视图服务器：把解析结果实时绘制成网页版游戏界面。

Live semantic view server: reconstructs the game UI in the browser from
parsed snapshots — every element at its real game position, native game
icons, streaming updates over SSE. Pure standard-library HTTP (zero
extra dependencies), single-file Canvas frontend.

用法 / usage::

    # 活体客户端（默认 http://127.0.0.1:8765）
    eve-view --pid 31336
    # 或从 Python
    eve_proxy_ng.serve_view(pid=31336, open_browser=True)

    # 离线样本回放（无需开游戏；画面静态但完整渲染）
    eve_proxy_ng.serve_view(sample="samples/process-sample-xxxx.zip")

技术要点：``GET /`` 页面；``GET /stream`` SSE 场景流（默认 250ms 一帧，
仅传输紧凑投影而非完整快照）；``GET /icon?path=res:/…`` 原生图标字节
（内存缓存，经 :func:`eve_proxy_ng.get_resource_image` 读共享缓存）。
"""

from __future__ import annotations

import argparse
import json
import sys
import threading
import time
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from . import UiReader, get_resource_image

_PAGE = Path(__file__).with_name("view.html")

# ---------------------------------------------------------------- scene ---

_INTERACTABLE, _PASSIVE, _BLOCKED = 1, 0, 2
_DISABLED = 2  # 禁用与遮挡同为"无效"视觉类（红），标志文字区分


def _occluders(info: dict | None) -> list[list[int]]:
    """Covering rects (game coords) for one node, from its interaction
    info (`occluded_by[].region`) — the precise occlusion geometry the
    frontend clips drawing against."""
    rects = []
    for occluder in (info or {}).get("occluded_by") or []:  # info: node dict (flattened)
        region = occluder.get("region") or {}
        if region.get("width", 0) > 0 and region.get("height", 0) > 0:
            rects.append([region.get("x", 0), region.get("y", 0),
                          region["width"], region["height"]])
    return rects[:12]


def _node(kind, region, label=None, sub=None, icon=None, cls=_PASSIVE,
          detail=None, interaction=None):
    r = region or {}
    node = {
        "k": kind,
        "x": r.get("x", 0),
        "y": r.get("y", 0),
        "w": r.get("width", 0),
        "h": r.get("height", 0),
        "l": label,
        "s": sub,
        "i": icon,
        "c": cls,
        "d": detail,
    }
    occ = _occluders(interaction)
    if occ:
        node["occ"] = occ
    return node


def _interactable(source: dict) -> int:
    # InteractionInfo is serde-flattened: is_click_reachable / occluded_*
    # live at the TOP LEVEL of the node dict, not under "interaction".
    if not source.get("is_enabled", True):
        return _DISABLED
    if not source.get("is_click_reachable", True):
        return _BLOCKED
    return _INTERACTABLE


def build_scene(snap: dict) -> dict:
    """把语义快照投影成紧凑的绘制场景。

    Project a semantic snapshot into a compact drawing scene (list of
    positioned nodes + canvas size + stats). Independent of any
    transport so it is directly unit-testable.
    """
    nodes: list[dict] = []
    taken: set[tuple] = set()
    elements = snap.get("interaction_elements") or []
    # Region -> icon, so special nodes (neocom…) can reuse the icon the
    # interaction extractor found for the same on-screen button.
    icon_by_region: dict[tuple, str] = {}
    for element in elements:
        region = element.get("region") or {}
        if element.get("icon"):
            key = (region.get("x"), region.get("y"), region.get("width"), region.get("height"))
            icon_by_region.setdefault(key, element["icon"])

    def take(node: dict) -> None:
        # Degenerate regions (e.g. collapsed scroll containers reporting
        # negative extents) are not drawable.
        if node["w"] <= 0 or node["h"] <= 0:
            return
        taken.add((node["x"], node["y"], node["w"], node["h"]))
        nodes.append(node)

    # --- neocom (left button strip: dedicated rendering with icons) ---
    for button in (snap.get("neocom") or {}).get("buttons") or []:
        region = button.get("region") or {}
        key = (region.get("x"), region.get("y"), region.get("width"), region.get("height"))
        name = button.get("name") or ""
        label = (name.removesuffix("Btn").removesuffix("DataNode")
                 .removesuffix("Button").removesuffix("Node")) or name
        take(_node("neocom", region,
                   label=label,
                   sub=button.get("hint"),
                   icon=icon_by_region.get(key),
                   cls=_INTERACTABLE,
                   detail={"name": name, "hint": button.get("hint")},
                   interaction=button))

    # --- generic windows (popups/gacha/activities…): frames with
    # captions; skip ones already framed by a specialized window ---
    for window in snap.get("other_windows") or []:
        region = window.get("region") or {}
        key = (region.get("x"), region.get("y"), region.get("width"), region.get("height"))
        if key in taken or region.get("width", 0) < 120:
            continue
        caption = (window.get("caption")
                   or (window.get("type_name") or "").removesuffix("Wnd").removesuffix("Window"))
        take(_node("window", region,
                   label=caption,
                   cls=_INTERACTABLE if window.get("is_enabled", True) else _DISABLED,
                   interaction=window,
                   detail={
                       "kind": "other_window",
                       "type_name": window.get("type_name"),
                       "name": window.get("name"),
                       "elements": len(window.get("element_addresses") or []),
                   }))

    # --- character-select slots (login screen cards) ---
    for slot in ((snap.get("character_select") or {}).get("slots") or []):
        take(_node("charslot", slot.get("region"),
                   label=slot.get("name"),
                   sub=" / ".join((slot.get("details") or [])[:2]) or None,
                   cls=_interactable(slot),
                   interaction=slot,
                   detail={
                       "index": slot.get("index"),
                       "details": slot.get("details"),
                       "occluded_percent": slot.get("occluded_percent"),
                   }))

    # --- windows (frame + caption) ---
    # List-typed window sections iterate directly; single-window sections
    # (station/fitting) are Option<struct> — wrap in a one-item list.
    def _windows_of(key: str, title: str | None) -> list:
        value = snap.get(key)
        if value is None:
            return []
        if isinstance(value, list):
            return [(window, title) for window in value]
        return [(value, title)]

    for key, title in (
        ("overview_windows", None),
        ("inventory_windows", None),
        ("station_window", "站内服务"),
        ("fitting_window", "装配"),
    ):
        for window, fixed_title in _windows_of(key, title):
            caption = fixed_title or window.get("caption") or window.get("window_caption")
            take(_node("window", window.get("region"), label=caption, detail={
                "kind": key, "caption": caption,
            }, interaction=window))
            if key == "overview_windows":
                for tab in window.get("tabs") or []:
                    take(_node("tab", tab.get("region"), label=tab.get("name"),
                               cls=_INTERACTABLE if tab.get("is_selected") else _PASSIVE,
                               detail={"kind": "tab", "is_selected": tab.get("is_selected")},
                               interaction=tab))
                for entry in window.get("entries") or []:
                    take(_node("overview", entry.get("region"),
                               label=entry.get("object_name"),
                               sub=_fmt_distance(entry.get("distance_meters")),
                               icon=entry.get("icon"),
                               cls=_interactable(entry),
                               detail={
                                   "icon_name": entry.get("icon_name"),
                                   "indications": entry.get("indications"),
                                   "occluded_percent": entry.get("occluded_percent"),
                               },
                               interaction=entry))
            elif key == "inventory_windows":
                for item in window.get("items") or []:
                    qty = item.get("quantity")
                    take(_node("item", item.get("region"),
                               label=item.get("name"),
                               sub=f"×{qty}" if qty else None,
                               cls=_PASSIVE if not item.get("is_selected") else _BLOCKED,
                               detail={"is_selected": item.get("is_selected")},
                               interaction=item))

    for menu in snap.get("context_menus") or []:
        take(_node("menu", menu.get("region"), detail={"kind": "context_menu"},
                    interaction=menu))
        for entry in menu.get("entries") or []:
            take(_node("menuitem", entry.get("region"), label=entry.get("text"),
                       cls=_interactable(entry), detail={"kind": "menu_entry"},
                       interaction=entry))
    for menu in snap.get("util_menus") or []:
        take(_node("menu", menu.get("region"), detail={"kind": "util_menu"},
                    interaction=menu))
        for row in menu.get("checkboxes") or []:
            take(_node("menuitem", row.get("region"),
                       label=("☑ " if row.get("is_checked") else "☐ ") + (row.get("text") or ""),
                       cls=_interactable(row), detail={"checked": row.get("is_checked")},
                       interaction=row))

    # --- ship HUD ---
    ship = snap.get("ship_ui")
    if ship:
        for rack, buttons in (
            ("high", ship.get("module_buttons_high") or []),
            ("mid", ship.get("module_buttons_mid") or []),
            ("low", ship.get("module_buttons_low") or []),
        ):
            for button in buttons:
                cls = _INTERACTABLE
                if button.get("is_active"):
                    cls = _BLOCKED  # 前端用金色高亮 active，这里只透传状态
                ov = button.get("overload")
                if ov:
                    take(_node("ovarc", ov.get("region"),
                               sub=f"超载:{ov.get('state')}",
                               cls=_INTERACTABLE if ov.get("state") != "disabled" else _PASSIVE,
                               detail={"kind": "overload_arc", "state": ov.get("state")}))
                take(_node("module", button.get("region"),
                           label=button.get("module_name"),
                           sub=rack,
                           icon=button.get("icon"),
                           cls=cls,
                           interaction=button,
                           detail={
                               "type_id": button.get("type_id"),
                               "icon_name": button.get("icon_name"),
                               "is_active": button.get("is_active"),
                               "is_busy": button.get("is_busy"),
                           }))
        for button in ship.get("hud_buttons") or []:
            take(_node("hudbtn", button.get("region"),
                       label=button.get("label") or (button.get("kind") or "").removeprefix("hud."),
                       sub=(button.get("kind") or "").removeprefix("hud."),
                       icon=button.get("icon"),
                       cls=(_INTERACTABLE if button.get("is_enabled", True)
                            and button.get("is_click_reachable", True) else _BLOCKED),
                       interaction=button,
                       detail={
                           "kind": button.get("kind"),
                           "is_on": button.get("is_on"),
                           "is_enabled": button.get("is_enabled"),
                           "is_click_reachable": button.get("is_click_reachable"),
                       }))
        hp = ship.get("hitpoints") or {}
        take(_node("hud", ship.get("region"), detail={
            "capacitor_percent": ship.get("capacitor_percent"),
            "shield": hp.get("shield_percent"),
            "armor": hp.get("armor_percent"),
            "structure": hp.get("structure_percent"),
            "speed_text": ship.get("speed_text"),
            "indication": ship.get("indication"),
        }))

    window_names = {
        w.get("address"): (w.get("caption") or w.get("type_name") or "?")
        for w in snap.get("other_windows") or []
    }

    # --- generic interaction elements (dedup against special nodes) ---
    for element in elements:
        region = element.get("region") or {}
        key = (region.get("x"), region.get("y"), region.get("width"), region.get("height"))
        if key in taken:
            continue
        info = {"is_click_reachable": True, "occluded_percent": 0}
        info.update({k: element[k] for k in ("is_click_reachable", "occluded_percent") if k in element})
        if not info.get("is_click_reachable", True) or info.get("occluded_percent", 0) >= 50:
            cls = _BLOCKED
        elif element.get("role"):
            cls = _INTERACTABLE
        else:
            cls = _PASSIVE
        label = element.get("label") or element.get("text")
        node = _node("element", region,
                   interaction=element,
                   label=(label[:48] + "…") if label and len(label) > 48 else label,
                   sub=element.get("icon_name"),
                   icon=element.get("icon"),
                   cls=cls,
                   detail={
                       "type_name": element.get("type_name"),
                       "name": element.get("name"),
                       "role": element.get("role"),
                       "address": element.get("address"),
                       "hint": element.get("hint"),
                       "icon": element.get("icon"),
                       "occluded_percent": info.get("occluded_percent"),
                       "is_on_screen": element.get("is_on_screen", True),
                       "window": window_names.get(element.get("window_address")),
                   })
        vr = element.get("visible_region")
        if vr and (vr.get("x"), vr.get("y"), vr.get("width"), vr.get("height")) != (
                region.get("x"), region.get("y"), region.get("width"), region.get("height")):
            node["clip"] = [vr.get("x", 0), vr.get("y", 0),
                            vr.get("width", 0), vr.get("height", 0)]
        take(node)

    # --- client canvas: the game window's own area (from the root
    # layer regions), NOT the union of node extents (scroll strips and
    # virtualized grids carry off-screen cells far beyond the window). ---
    cs = snap.get("client_size") or {}
    client_w, client_h = cs.get("width", 0), cs.get("height", 0)
    if client_w <= 0:
        client_w = max(((l.get("region") or {}).get("x", 0) + (l.get("region") or {}).get("width", 0)
                        for l in snap.get("layers") or []), default=0)
    if client_h <= 0:
        client_h = max(((l.get("region") or {}).get("y", 0) + (l.get("region") or {}).get("height", 0)
                        for l in snap.get("layers") or []), default=0)
    if client_w <= 0:
        client_w = max((n["x"] + n["w"] for n in nodes), default=1280)
    if client_h <= 0:
        client_h = max((n["y"] + n["h"] for n in nodes), default=720)

    # Nodes entirely outside the client area are not rendered by the game.
    nodes[:] = [n for n in nodes
                if n["x"] < client_w and n["y"] < client_h
                and n["x"] + n["w"] > 0 and n["y"] + n["h"] > 0
                and (n.get("d") or {}).get("is_on_screen") is not False]

    width, height = client_w, client_h
    state = snap.get("game_state") or {}
    return {
        "t": int(time.time() * 1000),
        "tree": build_tree(snap),
        "size": [max(1280, width), max(720, height)],
        "state": {"screen": state.get("screen"), "modal": state.get("blocked_by_modal")},
        "stats": {"nodes": len(nodes), "node_count": snap.get("node_count")},
        "nodes": nodes,
    }


def _fmt_distance(meters) -> str | None:
    if meters is None:
        return None
    try:
        meters = int(meters)
    except (TypeError, ValueError):
        return None
    if meters >= 1_000_000_000:
        return f"{meters / 149_597_870_700:.1f} AU"
    if meters >= 1_000:
        return f"{meters / 1000:.0f} km"
    return f"{meters} m"


# ----------------------------------------------------------------- tree ---

def _titem(label, sub=None, address=None, rect=None, path=None, children=None, off=False):
    item = {"l": label}
    if sub:
        item["s"] = sub
    if address:
        item["a"] = address
    if rect:
        r = rect if isinstance(rect, (list, tuple)) else [
            rect.get("x"), rect.get("y"), rect.get("width"), rect.get("height")]
        item["r"] = r
    if path is not None:
        item["p"] = path
    if children is not None:
        item["c"] = children
    if off:
        item["off"] = True
    return item


def _preview(value) -> str | None:
    """标量/小字典的诚实预览（点击行可看原始 JSON）。"""
    if value is None or isinstance(value, (bool, int, float)):
        text = json.dumps(value, ensure_ascii=False)
        return text[:40]
    if isinstance(value, str):
        return value[:40] or None
    if isinstance(value, list):
        return f"[{len(value)}]"
    if isinstance(value, dict):
        return "{...}"
    return None


_ELEMENT_FLAGS = ("is_enabled", "is_click_reachable", "is_on_screen")
_ELEMENTS_BY_ADDR: dict = {}


def _element_row(element: dict, path: list) -> dict:
    """interaction_elements 成员行——字段全部来自元素自身（忠实）。"""
    flags = []
    if element.get("is_enabled") is False:
        flags.append("禁用")
    if element.get("is_click_reachable") is False:
        flags.append("点击不可达")
    if (element.get("occluded_percent") or 0) >= 50:
        flags.append("遮挡")
    if element.get("is_on_screen") is False:
        flags.append("屏外")
    sub = " / ".join(filter(None, [
        element.get("role"), element.get("icon_name"), *flags])) or None
    return _titem(
        element.get("label") or element.get("type_name") or "?",
        sub=sub, address=element.get("address"), rect=element.get("region"),
        path=path, off=element.get("is_enabled") is False)


def _render(key: str, value, path: list, depth: int) -> dict:
    """通用递归渲染：树 = 快照本体的结构镜像。

    dict → 子键（serde 字段序）；list → 编号成员；标量 → 叶子（预览即
    值，点击看原始 JSON）。interaction_elements 成员带地址/矩形联动。
    """
    if isinstance(value, dict):
        if not value:
            return _titem(key, sub="{}", path=path)
        children = [_render(k, v, path + [k], depth + 1) for k, v in value.items()]
        return _titem(key, sub=f"{len(value)} 字段", path=path, children=children)
    if isinstance(value, list):
        if not value:
            return _titem(key, sub="[]", path=path)
        # element_addresses：地址数组解析为元素行（与 interaction_
        # elements 的 join，两者都是快照字段）。
        if key == "element_addresses" and len(value) > 0 and isinstance(value[0], str) and _ELEMENTS_BY_ADDR:
            rows = []
            for addr in value:
                pair = _ELEMENTS_BY_ADDR.get(addr)
                if pair:
                    e, idx = pair
                    rows.append(_element_row(e, ["interaction_elements", idx]))
            if rows:
                return _titem(f"{key} ({len(value)})", children=rows)
        # interaction_elements 是全量数据源（根的 element_addresses
        # 通过地址 join 到此），但树中只展开无归属的——已归属元素
        # 在各自语义根下显示，避免双重出现。
        if key == "interaction_elements" and isinstance(value, list) and value and isinstance(value[0], dict):
            owned = sum(1 for e in value if e.get("window_address"))
            unowned = [e for e in value if not e.get("window_address")]
            rows = [_element_row(e, ["interaction_elements",
                                     value.index(e)])
                    for e in unowned[:100]]
            sub = f"{len(unowned)} 无归属"
            if owned:
                sub += f" · 另 {owned} 个已归入上方各语义根"
            return _titem(f"{key} ({len(value)})", sub=sub,
                          path=path, children=rows)
        rows = []
        for i, item in enumerate(value):
            item_path = path + [i]
            if path and path[0] == "interaction_elements":
                rows.append(_element_row(item, item_path))
            elif isinstance(item, (dict, list)):
                preview = None
                if isinstance(item, dict):
                    for candidate in ("label", "name", "caption", "text",
                                      "type_name", "object_name", "module_name"):
                        if item.get(candidate):
                            preview = str(item[candidate])[:24]
                            break
                rows.append(_render(f"[{i}]", item, item_path, depth + 1)
                            if not preview
                            else _titem(f"[{i}] {preview}", sub=_preview(item),
                                        rect=item.get("region") if isinstance(item, dict) else None,
                                        path=item_path,
                                        children=None if isinstance(item, list) else
                                        [_render(k, v, item_path + [k], depth + 2)
                                         for k, v in item.items()]
                                        if isinstance(item, dict) and len(item) <= 24 else None))
            else:
                rows.append(_titem(f"[{i}]", sub=_preview(item), path=item_path))
        return _titem(f"{key} ({len(value)})", path=path, children=rows)
    return _titem(key, sub=_preview(value), path=path)


def build_tree(snap: dict) -> list:
    """语义结构树：agent 的 read_snapshot() 快照的忠实结构镜像。

    顶层区段 = 快照顶层键（原序）；嵌套 = 真实字段/数组结构；每行可点
    击查看该片段的原始 JSON。不做任何视图层重组/认领/摘要拼装——对树
    的结构诉求应通过修改快照结构实现（网页与 API 永远同源）。
    """
    global _ELEMENTS_BY_ADDR
    _ELEMENTS_BY_ADDR = {
        e.get("address"): (e, i)
        for i, e in enumerate(snap.get("interaction_elements") or [])
    }
    return [_render(key, value, [key], 0) for key, value in snap.items()]


# ---------------------------------------------------------------- server ---

class _ViewServer:
    """Snapshot polling + SSE fan-out + icon cache (all in-process)."""

    def __init__(self, reader: UiReader, interval_ms: int = 250, static: bool = False):
        self.reader = reader
        self.interval = max(50, interval_ms) / 1000.0
        self.static = static  # sample replay: read once, keep serving
        self.scene: dict = {"error": "waiting for first frame"}
        self.snapshot: dict = {}
        self.error: str | None = None
        self._clients: list = []
        self._lock = threading.Lock()
        self._icons: dict[str, bytes] = {}
        self.updates = 0

    # -- polling ------------------------------------------------------------
    def start(self) -> None:
        threading.Thread(target=self._poll_loop, name="view-poll", daemon=True).start()

    def _poll_loop(self) -> None:
        while True:
            started = time.perf_counter()
            try:
                snap = self.reader.read_snapshot()
                scene = build_scene(snap)
                scene["stats"]["build_ms"] = round((time.perf_counter() - started) * 1000, 1)
                self.error = None
                self._publish(scene, snap)
                if self.static:
                    time.sleep(3600)
                    continue
            except Exception as exc:  # noqa: BLE001 — surface any read error
                self.error = f"{type(exc).__name__}: {exc}"
                self._publish({"error": self.error})
            elapsed = time.perf_counter() - started
            if elapsed < self.interval:
                time.sleep(self.interval - elapsed)

    def _publish(self, scene: dict, snap: dict | None = None) -> None:
        self.scene = scene
        if snap is not None:
            self.snapshot = snap
        self.updates += 1
        payload = json.dumps(scene, ensure_ascii=False).encode("utf-8")
        with self._lock:
            dead = []
            for queue in self._clients:
                try:
                    queue.put_nowait(payload)
                except Exception:  # noqa: BLE001 — full/disconnected client
                    dead.append(queue)
            for queue in dead:
                self._clients.remove(queue)

    # -- SSE registry -------------------------------------------------------
    def subscribe(self):
        import queue

        queued: "queue.Queue[bytes]" = queue.Queue(maxsize=4)
        with self._lock:
            self._clients.append(queued)
        return queued

    def unsubscribe(self, queued) -> None:
        with self._lock:
            if queued in self._clients:
                self._clients.remove(queued)

    # -- icons --------------------------------------------------------------
    def icon_bytes(self, res_path: str) -> bytes | None:
        cached = self._icons.get(res_path)
        if cached is not None:
            return cached
        try:
            data = get_resource_image(res_path)
        except Exception:  # noqa: BLE001 — unknown resource / no client
            data = b""
        if len(self._icons) > 4096:
            self._icons.clear()
        self._icons[res_path] = data
        return data or None


def _make_handler(server: _ViewServer):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):  # quiet
            pass

        def _send(self, content_type: str, body: bytes, cache: bool = False):
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            if cache:
                self.send_header("Cache-Control", "public, max-age=86400")
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):  # noqa: N802 — http.server API
            url = urlparse(self.path)
            if url.path in ("/", "/index.html"):
                self._send("text/html; charset=utf-8", _PAGE.read_bytes())
            elif url.path == "/scene":
                body = json.dumps(server.scene, ensure_ascii=False).encode("utf-8")
                self._send("application/json; charset=utf-8", body)
            elif url.path == "/icon":
                path = (parse_qs(url.query).get("path") or [""])[0]
                data = server.icon_bytes(path) if path.startswith("res:") else None
                if data:
                    self._send("image/png", data, cache=True)
                else:
                    self.send_response(404)
                    self.send_header("Content-Length", "0")
                    self.end_headers()
            elif url.path == "/snapshot":
                fragment = server.snapshot
                raw_path = (parse_qs(url.query).get("path") or [""])[0]
                for part in filter(None, raw_path.split(",")):
                    part = int(part) if part.lstrip("-").isdigit() else part
                    fragment = fragment[part]
                body = json.dumps(fragment, ensure_ascii=False, indent=1).encode("utf-8")
                self._send("application/json; charset=utf-8", body)
            elif url.path == "/stream":
                self._stream()
            else:
                self.send_response(404)
                self.send_header("Content-Length", "0")
                self.end_headers()

        def _stream(self):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            queued = server.subscribe()
            try:
                self.wfile.write(b"retry: 2000\n\n")
                self.wfile.flush()
                while True:
                    payload = queued.get()
                    self.wfile.write(b"data: " + payload + b"\n\n")
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError, OSError):
                pass
            finally:
                server.unsubscribe(queued)

    return Handler


def serve_view(
    pid: int | None = None,
    sample: str | None = None,
    host: str = "127.0.0.1",
    port: int = 8765,
    interval_ms: int = 250,
    open_browser: bool = False,
) -> None:
    """启动语义视图服务器（阻塞运行，Ctrl-C 停止）。

    Serve the live semantic view and block until interrupted. One of
    ``pid`` (live client) or ``sample`` (offline replay) selects the
    reader, exactly like :class:`eve_proxy_ng.UiReader`.

    Args:
        pid: 活体客户端进程 ID。Live client process id.
        sample: 离线样本 zip 路径（画面静态）。Path to a recorded sample.
        host: 监听地址，默认仅本机。Bind address (default localhost).
        port: 监听端口（默认 8765）。TCP port.
        interval_ms: 场景刷新间隔毫秒。Refresh interval in ms.
        open_browser: 启动后自动打开浏览器。Open the page automatically.

    Raises:
        ValueError: pid 与 sample 都未提供。Neither source argument given.
    """
    reader = UiReader(pid=pid, sample=sample)
    reader.find_ui_root()
    server = _ViewServer(reader, interval_ms=interval_ms, static=sample is not None)
    server.start()
    httpd = ThreadingHTTPServer((host, port), _make_handler(server))
    httpd.daemon_threads = True
    url = f"http://{host}:{port}/"
    print(f"eve-proxy-ng 语义视图: {url}  (Ctrl-C 停止)")
    if open_browser:
        webbrowser.open(url)
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\nstopped")


def main(argv: list[str] | None = None) -> int:
    """``eve-view`` 命令行入口。Console-script entry point."""
    ap = argparse.ArgumentParser(
        prog="eve-view",
        description="以网页实时重构游戏界面（语义元素按游戏坐标绘制，原生图标）",
    )
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--pid", type=int, help="活体客户端进程 ID")
    src.add_argument("--sample", help="离线样本 zip 路径")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--interval-ms", type=int, default=250)
    ap.add_argument("--open", action="store_true", help="自动打开浏览器")
    args = ap.parse_args(argv)
    serve_view(pid=args.pid, sample=args.sample, host=args.host, port=args.port,
               interval_ms=args.interval_ms, open_browser=args.open)
    return 0


if __name__ == "__main__":
    sys.exit(main())
