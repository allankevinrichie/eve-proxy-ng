# 录制数据格式

`eve-cli record` 产出时间对齐的三路数据流，目录结构：

```
record-<时间戳>/
├── manifest.json          # 会话元数据（见下）
├── input.jsonl           # 人类键鼠事件（全局 LL 钩子）
└── clients/
    ├── <pid>.jsonl       # 语义观测流（增量 NDJSON）
    └── <pid>.mp4         # 窗口视频（CFR H.264，默认 720p 缩放）
```

## 时间对齐

三路统一使用**会话相对毫秒** `t_ms`（manifest 的 `started_at_unix_ms`
为 epoch 起点；观测停止等控制事件也带 `t_ms`）。视频帧 `n` 的时刻 =
`video_anchor_t_ms + n / video_fps`，且每帧像素里有独立的时间水印
`T+SS.S`（即使编码堆积/丢帧也不会破坏对齐——直接看像素即可互验）。

## manifest.json 关键字段

| 字段 | 含义 |
|---|---|
| `started_at_unix_ms` | epoch 起点（`t_ms` 换算墙钟） |
| `params.*` | 本次录制全部参数 |
| `clients[].video_encoder` | 实际选中的编码后端（含探测/闸门过程日志于 stderr） |
| `clients[].video_native_width/height` | 捕获原始窗口尺寸 |
| `clients[].video_width/height` | 视频实际尺寸（缩放后） |
| `clients[].video_scale` | 精确缩放因子（float） |
| `clients[].video_client_offset` | 客户区偏移，**已换算到缩放后视频空间** |
| `clients[].video_dropped_frames` | 编码跟不上时的丢帧计数（同时 WARN） |
| `clients[].observe_frames / observe_slow_reads` | 观测帧数 / 慢于目标间隔的帧数 |

### 视频坐标换算

```
视频像素坐标 = (客户区坐标 + 原始偏移) × video_scale
```

`video_client_offset` 已乘过 scale，可直接用：
`video_x = input_client_x + offset[0]`、`video_y = input_client_y + offset[1]`
（input.jsonl 事件自带换算好的客户区坐标）。

## input.jsonl — 键鼠流

每行一个事件（或一个 16ms 轨迹聚合桶）：

```json
{"t_ms": 1234, "kind": "mouse_move", "pid": 31336,
 "x": 1351, "y": 550, "injected": false}
```

- 归属：按键按前台窗口、鼠标按 `WindowFromPoint` 命中窗口（EVE 无全局快捷键），
  在钩子回调内完成（无排队延迟）；拖拽延续归属到按下时刻的客户端
- `injected`: true 表示该事件来自程序注入（`LLMHF/LLKHF_INJECTED`）——
  **注入输入如实记录不过滤**，用于执行层调试对照
- 未归属任何客户端的事件带 `pid: null`（如桌面操作）

## clients/\<pid\>.jsonl — 观测增量流

首行 `snapshot_full`（完整快照），其后每帧一行：内容有变时为
`snapshot_delta`，无变化时为空 delta（仍占一行，保帧号连续）。

```json
{"frame": 17, "kind": "snapshot_delta", "t_ms": 8500,
 "read_ms": 31, "parse_ms": 2,
 "parse_detail": {"regioned_us": 2100, "interaction_us": 640, "extractors_us": 900},
 "changed": {"overview_windows": [...], "interaction_elements": {...}}}
```

回放方法：

1. 按 `snapshot_full` 建立初始状态；
2. 逐帧 fold `snapshot_delta`：
   - `cleared` 列出的区段置 null；
   - keyed 区段（如 `interaction_elements`）按元素 `address` 应用
     added/updated/removed，`order` 数组给出最终顺序；
3. `read_ms`/`parse_ms`/`parse_detail` 是性能遥测，可剥离。

!!! tip "为什么每帧都是完整重读"
    观测线程每帧从内存读取整棵 UI 树（点时快照）再做 diff——
    动态值从不跨帧缓存，因此任何 UI 更新都不会被漏掉；增量只发生在
    序列化层，语义上等价于逐帧全量。
