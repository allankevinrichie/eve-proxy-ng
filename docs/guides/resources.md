# 资源与图标识别

游戏内"名字→含义"的知识从哪来、怎么更新、识别时按什么优先级取。

## 数据来源：只从本地客户端

类型表（typeID → 本地化名 + 图标）全部**从已安装的客户端提取**：

- 客户端自带的官方 FSD 解码器（`bin64\<name>Loader.pyd`）读取
  `res:/staticdata/*.fsdbinary` + 中文本地化 pickle
- 62,467 个类型的中文名，**含国服独有类型**（TQ SDE 下载路径没有这些）
- 所需 py2.7 运行时随 Python 包分发，`eve-cli icons update` 完全离线

## 三层级联与诊断

| 层 | 位置 | 优先级 |
|---|---|---|
| 手工覆盖 | 代码内（实测校准的显示名） | 最高 |
| 用户 overlay | `%LOCALAPPDATA%\eve_proxy_ng\resources\types.<flavor>.json` | 中 |
| 嵌入基线 | 随二进制内嵌（构建时快照） | 兜底 |

```python
eve_proxy_ng.resources_info()
# {'loaded_from': 'embedded-baseline', 'type_count': 62467, 'data_dir': '...'}

eve-cli icons show res:/ui/texture/icons/6_64_14.png --verbose
# res:/ui/texture/icons/6_64_14.png → 三钛合金   [embedded-baseline]
```

客户端大版本更新后刷新 overlay：

```bash
eve-cli icons update          # 默认 client 源，秒级完成，下一进程生效
```

## 识别策略：按精确度取用

```text
typeID（ModuleButton_<typeID> 节点名）
  → eve_proxy_ng.type_name(3638)            "民用加特林磁轨炮"   ← 精确变体
图标家族（同图标=同家族，民用/T1/T2 共享）
  → eve_proxy_ng.module_name_from_icon(...)  "民用加特林磁轨炮"   ← 家族级
bracket 图标词干（总览条目）
  → entry["icon_name"]                 "stargate"           ← 类别级
```

- **精确变体**（品质/衍生型）只能靠 typeID；家族名会混变体
- 总览条目没有 typeID，`icon_name` 是其类别通道（stargate /
  citadelLarge / skyhook_bracket …）
- 未收录图标：导出 PNG 给视觉模型，或悬停拿 tooltip

## 图标图片本体

图片字节直接读客户端共享缓存（离线、无内存访问）：

```python
png = eve_proxy_ng.get_resource_image("res:/ui/texture/icons/13_64_5.png")
```

批量导出用 `eve-cli icons export --out icons/`（按表内全部图标导出，
供视觉模型建库）。

## py3 客户端迁移

与 py2 绑定的只有：py2.7 运行时（已随包分发）、提取 payload（已写成
2/3 兼容源码）、客户端自己的 pyd。客户端 python 版本由
`bin64\pythonXY.dll` 自动探测；未来 py3 客户端只需为引导逻辑补一个
py3 embeddable 运行时，数据格式与解析链路不变。
