//! Python bindings for eve-memory / eve-semantics (`eve_proxy_ng` module).
//!
//! Exposes client discovery, window operations, memory reading (from live
//! clients or recorded samples), and semantic snapshots for agent layers.

use pyo3::prelude::*;

/// 资源表现状诊断：数据来自哪一层、规模与路径。
///
/// Which resource layer is serving lookups (user overlay, embedded
/// baseline, or empty), how many types it carries, and where the
/// overlay would be written. Layers, highest priority first: manual
/// overrides (code) → user overlay (`%LOCALAPPDATA%\eve_proxy_ng\resources`,
/// or `$EVE_NG_DATA_DIR`) → embedded baseline derived from the local
/// client via its own FSD loaders (`eve-cli icons update`).
///
/// Returns:
///     `dict`：`loaded_from`（层级描述）、`type_count`、
///     `data_dir`（overlay 根目录，可能为 None）。
///     `loaded_from` / `type_count` / `data_dir` keys.
#[pyfunction]
fn resources_info() -> PyResult<pyo3::PyObject> {
    let store = eve_semantics::resources::ResourceStore::global();
    Python::with_gil(|py| {
        let dict = pyo3::types::PyDict::new(py);
        dict.set_item("loaded_from", &store.loaded_from)?;
        dict.set_item("type_count", store.type_count)?;
        match eve_semantics::resources::data_dir() {
            Some(dir) => dict.set_item("data_dir", dir.display().to_string())?,
            None => dict.set_item("data_dir", py.None())?,
        }
        Ok(dict.into())
    })
}

/// typeID → 本地化类型名（精确通道）。
///
/// Exact localized type name for a typeID — the precise resolution
/// channel for `ModuleButton_<typeID>` node names. Chinese names (and
/// 国服 exclusives) come straight from the local client's FSD data.
///
/// Args:
///     type_id: 类型 ID。The type identifier.
///
/// Returns:
///     `str | None`：类型名；未知 ID 为 None。
///     The localized name, or `None` when unknown.
#[pyfunction]
fn type_name(type_id: u64) -> Option<String> {
    eve_semantics::resources::ResourceStore::global()
        .type_name(type_id)
        .map(String::from)
}

/// 图标资源路径 → 模块家族显示名。
///
/// Module family display name for an icon resource path (the HUD shows
/// module buttons as icons only; the same icon means the same module
/// family on every ship).
///
/// Args:
///     icon: 图标资源路径，形如 `res:/ui/texture/icons/12_64_8.png`。
///         Icon resource path.
///
/// Returns:
///     `str | None`：家族名（同图标最常见变体）；未收录为 None。
///     The family name, or `None` when unmapped.
#[pyfunction]
fn module_name_from_icon(icon: &str) -> Option<String> {
    eve_semantics::resources::ResourceStore::global()
        .module_name(icon)
        .map(String::from)
}

/// 发现运行中的 EVE Online 客户端（覆盖全部服务器 flavor）。
///
/// Discover all running EVE Online clients, across every server flavor
/// (网易曙光/经典服 + 国际服).
///
/// Returns:
///     `list[GameClient]`：每个已运行客户端一项，含 PID、服务器
///     (flavor)、角色名、窗口标题/句柄与 exe 路径。
///     One `GameClient` per running client: pid, flavor, character
///     name, window title/handle and exe path.
///
/// Raises:
///     RuntimeError: 系统进程枚举失败。If system process enumeration
///     fails.
///
/// Example:
///     >>> import eve_proxy_ng
///     >>> for c in eve_proxy_ng.discover_clients():
///     ...     print(c.pid, c.flavor, c.character_name)
#[pyfunction]
fn discover_clients() -> PyResult<Vec<PyGameClient>> {
    let clients = eve_memory::discover_clients()
        .map_err(|e: eve_memory::Error| {
            PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
        })?;
    Ok(clients.into_iter().map(PyGameClient::from).collect())
}

