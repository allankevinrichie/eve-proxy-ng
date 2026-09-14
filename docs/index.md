# eve-proxy-ng

<div class="eve-hero" markdown>

<!-- 对齐 EVE Online 官网的深空科幻风：标题 + 副标题 + 双 CTA -->
<!-- Deep-space sci-fi hero aligned with the EVE Online website. -->

# :material-satellite-variant: eve-proxy-ng

**EVE Online 感知与语义层** —— 把游戏 UI 变成 agent 可直接消费的结构化数据。

Rust 实现 · Python 绑定 · 三路同步会话录制 · 多客户端 · 多服务器

[:material-rocket-launch: 快速上手](quickstart.md){ .md-button .md-button--primary }
[:material-book-open-variant: 使用指南](guides/perception.md){ .md-button }
[:material-api: Python API](api.md){ .md-button }

</div>

---

<div class="grid cards" markdown>

-   :material-radar:{ .eve-cyan } **客户端发现**

    ---

    一行代码枚举全部运行中的客户端：PID、服务器（曙光/经典/国际）、角色名、
    窗口句柄。窗口置前/最小化/恢复随取随用。

-   :material-memory:{ .eve-cyan } **内存级 UI 读取**

    ---

    直接从进程内存读取整棵 UI 树（每帧完整点时快照，不漏任何更新）。
    活体进程或离线 dump 样本，同一套 API。热读 ~35ms / 帧。

-   :material-text-box-check:{ .eve-cyan } **语义快照**

    ---

    总览条目、模块按钮（精确 typeID）、电容/电量、菜单、货舱、滚动视口、
    模态判定 —— 全部带 `is_interactable` / `occluded_by` 遮挡分析。

-   :material-target:{ .eve-cyan } **命中测试**

    ---

    `hit_test(x, y)` 按客户端自身的 `_pickState` 路由回答"这一下会点中谁"，
    执行层不再猜测几何。

-   :material-record-circle:{ .eve-cyan } **会话录制**

    ---

    `eve-cli record` 三路时间对齐采集：语义观测增量 + 人类键鼠（含注入标志）+
    H.264 硬编窗口视频（NVENC/AMF/QSV 自动探测，带像素级时间水印）。

-   :material-package-variant-closed:{ .eve-cyan } **开箱即用**

    ---

    `pip install eve-proxy-ng` 同时装好 Python 模块、`eve-cli` 命令、全部 ffmpeg
    依赖与本套离线文档 —— 无需 Rust 工具链。

</div>

## 服务器支持

| Flavor | 运营 | 状态 |
|---|---|---|
| `infinity` 曙光服 | 网易 | 基线，实机全链路验证 |
| `serenity` 经典服 | 网易 | 回退曙光基线，差异按需覆盖 |
| `tranquility` 国际服 | CCP | 回退曙光基线，差异按需覆盖 |

!!! warning "EULA 警告"
    内存读取属 EULA 违规行为，本项目仅用于学习研究，风险自担。
    Memory reading violates the game EULA — research use only.
