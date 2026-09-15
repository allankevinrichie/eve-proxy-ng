# 感知与交互

面向 agent / 自动化场景的感知层使用指南：怎么稳定地读、读什么、
以及决定"能不能点、点了会中谁"。

## 发现客户端与多客户端

```python
import eve_proxy_ng

clients = eve_proxy_ng.discover_clients()
for c in clients:
    print(c.pid, c.flavor, c.character_name, c.exe_path)
```

- 一台机器多开（不同服务器、同服多号）全部可见，`pid` 是各客户端的稳定标识
- `flavor`：`infinity`（曙光）/`serenity`（经典）/`tranquility`（国际）
- `character_name` 在角色选择界面之前为 `None`
- `window_handle` 是 HWND 整数，可用于你自己的 Win32 互操作

## 建立读取器：活体 vs 样本

```python
reader = eve_proxy_ng.UiReader(pid=31336)          # 活体进程
reader = eve_proxy_ng.UiReader(sample="dump.zip")  # 离线样本回放（开发/测试）
```

两种模式同一套 API。**第一次调用 `find_ui_root()` 是全内存冷扫描
（~20 秒）**，之后内部缓存热读（in_space ~2600 节点约 30ms/帧）。
场景切换时 UI 根会搬家——读取失败会自动失效缓存，下一次调用重新
搜索，不需要你处理。

## 读懂一帧：语义快照导览

```python
snap = reader.read_snapshot()
```

| 区块 | 内容 | 典型用途 |
|---|---|---|
| `game_state` | `screen`: in_space / docked / character_select；`blocked_by_modal` | 行为循环的第一分支 |
| `ship_ui` | 模块按钮（`type_id`/`module_name` 精确中文名/`icon_name` 家族）、电容、电量、速度 | 舰船状态与武器管理 |
| `overview_windows[]` | 总览条目：名称/距离/`icon_name`（bracket 类别）/交互性/遮挡 | 目标选择 |
| `context_menus` / `util_menus` | 右键菜单与工具菜单（含菜单项图标） | 菜单操作 |
| `inventory_windows` | 货舱/机库物品（数量/分组） | 物流 |
| `neocom` | Neocom 按钮组 | 导航入口 |
| `interaction_elements[]` | **全部可操作元素**：`label`（人可读标识，text>role>hint>name 同源优先级）/role/text/hint/`icon`/`icon_name`/交互性 | 通用操作层 |
| `scrollable_views[]` | 滚动视口三元组 + `scroll_to_reveal_px` | 列表定位 |

!!! note "帧语义"
    每次 `read_snapshot()` 都是一次完整点时快照——动态值从不跨帧
    缓存，连续调用就是连续帧，不会漏 UI 更新。不需要（也没有）
    "刷新"或"失效"调用。

### 模块按钮的识别优先级

```python
for m in snap["ship_ui"]["module_buttons_high"]:
    print(m["type_id"], m["module_name"], m["icon_name"])
    # 3638 民用加特林磁轨炮 民用加特林磁轨炮
```

`module_name` 优先取 **typeID 精确中文名**（本地客户端表，含国服
独有变体），无 typeID 时回退 `icon_name` 图标家族名。悬停时可再经
`module_button_tooltip` 拿到模块名+快捷键做二次确认。

### 总览条目的类别通道

条目的 `icon_name` 来自 bracket 图标（`stargate`、`citadelLarge`、
`skyhook_bracket`…）。名字相近或本地化混排时，用它判类别比文本可靠：

```python
gates = [e for e in snap["overview_windows"][0]["entries"]
         if e.get("icon_name") == "stargate"]
```

## 决定"能不能点"：遮挡与命中测试

每个可交互元素自带判定字段，**以客户端自身的命中路由为准**，不是
几何猜测：

- `is_interactable` / `occluded_percent`（3×3 采样遮挡比）/
  `occluded_by`（覆盖者列表）——总览行、按钮、菜单项、面板全都有
- 透明隐藏（opacity < 0.5）与 `_pickState=0`（点击穿透）都计入不可交互
- 滚动容器外的列表条目会被视口裁剪正确判为不可命中

对坐标做判定用命中测试：

```python
hit = reader.hit_test(1351, 550)
if hit and not hit["blocked"]:
    print("点击将命中", hit["type_name"], hit.get("name"), hit["region"])
```

典型决策流：**元素定位 → `is_interactable`? → 需要时 `hit_test` 复核
落点 → 执行注入输入 → `injected` 标志可在录制中回溯对照**。

## 窗口管理

```python
client = clients[0]
client.activate_window()    # 置前并聚焦（注入键盘前通常需要）
client.minimize_window()
client.restore_window()
```

多客户端轮询时注意：键盘输入归属前台窗口，激活目标客户端再注入；
鼠标按视觉位置归属，无需激活。窗口客户区截图与 UI 树坐标同源
（见 [会话录制与分析](recording.md) 的坐标换算）。

## 连续轮询与性能预期

- 轮询就是循环调用 `read_snapshot()`，节奏由你控制；返回里没有
  内置节流（录制器的 `--interval-ms` 就是这么实现的）
- 预期（release 构建、in_space ~2600 节点）：单帧读取 ~30ms
  （~30fps 上限）；**debug 构建慢 3-5 倍，一切测速以 release 为准**
- 场景越复杂节点越多（会战预期 8k-15k），读耗时线性增长——按
  `read_ms` 遥测自适应轮询间隔是好习惯（录制器每帧都记这个数）

## 下一步

- [会话录制与分析](recording.md)——采集人类操作数据集
- [资源与图标识别](resources.md)——名字与图标的解析策略
- [Python API](../api.md)——全部接口细节

### 窗口容器与元素归属

- `other_windows[]`：全部顶层窗口容器（特化窗口之外的安全网 + 弹窗类：每日登录/抽卡/活动等 `Wnd/Window` 命名容器），带 `address`/`caption`/`element_addresses`
- 每个 `interaction_element` 带 `window_address`（所属窗口地址，HUD/游离元素为 None）——双向 join，一眼知道这个按钮属于哪个窗口
- 特化窗口（总览/库存/装配/站内/聊天）的子项直接是其结构化字段

### 元素的"如实"几何字段

- `client_size`（快照顶层）：游戏客户区尺寸，所有 region 的坐标系
- `region`：布局矩形（含虚拟化列表的屏外格）
- `visible_region`：被滚动视口/窗口框裁剪后的**实际可见矩形**（仅当与 region 不同时出现）
- `is_on_screen`：`false` = 完全在窗口框/客户区外（游戏不渲染，如虚拟列表的屏外格）

操作前判定链：`is_on_screen` → `visible_region`（点要落在可见部分）→ `is_interactable`/`occluded_by`。