/// 读取游戏资源文件（离线，不碰内存）。
///
/// Read one game resource file — e.g. an icon PNG
/// `res:/ui/texture/icons/13_64_5.png` — from the game's
/// content-addressed shared cache on disk. No memory access, works
/// without a running client when `shared_cache_root` is given.
///
/// Args:
///     res_path: 资源路径，形如 `res:/ui/texture/icons/<id>.png`。
///         Resource path, e.g. `res:/ui/texture/icons/13_64_5.png`.
///     shared_cache_root: 共享缓存根目录
///         (形如 `C:/EVE/SharedCache`)。缺省时从第一个运行中
///         客户端的 exe 路径推导。Shared-cache root directory;
///         derived from the first running client when omitted.
///
/// Returns:
///     `bytes`：文件的原始内容（PNG 即图片字节）。
///     Raw file bytes (for PNGs, the image itself).
///
/// Raises:
///     RuntimeError: 找不到运行中客户端且未传 `shared_cache_root`、
///         缓存打开失败或资源不存在。No running client and no explicit
///         root, cache open failure, or unknown resource path.
#[pyfunction]
#[pyo3(signature = (res_path, shared_cache_root=None))]
fn get_resource_image(
    res_path: &str,
    shared_cache_root: Option<&str>,
) -> PyResult<pyo3::Py<pyo3::types::PyBytes>> {
    let (root, flavor) = match shared_cache_root {
        Some(root) => (std::path::PathBuf::from(root), "infinity".to_string()),
        None => {
            let clients = eve_memory::discover_clients()
                .map_err(|e: eve_memory::Error| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string())
                })?;
            let client = clients
                .first()
                .ok_or_else(|| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                        "no running client; pass shared_cache_root",
                    )
                })?;
            let root = eve_memory::ResourceCache::root_from_client_exe(&client.exe_path)
                .ok_or_else(|| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(
                        "cannot derive shared cache root from client exe path",
                    )
                })?;
            let flavor = client
                .exe_path
                .parent()
                .and_then(|bin| bin.parent())
                .and_then(|dir| dir.file_name())
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "infinity".to_string());
            (root, flavor)
        }
    };
    let cache = eve_memory::ResourceCache::open(&root, &flavor)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
    let bytes = cache
        .read(res_path)
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
    Python::with_gil(|py| Ok(pyo3::types::PyBytes::new(py, &bytes).unbind()))
}

/// 一个正在运行的游戏客户端。
///
/// One running game client, as returned by
/// `eve_proxy_ng.discover_clients`. Attributes are plain values
/// (`pid`/`flavor`/…); window operations are methods.
#[pyclass(name = "GameClient")]
struct PyGameClient {
    pid: u32,
    flavor: String,
    character_name: Option<String>,
    window_title: String,
    window_handle: Option<isize>,
    /// 客户端 exe 完整路径。Full path of the client executable.
    #[pyo3(get)]
    exe_path: String,
}

#[pymethods]
impl PyGameClient {
    /// 进程 ID。Process identifier of the client.
    #[getter]
    fn pid(&self) -> u32 {
        self.pid
    }

    /// 服务器标识：`infinity`(曙光) / `serenity`(经典) /
    /// `tranquility`(国际)。Server flavor tag.
    #[getter]
    fn flavor(&self) -> &str {
        &self.flavor
    }

    /// 已登录角色名；未到角色选择后为 `None`。
    /// Logged-in character name, or `None` before selection.
    #[getter]
    fn character_name(&self) -> Option<&str> {
        self.character_name.as_deref()
    }

    /// 主窗口标题（含角色名，如 `星战前夜：晨曦 [Infinity] - …`）。
    /// Main window title (includes character name).
    #[getter]
    fn window_title(&self) -> &str {
        &self.window_title
    }

    /// 窗口句柄 (HWND) 整数值；窗口已消失时为 `None`。
    /// Window handle as an integer, or `None` if gone.
    #[getter]
    fn window_handle(&self) -> Option<isize> {
        self.window_handle
    }

    /// 激活（置前并聚焦）客户端窗口。
    ///
    /// Bring the client window to the front and focus it.
    ///
    /// Returns:
    ///     `bool`：是否成功（无窗口时 False）。
    ///     Whether the activation succeeded.
    fn activate_window(&self) -> bool {
        self.window_handle
            .and_then(eve_memory::WindowHandle::from_raw)
            .map(|w| w.activate())
            .unwrap_or(false)
    }

    /// 最小化客户端窗口。Minimize the client window.
    ///
    /// Returns:
    ///     `bool`：是否成功。Whether it succeeded.
    fn minimize_window(&self) -> bool {
        self.window_handle
            .and_then(eve_memory::WindowHandle::from_raw)
            .map(|w| w.minimize())
            .unwrap_or(false)
    }

    /// 恢复（还原）客户端窗口。Restore the client window.
    ///
    /// Returns:
    ///     `bool`：是否成功。Whether it succeeded.
    fn restore_window(&self) -> bool {
        self.window_handle
            .and_then(eve_memory::WindowHandle::from_raw)
            .map(|w| w.restore())
            .unwrap_or(false)
    }

    fn __repr__(&self) -> String {
        format!(
            "<GameClient pid={} flavor={} char={}>",
            self.pid,
            self.flavor,
            self.character_name.as_deref().unwrap_or("-")
        )
    }
}

