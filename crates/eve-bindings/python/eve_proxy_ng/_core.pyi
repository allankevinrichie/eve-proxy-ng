"""eve_proxy_ng 的 Rust 核心：客户端发现、窗口操作、内存读取与语义快照。

Rust core of the `eve_proxy_ng` package: client discovery, window
operations, memory reading (live process or recorded sample), and
semantic snapshots. Prefer importing from `eve_proxy_ng` (the package
re-exports everything); this module is the compiled extension.
"""

from typing import Any, Optional

def resources_info() -> dict[str, Any]:
    r"""资源表现状诊断:数据来自哪一层、规模与路径。

    Which resource layer is serving lookups (user overlay, embedded
    baseline, or empty), how many types it carries, and where the
    overlay would be written. Layers, highest priority first: manual
    overrides (code) → user overlay (`%LOCALAPPDATA%\eve_proxy_ng\resources`,
    or `$EVE_NG_DATA_DIR`) → embedded baseline derived from the local
    client via its own FSD loaders (`eve-cli icons update`).

    Returns:
        `dict`:`loaded_from`(层级描述)、`type_count`、
        `data_dir`(overlay 根目录,可能为 None)。
        `loaded_from` / `type_count` / `data_dir` keys.
    """

def type_name(type_id: int) -> Optional[str]:
    """typeID → 本地化类型名(精确通道)。

    Exact localized type name for a typeID — the precise resolution
    channel for `ModuleButton_<typeID>` node names. Chinese names (and
    国服 exclusives) come straight from the local client's FSD data.

    Args:
        type_id: 类型 ID。The type identifier.

    Returns:
        `str | None`:类型名;未知 ID 为 None。
        The localized name, or `None` when unknown.
    """

def module_name_from_icon(icon: str) -> Optional[str]:
    """图标资源路径 → 模块家族显示名。

    Module family display name for an icon resource path (the HUD shows
    module buttons as icons only; the same icon means the same module
    family on every ship).

    Args:
        icon: 图标资源路径,形如 `res:/ui/texture/icons/12_64_8.png`。
            Icon resource path.

    Returns:
        `str | None`:家族名(同图标最常见变体);未收录为 None。
        The family name, or `None` when unmapped.
    """

def discover_clients() -> list[GameClient]:
    """发现运行中的 EVE Online 客户端（覆盖全部服务器 flavor）。

    Discover all running EVE Online clients, across every server flavor
    (网易曙光/经典服 + 国际服).

    Returns:
        `list[GameClient]`:每个已运行客户端一项,含 PID、服务器
            (flavor)、角色名、窗口标题/句柄与 exe 路径。
            One `GameClient` per running client: pid, flavor, character
            name, window title/handle and exe path.

    Raises:
        RuntimeError: 系统进程枚举失败。If system process enumeration
            fails.

    Example:
        >>> import eve_proxy_ng
        >>> for c in eve_proxy_ng.discover_clients():
        ...     print(c.pid, c.flavor, c.character_name)
    """

def get_resource_image(res_path: str, shared_cache_root: Optional[str] = ...) -> bytes:
    """读取游戏资源文件(离线,不碰内存)。

    Read one game resource file — e.g. an icon PNG
    `res:/ui/texture/icons/13_64_5.png` — from the game's
    content-addressed shared cache on disk. No memory access, works
    without a running client when `shared_cache_root` is given.

    Args:
        res_path: 资源路径,形如 `res:/ui/texture/icons/<id>.png`。
            Resource path, e.g. `res:/ui/texture/icons/13_64_5.png`.
        shared_cache_root: 共享缓存根目录
            (形如 `C:/EVE/SharedCache`)。缺省时从第一个运行中
            客户端的 exe 路径推导。Shared-cache root directory;
            derived from the first running client when omitted.

    Returns:
        `bytes`:文件的原始内容(PNG 即图片字节)。
            Raw file bytes (for PNGs, the image itself).

    Raises:
        RuntimeError: 找不到运行中客户端且未传 `shared_cache_root`、
            缓存打开失败或资源不存在。No running client and no explicit
            root, cache open failure, or unknown resource path.
    """

