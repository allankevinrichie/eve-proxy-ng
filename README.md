# eve-proxy-ng

[![CI](https://github.com/allankevinrichie/eve-proxy-ng/actions/workflows/ci.yml/badge.svg)](https://github.com/allankevinrichie/eve-proxy-ng/actions/workflows/ci.yml)
[![Release](https://github.com/allankevinrichie/eve-proxy-ng/actions/workflows/release.yml/badge.svg)](https://github.com/allankevinrichie/eve-proxy-ng/actions/workflows/release.yml)
[![Docs](https://img.shields.io/badge/docs-GitHub%20Pages-2bd9ff)](https://allankevinrichie.github.io/eve-proxy-ng/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue)](LICENSE)
[![Python 3.12+](https://img.shields.io/badge/python-3.12%2B-blue)](https://www.python.org/)

**EVE Online 感知与语义代理** —— 把游戏状态变成 AI agent 可直接消费的结构化数据。

Rust 实现 + Python 绑定（`pip install eve-proxy-ng`，内置 `eve-cli` 与全部依赖）+ 三路同步会话录制。支持网易国服（曙光服 / 经典服）与国际服的多客户端内存读取：游戏 UI → 语义快照（总览行、模块按钮（精确 typeID + 中文名）、菜单、货舱、遮挡与命中测试），全部资源（类型表/本地化）只从本地客户端提取。

[快速上手](#快速开始) · [在线文档](https://allankevinrichie.github.io/eve-proxy-ng/) · [Python API](https://allankevinrichie.github.io/eve-proxy-ng/api/) · [CLI 参考](https://allankevinrichie.github.io/eve-proxy-ng/cli/)

> ⚠️ 内存读取属 EULA 违规行为，本项目仅用于学习研究，风险自担。

## 当前状态（2026-09-12）

已在**网易曙光服（Infinity）**真实客户端上全链路验证：

| 能力 | 状态 |
|---|---|
| 客户端发现（PID/窗口/服务器识别/角色名） | ✅ 已验证 |
| 窗口操作（激活/最小化/恢复/客户区截图） | ✅ 已实现 |
| 内存 dump（Sanderling 兼容 zip）+ 离线回放 | ✅ 8.1GB 样本已录制 |
| UIRoot 发现 + UI 树读取（Py2 布局） | ✅ 2667 节点 / 热读 0.33s |
| 语义提取（ship/overview/menu/inventory/neocom/chat/panels/generic） | ✅ 中文 UI 全部正确 |
| **遮挡与命中测试**（`_pickState` 路由 + 采样遮挡 + `hit_test`） | ✅ 与真实画面互相印证 |
| **会话录制**（观测增量 + 人类键鼠 + H.264 硬编视频三路同步） | ✅ 曙光服实机验证（NVENC 首次启用） |
| Python 3 binding（`eve_proxy_ng` 模块） | ✅ abi3 wheel，双语 docstring + mkdocs 离线文档 + eve-cli 一体打包 |
| 样本回归测试 | ✅ 19 项断言全过 |

性能：冷启动（全内存扫描定位 UIRoot）~20s；观测管线**双层流水线**——帧间两级（读线程 paced 全树重读 + 解析线程消费快照，有界通道背压，帧间隔=max(read,parse)）+ 阶段内并行（read 内：预取线程与 walk **竞速填充**重叠——FrameReader 页表 Mutex+Arc 并发化，miss 自读、同帧双读无害；parse 内：interaction 遮挡探测 rayon 并行，indexed collect 保 z 序）。实测（release，曙光服 in_space ~2570 节点）：`--interval-ms 20` 下 **30.9fps**（帧间隔 p50 33ms=read 主导，parse p50 3ms 全隐入；interaction 699µs；FrameReader 页表 16 分片锁降低预取与 walk 的争用）。walk 子树级并行已实现为**默认关闭的开关**（`UiReader::with_parallel_walk`）：当前树规模实测负收益（节点工作量 ~10µs 太细，且并行模式失去 last-page 快路径）——留作会战大树（每子树工作量×5 改变平衡点）时启用。每帧仍是完整点时快照——不漏 UI 更新。读层：白名单前置、type-object 缓存、`FrameReader` 4KB 页缓存 + last-page 寄存器、布局复合原语（`object_words`/`list_words`，trait 带默认实现 Py3 免迁移）、known_pages 合并段预取（gap≤4KB、段≤1MB）。语义层：`pick_state`/`_opacity` 预计算进 `RegionedNode`、hit_test 零分配 layers、直接子索引列表（`parse_ui_tree_timed` 提供分段计时，observe 记 `parse_detail`）。`eve-cli bench` 可复测（**务必 release 构建**，debug 慢 3-5 倍会严重失真）。

## 快速开始

```bash
# Rust 工具链 + uv（Python 侧）
cargo build --release
uv venv --python 3.12

# 视频录制构建前置（ffmpeg 进程内编码，见下"构建前置"节）
#   third_party/ffmpeg-7.1-win64-gpl-shared + FFMPEG_DIR + LIBCLANG_PATH

# 发现运行中的客户端（任意服务器）
./target/release/eve-cli.exe clients
#   pid 50632  曙光服 (Infinity)  char aiyoggle  ...

# 录制内存 dump（开发/回归用，游戏窗口需可见）
./target/release/eve-cli.exe dump --pid 50632

# 从 dump 离线读 UI 树（Sanderling 兼容 JSON）／语义快照
./target/release/eve-cli.exe read --sample samples/process-sample-XXXX.zip --output-file tree.json
./target/release/eve-cli.exe snapshot --sample samples/process-sample-XXXX.zip

# 类型普查（漂移调试：客户端更新后看哪些类型改名/新增）
./target/release/eve-cli.exe types --pid 50632 --limit 50

# 会话录制（人操作 + 语义观测 + 窗口视频，三路时间对齐；Ctrl-C 优雅停止）
./target/release/eve-cli.exe record                       # 所有客户端
./target/release/eve-cli.exe record --pid 50632 --duration-sec 120
./target/release/eve-cli.exe record --video-size 1080p --encoder libx264
./target/release/eve-cli.exe record --no-video            # 只录观测+输入两路

# Python binding（开发模式：从源码构建 wheel）
maturin build --release -m crates/eve-bindings/Cargo.toml -i .venv/Scripts/python.exe
uv pip install --python .venv/Scripts/python.exe target/wheels/eve_proxy_ng-*.whl
.venv/Scripts/python.exe tests/regression.py

# 一体化发行 wheel（eve-cli.exe + ffmpeg DLL 闭包 + 离线文档站点 → 单文件 pip 包）
#   产物 ~51MB：pip install 后 Python 模块 / eve-cli 命令 / 离线文档 三者齐备
.venv/Scripts/python.exe scripts/package.py
.venv/Scripts/python.exe tests/check_stubs.py   # Rust docstring ↔ .pyi stub 同步守护

# 文档站点（MkDocs Material + mkdocstrings，EVE 官网深空风格；构建进 wheel）
.venv/Scripts/mkdocs.exe serve
```

### 构建前置：视频编码（ffmpeg 进程内库）

`eve-recorder` 通过 **ffmpeg-next 9.0.0 直接调 ffmpeg 库 API**（avcodec send/receive + avformat mux + swscale），不再有子进程/管道/PATH 依赖。库本体不入 git，需一次性准备：

```bash
# 1) 下载 BtbN GPL shared 构建并解压（ffmpeg-next 链接其 import lib）
#    https://github.com/BtbN/FFmpeg-Builds/releases  →  autobuild 最新含 7.1 的版本
#    文件名形如 ffmpeg-n7.1.x-…-win64-gpl-shared-7.1.zip
mkdir third_party && unzip <zip> -d third_party/   # → third_party/ffmpeg-7.1-win64-gpl-shared/
#    （本机用的是 autobuild-2026-07-31-14-10 的 ffmpeg-n7.1.5-12-g1fdbca85aa-win64-gpl-shared-7.1）

# 2) 构建环境（bindgen 还需要 LLVM：winget install LLVM.LLVM）
export FFMPEG_DIR="<repo>/third_party/ffmpeg-7.1-win64-gpl-shared"
export LIBCLANG_PATH="C:/Program Files/LLVM/bin"
cargo build --release

# 3) 运行时把 DLL 带上（或复制 target/release/）
export PATH="C:\\Users\\allan\\projects\\eve-proxy-ng\\third_party\\ffmpeg-7.1-win64-gpl-shared\\bin;$PATH"
```

**为什么选 7.1 而不是更新的 8/9**：nvenc SDK 11.1 头（2021 年）只要求 NVIDIA 驱动 ≥471.41，覆盖面最大的"照顾主流硬件和驱动"；更新的构建绑定 SDK 12/13 头要求 ≥522/≥565 驱动（本机 CLI ffmpeg 9 就因此拒绝 560 系驱动上的 RTX 5070 NVENC，而 7.1 库内 API 实测 22x 实时直接可用）。GPL 构建含 libx264 兜底；动态链接个人使用无碍，若日后闭源分发需换 lgpl 构建弃 x264。多客户端同录注意 GeForce 消费级卡 NVENC 并发路数上限（新驱动 8 路），超限会回落软编。

```python
import eve_proxy_ng

for c in eve_proxy_ng.discover_clients():
    print(c.pid, c.flavor, c.character_name)

reader = eve_proxy_ng.UiReader(pid=50632)          # 或 sample="xxx.zip"
reader.find_ui_root()                         # 首次 ~25s，内部缓存
snapshot = reader.read_snapshot()             # Python dict
for entry in snapshot["overview_windows"][0]["entries"]:
    print(entry["object_name"], entry["distance_meters"],
          entry["is_interactable"], entry["occluded_percent"])

reader.hit_test(1351, 550)                    # 点击会落到哪个节点
# {'type_name': 'Sprite', 'name': 'activitypic', 'pick_state': 1, ...}
```

### 遮挡与命中测试（agent 操作前的判定）

窗口相互重叠时，agent 需要知道目标元素是否被覆盖、能不能点。判定**以客户端自身的命中路由为准**，而非猜测几何：

- **`_pickState`**（客户端自己的命中测试标志，2026-07 加入白名单）：`0` = 该子树不响应输入（点击**穿透**到下层）、`1` = 接收输入、`2` = 传递给子元素。
- **层级顺序 = 根层树序**（实测验证：`l_hint`(tooltip 最上) → `l_menu` → `l_modal` → … → `l_main` → `l_viewstate` 最底，由客户端预排序；可用 `FlavorProfile.layer_order_topmost_first` 覆盖）。
- **滚动视口裁剪**：滚动容器（`Scroll`/`ScrollContainer`/`BasicDynamicScroll`/`__clipper`）的后代只有落在视口内的部分可命中——超出视口的列表条目会被正确判为不可交互。
- **`hit_test(x, y)`**：从最顶层按 `_pickState` 路由下降，返回点击实际落点（类型/名称/区域/pickState）。
- **每个可交互元素**（总览行、模块按钮、菜单项、消息框按钮、Neocom、面板、窗口…）自带 `is_interactable` / `occluded_percent`（3×3 采样）/ `occluded_by`（覆盖者列表，含类型/名称/区域/pickState）/ `pick_state` / `opacity`。
- 实测案例：dump 时刻活动宣传图（`activitypic`）+ 抽卡窗口盖住总览大部分行——快照判为不可交互，与真实画面一致。

注意：容器内部顺序是约定而非保证（参考项目为此硬编码过"已知遮挡类型"）；`occluded_by` 总是报告赢家节点，误判可检查；透明隐藏（`_opacity < 0.5`）也计入不可交互。

### agent 友好层（验证会话中沉淀）

| 快照字段 | 用途 |
|---|---|
| `game_state` | `{screen: in_space/docked/character_select, blocked_by_modal}` —— 一眼知道在哪个界面、有没有模态挡住（判据优先级 ShipUI > LobbyWnd > l_charsel，防登录残留层误判） |
| `interaction_elements[].role` | 语义角色（`hud.open_cargo`、`station.undock`、`targeting.auto_lock_back`、`weapons.stop_focus_fire`…），来自 `roles.rs` 实测映射表；未知类型回退原始类型名 |
| `interaction_elements[].text/hint` | 标签文本（含子节点回退 + markup 剥离） |
| `ship_ui.module_buttons[]` | **`type_id`（精确变体身份，来自节点名 `ModuleButton_<typeID>`）** + `icon`（家族指纹）+ `module_name`（优先级：typeID 精确查本地客户端表（6.2 万类型中文名，含国服独有）→ 图标家族名 `icon_name`（含实测覆盖））。命名后缀 ID 语义因类型而异：`ModuleButton_`=typeID、`__inflightbracket_`=itemID（太空实例）|
| `overview_windows[].entries[].icon_name` | **条目 bracket 图标语义名**（`stargate`/`citadelLarge`/`skyhook_bracket`…）——名字歧义时的物体类别通道；`interaction_elements[]` 同样带 `icon`/`icon_name`（HUD/Neocom/菜单按钮的图标信息） |
| `module_button_tooltip` | 悬停模块按钮时出现：模块名 + 快捷键（CTRL-F3 风格解析）——识别未收录图标的通用通道 |

**资源表（本地客户端提取）**：

```bash
eve-cli icons update                  # 默认 --source client：只从本地客户端提取
eve-cli icons update --source sde     # 备选：fuzzwork TQ SDE 下载
eve-cli icons update --write-baseline # 维护者：写回仓库基线（data/types.<flavor>.json[.gz]）
eve-cli icons show res:/ui/texture/icons/13_64_5.png --verbose
```

**全部资源只从本地客户端提取**（2026-09-14 起）：客户端自带每个 FSD 表的官方解码器（`bin64\<name>Loader.pyd`，Python 2 扩展模块），`icons update` 借它们读取 `res:/staticdata/*.fsdbinary`（经 `resfileindex.txt` 内容寻址）+ `res:/localizationfsd/localization_fsd_zh.pickle`（33.5 万条中文 message），派生 **62,467 个类型的中文名 + 图标**（含国服独有类型，比 TQ SDE 多约 1 万条）——零网络、零格式逆向（解码器随客户端版本更新永不过时）。py2.7 运行时随 Python 包分发（`eve_proxy_ng/tools/py27`，PSF 许可），pip 安装后离线可用；仓库场景首次运行自动引导（MSI 管理性解包，一次性）。**py3 迁移边界**：py2 绑定仅限运行时+payload（已 2/3 兼容）+客户端 pyd 三样，客户端 python 版本自动探测（bin64\pythonXY.dll），未来 py3 客户端只需为引导补 py3 运行时，数据链路不变。查询走三层级联：手工覆盖（代码）→ 用户 overlay（`%LOCALAPPDATA%\eve_proxy_ng\resources`，update 的写入点）→ 嵌入基线（`data/types.<flavor>.json.gz`，include_bytes! 进二进制）；旧的 icons_data.rs/types_data.rs 生成源码已删除。维护者流程另有 `scripts/derive_client_resources.py`（py3 编排器，同机制）。

**图标图片通道**（多模态 agent 补充信息）：图标→含义是**家族级**关系（同图标对应民用/T1/T2 变体，客户端运行时再叠加品质角标），静态表给家族名，精确变体靠悬停 tooltip 或 `eve_proxy_ng.type_name(typeID)`（模块按钮节点名自带 typeID）；图标 PNG 本身可离线读取——游戏资源以内容寻址存于 `SharedCache\ResFiles\`（`resfileindex.txt` 索引），`eve-cli icons export` 批量导出或 `eve_proxy_ng.get_resource_image("res:/…")` 单取 PNG bytes，供视觉模型识别未收录图标。

### 会话录制（`record`：模仿学习 / 执行层数据集）

录制"人玩 EVE"的完整会话，产出**时间对齐的三路数据**（新 crate `eve-recorder`）：

```
record-<时间戳>/
├── manifest.json          # 会话元数据：epoch 起点、参数、每客户端统计、
│                          #   视频锚点(anchor_t_ms)/编码器/缩放(scale/native 尺寸)/
│                          #   客户区偏移(已换算到缩放后空间)
├── input.jsonl            # 人类键鼠事件（全局 LL 钩子）
└── clients/
    ├── <pid>.jsonl        # 语义观测流（增量 NDJSON）
    └── <pid>.mp4          # 窗口视频（CFR H.264，默认 720p 缩放）
```

- **三路帧率均可调**：视频 `--video-fps`（默认 15）、观测 `--interval-ms`（默认 500 = 2fps，即轮询间隔下限；实际帧率 = 1/max(interval, 单帧读取耗时)）、输入轨迹 `--move-every-ms`（默认 16 ≈ 60fps，0 = 不节流全保真；离散事件按键/点击/滚轮不受此参数影响，始终全保真）。
- **预热 barrier 同步起跑**：三路各自预热（UIRoot 冷扫描 ~20-50s、编码器探测、WGC/ffmpeg 初始化、钩子安装）全部完成后才创建会话时钟并统一放行——不存在某路的头 50 秒埋在另一路的初始化里。
- **帧内时间戳水印**：每帧视频右下角烧入 `T+SS.S`（采集时刻，像素级、不依赖容器），编码堆积/丢帧也不会破坏对齐。
- **积压报警**：视频编码跟不上时丢帧保时间轴并 WARN（manifest 记 `video_dropped_frames`）；观测读取慢于目标间隔 WARN（`observe_slow_reads`）；输入通道溢出计数——绝不静默积压。
- **输入流**：`WH_KEYBOARD_LL`/`WH_MOUSE_LL` 事件驱动捕获；归属在**事件时刻**于钩子回调内判定——按键按前台窗口（EVE 无全局快捷键，按键语义上只属于前台客户端）、鼠标按 `WindowFromPoint` 命中窗口的 PID，按住期间延续按下时的归属（拖拽出窗不断流）；注入输入（SendInput 类，`LLMHF_INJECTED`）**如实记录不丢弃**，每条事件带 `injected: true/false` 字段区分人与执行层注入（录制器因此兼作执行层的调试对照工具）。每事件带 `t_ms / pid / 屏幕坐标 / 客户区坐标（事件时刻换算）/ mods / injected / vk_name`。
- **观测流**：每客户端一线程轮询 `UiSnapshot`（默认 500ms，`--interval-ms`），按顶层区块做帧间 diff，只写变化区块；`interaction_elements`（最大区块）按元素 `address` 做 keyed diff（added/updated/removed/order）。首帧 `snapshot_full`，后续 `snapshot_delta`（静止帧也是空 delta，帧号连续可诊断）。
- **视频流（进程内编码，无子进程）**：Windows Graphics Capture 按 HWND 捕获（被遮挡仍正确）→ swscale 一步 BGRA→NV12 缩放 → NV12 帧内时间戳水印 → **ffmpeg-next（ffmpeg 7.1 库内 API）** 编码 H.264 写 mp4。编码后端自动探测 + **吞吐闸门**（`h264_nvenc → h264_amf → h264_qsv → libx264`，硬件后端需在输出尺寸下实测 ≥1.5x 实时才选中；本机实测：RTX 5070 NVENC 22x 自动选中，Intel iGPU HEVC 0.76x 这类"探测可用但稳态丢帧"的后端被正确拦下）；可 `--encoder` 指定后端。视频帧 n 的时刻 = `anchor_t_ms + n/fps`。
- **分辨率档位**：`--video-size 720p|1080p|native`（默认 **720p**）。按**像素总数 ≤ 预算**（720p=921,600、1080p=2,073,600）等比缩放不裁剪不变形，宽高取偶（4:2:0）；native 仅偶数化不缩放。**坐标换算**：`视频像素 = (客户区坐标 + native 偏移) × video_scale`，manifest 记录 `video_native_width/height`、`video_scale`（精确浮点）与 `video_client_offset`（已换算到缩放空间）——三档实测同一窗口偏移在各自缩放空间完全一致。
- **时间对齐**：三路统一用会话相对毫秒 `t_ms`（manifest 记 epoch 起点）；输入事件的客户区坐标 + manifest 的 `video_client_offset` = 视频像素坐标；点击可用 `hit_test` 反查当帧语义元素。
- **优雅停止**：Ctrl-C（或 `--duration-sec`）→ 拆钩子 → 观测/捕获线程各自 flush → ffmpeg 排空编码器写完 mp4 moov → manifest 落盘。
- **体积参考**：H.264 目标码率 6Mbps（vbr）；观测流空闲 0 字节/帧、变化帧几 KB。

回放提示：分析时按 `snapshot_full` 建状态、逐帧 fold `snapshot_delta`（`cleared` 区段置 null、keyed 区段按 address 应用 added/updated/removed + order 重排）。
| `scrollable_views[]` | 滚动三元组（视口/内容高/手柄）+ 每条 `is_visible` 与 `scroll_to_reveal_px`（实测数学：内容位移 156px vs 手柄推算 153px） |
| `fitting_window` | 槽位（编号/区域/已装模块名/空槽）+ 属性（电容/DPS/EHP/锁定/导航） |
| `station_window` | 9 服务按钮（稳定 ID：lpstore/industry/market…）+ 离站/控制权/标签页 |

## 架构

```
┌─ eve-cli ── clients / dump / read / snapshot / types / record ────┐
│                                                                  │
│  eve-memory（感知层）              eve-semantics（语义层）        │
│  ┌──────────────────────┐        ┌─────────────────────────┐    │
│  │ discovery  window    │        │ RegionedTree            │    │
│  │   │                  │  UiNode│  · DisplayRegion 累计    │    │
│  │ source ──▶ pyobject ─│───────▶│  · 兄弟遮挡计算          │    │
│  │  ▲ MemorySource      │        │  · 子树索引             │    │
│  │  ├ LiveProcess (RPM) │        │ extractors              │    │
│  │  └ DumpSource (zip)  │        │  ship/overview/menu/...  │    │
│  │ uitree (白名单+树)    │        │ FlavorProfile           │    │
│  │ dump (save/replay)   │        │  (曙光基线，按服分发)     │    │
│  └──────────┬───────────┘        └────────────┬────────────┘    │
│             │ UiSnapshot                        │ UiSnapshot      │
│  ┌──────────▼───────────────────────────────────▼────────────┐  │
│  │ eve-recorder（会话录制）                                    │  │
│  │  input: LL 钩子+60fps 轨迹聚合 │ observe: 轮询+区块增量 diff │  │
│  │  capture: WGC+swscale 缩放+水印 │ video_encoder: H.264 硬编    │  │
│  │  session: 时钟/编排/Ctrl-C     │ （nvenc/amf/qsv/x264+闸门）  │  │
│  └────────────────────────────────────────────────────────────┘  │
│  eve-bindings：eve_proxy_ng Python 模块（PyO3, abi3-py312）            │
└──────────────────────────────────────────────────────────────────┘
```

### 三个变化轴，三个抽象

| 变化轴 | 抽象 | 现状 |
|---|---|---|
| 活进程 vs 离线回放 | `MemorySource` trait | `LiveProcess` / `DumpSource`，读取逻辑零改动复用于离线测试 |
| CPython 2 vs 3（客户端迁移） | `PythonLayout` trait | `Py2Layout`（全部现网客户端）；迁移后新增 `Py3Layout` 即可，遍历器/语义层不动 |
| 服务器版本（曙光/经典/国际） | `FlavorProfile` 表 | 曙光（Infinity）为基线；经典/国际回退基线，差异实测后在各自 profile 覆盖（如表头译名、机动关键词） |

### 服务器识别

| Flavor | 运营 | 常用名 | 内部标识（窗口标题/安装路径） |
|---|---|---|---|
| `Infinity` | 网易 | 曙光服（**基线**） | `infinity` |
| `Serenity` | 网易 | 经典服（晨曦） | `serenity` |
| `Tranquility` | CCP | 国际服 | `tranquility` |

## 离线样本工作流

1. `eve-cli dump --pid <pid>` 录制（zip 内部结构与 Sanderling `ProcessSample` 兼容：`Process/Memory/0x<base>` + `copy-memory-log` + 前后客户区 BMP 截图）；
2. 一切开发与测试都在 dump 回放上做（`--sample`），无需开游戏、完全可复现；
3. `tests/regression.py` 固化关键字段断言；客户端更新后重录样本、更新期望值即完成迁移验证；
4. 样本不入 git（`.gitignore` 排除 `samples/`）。

## 关键实现知识（踩坑记录）

- **宽整数**：客户端大量 int 对象高 32 位是垃圾，真实值在低 32 位 —— 这正是参考格式 `int_low32` 字段存在的原因；`eve-semantics::region::read_i64` 统一处理。
- **距离列物理位置**：总览默认布局中距离列在最左（不是名字列），列语义必须按表头文本（本地化）匹配，走 `FlavorProfile`。
- **树序即 z 序**：根层子节点由客户端预排序为最上→最下（`l_hint` 最顶、`l_viewstate` 最底）；`_pickState=0` 是"穿透型死区"（子树禁用输入，点击落到下层），不是挡墙。
- **`_display == false` 剪枝整棵子树**；无 `_display*` 的子树对语义层不可见（通知条目走 `_left/_top` 静态回退）。
- **children 链**：`children` 项 → 实例 dict(0x10) → `_childrenObjects` → （一次 `PyChildrenList` 间接）→ list。
- **RegionedTree 子节点**：先序布局中子节点与子树交错，`children_of` 必须按深度过滤（不能切片）——曾因切片导致孙节点混入，表现为面板/窗口列表出现大量控件噪声。
- **国服特有 UI**：`InfoPanelESS` / `InfoPanelAdaptTask` / `InfoPanelBattlepass` / `GachaWindow` / `activitypic` 活动宣传图等通过 generic 兜底与命中测试天然可见。

## 为 agent 层预留的接口

`eve_proxy_ng.UiReader`（live/sample 双模式 + root 缓存）、`read_tree_json()`（Sanderling 兼容原始树）、`read_snapshot()`（强类型语义）、`GameClient` 的窗口操作（`activate_window/minimize_window/restore_window`）。执行层（输入注入）不在本仓库范围内。

## 参考与致谢

算法与游戏结构知识来自 [Sanderling](https://github.com/Arcitectus/Sanderling)（Apache-2.0，BlindGuyNW fork 2026-07 版本为直接参照）—— 本项目仅参照其思路与语义知识，全部代码为原创 Rust 实现。调研背景见 [history.md](history.md)。

## 许可证与第三方组件

- **本项目代码**：[MIT](LICENSE)（Copyright (c) 2026 Ziheng Zhang）
- **ffmpeg GPL shared 构建**（BtbN 7.1，含 libx264）：随发行 wheel 分发其 DLL。GPL 动态链接对个人使用无碍；**若日后闭源分发，需换 lgpl 构建并弃 x264**
- **CPython 2.7.18**（python.org MSI，PSF 许可）：随 wheel 分发，用于驱动客户端自带的 FSD 解码器（`eve_proxy_ng/tools/py27/`，附其 LICENSE）
- **游戏数据**：类型表/本地化/图标全部派生自本地客户端自带数据，随客户端许可使用；**录制数据集可能含账号信息，勿公开分发**
