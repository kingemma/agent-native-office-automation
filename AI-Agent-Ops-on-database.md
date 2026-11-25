# AI Agent 操作业务数据：完整链路与设计动机

> 基于当前仓库代码实测梳理（2026-11-22）。所有路径均相对仓库根目录。
> 核心原则一句话：**模型只负责决定“调用哪个工具、传什么参数”，绝不直接接触数据库；所有写入都经过同一个服务端入口，并由会话身份、RBAC、事务和审计兜底。**

---

## 一、链路总览

```
浏览器 / Tauri / 手机
  │  POST /api/agent/stream（Bearer Token）
  ▼
Axum require_user()  ──►  resolve_session(token) ──►  SessionUser（真实身份）
  │
  ▼
agent::run_with_history()
  ├─ 无模型配置 ──► intent::run_local_stream()  ── 本地意图引擎（正则+关键词）
  └─ 有模型配置 ──► run_llm() ──► Rig Agent ──► 本地意图引擎兜底（仅在未写库时）
  │
  ▼
tools::call(state, user, tool, args)   ← 唯一写入口，所有通道共用
  ① 工具白名单  ② RBAC  ③ 本人数据范围  ④ 审批节点归属  ⑤ 敏感操作确认
  │
  ▼
Store::transaction(run_tool_tx)  ──►  Postgres（sqlx，事务内）
  │                                      ├─ 业务表写入
  │                                      ├─ counters 原子取号
  │                                      ├─ custom_field_values（EAV）
  │                                      └─ approval_flows / notifications
  ▼
store.audit(...)（事务外单独提交） ──►  audit_events
  │
  ▼
SSE: Think / Delta / Reply / Error ──► 前端；敏感操作时前端弹窗 + confirmed=true 重试
```

涉及的核心文件：

| 层 | 文件 | 职责 |
|---|---|---|
| 客户端 | `frontend/src/api/bridge.ts`、`frontend/src/store.ts` | 发流式请求、拆 SSE、确认后重试 |
| 路由 | `crates/oa-server/src/main.rs` | 鉴权、SSE 输出、工具直调 |
| 编排 | `crates/oa-core/src/agent/mod.rs` | 模式分流、Rig Agent 构建、失败回退 |
| 意图 | `crates/oa-core/src/agent/intent.rs` | 本地解析、多步计划、跨步引用 |
| 工具 | `crates/oa-core/src/agent/tools.rs` | 32 个工具、统一入口 call()、五道校验 |
| 权限 | `crates/oa-core/src/auth.rs` | SessionUser、权限矩阵、会话解析 |
| 存储 | `crates/oa-core/src/db.rs`、`store.rs` | Repo/Store/Tx、SQL 方言、事务、审计 |

---

## 二、完整链路（逐步展开）

### 1. 客户端发起请求

- `frontend/src/api/bridge.ts:314` `agentChat` → `POST /api/agent/chat`；`:351` `agentStream` → `POST /api/agent/stream`；`:395` `agentPlan` → `/api/agent/plan`；`:400` `callTool` → `/api/tools/call`。
- `agentStream` 没有用 `EventSource`，而是用 `fetch` + `ReadableStream` 手工拆 SSE —— 因为 `EventSource` 既不能设置 `Authorization` 请求头，也不能发 POST 请求体（`bridge.ts:348` 注释）。
- `frontend/src/store.ts:299` `sendAgent` 管理会话状态；`:349-367` 收到 `pending` 后弹确认框，用户同意则调用 `callTool(pending.tool, {...pending.args, confirmed: true})`，最后 `bump()` 刷新各台账。
- Tauri 桌面端没有业务 invoke：`src-tauri/src/lib.rs:3-5` 明确“桌面与移动端不再是数据宿主，没有本地 SQLite、没有 invoke 业务命令”，全部走 REST。

### 2. 服务端鉴权：拿到真实身份

- `main.rs:58` `require_user()` 从 `Authorization: Bearer <token>` 取令牌，调用 `store.resolve_session(&token)`；失败返回 401。
- `auth.rs:234` `resolve_session` 关联 `users` 与 `employees`，过滤过期会话、停用账号和 `account_frozen` 离职冻结账号。
- 得到的 `SessionUser`（`auth.rs:38-45`）包含 `employee_id / username / display_name / role / permissions`，它是后续所有写入和审计的 `actor` 来源。

