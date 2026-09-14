# 快速上手

## 安装

```bash
pip install eve_proxy_ng-0.1.0-cp312-abi3-win_amd64.whl
```

一次安装同时提供：

- Python 模块 `eve_proxy_ng`（含类型标注 `.pyi` stub）
- `eve-cli` 命令（随包分发 `eve-cli.exe` 与全部 ffmpeg DLL 依赖）
- 离线文档（`eve_proxy_ng.docs_dir()` 指向的 HTML 站点）

要求：Windows、Python ≥ 3.12、x64。无需 Rust 工具链。

!!! tip "从源码构建"
    开发者从仓库构建见项目 README（`maturin build` + `third_party` ffmpeg 库前置）。

## 三十秒感知你的客户端

```python
import eve_proxy_ng

for c in eve_proxy_ng.discover_clients():
    print(c.pid, c.flavor, c.character_name, c.window_title)
    # 31336 infinity aiyoggle 星战前夜：晨曦 [Infinity] - aiyoggle
```

## 读取语义快照

```python
reader = eve_proxy_ng.UiReader(pid=31336)   # 或 sample="dump.zip" 离线回放
reader.find_ui_root()                 # 首次冷扫描 ~20s，内部缓存

snap = reader.read_snapshot()
print(snap["game_state"])             # {'screen': 'in_space', ...}

for entry in snap["overview_windows"][0]["entries"][:5]:
    print(entry["object_name"], entry["distance_meters"],
          entry["is_interactable"], entry["occluded_percent"])
    # X4UV-Z 1225000 False 33.3

for m in snap["ship_ui"]["module_buttons"]:
    print(m["slot_index"], m["module_name"], m["type_id"])
```

!!! note "帧语义"
    每次 `read_snapshot()` 都是一次完整的点时快照（point-in-time）——
    动态值从不跨帧缓存，因此不会漏掉任何 UI 更新。连续调用即连续帧。

## 命中测试：点击会落到谁身上

```python
hit = reader.hit_test(1351, 550)
print(hit)
# {'type_name': 'Sprite', 'name': 'activitypic', 'pick_state': 1,
#  'region': {'x': 966, 'y': 463, ...}, 'blocked': False, ...}
```

遮挡与穿透判定来自客户端自身的 `_pickState` 路由 + 滚动视口裁剪，
与真实画面互相印证过。

## 窗口操作与资源读取

```python
clients = eve_proxy_ng.discover_clients()
clients[0].activate_window()      # 置前聚焦

png = eve_proxy_ng.get_resource_image("res:/ui/texture/icons/13_64_5.png")
open("icon.png", "wb").write(png)  # 离线导出任意游戏资源图标
```

## 类型名与图标家族（本地客户端数据）

```python
eve_proxy_ng.type_name(645)            # '多米尼克斯级'（中文，来自本地客户端）
eve_proxy_ng.module_name_from_icon("res:/ui/texture/icons/6_64_14.png")
                                  # '三钛合金'
eve_proxy_ng.resources_info()
# {'loaded_from': 'embedded-baseline', 'type_count': 62467,
#  'data_dir': 'C:\\Users\\...\\eve_proxy_ng'}
```

62k+ 类型的中文名与国服独有类型直接由客户端自身的 FSD 数据派生（随包
内嵌基线；`eve-cli icons update` 可随时从已装游戏刷新到用户 overlay）。

## 语义视图：网页实时重构游戏画面

```bash
eve-view --pid 31336 --open    # 浏览器里按游戏坐标实时绘制解析结果
```

总览行带原生图标与距离、模块按钮用游戏图标、悬停看全部语义细节
——详见[语义视图指南](guides/live-view.md)。

## 命令行：eve-cli

```bash
eve-cli clients                                  # 发现客户端
eve-cli read --pid 31336 --output-file tree.json # UI 树 JSON
eve-cli snapshot --pid 31336                     # 语义快照打印
eve-cli record --duration-sec 120                # 三路会话录制
eve-cli record --video-size 1080p --encoder libx264
```

完整子命令与参数见 [eve-cli 参考](cli.md)，录制产物格式见
[录制数据格式](data-format.md)。

## 离线文档

```python
import eve_proxy_ng, webbrowser
webbrowser.open(str(eve_proxy_ng.docs_dir() / "index.html"))
```

## 下一步

- [Python API](api.md) —— 全部类与函数的详细文档
- [录制数据格式](data-format.md) —— 模仿学习数据集的回放方法