class GameClient:
    """一个正在运行的游戏客户端。

    One running game client, as returned by `eve_proxy_ng.discover_clients`.
    Attributes are plain values (`pid`/`flavor`/…); window operations
    are methods.
    """

    pid: int
    """进程 ID。Process identifier of the client."""

    flavor: str
    """服务器标识:`infinity`(曙光) / `serenity`(经典) /
    `tranquility`(国际)。Server flavor tag."""

    character_name: Optional[str]
    """已登录角色名;未到角色选择后为 `None`。
    Logged-in character name, or `None` before selection."""

    window_title: str
    """主窗口标题(含角色名,如 `星战前夜:晨曦 [Infinity] - …`)。
    Main window title (includes character name)."""

    window_handle: Optional[int]
    """窗口句柄 (HWND) 整数值;窗口已消失时为 `None`。
    Window handle as an integer, or `None` if gone."""

    exe_path: str
    """客户端 exe 完整路径。Full path of the client executable."""

    def activate_window(self) -> bool:
        """激活(置前并聚焦)客户端窗口。

        Bring the client window to the front and focus it.

        Returns:
            `bool`:是否成功(无窗口时 False)。
            Whether the activation succeeded.
        """

    def minimize_window(self) -> bool:
        """最小化客户端窗口。Minimize the client window.

        Returns:
            `bool`:是否成功。Whether it succeeded.
        """

    def restore_window(self) -> bool:
        """恢复(还原)客户端窗口。Restore the client window.

        Returns:
            `bool`:是否成功。Whether it succeeded.
        """

class UiReader:
    """UI 树读取器：活体进程或离线样本。

    Reader over one game client — either a live process (`pid=`) or a
    recorded sample archive (`sample=`); exactly one of the two.
    Locates the UI root once (`find_ui_root`, 冷扫描 ~20s 并内部缓存
    / cold scan ~20s, cached), then reads are hot and cheap.

    Args:
        pid: 活体客户端进程 ID。Live client process id.
        sample: dump 样本 zip 路径（`eve-cli dump` 产物，
            与 Sanderling ProcessSample 兼容）。Path to a recorded
            sample archive.

    Raises:
        ValueError: 两个参数都没传。Neither argument given.
        RuntimeError: 进程打开失败或样本损坏。Process open or
            sample load failure.

    Examples:
        >>> reader = eve_proxy_ng.UiReader(pid=31336)      # 活体 / live
        >>> reader = eve_proxy_ng.UiReader(sample="a.zip") # 回放 / replay
    """

    def __init__(self, pid: Optional[int] = ..., sample: Optional[str] = ...) -> None: ...

    def find_ui_root(self) -> str:
        """定位并缓存 UI 根(首次为全内存冷扫描,~20s;此后热读)。

        Discover and cache the UI root address. The first call is a full
        cold scan (~20s); every later read reuses the cached root and is
        fast. The root moves on scene changes — a failed read
        invalidates it internally so the next call re-searches.

        Returns:
            `str`:UI 根地址,0x 十六进制。The root address as a
                `0x…` hex string.

        Raises:
            RuntimeError: 扫描失败(找不到 UIRoot / 进程退出)。
                Scan failure (no UIRoot found / process gone).
        """

    def read_tree_json(self) -> str:
        """读取完整 UI 树并序列化为 Sanderling 兼容 JSON 文本。

        Read the full UI tree (one point-in-time snapshot) and return it
        as Sanderling-compatible JSON text — object addresses are
        decimal strings, wide ints carry `int_low32`.

        Returns:
            `str`:整棵树的 JSON(in_space ~2600 节点,数 MB)。
                The whole tree as JSON (~2600 nodes in space, several MB).

        Raises:
            RuntimeError: 读取失败。Read failure.
        """

    def read_snapshot(self, flavor: Optional[str] = ...) -> dict[str, Any]:
        """读取 UI 树并解析为语义快照(嵌套 dict)。

        Read the tree and return the semantic snapshot as plain Python
        objects. Top-level keys include `game_state` (screen /
        blocked_by_modal), `ship_ui` (module buttons with type_id,
        capacitor, hitpoints), `overview_windows` (entries with
        name/distance/interactability), `context_menus`,
        `inventory_windows`, `neocom` and `interaction_elements`
        (every actionable element with role / is_interactable /
        occluded_by). Full schema in the bundled docs.

        Args:
            flavor: 服务器语义档:`"infinity"`(默认)/`"serenity"`
                /`"tranquility"`。Semantic profile per server;
                defaults to 曙光/infinity.

        Returns:
            `dict`:快照(可直接下标访问)。The snapshot as nested
                dicts and lists.

        Raises:
            RuntimeError: 读取失败。Read failure.
        """

    def hit_test(self, x: int, y: int) -> Optional[dict[str, Any]]:
        """命中测试:窗口客户区坐标 (x, y) 处的点击会落到哪个节点。

        Where would a click at window-client coordinates `(x, y)`
        land? Routes through the client's own hit-test semantics
        (`_pickState`) topmost-first, with scroll-viewport clipping.

        Args:
            x: 客户区横坐标(像素)。Client-area x in pixels.
            y: 客户区纵坐标(像素)。Client-area y in pixels.

        Returns:
            `dict | None`:命中节点报告——`type_name`/`name`/
                `address`/`region`(x/y/width/height)/`pick_state`/
                `blocked`/`text`;该点不接收输入时为 `None`。
                A hit report dict, or `None` when no input-taking node
                covers the point.

        Raises:
            RuntimeError: 读取失败。Read failure.
        """
