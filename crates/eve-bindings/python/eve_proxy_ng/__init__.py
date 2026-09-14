"""eve_proxy_ng —— EVE Online 感知与语义层 Python 绑定。

EVE Online perception and semantics for Python agents. Discover
clients, read the UI tree from memory (live process or recorded
sample), and get structured semantic snapshots — overview entries,
module buttons, menus, inventories, hit testing — ready for agent
consumption. 支持网易国服（曙光/经典）与国际服。

核心 API（自编译扩展 `eve_proxy_ng._core` 重导出 / re-exported from
the compiled extension）:

- `discover_clients` — 发现运行中的客户端
- `UiReader` — UI 树/语义快照/命中测试
- `GameClient` — 客户端信息与窗口操作
- `get_resource_image` — 离线读取游戏资源（图标等）

附带设施（包内自带 / bundled inside the package）:

- `eve_proxy_ng.cli` — ``eve-cli`` 命令（console script），包装随包
  分发的 ``eve-cli.exe`` 与全部 ffmpeg DLL 依赖
- `docs_dir` — 随包分发的离线文档（HTML）路径

注意：内存读取属 EULA 违规行为，本项目仅用于学习研究。
Warning: memory reading violates the game EULA; research use only.
"""

from pathlib import Path

from ._core import (
    GameClient,
    UiReader,
    discover_clients,
    get_resource_image,
    module_name_from_icon,
    resources_info,
    type_name,
)

__all__ = [
    "GameClient",
    "UiReader",
    "discover_clients",
    "get_resource_image",
    "module_name_from_icon",
    "resources_info",
    "type_name",
    "bin_dir",
    "docs_dir",
    "__version__",
]

__version__ = "0.1.0"


def bin_dir() -> Path:
    """返回随包分发的二进制目录（eve-cli.exe + ffmpeg DLL）。

    Return the bundled binaries directory (``eve-cli.exe`` plus the
    ffmpeg shared libraries it depends on), inside the installed
    package. Add it to ``PATH`` / ``os.add_dll_directory`` before
    spawning the CLI manually.
    """
    return Path(__file__).parent / "bin"


def docs_dir() -> Path:
    """返回随包分发的离线文档（HTML 站点）目录。

    Return the directory of the bundled offline documentation (a
    static MkDocs HTML site). Open ``docs_dir() / "index.html"`` in a
    browser to read the full API reference without network access.
    """
    return Path(__file__).parent / "docs"
