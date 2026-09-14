# EVE Online 内存读取路线：调研总结与 Rust 重写蓝图

> **用途**：在新 session 中用 Rust 从零重写本方案时的完整输入材料。
> **调研时间**：2026-09-11 / 09-12（两轮：路线全景 + 实现细节 + 执行侧）。
> **事实状态**：文中结论均经源码/论坛/官方文档交叉验证；个别标注【推断】或【待自测】。
>
> ⚠️ **风险声明（已知情决策）**：内存读取属 EULA 违规（6.A.3 兜底条款），CCP 按行为侧检测执法（无内核反作弊，无"仅因读内存被封"的公开案例），两振政策（首犯 3 天 / 再犯永封）+ 账号连坐。实务：小号先行、控制在线时长、拟人化输入节奏、bot 与游戏分 Windows 用户隔离。

---

## 1. 方案总览

目标：用 AI agent 完成 EVE Online 重复性任务，感知层采用**进程内存读取**获取完整结构化 UI 树（带坐标/文本/状态），配合 ESI API（信息面）+ 本地日志（事件面）。

```
EVE 客户端 (exefile.exe, 64-bit)
   │ ReadProcessMemory（只读，同用户，无需管理员）
   ▼
Rust 读取器（对标 Sanderling read-memory-64-bit）
   │ 原始 UI 树（Python 对象图 → 自定义序列化）
   ▼
Rust 语义提取层（按任务增量移植 ParseUserInterface.elm 的游戏知识）
   │ 紧凑结构化状态（Overview 行 / 模块 / 电容 / 库存 / 菜单…）
   ▼
状态服务（HTTP / MCP）──▶ LLM agent 决策 ──▶ ESI 补充数据
   │ effect 序列（5 原语 + 随机延迟 + 曲线轨迹）
   ▼
Rust 输入执行器（SendInput + ClientToScreen 坐标换算）
```

关键架构决策（已定）：
- **不寄生 BotLab 闭源客户端**，感知 + 执行全自建；
- **语义提取按任务增量移植**（不一次性移植 3700 行 Elm，也不引入 Pine VM）；
- **离线采样驱动开发**（sample 库 + 回归测试，是对 UI 漂移和 Python 3 迁移的唯一结构性对冲）；
- 信息面任务（市场数据、工业状态、合同监控、技能队列等）走 ESI，**不**用客户端。已有现成 EVE MCP server：pfh59/eve-mcp-server（C#，52 工具，2026-09 活跃）。

---

## 2. 生态与参考实现

