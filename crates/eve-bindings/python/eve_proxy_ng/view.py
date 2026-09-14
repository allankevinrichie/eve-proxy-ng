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


def _node(kind, region, label=None, sub=None, icon=None, cls=_PASSIVE, detail=None):
    r = region or {}
    return {
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


def _interactable(node_with_interaction: dict) -> int:
    info = node_with_interaction.get("interaction") or {}
    if not info.get("is_interactable", True):
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
                   detail={"name": name, "hint": button.get("hint")}))

    # --- windows (frame + caption) ---
    for key, title in (
        ("overview_windows", None),
        ("inventory_windows", None),
        ("station_window", "站内服务"),
        ("fitting_window", "装配"),
    ):
        for window in snap.get(key) or []:
            caption = title or window.get("caption") or window.get("window_caption")
            take(_node("window", window.get("region"), label=caption, detail={
                "kind": key, "caption": caption,
            }))
            if key == "overview_windows":
                for tab in window.get("tabs") or []:
                    take(_node("tab", tab.get("region"), label=tab.get("name"),
                               cls=_INTERACTABLE if tab.get("is_selected") else _PASSIVE,
                               detail={"kind": "tab", "is_selected": tab.get("is_selected")}))
                for entry in window.get("entries") or []:
                    take(_node("overview", entry.get("region"),
                               label=entry.get("object_name"),
                               sub=_fmt_distance(entry.get("distance_meters")),
                               icon=entry.get("icon"),
                               cls=_interactable(entry),
                               detail={
                                   "icon_name": entry.get("icon_name"),
                                   "indications": entry.get("indications"),
                                   "occluded_percent": (entry.get("interaction") or {}).get("occluded_percent"),
                               }))
            elif key == "inventory_windows":
                for item in window.get("items") or []:
                    qty = item.get("quantity")
                    take(_node("item", item.get("region"),
                               label=item.get("name"),
                               sub=f"×{qty}" if qty else None,
                               cls=_PASSIVE if not item.get("is_selected") else _BLOCKED,
                               detail={"is_selected": item.get("is_selected")}))

    for menu in snap.get("context_menus") or []:
        take(_node("menu", menu.get("region"), detail={"kind": "context_menu"}))
        for entry in menu.get("entries") or []:
            take(_node("menuitem", entry.get("region"), label=entry.get("text"),
                       cls=_interactable(entry), detail={"kind": "menu_entry"}))
    for menu in snap.get("util_menus") or []:
        take(_node("menu", menu.get("region"), detail={"kind": "util_menu"}))
        for row in menu.get("checkboxes") or []:
            take(_node("menuitem", row.get("region"),
                       label=("☑ " if row.get("is_checked") else "☐ ") + (row.get("text") or ""),
                       cls=_interactable(row), detail={"checked": row.get("is_checked")}))

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
                take(_node("module", button.get("region"),
                           label=button.get("module_name"),
                           sub=rack,
                           icon=button.get("icon"),
                           cls=cls,
                           detail={
                               "type_id": button.get("type_id"),
                               "icon_name": button.get("icon_name"),
                               "is_active": button.get("is_active"),
                               "is_busy": button.get("is_busy"),
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

    # --- generic interaction elements (dedup against special nodes) ---
    for element in elements:
        region = element.get("region") or {}
        key = (region.get("x"), region.get("y"), region.get("width"), region.get("height"))
        if key in taken:
            continue
        info = {"is_interactable": True, "occluded_percent": 0}
        info.update(element.get("interaction") or {})
        if not info.get("is_interactable", True) or info.get("occluded_percent", 0) >= 50:
            cls = _BLOCKED
        elif element.get("role"):
            cls = _INTERACTABLE
        else:
            cls = _PASSIVE
        hint = element.get("hint")
        label = (element.get("text") or element.get("role")
                 or element.get("name")
                 or (hint[:16] if hint else None))
        take(_node("element", region,
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
                   }))

    # --- canvas size + stats ---
    width = max((n["x"] + n["w"] for n in nodes), default=1280)
    height = max((n["y"] + n["h"] for n in nodes), default=720)
    state = snap.get("game_state") or {}
    return {
        "t": int(time.time() * 1000),
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


# ---------------------------------------------------------------- server ---

class _ViewServer:
    """Snapshot polling + SSE fan-out + icon cache (all in-process)."""

    def __init__(self, reader: UiReader, interval_ms: int = 250, static: bool = False):
        self.reader = reader
        self.interval = max(50, interval_ms) / 1000.0
        self.static = static  # sample replay: read once, keep serving
        self.scene: dict = {"error": "waiting for first frame"}
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
                self._publish(scene)
                if self.static:
                    time.sleep(3600)
                    continue
            except Exception as exc:  # noqa: BLE001 — surface any read error
                self.error = f"{type(exc).__name__}: {exc}"
                self._publish({"error": self.error})
            elapsed = time.perf_counter() - started
            if elapsed < self.interval:
                time.sleep(self.interval - elapsed)

    def _publish(self, scene: dict) -> None:
        self.scene = scene
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
