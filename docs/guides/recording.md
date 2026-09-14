# 会话录制与分析

用 `eve-cli record` 采集"人玩 EVE"的完整会话——观测、键鼠、视频三路
时间对齐数据，用于模仿学习数据集、执行层调试或问题回放。

## 录制一条会话

```bash
eve-cli record --duration-sec 120                 # 全部客户端，2 分钟
eve-cli record --pid 31336 --video-size 1080p     # 单客户端，1080p 档
eve-cli record --interval-ms 100 --video-fps 15   # 10fps 观测
```

启动流程：三路各自**预热**（UIRoot 冷扫描、编码器探测与吞吐闸门、
WGC/钩子初始化）→ 全部就绪 → 统一开跑（t=0 对齐，不存在某路把
初始化时间埋进另一路）。停止方式：`--duration-sec` 到时或 Ctrl-C，
两者走同一条优雅收尾路径（拆钩子 → 各流 flush → 编码器排空写完
mp4 → manifest 落盘）。

### 参数怎么选

| 参数 | 默认 | 建议 |
|---|---|---|
| `--interval-ms` | 500 | 观测轮询下限。分析人机交互取 100-250；纯状态记录 500-1000 够用。实际帧率 = 1/max(interval, 单帧读取耗时)，慢于目标会记入 `observe_slow_reads` 并 WARN |
| `--video-fps` | 15 | 交叉核对操作时刻够用；需要精细轨迹分析再提（编码吞吐有闸门保护，跟不上会丢帧并计数） |
| `--video-size` | 720p | 720p/1080p/native 三档，像素总数 ≤ 预算、等比缩放 |
| `--encoder` | auto | auto 自动探测硬编（nvenc→amf→qsv）并实测吞吐 ≥1.5x 才选；诊断时手选 `libx264` |
| `--move-every-ms` | 16 | 鼠标轨迹聚合桶；0 = 全保真（文件变大） |

### 积压绝不静默

视频跟不上 → 丢帧计数 + WARN + manifest `video_dropped_frames`；
观测慢帧 → WARN + `observe_slow_reads`。摘要行会打印全部计数。
**时间轴不受丢帧影响**：视频帧时刻 = `anchor + n/fps`，且每帧像素
里有 `T+SS.S` 水印独立佐证。

## 产物结构

```
record-<时间戳>/
├── manifest.json     # 元数据：epoch、参数、每客户端统计、缩放/偏移
├── input.jsonl       # 全局键鼠事件流
└── clients/
    ├── 31336.jsonl   # 语义观测（首帧 full，其后增量）
    └── 31336.mp4     # 窗口视频（CFR H.264）
```

字段细节见 [录制数据格式](../data-format.md)。

## 分析工作流

### 1. 还原人类操作序列（输入流）

```python
import json

events = [json.loads(l) for l in open("input.jsonl", encoding="utf-8")]
clicks = [e for e in events if e["kind"] == "mouse_down"]
for e in clicks[:5]:
    print(f"t={e['t_ms']}ms pid={e['pid']} at ({e['x']},{e['y']}) injected={e['injected']}")
```

- 每条事件带 `pid` 归属（按键=前台窗口、鼠标=落点窗口）、客户区
  坐标、`injected` 标志——**注入输入如实记录**，对照执行层调试时
  用它区分"人点的"和"程序点的"
- 未归属事件 `pid: null`（桌面操作等）

### 2. 回放观测流（增量 fold）

```python
state = None
for line in open("clients/31336.jsonl", encoding="utf-8"):
    frame = json.loads(line)
    if frame["kind"] == "snapshot_full":
        state = frame["snapshot"]
    elif frame["kind"] == "snapshot_delta":
        apply_delta(state, frame["changed"])   # cleared 置 null、keyed 按 address
```

`read_ms`/`parse_ms`/`parse_detail` 是性能遥测，分析时可剥离。
帧号连续（空 delta 也占一行），与视频帧号一一独立推进、靠 `t_ms`
对齐。

### 3. 视频交叉核对

坐标从输入流到视频像素的换算（manifest 已备好全部系数）：

```text
视频像素 = (输入事件客户区坐标 + video_client_offset)   # offset 已在缩放后空间
帧时刻   = video_anchor_t_ms + 帧号 / video_fps          # 与像素内水印互验
```

拿水印时间做最终仲裁：抽帧看右下角 `T+SS.S`，与推算时刻应在同一
帧周期内。定位"某一秒画面上是什么"→ 用该时刻最近的观测帧状态 +
`hit_test` 语义核对。

### 4. 典型任务示例

- **操作→语义映射**：对每个 click 事件，取 `t_ms` 之前最近的观测帧，
  用事件的客户区坐标做命中，得到"人点了哪个语义元素"训练对
- **执行层回归**：你的 agent 注入输入时同步录制，`injected=true`
  的事件序列 + 当帧观测 = 一次可回放的执行记录

## 注意事项

- 录制期间客户端窗口保持**非最小化**（WGC 按窗口捕获，遮挡无碍，
  最小化拿不到新帧）
- 游戏自己不产生输入时观测仍有慢帧波动属正常（与游戏渲染负载相关），
  看 `observe_slow_reads` 的比例而非个别帧
- 大量客户端同录会摊薄 CPU/GPU：先 2-3 开验证，再看各路丢帧计数
- 内存读取属 EULA 违规——数据集**不要公开分发**，本工具仅用于学习研究
