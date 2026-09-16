# Agent 感知设计：玩法域驱动的语义结构蓝图

> 目标：让结构树成为 agent 的眼睛——在任何玩法场景下，分析、判断、实操所需的
> 信息都能从快照中**高效、完整、带交互状态**地取得。
> 方法论借鉴自动化网页测试（ARIA 状态模型）：感知不止"看见什么"，还有
> "控件处于什么状态、操作后会有什么反馈"。

## 一、设计原则

1. **语义根优先**：每个 UI 表面归属一个语义根（窗口/区域），根拥有自己的元素
   （`element_addresses`，祖先链判定）——agent 先选根，再在根内取信息
2. **信息三分离**：状态值（读）/ 可操作集（元素+交互态）/ 反馈通道（变化的事件源）
3. **交互状态模型**（借 ARIA）：每个可操作元素至少可报告
   - `is_enabled`（控件自身禁用态）
   - `is_click_reachable`（遮挡/路由视角）
   - `is_on`（开关态）/ `gauge_percent`（量规态）
   - `is_checked`（复选态）/ `selection`（选择态，开发中）
   - `busy/loading`（进行中）、`value`（输入框当前值，开发中）
4. **游戏内 Ground Truth 优先**：状态读游戏自身信号（纹理名/_checked/
   _interaction_state），不做几何推断

## 二、玩法域 → 信息需求 → 语义根映射

### P1 战斗（PvE/PvP 共用）——最高优先级

| 需求 | 信息/交互 | 语义根 | 现状 |
|---|---|---|---|
| 锁定目标管理 | 当前锁定列表（目标名/类型/距离/血条/被攻击指示）、剩余可锁数 | **`targets`**（新建） | ❌ 未提取（需锁定状态验证） |
| 目标选择 | 点击某个锁定目标 | targets[].region | 同上 |
| 武器操作 | 开火/停火/切换弹药 | ship_ui.hud_buttons ✓ + **模块弹药量** | 弹药数信号待验证 |
| 超载管理 | 模块/整槽超载 + 热量 | ModuleButton.overload ✓ + **heat_gauges** | 热量计在 element_addresses，待结构化 |
| 无人机 | 无人机舱/在太空/血量/指令（攻击/召回/放弃） | **`drone_window`**（新建） | ❌ 需开窗验证 |
| 电子战感知 | 被干扰/被网/被点的指示 | overview indications 部分 ✓（EWAR 文本待中文化） | 部分 |
| 传感器 | D-Scan 结果/角度/距离/扫描按钮 | **`dscan`**（新建） | ❌ 需开窗验证 |
| 概览 intel | 本地人数/敌对标记 | chat(local).members ✓ + overview ✓ | 大部分 ✓ |

### P2 导航/移动

| 需求 | 语义根 | 现状 |
|---|---|---|
| 跃迁/跳门/停靠指令 | **`selected_item_window.buttons`** 结构化（接近/环绕/跃迁/信息/锁定） | ❌ 仅窗口控件，动作按钮未分类（待选中某物验证） |
| 路线规划 | info_panels.route ✓（存在性）+ **跃迁段计数/下一跳** | 待样本 |
| 书签 | people&places → other_windows（通用） | 通用兜底 ✓ |
| 自动导航 | hud.autopilot ✓ | ✓ |
| 会话切换计时 | 跳门/停靠/换船 冷却（圆形倒计时） | **`session_timers`** 待验证（TimerContainer 结构已见） |

### P3 聊天/社交 intel

| 需求 | 语义根 | 现状 |
|---|---|---|
| 频道列表/当前频道 | chat_window_stacks.windows ✓ | ✓ |
| **消息流读取** | **`chat_window_stacks.messages`**（可见消息：发送者/时间/文本/链接） | 🔨 结构已探明（XmppChatEntry，htmlstr+文本），实现中 |
| 成员列表/人数 | windows[].users（空——成员经 element_addresses 部分覆盖） | 部分 |
| 输入框 | messages 旁 `input`（EditPlainText 元素已有） | ✓（元素态） |

### P4 市场/工业/资产

| 需求 | 语义根 | 现状 |
|---|---|---|
| 市场浏览/下单 | market_window → other_windows（通用）+ 元素 | 通用兜底；**结构化待样本** |
| 工业/蓝图 | 同上 | 通用兜底 |
| 资产/钱包 | Neocom 入口 + 窗口（通用） | 通用兜底 |
| 货舱内容 | inventory_windows ✓（含容量文本） | ✓ |
| 挖矿 | mining_scan 按钮 ✓ + 矿石扫描结果窗口 | 结果窗待样本 |

### P5 角色成长

| 需求 | 语义根 | 现状 |
|---|---|---|
| 技能队列 | skill_queue 窗口（通用） | 通用兜底 |
| 装配/模拟 | fitting_window ✓（槽位/属性）+ 电容模拟 | ✓ |
| 克隆/植入体 | character sheet（通用） | 通用兜底 |

## 三、交互状态模型（对照 ARIA）

| ARIA 概念 | 本快照字段 | 游戏信号源 |
|---|---|---|
| aria-disabled | `is_enabled` | `_interaction_state` 的 disabled |
| 点击可达（自定义） | `is_click_reachable` | 中心命中路由 + pick/opacity |
| aria-checked | 复选行 `is_checked` / 复选框元素经 `_checked` | `_checked` |
| aria-pressed | `is_on`（HUD 按钮/相机） | busy 精灵存在性 / hint 翻转 |
| 进度量规 | `gauge`（货柜条） | 条高校准 |
| role=gridcell 选中 | **`is_selected`**（待） | 选中项高亮（库存 ItemEntry 变体?） |
| 输入值 | **`text_value`**（待） | EditPlainText `_setText`（聊天输入实测可用） |
| 忙碌/加载 | busy 精灵 alpha/存在 | 已见（矿扫描动画等） |
| aria-invalid/校验 | 暂无对应（游戏少见表单） | — |

## 四、实施批次

- **T1（本轮）**：聊天消息流（结构已验证）＋ 交互状态文档化
- **T2（需配合）**：锁定 1-2 个目标 → `targets` 根；开无人机窗口 → `drone_window`；
  开 D-Scan → `dscan`；选中一个总览条目 → `selected_item` 动作按钮结构化
- **T3（需配合）**：跳门/停靠动作 → 会话计时器；装填弹药模块 → 弹药量信号；
  开技能队列/市场 → 评估结构化深度 vs 通用兜底
- **T4**：热量计结构化、EWAR 中文指示词表、消息增量（录制流内聊天 diff）

## 五、验证纪律

每批遵循：探测真实树结构 → 实现提取器 → 活体验证字段值 → 全量测试 →
如实更新本文档"现状"列。