| 项目 | 许可证 | 状态（2026-09） | 对 Rust 重写的价值 |
|---|---|---|---|
| [Arcitectus/Sanderling](https://github.com/Arcitectus/Sanderling) | Apache-2.0 | main 分支 2026-02-22 仍有提交；release v2025-10-24 落后于 main | **读取器算法的唯一权威参考**（EveOnline64.cs） |
| [BlindGuyNW/Sanderling](https://github.com/BlindGuyNW/Sanderling) | Apache-2.0（fork） | 领先上游 66 commits（2026-07，与 claude 联合署名开发） | **最佳基线参考**：ASP.NET HTTP 封装（alternate-ui-host）+ CLAUDE.md 实测工程规则 |
| [Viir/bots](https://github.com/Viir/bots) | MIT（注意文件名是 License.txt） | 2026-08-11 活跃 | **effect 模型与 bot 决策参考**（EffectOnWindow.elm、BotFramework.elm 的坐标换算源码） |
| ViV99/Sanderling | — | 纯 Viir 镜像 | 无价值 |
| axinSoochow/SanderlingAxin | — | 纯镜像，无独立提交 | 无价值 |
| d270rg/Sanderling-read + sanderling-js-wrapper | — | 2024 个人项目 | "读取器→HTTP 服务→外部程序"的最小先例 |

注意：**无 NuGet 包、无官方 Python/HTTP 绑定**（issue #92 挂一年无人回）。自行封装是唯一路径，也正因如此 Rust 重写没有生态包袱。

---

## 3. 感知层技术规格（Rust 重写核心）

以下规格从 Sanderling 源码（Program.cs / EveOnline64.cs / ProcessSample.cs）与 Elm 解码器（MemoryReading.elm）三处交叉验证。

### 3.1 进程访问

- 进程名：`exefile`（64 位客户端；任务管理器中确认不带 "(32 bit)" 后缀）。
- 多客户端：按进程名枚举全部实例，每个进程配主窗口句柄/标题/Z-order（`WinApi.ListWindowHandlesInZOrder()`，缺省 zIndex=9999）。多客户端下 **PID 选择错误会静默失败**（2026-03-21 论坛"UI 消失"事故实为用户选错 PID）。
- 打开进程：`OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ)`，同用户普通权限即可，无需管理员。
- 内存区域枚举：遍历所有已提交（committed）内存区域。
- 平台：仅 Windows（上游声明只测过 Windows；wine 有 open issue 不支持）。

### 3.2 UI 树根定位算法

1. 扫描进程已提交内存区域，寻找 CPython 的 `PyTypeObject` 结构；
2. 读取其 `tp_name` 字段，过滤出名为 **`"UIRoot"`** 的类型对象；
3. 枚举该类型的实例地址 → UI 树根；
4. 发现多个候选根时，取**节点数最多**的树输出。

补充：
- 根搜索较慢（首次 ~20s，alternate-ui 活进程模式为每进程后台 Task + 轮询 InProgress 状态）；找到后缓存地址，后续读取直接用。
- 读取深度上限 **99** 层。
- v2025-10-24 release 的性能优化就在这一段：并行 root 搜索、`getPythonTypeNameFromPythonTypeObjectAddress` 优化、降低栈深——Rust 重写时天然吸收（rayon 并行扫区域）。

### 3.3 CPython 对象解码（当前客户端 = Python 2 布局）

客户端 UI 层是 Stackless Python（CarbonUI），UI 节点即活的 Python 对象。读取器需要解码的对象类型：

- 基础类型：`str` / `unicode`（**str/unicode 并存 = Py2 时代布局**）/ `int` / `bool` / `float`
- 游戏类型：`PyColor` / `Bunch` / `Link`
- 容器：dict（逐条 `PyDictEntry` 解析）、children 引用列表

**⚠️ 结构体偏移量不要依赖本文档**（本文档未收录具体偏移）。权威来源：
1. `EveOnline64.cs`（C# 实现里的实际偏移与解码逻辑）；
2. CPython 2.7 源码头文件（Include/object.h、stringobject.h、unicodeobject.h、intobject.h、dictobject.h 等）对照；
3. 漂移调试用 Cheat Engine / Reclass（上游 DIY 指南：https://forum.botlab.org/t/advanced-do-it-yourself-memory-reading-in-eve-online/68 ）。

历史演进注脚：2024-05-26 观察到 dict 键 `_setText` 指向 `Link` 类型对象（2023-01 Photon UI 后新增 `_texturePath/_opacity/_bgColor/isExpanded` 键）——说明 dict 白名单与类型解码随版本有小步演进。

### 3.4 遍历与过滤规则

- **dict 键白名单**（决定输出哪些字段，来自 `DictEntriesOfInterestKeys`）：
  `_top, _left, _width, _height, _displayX, _displayY, _displayWidth, _displayHeight, _name, _text, _setText, children, texturePath, _bgTexturePath, _hint, _display` ＋
  `lastShield, lastArmor, lastStructure`（血条）、`_lastValue`（电容）、`ramp_active`（模块激活）、`_rotation`（模块转速）、`_color`（Overview 图标色）、`_sr, htmlstr`（文本行）、`_texturePath, _opacity, _bgColor, isExpanded`
- `_display` 为 `false` 的节点**直接丢弃**；
- 上限：dict 槽位 10000、字符串截断 4000 字符、children 列表 4000。

### 3.5 输出数据格式

上游原始 JSON 节点（Elm 兼容，地址序列化为字符串）：

```json
{
  "pythonObjectAddress": "1400153429392",
  "pythonObjectTypeName": "ShipUI",
  "dictEntriesOfInterest": { "_displayX": 545, "lastShield": 0.87, "...": "..." },
  "otherDictEntriesKeys": ["_display", "pickState"],
  "children": [ { "...递归同构..." } ]
}
```

值类型序列化：标量直接出；`Bunch` → `{entriesOfInterest:{...}}`；通用引用 → `{address, pythonObjectTypeName}`。

**Rust 决策建议**：内部模型用 `u64` 地址；序列化格式自定义，但**提供 Sanderling JSON 兼容输出**（地址转字符串）——这样同一 sample 的 C# exe 输出可作为 golden test 对照，迁移期交叉验证。（CLI 上游语义：`--pid` / `--source-file` / `--root-address` / `--output-file` / `--warmup-iterations` / `--remove-other-dict-entries`，建议对齐命名。）

### 3.6 性能特征（社区实测口径）

| 操作 | 耗时 |
|---|---|
| 全树读取（单线程朴素） | ~10s |
| 全树读取（上游当前实现） | **~0.3s** |
| 2017 年 Viir 实测典型查询（HP/模块/Overview） | <0.5s |
| **子树读取**（从任意已知节点地址起，`--root-address`） | **~9ms** |
| UI root 搜索（首次） | ~20s（之后缓存） |
| 快照 + 双线程流水线（采集线程 + 消费线程） | 0.25–1s/次 |

含义：1–2Hz 全量轮询 + 事件触发的 9ms 子树精确读取，性能完全够 agent 用。UI 树规模：>1000 节点、数万属性。优化抓手：类型对象缓存（address→tp_name，静态区不重读）、并行区域扫描、子树读取 API 化。

---

## 4. 语义层：从 Python 对象图到游戏语义

上游的语义映射在 `implement/alternate-ui/source/src/EveOnline/ParseUserInterface.elm`（~3700 行），机制 = `pythonObjectTypeName` 字符串匹配 + dict 键读取，输出强类型 `ParsedUserInterface`（**30 个顶层字段**）：

`uiTree, contextMenus, shipUI, targets, infoPanelContainer, overviewWindows, selectedItemWindow, dronesWindow, fittingWindow, probeScannerWindow, directionalScannerWindow, stationWindow, inventoryWindows, chatWindowStacks, agentConversationWindows, marketOrdersWindow, surveyScanWindow, bookmarkLocationWindow, repairShopWindow, characterSheetWindow, fleetWindow, watchListPanel, standaloneBookmarkWindow, moduleButtonTooltip, heatStatusTooltip, neocom, messageBoxes, layerAbovemain, keyActivationWindow, compressionWindow`

典型语义示例：
- **OverviewWindowEntry** = `{ uiNode, cellRightClickEntries, isSelectedCellMainIcon, objectName, type }`；行内容 = 列名→文本字典（`cellsTexts` / `textsLeftToRight`）、距离解析为整数米、`commonIndications`（targeting / targetedByMe / isJammingMe / isWarpDisruptingMe）。
- **ShipUI**：moduleButtons 按 上/中/下 排分组，含 `isActive / isBusy / rampRotationMilli`；电容 `pmarks` → 百分比；护盾/装甲/结构百分比；`indication`（warp/align/orbit）。
- **DisplayRegion 计算**：`_displayX/_displayY/_displayWidth/_displayHeight`，缺失时回退 `_left/_top/_width/_height` → `{x0,y0,x1,y1}`。

**Rust 移植策略（已定）**：以 ParseUserInterface.elm 为规格说明书，按目标任务增量写提取器（如市场改价任务只需：marketOrdersWindow + inventoryWindows + contextMenus 三个）。每个提取器对着离线 sample 开发 + 回归测试。LLM 适合辅助这类"有明确规格 + 有测试数据"的移植。

上游维护者修复 UI 变更的标准流程（照抄）：论坛报告附 sample → 离线复现 → 缺属性则加 `DictEntriesOfInterestKeys` 键 → 解析错则改 ParseUserInterface（Rust 版即提取器）→ 把观察到的确切字符串加为测试用例（带日期注释）。

---

## 5. 离线采样工作流（Rust 工具必须兼容的格式）

**sample zip 格式**（ProcessSample.cs）：
- `Process/Memory/0x<区域基址>`：每个已提交内存区域一个原始字节文件；
- `copy-memory-log`：UTF-8，换行分隔日志（回放端丢弃）；
- `begin-main-window-client-area.bmp` / `end-main-window-client-area.bmp`：采样前后客户区截图（回放端丢弃，但**开发期人眼对照极有用**——"树里这个节点在截图哪里"）。

**录制**：`read-memory-64-bit.exe save-process-sample --pid=<PID> [--delay=<秒>]` → `process-sample-<SHA256前10位>.zip`。要求窗口可见、非最小化。

**回放**：`read-memory-eve-online --source-file=<sample.zip> --output-file=<out.json>`——无需启动游戏，"想读多少遍读多少遍"。

**Rust 版建议**：保持 zip 内部结构兼容 → 可直接复用未来社区 sample，且 C# exe 输出可作为 Rust 实现的 golden test 基准。sample 不进 git（上游惯例，.gitignore 排除），单独建库管理。

---

## 6. 执行层：effect 模型 + 输入注入

### 6.1 effect 模型（MIT，来自 Viir/bots 的 EffectOnWindow.elm）

5 个原语，任何语言可 1:1 复刻：

```
MouseMoveTo(Location2d) | KeyDown(VirtualKeyCode) | KeyUp(VirtualKeyCode)
| ButtonDown(MouseButton) | ButtonUp(MouseButton)      // 鼠标仅左/右键
```

- 点击 = `MouseMoveTo + ButtonDown + ButtonUp`；
- 拖拽 = 起点 → 按下 → **中间航点列表** → 终点 → 抬起（天然支持曲线轨迹）；
- 键码 = Windows 虚拟键码；
- 坐标语义 = **窗口客户区坐标**。

### 6.2 坐标换算（BotFramework.elm 1548-1566 行官方公式）

```
屏幕坐标 = UI树坐标 + ClientToScreen(窗口句柄)
```

三个必处理项：
1. 注入进程**必须 `SetProcessDPIAware()`**，否则 DPI 虚拟化导致错位（最容易踩的坑）；
2. 多显示器：ClientToScreen/SetCursorPos 用虚拟屏幕坐标，自动正确；
3. 历史遗留 1–3px 系统性偏移问题（社区无定论）→ 上线前自测校准；上游点击前还会把可点区域向内收缩安全边距。

### 6.3 输入注入方案（EVE 实证）

| 方案 | EVE 上的结论 |
|---|---|
| **SendInput / pyautogui（VK）** | ✅ 实证可用（多个活跃开源 bot，如 darkmatter2222/EVE-Online-Bot 2026-09 活跃）；EVE 无内核反作弊，不拦截 |
| SendInput + 扫描码（pydirectinput 类） | 保险冗余，EVE 未见必须 |
| PostMessage/SendMessage 后台输入 | ❌ **不可用**（论坛多人复现失败；AHK ControlClick 仅焦点态有效） |
| Interception 驱动 / vJoy | 不必要（且 Interception 会被其他游戏的反作弊指纹，污染同机环境） |
| 多开后台化 | 唯一社区验证方案：**每 bot 一个 RDP 会话**；注册表 `RemoteDesktop_SuppressWhenMinimized=2` 防最小化停摆 |

### 6.4 输入时序（BlindGuyNW 实测规则，直接采纳）

- 光标必须**物理位于客户区内**再点击；
- 鼠标移动后 **≥60ms** 再点；
- 打字用 `WM_CHAR`；
- 拖拽事件间隔 **~150ms**；
- 组合键事件间隔 **20–40ms**。

### 6.5 拟人化（行为侧风险的必要工程）

风险主要在服务端行为检测（Viir 本人口径："客户端检测不到你的进程，真正的问题是你的 bot 行为蠢不蠢"）：
- 动作间**随机延迟区间**（不用固定间隔——BotLab 默认固定 210ms/30ms 反而是反面教材）；
- 鼠标走 **WindMouse 类曲线**（风力+重力模型；学术研究：直线/等间隔/高效率轨迹是人机最显著区分特征）；
- 点击目标像素抖动（"从不点按钮同一像素"）；
- 运行节奏留休息窗口，别 23/7。

---

## 7. 维护性与 Python 3 迁移应对

- **UI 漂移**：CCP 平均数月改一次 UI 结构（2023-02 / 2025-10 / 2026-03 均有记录；渲染层 DX12 等变更**不影响**读取——读的是 Python 对象图而非 GPU 数据）。2026-03-21 "UI 从树中消失"事件最终查实是用户选错 PID，**2026-03 后系统仍可用**。
- **Python 2→3 迁移（最大中期风险）**：CCP 于 2026-08-25 正式启动（240 万行，先 futurize，~2 万处行为差异待审；Simon Willison 报道）。当前读取器按 Py2 对象布局解析（str/unicode 并存）；**客户端切换后对象布局全变（PEP 393 等），读取器需大重写**。应对 = 自建 sample 库 + 提取器回归测试 + 掌握 Reclass 漂移调试流程，做到"上游断更也能自己修"。建议在 Rust 读取器中把 **CPython 布局抽象成 trait/配置**（Py2 现状 + 预留 Py3），迁移落地时只换布局层。
- **兼容性锚点**：release v2025-10-24 ≈ EVE 23.01（2025-10，release 未标注版本号，推断）；main 分支含此后修复（2025-10-29 inventory gauge 适配等）。**Rust 重写前先用上游 C# exe 对当前正式服跑一次全树读取验证兼容性**（P0 第一步）。

---

## 8. Rust 工程建议

### 8.1 crate 选型

| 用途 | 建议 |
|---|---|
| Win32 FFI | `windows`（windows-rs）：OpenProcess / ReadProcessMemory / VirtualQueryEx / EnumWindows / ClientToScreen / SendInput / SetProcessDPIAware / GetWindowTextW |
| 进程枚举 | Toolhelp32Snapshot（windows crate）或 `sysinfo` |
| 并行区域扫描 | `rayon` |
| 序列化 | `serde` + `serde_json`（含 Sanderling JSON 兼容输出） |
| sample zip | `zip` + `image`（BMP 截图） |
| HTTP 服务（可选） | `axum` / `tokio` |
| MCP 接入（可选） | 官方 Rust SDK `rmcp` |
| 随机延迟/轨迹 | `rand`（WindMouse 自实现，~50 行） |

### 8.2 模块划分建议

```
eve-agent/
├── reader/            # 对标 read-memory-64-bit
│   ├── process.rs     # 进程枚举/打开（exefile, VM_READ）
│   ├── memmap.rs      # 区域枚举（VirtualQueryEx）
│   ├── pyobject/      # CPython 布局层（trait 抽象 Py2/Py3）
│   │   ├── mod.rs     #   PyObject header、解码分发
│   │   ├── py2.rs     #   str/unicode/int/bool/float/PyColor/Bunch/Link
│   │   └── scan.rs    #   PyTypeObject 扫描 + tp_name=="UIRoot" 定位
│   ├── tree.rs        # 遍历 + 白名单 + 过滤 + 深度99 + 多根取最大
│   └── sample.rs      # save/load Sanderling 兼容 zip
├── model/             # 节点/DisplayRegion 类型 + serde
├── semantics/         # 按任务增量：overview.rs / market.rs / inventory.rs / menu.rs
├── actor/             # effect 5 原语 + WindMouse + 随机延迟
│   ├── effect.rs
│   ├── coords.rs      # ClientToScreen + DPI + 偏移校准
│   └── input.rs       # SendInput / WM_CHAR
├── service/           # axum HTTP 或 rmcp MCP
└── tests/             # golden: sample.zip → C# exe JSON vs Rust JSON diff
```

### 8.3 里程碑（P0–P4，含 Rust 特有验收）

| 阶段 | 内容 | 验收 |
|---|---|---|
| P0 | 上游 C# exe 对当前客户端跑通全树读取（兼容性锚点）；编译 Rust reader；录制首批 sample | golden test：同一 sample，C# JSON 与 Rust JSON 一致（或差异全部可解释） |
| P1 | 语义提取器（首个任务的 2–3 个窗口）+ sample 回归 | sample 回放 100% 字段正确 |
| P2 | 状态服务（HTTP/MCP）+ 接入 ESI MCP | agent 能查询 UI 快照 |
| P3 | effect 执行器 + 坐标校准 + **人工确认环节** | 单任务闭环跑通 |
| P4 | 拟人化 / 异常恢复（messageBoxes、contextMenus）/ 自动化加深 / 多开 RDP | 无人值守 N 小时 + 人工抽检 |

---

## 9. 关键源码路径速查（上游仓库内）

| 用途 | 路径（Arcitectus/Sanderling） |
|---|---|
| CLI 入口/参数 | `implement/read-memory-64-bit/Program.cs` |
| 内存读取核心（白名单、root 搜索、遍历、解码） | `implement/read-memory-64-bit/EveOnline64.cs` |
| 采样 zip 格式 | `implement/read-memory-64-bit/ProcessSample.cs` |
| WinApi（窗口/Z-order/截图） | `implement/read-memory-64-bit/WinApi.cs` |
| 原始 JSON 解码器（Elm） | `implement/alternate-ui/source/src/EveOnline/MemoryReading.elm` |
| 语义解析规格（~3700 行游戏知识） | `implement/alternate-ui/source/src/EveOnline/ParseUserInterface.elm` |
| 前后端接口契约（Elm） | `implement/alternate-ui/source/src/EveOnline/VolatileProcessInterface.elm` |
| HTTP host 参考（BlindGuyNW fork） | `implement/alternate-ui-host/{Program.cs, VolatileHost.cs, InputViaWindowMessages.cs}` |
| 工程规则/工作流（BlindGuyNW fork） | `CLAUDE.md`、`start-alternate-ui.ps1` |
| effect 模型（Viir/bots） | `implement/applications/eve-online/eve-online-combat-anomaly-bot/Common/EffectOnWindow.elm` |
| 坐标换算公式（Viir/bots） | `.../BotFramework.elm`（1548–1566 行） |

---

## 10. 来源清单

**核心仓库**：[Arcitectus/Sanderling](https://github.com/Arcitectus/Sanderling)（含 [releases](https://github.com/Arcitectus/Sanderling/releases)、[Program.cs](https://github.com/Arcitectus/Sanderling/blob/master/implement/read-memory-64-bit/Program.cs)）· [BlindGuyNW/Sanderling](https://github.com/BlindGuyNW/Sanderling) · [Viir/bots](https://github.com/Viir/bots)（[开发指南](https://github.com/Viir/bots/blob/main/guide/eve-online/developing-for-eve-online.md)、[ParsedUserInterface 文档](https://github.com/Viir/bots/blob/main/guide/eve-online/parsed-user-interface-of-the-eve-online-game-client.md)）

**论坛/指南**：[DIY 内存读取指南](https://forum.botlab.org/t/advanced-do-it-yourself-memory-reading-in-eve-online/68) · [UI changes 3-21-26](https://forum.botlab.org/t/ui-changes-3-21-26) · [how-safe-is-it](https://forum.botlab.org/t/how-safe-is-it-to-use-on-eve-online/2415) · [mouse-clicks（PostMessage 失败）](https://forum.botlab.org/t/mouse-clicks/62) · [multi-instances（RDP）](https://forum.botlab.org/t/multi-instances-support/326) · [window-coordinates-discrepancy](https://forum.botlab.org/t/window-coordinates-discrepancy/431) · [how-to-run-without-botengine](https://forum.botlab.org/t/how-to-run-a-bot-without-botengine-exe/3738)

**输入/拟人化**：[WindMouse](https://ben.land/post/2021/04/25/windmouse-human-mouse-movement/) · [darkmatter2222/EVE-Online-Bot](https://github.com/darkmatter2222/EVE-Online-Bot)（pyautogui 实证） · [pydirectinput](https://github.com/learncodebygaming/pydirectinput)

**宏观背景**：[Python 3 迁移报道（2026-08-25）](https://simonwillison.net/2026/Aug/25/eve-online-move-to-python-3/) · [ESI 现状](https://www.eveonline.com/news/view/esi-delivered-the-next-chapter) · [Third Party Policies](https://support.eveonline.com/hc/en-us/articles/8564030965660-Third-Party-Policies) · [EULA](https://support.eveonline.com/hc/en-us/articles/8413329735580-EVE-Online-End-User-License-Agreement) · [客户端修改政策](https://www.eveonline.com/news/view/client-modification-the-eula-and-you) · [Team Security 月度封禁](https://www.eveonline.com/news/view/monthly-ban-report-june-2026) · [EVE MCP server](https://forums.eveonline.com/t/eve-mcp-server-quering-esi-from-claude-copilot-and-other-ai-assistants/516433)

**明确未找到/待自测清单**：① 公开可下载的真实内存读取 JSON 样例（自行录制）；② v2025-10-24 对应的确切 EVE 客户端版本号（P0 自验）；③ CPython 结构体偏移的权威数字（以 EveOnline64.cs + CPython 2.7 源码为准）；④ 系统性读取频率基准（只有分散实测：全树 ~0.3s / 子树 ~9ms / root 搜索 ~20s）。