impl From<eve_memory::GameClient> for PyGameClient {
    fn from(client: eve_memory::GameClient) -> PyGameClient {
        let window_handle = client.window_handle_raw();
        PyGameClient {
            pid: client.pid,
            flavor: client.flavor.internal_tag().unwrap_or("unknown").to_string(),
            character_name: client.character_name,
            window_title: client.window_title,
            window_handle,
            exe_path: client.exe_path.display().to_string(),
        }
    }
}

/// UI 树读取器：活体进程或离线样本。
///
/// Reader over one game client — either a live process (`pid=`) or a
/// recorded sample archive (`sample=`); exactly one of the two.
/// Locates the UI root once (`find_ui_root`, 冷扫描 ~20s 并内部缓存
/// / cold scan ~20s, cached), then reads are hot and cheap.
///
/// Args:
///     pid: 活体客户端进程 ID。Live client process id.
///     sample: dump 样本 zip 路径（`eve-cli dump` 产物，
///         与 Sanderling ProcessSample 兼容）。Path to a recorded
///         sample archive.
///
/// Raises:
///     ValueError: 两个参数都没传。Neither argument given.
///     RuntimeError: 进程打开失败或样本损坏。Process open or
///     sample load failure.
///
/// Examples:
///     >>> reader = eve_proxy_ng.UiReader(pid=31336)      # 活体 / live
///     >>> reader = eve_proxy_ng.UiReader(sample="a.zip") # 回放 / replay
#[pyclass(name = "UiReader")]
struct PyUiReader {
    reader: eve_memory::UiReader,
}

#[pymethods]
impl PyUiReader {
    #[new]
    #[pyo3(signature = (pid=None, sample=None))]
    fn new(pid: Option<u32>, sample: Option<&str>) -> PyResult<PyUiReader> {
        let reader = match (pid, sample) {
            (Some(pid), _) => eve_memory::UiReader::live(pid),
            (None, Some(sample)) => eve_memory::UiReader::sample(std::path::Path::new(sample)),
            (None, None) => {
                return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "pass pid= or sample=",
                ))
            }
        }
        .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
        Ok(PyUiReader { reader })
    }

    /// 定位并缓存 UI 根（首次为全内存冷扫描，~20s；此后热读）。
    ///
    /// Discover and cache the UI root address. The first call is a full
    /// cold scan (~20s); every later read reuses the cached root and is
    /// fast. The root moves on scene changes — a failed read
    /// invalidates it internally so the next call re-searches.
    ///
    /// Returns:
    ///     `str`：UI 根地址，0x 十六进制。The root address as a
    ///     `0x…` hex string.
    ///
    /// Raises:
    ///     RuntimeError: 扫描失败（找不到 UIRoot / 进程退出）。
    ///     Scan failure (no UIRoot found / process gone).
    fn find_ui_root(&mut self) -> PyResult<String> {
        self.reader
            .find_ui_root()
            .map(|root| format!("{:#x}", root.0))
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))
    }

    /// 读取完整 UI 树并序列化为 Sanderling 兼容 JSON 文本。
    ///
    /// Read the full UI tree (one point-in-time snapshot) and return it
    /// as Sanderling-compatible JSON text — object addresses are
    /// decimal strings, wide ints carry `int_low32`.
    ///
    /// Returns:
    ///     `str`：整棵树的 JSON（in_space ~2600 节点，数 MB）。
    ///     The whole tree as JSON (~2600 nodes in space, several MB).
    ///
    /// Raises:
    ///     RuntimeError: 读取失败。Read failure.
    fn read_tree_json(&mut self) -> PyResult<String> {
        let tree = self
            .reader
            .read_tree()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
        serde_json::to_string(&tree)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))
    }

    /// 读取 UI 树并解析为语义快照（嵌套 dict）。
    ///
    /// Read the tree and return the semantic snapshot as plain Python
    /// objects. Top-level keys include `game_state` (screen /
    /// blocked_by_modal), `ship_ui` (module buttons with type_id,
    /// capacitor, hitpoints), `overview_windows` (entries with
    /// name/distance/interactability), `context_menus`,
    /// `inventory_windows`, `neocom` and `interaction_elements`
    /// (every actionable element with role / is_click_reachable /
    /// occluded_by). Full schema in the bundled docs.
    ///
    /// Args:
    ///     flavor: 服务器语义档：`"infinity"`(默认)/`"serenity"`
    ///         /`"tranquility"`。Semantic profile per server;
    ///         defaults to 曙光/infinity.
    ///
    /// Returns:
    ///     `dict`：快照（可直接下标访问）。The snapshot as nested
    ///     dicts and lists.
    ///
    /// Raises:
    ///     RuntimeError: 读取失败。Read failure.
    #[pyo3(signature = (flavor=None))]
    fn read_snapshot(&mut self, flavor: Option<&str>) -> PyResult<pyo3::PyObject> {
        let tree = self
            .reader
            .read_tree()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
        let flavor = match flavor {
            Some("serenity") => eve_memory::Flavor::Serenity,
            Some("tranquility") => eve_memory::Flavor::Tranquility,
            _ => eve_memory::Flavor::Infinity,
        };
        let snapshot = eve_semantics::parse_ui_tree(&tree, flavor);
        let json = serde_json::to_value(&snapshot)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
        Python::with_gil(|py| json_to_python(py, &json))
    }

    /// 命中测试：窗口客户区坐标 (x, y) 处的点击会落到哪个节点。
    ///
    /// Where would a click at window-client coordinates `(x, y)`
    /// land? Routes through the client's own hit-test semantics
    /// (`_pickState`) topmost-first, with scroll-viewport clipping.
    ///
    /// Args:
    ///     x: 客户区横坐标（像素）。Client-area x in pixels.
    ///     y: 客户区纵坐标（像素）。Client-area y in pixels.
    ///
    /// Returns:
    ///     `dict | None`：命中节点报告——`type_name`/`name`/
    ///     `address`/`region`(x/y/width/height)/`pick_state`/
    ///     `blocked`/`text`；该点不接收输入时为 `None`。
    ///     A hit report dict, or `None` when no input-taking node
    ///     covers the point.
    ///
    /// Raises:
    ///     RuntimeError: 读取失败。Read failure.
    fn hit_test(&mut self, x: i64, y: i64) -> PyResult<Option<pyo3::PyObject>> {
        let tree = self
            .reader
            .read_tree()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
        let profile = eve_semantics::FlavorProfile::for_flavor(eve_memory::Flavor::Infinity);
        let regioned = eve_semantics::region::RegionedTree::build(&tree);
        let hit = match eve_semantics::interaction::hit_test(
            &regioned,
            x,
            y,
            profile.layer_order_topmost_first,
        ) {
            Some(hit) => hit,
            None => return Ok(None),
        };
        let report: eve_semantics::interaction::HitTestReport = hit.into();
        let json = serde_json::to_value(&report)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(e.to_string()))?;
        Python::with_gil(|py| json_to_python(py, &json)).map(Some)
    }
}

