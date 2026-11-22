# 功能完整性盘点与开发计划

> 更新于 2026-11-22。盘点基准：`crates/oa-core/src/domain.rs` 的 `FEATURES` 验收矩阵（33 项）
> + 前后端逐文件核对。状态：`fully` 已实现 / `partial` 部分实现 / `missing` 缺失或存根。

## 结论

- **33 项验收点已全部落地**（P0 正确性与安全、P1 功能补全、P2 一致性与打磨均已完成）。
- 五大业务域（人事/财务/合同/考勤/资产）的台账、写工具、审批状态机、本地意图引擎、审计、
  Postgres 单一后端（2026-11-22 完成 SQLite 移除）、seed 重置、桌面三平台构建都是**真实实现**，没有空壳。
- 工具数增至 **23**（`hr.approve` 打通人事异动审批链，另新增 3 个自定义字段工具）。

---

## 一、未实现 / 部分实现清单

### P0 · 正确性与安全（✅ 已于 2026-11-20 全部修复）

| 编号 | 位置 | 缺口 | 证据 |
|---|---|---|---|
| HR-02 | 离职冻结 | `hr.offboard` 只写 `employees.account_frozen=1`，登录与会话解析只查 `users.active`，**离职后账号仍可登录**，原会话也有效 | `tools.rs:628-631` vs `auth.rs:182,221-224` |
| FIN-02 | 预算拦截 | 预算表存在、创建占用、驳回释放都对，但 **`budget_ok=0` 时不拦截**，照常落库只提示"请人工复核" | `tools.rs:770,824-828` |
| AG-01 | LLM 工具注册 | LLM 模式只 `.tool()` 注册了 **17 个**，漏 `contract.approve` 与 `attendance.approve`，LLM 模式下无法审批合同/补卡 | `agent/mod.rs:296-312` |
| — | 审计接口 | `/api/audit` 只 `require_user` 未 `require_admin`，**任意登录用户可读全量审计日志**（含工具/操作者/入参/结果）；而 `tables(audit_events)` 却 admin-only，策略不一致 | `main.rs:483-496` |
| CT-01 | Postgres 兼容 | ✅ 已修复：日期改由 Rust 侧计算，SQL 方言差异（`SUBSTR(id,-N)`、标量 `MAX`）已纠正；服务端仅接受 Postgres 连接串 | `db.rs`、`tools.rs`、`store.rs` |

### P1 · 声明了但只做了一半（✅ 已于 2026-11-21 全部补全）

| 编号 | 位置 | 缺口 | 证据 |
|---|---|---|---|
| AS-01 | 资产二维码 | 验收点写"二维码"，实际 `qr_code` 列只存 `OA-{id}` 文本，**没有生成/编码/图片**，前端也无任何二维码 UI | `tools.rs:1425,1433`；`AssetView.vue` 详情无 qr 字段 |
| FIN-02/04 | 分部门预算 | 预算只按 `dept_id='ALL'` 占用与统计，seed 里 D02/D04/D08 的分部门预算**从未参与校验或执行率** | `tools.rs:762-789,944-975` |
| — | HR 审批链 | 入职/离职/晋升都写 `approval_flows(entity_type='hr_case')`，但**没有任何工具/路由能审批 hr_case**，`approval_target` 只覆盖 claim/contract/makeup | `tools.rs:496-504`；审批工具缺 `hr.*.approve` |
| AT-02 | 补卡审批 | 验收点写"逐级审批"，实现只有**单级直属主管** | `tools.rs:1231` |
| AT-01 | 打卡状态 | 下班打卡按最后一次卡重算 status，**会覆盖上午的迟到**（09:30 到、18:00 走会记成"正常"），迟到统计失真 | 实测复现；`tools.rs:1155-1197` |
| AG-03 | 离职编排 | 离职意图是单步 `hr.offboard`，回收在工具内部完成，**未按 PREAMBLE 显式编排 asset.return/contract.query 多步轨迹**（入职是三步编排） | `intent.rs:358-368` vs `370-406` |
| HR-02 | 交接通知 | 离职交接通知**只在传了 `handover_to` 时**才写，未指定交接人则无声无息 | `tools.rs:655-657` |

### P2 · 一致性与展示（✅ 已于 2026-11-21 全部打磨）

