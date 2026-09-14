# eve-cli 参考

`eve-cli` 随 Python 包分发（`pip install eve-proxy-ng` 后即可在任意环境使用，
ffmpeg 等全部依赖已内置）。源码在仓库 `crates/eve-cli`。

全局行为：

- 所有读取命令同时支持 `--pid <N>`（活体）或 `--sample <zip>`（离线回放）
- JSON 输出与 Sanderling 兼容（对象地址为十进制字符串，宽整数带 `int_low32`）
- 性能敏感命令务必用 release 构建（debug 慢 3-5 倍严重失真）

---

## clients — 客户端发现

```bash
eve-cli clients
# pid 50632  曙光服 (Infinity)  char aiyoggle  "星战前夜：晨曦 [Infinity] - aiyoggle"
```

## dump — 录制内存样本

```bash
eve-cli dump --pid 50632 [--out-dir samples] [--delay 5]
```

把客户端完整内存快照录成 Sanderling 兼容 zip（含前后客户区截图），
供离线开发与回归测试。**录制时游戏窗口需可见。**

## read — 读取 UI 树

```bash
eve-cli read --pid 50632 [--output-file tree.json] [--root-address 0x...] \
             [--remove-other-dict-entries] [--warmup-iterations 3]
eve-cli read --sample samples/process-sample-XXXX.zip
```

输出整棵 UI 树 JSON（in_space ~2600 节点，数 MB）。
`--root-address` 跳过发现直接从指定 UIRoot 读；`--warmup-iterations`
先做 N 次热身再计时读。

## snapshot — 语义快照

```bash
eve-cli snapshot --pid 50632
```

打印语义快照（game_state / ship_ui / overview_windows / …）。

## types — 类型普查

```bash
eve-cli types --pid 50632 --limit 50
```

列出内存中的 Python 类型与实例计数。客户端版本更新后用它对比哪些类型
改名/新增（漂移调试）。

## bench — 观测管线测速

```bash
eve-cli bench --pid 50632 --interval-ms 20 --frames 200
```

实测观测管线帧率（读+解析+diff 全链路），输出 p50/p95 帧间隔与分段耗时。

## icons — 图标/类型资源表

```bash
eve-cli icons update                          # 从本地客户端提取（默认）
eve-cli icons update --source sde             # 备选：fuzzwork TQ SDE 下载
eve-cli icons update --lang en                # 英文名（默认 zh 中文）
eve-cli icons update --write-baseline         # 维护者：写回仓库基线
eve-cli icons show res:/ui/texture/icons/12_64_8.png --verbose
eve-cli icons export --out icons-out/         # 从共享缓存导出 PNG 本体
```

资源表的三层级联（优先级从高到低）：

1. **手工覆盖**（代码内，实测校准的显示名）；
2. **用户 overlay**：`%LOCALAPPDATA%\eve_proxy_ng\resources\types.<flavor>.json`
   （或 `$EVE_NG_DATA_DIR`）——`icons update` 的写入点；
3. **嵌入基线**：随二进制内嵌的 `data/types.<flavor>.json.gz`。

`icons update --source client` 的机制：客户端自带的官方 FSD 解码器
（`bin64\<name>Loader.pyd`，Python 2 扩展）读取本地
`types/iconids/groups/categories` FSD 表 + zh 本地化 pickle——
**中文名与国服独有类型直接来自安装的游戏**，比 TQ SDE 下载更全更准。
所需的 **py2.7 运行时随 Python 包分发**（`eve_proxy_ng/tools/py27`，PSF 许可），
pip 安装后完全离线可用；仓库/独立 CLI 场景则首次运行时自动引导
（python.org MSI 管理性解包，免安装免管理员，约 20MB 一次性）。
`--verbose` 会显示每次查询由哪层应答。

**py3 客户端迁移边界**：与 py2 绑定的只有三样——py2.7 运行时、提取
payload 脚本（已写成 2/3 兼容源码子集）、客户端自身的 pyd。客户端
python 版本由 `bin64\pythonXY.dll` 自动探测；未来 py3 客户端落地时，
只需为引导逻辑补一个 py3 embeddable 运行时，payload 与数据格式不变。

## record — 三路会话录制

```bash
eve-cli record [--pid 50632 ...] [--duration-sec 120] [--out-dir record-…] \
               [--interval-ms 500] [--video-fps 15] [--move-every-ms 16] \
               [--video-size 720p|1080p|native] \
               [--encoder auto|h264_nvenc|h264_amf|h264_qsv|libx264] \
               [--no-video]
```

| 参数 | 默认 | 说明 |
|---|---|---|
| `--pid` | 全部客户端 | 可多次指定；每客户端一路观测+视频 |
| `--duration-sec` | 手动 Ctrl-C | 录制时长上限 |
| `--interval-ms` | 500 | 观测轮询间隔下限（实际帧率 = 1/max(interval, 单帧耗时)） |
| `--video-fps` | 15 | 视频 CFR 帧率 |
| `--move-every-ms` | 16 | 鼠标轨迹聚合桶（0 = 不节流全保真；按键/点击/滚轮始终全保真） |
| `--video-size` | 720p | 分辨率档：按像素总数 ≤ 预算（720p=921,600 / 1080p=2,073,600）等比缩放；native 只偶数化 |
| `--encoder` | auto | 编码后端。auto 进程内探测 + 吞吐闸门（≥1.5x 实时）：`h264_nvenc → h264_amf → h264_qsv → libx264` |
| `--no-video` | — | 只录观测+输入两路 |

三路预热全部就绪后统一开跑（barrier 同步起跑）；每帧视频右下角有
`T+SS.S` 像素时间水印；任何积压（视频丢帧/观测慢帧）都会 WARN 并计入
manifest，绝不静默。

产物格式与回放方法见 [录制数据格式](data-format.md)。