### 3. 模式分流：本地引擎 or LLM

- `agent/mod.rs:73` `desired_mode()`：`OA_AGENT_MODE=local/offline` 强制本地，`llm/cloud` 强制模型，默认 `auto` —— 有可用模型配置走 LLM，否则走本地。
- `llm.rs:160` `effective()` 的配置优先级：**数据库 `settings.llm.config` > 环境变量 > 无配置**（管理员在界面保存的配置优先）。
- `run_with_history()`（`mod.rs:185`）按上述结果分流到 `intent::run_local_stream()` 或 `run_llm()`。

### 4. 意图解析与计划生成

**本地引擎**（无外部密钥也能完整工作，`intent.rs:3-5`）：

- `intent.rs:264` `plan_for()` 用正则 + 关键词解析指令中的金额（`money_cents`）、日期、部门、人员。
- 多步流程示例：入职 = `hr.onboard → asset.assign → contract.create`（`intent.rs:407-444`）。
- 跨步骤引用用占位符 `"$0.employee_id"` 表达，`resolve_args()`（`intent.rs:613-630`）在真正执行第 1 步后替换成真实返回值。

**LLM 模式**：

- `mod.rs:293-319` 用 `AgentBuilder` 构建 Rig Agent，注入 `PREAMBLE`（`mod.rs:39-53`）+ 当日日期提示 `today_hint()`，注册 **32 个工具**，`default_max_turns(10)`。
- `PREAMBLE` 明确要求：写入必须调工具、跨模块联动、敏感操作停下等确认、**不要相信模型自报的 `user_id` / `tenant_id`**。
- `mod.rs:347-350` `.tool_context(ToolContext::new()).max_turns(10)` 启动流式推理；工具实例在构建时就绑定了 `SessionUser`（`mod.rs:321` 注释）。

### 5. 工具名映射与执行

- `tools.rs:161-188` 宏 `oa_tool!` 为每个工具生成符合 OpenAI 命名规范 `^[a-zA-Z0-9_-]+$` 的 wire name（如 `finance-claim-create`），`call()` 内部再映射回业务名 `finance.claim.create`；对照表 `WIRE_TO_INTERNAL`（`:224-248`）有测试保证可逆且无重复。
- 无论来自 LLM、本地引擎，还是前端表单直调 `/api/tools/call`，最终都进入同一个 `tools::call()`（`tools.rs:660`）。

### 6. 统一入口 call() 的五道校验

按顺序执行（`tools.rs:660-778`）：

| # | 校验 | 代码 | 目的 |
|---|---|---|---|
| 1 | 工具白名单 | `domain::TOOL_NAMES.contains(&name)` | 未注册工具直接拒绝 |
| 2 | RBAC | `user.can(name)` | 角色无权调用即拒绝；**拒绝也写审计** |
| 3 | 本人数据范围 | `tools.rs:677-697` | 普通员工只能给自己打卡/补卡/提报销，前端传别人的 `employee_id` 无效 |
| 4 | 审批节点归属 | `tools.rs:699-735` | 查 `approval_flows` 的当前 pending 节点，非指定审批人（除管理员）连确认弹窗都看不到 |
| 5 | 人工确认门禁 | `tools.rs:737-748` | `SENSITIVE_TOOLS`（7 项，见 `domain.rs:300-309`）未带 `confirmed=true` 时返回 `needs_confirmation: true`，不写库 |

### 7. 事务内写库 + 事务外审计