// `BoundObject::into_py` is the only uniform owned-`PyObject` conversion on
// the pyo3 0.23 Bound/Borrowed pair; the replacement API is not in this
// release.
#[allow(deprecated)]
fn json_to_python(py: Python<'_>, value: &serde_json::Value) -> PyResult<pyo3::PyObject> {
    use pyo3::conversion::IntoPyObject;
    use pyo3::types::PyList;
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(b) => Ok(b.into_pyobject(py)?.into_py(py)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into_pyobject(py)?.into_py(py))
            } else {
                let f: f64 = n.as_f64().unwrap_or_default();
                Ok(f.into_pyobject(py)?.into_py(py))
            }
        }
        serde_json::Value::String(s) => Ok(s.into_pyobject(py)?.into_py(py)),
        serde_json::Value::Array(items) => {
            let list = PyList::empty(py);
            for item in items {
                list.append(json_to_python(py, item)?)?;
            }
            Ok(list.into_py(py))
        }
        serde_json::Value::Object(map) => {
            let dict = pyo3::types::PyDict::new(py);
            for (key, item) in map {
                dict.set_item(key, json_to_python(py, item)?)?;
            }
            Ok(dict.into_py(py))
        }
    }
}

/// eve_proxy_ng 的 Rust 核心：客户端发现、窗口操作、内存读取与语义快照。
///
/// Rust core of the `eve_proxy_ng` package: client discovery, window
/// operations, memory reading (live process or recorded sample), and
/// semantic snapshots. Prefer importing from `eve_proxy_ng` (the package
/// re-exports everything); this module is the compiled extension.
#[pymodule]
fn _core(m: &Bound<'_, pyo3::types::PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(discover_clients, m)?)?;
    m.add_function(wrap_pyfunction!(get_resource_image, m)?)?;
    m.add_function(wrap_pyfunction!(resources_info, m)?)?;
    m.add_function(wrap_pyfunction!(type_name, m)?)?;
    m.add_function(wrap_pyfunction!(module_name_from_icon, m)?)?;
    m.add_class::<PyGameClient>()?;
    m.add_class::<PyUiReader>()?;
    Ok(())
}
