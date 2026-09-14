# 故障排查

按症状定位。多数问题一个字段或一条命令就能确诊。

## 读取与感知

**首次读取卡了 ~20 秒**
正常行为——UIRoot 全内存冷扫描。之后热读 ~30ms/帧。只发生一次
（每个读取器实例），预热需求见[感知指南](perception.md)。

**读取突然失败几帧后恢复**
场景切换时 UI 根搬家，读取器自动失效缓存并重新搜索。连续失败 8 次
才判定客户端退出。频繁伴随 `window_handle` 失效请确认客户端没崩溃。

**客户端更新后某些语义没了/变了**
客户端 UI 类型可能改名。`eve-cli types --pid <pid> --limit 100` 做
类型普查，与上一版对比新增/消失的类型名；样本回归（重录 dump +
`tests/regression.py`）定位具体断言。

**速度远低于预期**
十有八九是 debug 构建（慢 3-5 倍）。确认 `target/release/`；
一切性能结论以 release 为准。其次看场景规模：节点数见快照
`node_count`，会战大树读耗时线性增长。

## 资源表

**`type_name` 返回的名字不是中文 / 缺类型**
`eve_proxy_ng.resources_info()` 看 `loaded_from`——若显示旧 overlay，
`eve-cli icons update` 从当前客户端刷新。国服独有类型只在 client
源（TQ SDE 备选源没有）。

**`icons update` 报 py2.7 相关错误**
- wheel 安装场景：包内自带运行时（`eve_proxy_ng/tools/py27`），无需网络；
  若报"拒绝访问"，先手动跑一次该目录下的 `python.exe -V` 排除杀软拦截
- 仓库/独立 CLI 场景：首次会下载 python.org MSI 并管理性解包
  （免安装免管理员）；下载慢可手动放置到
  `%LOCALAPPDATA%\eve_proxy_ng\tools\py27`
- 报"client uses Python 3.x loaders"：客户端已是 py3——见
  [资源指南](resources.md)的迁移边界

## 录制

**视频丢帧（`video_dropped_frames` > 0）**
编码跟不上采集。摘要里的编码器名指示实际后端：auto 选中的硬编有
≥1.5x 实时闸门，正常不会丢；多客户端同录摊薄吞吐、或 GPU 被游戏
占满时会。降 `--video-fps` 或降档 `--video-size`；仍丢则手选
`--encoder libx264`（软编 24 逻辑核吞吐充足）。

**硬编没被选中（回退了 libx264）**
auto 的探测与吞吐日志在 stderr——每个候选后端的失败原因与实测倍率
都有记录。常见：驱动过旧（nvenc 要求 ≥471.41）、核显吞吐不足被闸门
拦下（正确行为，防"能开但稳态丢帧"）。

**观测帧率低于 `--interval-ms` 对应值**
看 jsonl 的 `read_ms`：单帧读取超过间隔时实际帧率被读取耗时封顶。
这是场景复杂度问题，不是录制器问题。

**mp4 时长/对齐疑问**
帧时刻 = `anchor + n/fps`，每帧右下角有 `T+SS.S` 像素水印独立仲裁。
抽帧（ffmpeg `select='eq(n,X)'`）肉眼核对即可。

## 安装与包

**`eve-cli` 命令找不到**：wheel 未装或不在当前 venv；
`pip install` 后 console script 在 venv 的 `Scripts/` 下。

**import eve_proxy_ng 报 DLL 错误**：扩展与 ffmpeg DLL 同包分发，正常安装
即用；若手动挪动了 `eve_proxy_ng/bin` 目录，把它加回 `PATH`。

## 风险提示

内存读取属 **EULA 违规行为**，可能导致账号处置；录制数据集包含账号
信息，**不要公开分发**。本项目仅用于学习研究，风险自担。