| 编号 | 位置 | 缺口 | 证据 |
|---|---|---|---|
| PL-01 | 验收矩阵过时 | 矩阵写"REST 与 Tauri 双通道命令对齐"，实际 Tauri 壳**已无任何 invoke 命令、全走 REST**；架构页仍写"Tauri invoke 直调 Rust（零网络开销）" | `src-tauri/lib.rs:3-4`；`model.ts:386`；`zh-CN.ts:282` |
| — | 验收页面缺失 | `GET /api/features` 与前端 `getFeatures()` 是**死代码**，没有页面渲染功能完成度 | `bridge.ts:489-493` 零引用；`ArchView.vue` 仅静态架构图 |
| — | 资产盘点展示 | `inventory()` 调 `asset.query(group_by)` 后**丢弃返回结果**，页面无盘点报表 | `AssetView.vue:112-114,172` |
| — | 趋势假数据 | 工作台趋势图 payroll 柱用 `v * 0.75` 硬算，并非真实人力成本序列 | `DashboardView.vue:182-183` |
| — | 设置页缺失 | 设置只有 `LlmSettingsModal` + 改密弹窗，无独立 `SettingsView` | `App.vue` |
| — | 硬编码残留 | `store.ts` 的 `CURRENT_USER_ID='E01'`/`CURRENT_ACTOR='Evan'` 用于默认展示 | `store.ts:47-48` |
| — | KPI 可见性 | `/api/metrics` 任意登录用户可读全公司 KPI（若属敏感数据需限制角色） | `main.rs:466-472` |

---

## 二、开发计划（建议顺序）

### 阶段 0 · 安全与正确性（先合入，回归风险低）

1. **离职冻结接入认证**：`auth.rs` 会话解析时联合查 `users.active AND employees.account_frozen`，
   冻结即拒绝登录并吊销会话。改动 `auth.rs` 一处 + 补测试。
2. **`/api/audit` 加 `require_admin`**：与 `tables(audit_events)` 对齐。改动 `main.rs` 一行。
3. **预算超额拦截**：`finance.claim.create` 里 `budget_ok=false` 时 `bail`（返回明确原因），
   保留 `budget_ok` 字段语义；`finance.summary` 预算执行率与拦截口径一致。改动 `tools.rs` + 测试。
4. **LLM 补齐 2 个审批工具**：`agent/mod.rs` 补 `.tool(ContractApprove)`、`.tool(AttendanceApprove)`。
5. **合同到期日 Postgres 兼容**：把 `date('now','+1 year')` 换成 Rust 侧算好的日期参数。
   （仓库已有"Rust 侧生成日期"的先例，见 claim 的 `today()` 注释。）

### 阶段 1 · 补全声明但未落地的能力

6. **HR 审批链打通**：新增 `hr.approve`（或 `hr_case` 审批）工具 + REST 路由 + 前端 `HrView` 审批弹窗，
   复用现有 `add_flow`/`approve_flow` 机制；`approval_target` 覆盖 hr_case。
7. **分部门预算**：预算占用/执行率从 `dept_id='ALL'` 改为按 `dept_id` 取实际行（无行则回落到 ALL），
   补 seed 与测试。
8. **资产二维码**：接入 `qrcode` crate（或前端 `qrcode` 库）真正生成可扫描二维码，
   `asset.register` 返回二维码（图片 data URL 或 SVG），`AssetView` 详情展示。
9. **补卡多级审批**：`attendance.makeup` 走 `approval_chain`（按直属主管逐级，与报销同机制）。
10. **打卡状态不丢迟到**：`attendance.checkin` 合并规则改为「迟到优先」（上午迟到 + 正常下班仍记迟到；
    迟到 + 早退并存时记迟到），或拆两个字段，需与业务确认口径。

### 阶段 2 · 一致性与打磨

11. **验收矩阵落地**：新增 `FeaturesView`（或扩展 `ArchView`）渲染 `/api/features`，
    并把 PL-01 等过时描述改为"五端统一 REST"。
12. **资产盘点展示**：`AssetView` 增加盘点结果区域（按类别统计）。
13. **趋势图去假数据**：`/api/metrics` 补真实人力成本序列，或去掉 payroll 柱。
14. **设置页**：把模型设置、改密、地址配置收敛成独立 `SettingsView`。
15. **文案一致性**：清理 `model.ts`/locales 里 "Tauri invoke" 旧描述；评估 `CURRENT_USER_ID` 硬编码。
16. **KPI 可见性**：按需给 `/api/metrics` 加角色限制。

---

## 三、已确认 100% 实现（抽查无缺）

- 入职建档+审批链（HR-01）、晋升异动+涨幅审批（HR-03）、花名册检索（HR-04）、组织树+在编人数（HR-05）
- 三类报销+明细标准校验（FIN-01/05）、审批状态机+付款队列（FIN-03）
- 合同创建/检索/到期预警/两级审批留痕（CT-01/02/03）
- 打卡判定与工时、补卡去重、月度报表（AT-01/03/04）
- 资产登记 SN 唯一、领用自动改派、归还回写、盘点检索（AS-01 的 SN 部分 / AS-02/03/04）
- Agent 双语意图、敏感确认门禁、全量审计、本地引擎覆盖 23 工具（AG-02/04/05/06）
- Postgres 单一后端 + 幂等 seed + 重置（PL-02）、KPI 与审计查询（PL-03）