- `tools.rs:750-763`：所有多步写入包进 `Store::transaction()`；`run_tool_tx()`（`:603-610`）把工具返回的 `ok:false` 转成 `Err(ToolOutcome)`，触发整体回滚。
- 典型例子 —— 报销创建（`tools.rs:1069-1120`）：预算硬拦截 → `next_id` 取号 → 写 `claims` → 占用预算 → 写明细 → 写自定义字段 → 生成审批链 → 写通知，任一步失败全部回滚。
- 典型例子 —— 驳回释放预算（`tools.rs:1182-1204`）：`used_cents = GREATEST(0, used_cents - ?1)`，与创建时按部门优先、`ALL` 兜底的口径一致。
- 自定义字段走 EAV：`write_custom_fields()`（`tools.rs:627-655`）先校验字段已启用且 `entity_type` 匹配，再 `INSERT ... ON CONFLICT DO UPDATE`，防止伪造 `field_id`。
- `tools.rs:775-776`：审计在**事务外**单独提交，确保失败调用也留痕；写入 `audit_events(tool, actor, payload, result)`，`actor` 恒为会话员工号。

### 8. 结果回推与人工确认闭环

- `mod.rs:84-93` 定义 SSE 帧：`Think`（思考步骤）、`Delta`（正文增量）、`Reply`（最终结果）、`Error`。
- LLM 失败时（`mod.rs:212-240`）：先 `audit_since(before)` 查这段时间内是否已有写库；**已写则绝不回退重跑**（避免重复写入），未写才回退本地引擎并标注 `llm-fallback`。
- 前端收到 `pending` → 弹窗 → 带 `confirmed=true` 走 `/api/tools/call` 重试（见第 1 步），形成人在回路闭环。

---

## 三、为什么这么实现

| 设计决策 | 具体做法 | 原因 |
|---|---|---|
| 所有通道共用一个写入口 | `tools::call()` 是 Rig Agent / 本地引擎 / REST / Tauri / 测试的唯一入口（`tools.rs:659`） | 避免“Agent 一套、表单一套”导致校验分叉；任何入口新增的门禁对所有通道同时生效 |
| 身份由宿主注入，不信模型 | 工具 struct 构造时绑定 `SessionUser`；preamble 禁止模型自报身份；actor 取 `user.actor()` | 防止提示注入（prompt injection）让模型冒充管理员操作数据 |
| RBAC 前置且拒绝也审计 | `user.can(name)` 在写库前拦截，拒绝结果也写 `audit_events`（`tools.rs:665-675`） | 非法尝试本身就是安全事件，便于追责；权限矩阵 `auth.rs:64-87` 是 REST / Tauri / Agent 共用的唯一事实来源 |
| 三层数据范围校验 | 角色 → 本人数据 → 当前审批节点（`tools.rs:677-735`） | 不信任前端传参；角色对了也不等于能改别人的单、审别人的节点 |
| 敏感操作人在回路 | 7 类敏感工具必须 `confirmed=true`；本地引擎暂停循环、前端弹窗 | 涨薪、离职、付款、合同签署等不可逆操作需要人确认后再落库 |
| `ok:false` 也回滚 | `run_tool_tx` 将业务失败转成 `Err` | 避免“扣了预算但单据没建成”这类半成品数据 |
| 审计在事务外提交 | `audit()` 独立于业务事务（`tools.rs:775`） | 业务回滚时审计仍保留完整尝试记录，包括失败和拒绝 |
| 失败不自动重跑 | 回退前先查 `audit_since(before)`（`mod.rs:229-231`） | 模型可能在报错前已经写了库；盲目重试 = 重复报销/重复入库 |
| 用审计表重建工具轨迹 | `TraceEntry` 从 `audit_events` 还原（`mod.rs:414-431`） | 审计表是唯一可靠的事实来源，比模型自述的执行过程可信 |
| 本地意图引擎兜底 | `auto` 模式下无密钥自动走本地；LLM 失败且未写库时再兜底 | 功能完整性不依赖外部 LLM 密钥/网络；测试与离线部署也能跑通全流程 |
| 统一 SQL 抽象 | `normalize_sql()` 把 `?N` 重写成 `$N`；`bind_pg()` 把 JSON 绑定成 PG 原生类型；`Repo` 统一 `Store`/`Tx`（`db.rs:1-8,35-43`） | 业务 SQL 只写一份；`Store` 与事务对象对上层是同一个接口，调用点无需区分 |
| 原子取号 | `INSERT ... ON CONFLICT DO NOTHING` + `UPDATE counters SET value=value+1 RETURNING value`（`db.rs:193-224`） | 多实例并发不撞号，替代 `MAX(id)+1` 的竞态写法 |
| 幂等 seed / 迁移 | `CREATE TABLE IF NOT EXISTS`、`ON CONFLICT DO NOTHING`、先探测列再 `ALTER`（`store.rs:379-407`） | 多实例同时启动、存量库升级都不会互相覆盖或建表失败 |
| 金额统一“分” | `*_cents` 字段 + 显示层除 100（`tools.rs:75-76` 注释） | 避免浮点误差和“元/分差 100 倍”的经典 bug |
| 历史只回传纯文本 | `ChatTurn` 只含 role/text，工具 JSON 不回灌（`mod.rs:139-143`） | 防止上下文膨胀和上一轮工具结果被当成新事实；最多 20 轮、每轮 2000 字符 |
| 注入基准日期 | `today_hint()` 把“下周一”等相对日期换算成 `YYYY-MM-DD`（`mod.rs:55-70`） | 模型无系统时钟，不给基准就会反复追问用户已知信息 |
| 流式用 fetch 拆 SSE | 不用 `EventSource` | 需要携带 Bearer 头和 POST body |
| OpenAI 兼容端点显式 `Route::Chat` | `llm.rs:269-272` | DeepSeek/Qwen 等端点不实现 `/v1/responses`，走错路由必失败；本地端点 Key 用 `"EMPTY"` 占位 |

---

## 四、与表单写入通道的关系

表单不经过 Agent 的意图解析和计划生成，但**直接调用同一个 `tools::call()`**（`frontend/src/store.ts:417-449` `submitTool`）。因此：

- 权限、本人数据范围、审批节点归属、敏感确认、事务、审计全部一致；
- 差别只在“谁决定调用哪个工具”：表单由 UI 明确指定，Agent 由意图引擎或模型决定；
- 表单侧若已预确认，后端仍会独立校验 `confirmed` 和所有 RBAC，不信任前端声明。

---

## 五、两个容易误解的点（已与代码对齐）

1. **工具数量是 32 个**：`agent/mod.rs:296-325` 注册 32 个 `.tool()`，`domain.rs` 与 `tools.rs` 的测试断言也是 32；代码注释、README、前后端界面文案均已统一为 32。README 工具表按 6 个域列出全部 32 个，含自定义字段域的 3 个工具、请假与加班各 3 个工具、余额设置的 1 个工具。
2. **没有使用 rig Hook，确认门禁靠三层协作实现**：Rust 代码中**没有实现任何 rig Hook**。真正的门禁由三处协作完成：`PREAMBLE` 指示模型停下（`mod.rs:46-47`）、`tools.rs:737-748` 返回 `needs_confirmation`、`intent.rs:735-742` 暂停循环并置 `pending`，最终由前端 `store.ts:349` 弹窗并带 `confirmed=true` 重试。相关注释、工具消息与界面文案均已按此实现表述，不再出现“AgentHook 拦截转审批”之类的规划性描述。

---

## 六、一次典型写操作的完整时序（示例：员工说“提交一笔 328 元差旅报销”）

1. `AgentWorkbench.vue` → `sendAgent()` → `POST /api/agent/stream`，携带 Bearer Token 与历史。
2. `require_user()` 解析出员工 E08（销售经理）。
3. 无模型配置 → `intent::run_local_stream()`；`plan_for()` 解析出 `finance.claim.create`，金额 32800 分。
4. `tools::call()`：工具在白名单 → E08 有 `finance.claim.create` 权限 → 本人数据校验通过 → 非敏感工具无需确认。
5. `Store::transaction()`：查预算（D04 剩余额度）→ `next_id("claims", "BX-2026", 4)` 原子取号 → 写 `claims` → 更新 `budgets.used_cents` → 写 `claim_items` → 生成审批链 `E10` → 写 `notifications`。
6. 事务提交；事务外写 `audit_events(tool='finance.claim.create', actor='E08', ...)`。
7. SSE 推送 Think / Reply；前端刷新台账并显示“报销单 BX-2026-0049 已创建，当前由 E10 审批”。
