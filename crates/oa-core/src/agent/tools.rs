//! Agent 工具集 · 19 个强类型 Tool，JSON Schema 由 TOOL_SPECS 单点生成。
//!
//! 采用 Rig 0.46 的 `Tool` trait（`const NAME` / `Args` / `Output` / `call`）。
//! 每个工具用宏生成轻量 struct，共享 `call()` → `run_sql()` 分发，避免 19 份重复样板。
//!
//! 与 MVP 的区别（全部为真实落库逻辑，非占位返回）：
//! - 事务包裹多步写入，失败整体回滚；
//! - 参数同时兼容前端别名（camelCase / snake_case）与两套金额量纲（元 / 分）；
//! - 预算占用与超额拦截、审批链、付款队列、资产履历、通知、审计全部真实写入；
//! - 敏感操作强制 `confirmed=true`，由 Agent 与前端表单共同遵守。

use crate::auth::SessionUser;
use crate::db::{next_id, Repo};
use crate::params;
use crate::{clock, domain};
use crate::AppState;
use anyhow::{bail, Result};
use rig::prelude::*;
use rig::tool::ToolExecutionError;
use serde_json::{json, Map, Value};

/// (tool name, domain, description, [(参数名, JSON Schema 类型)])
pub const TOOL_SPECS: &[(&str, &str, &str, &[(&str, &str)])] = &[
    ("hr.onboard", "hr", "创建员工档案并启动入职流程（资料→合同→设备→培训）", &[("name", "string"), ("dept_id", "string"), ("position", "string"), ("entry_date", "string"), ("salary_cents", "integer")]),
    ("hr.offboard", "hr", "发起离职流程、冻结账号、自动回收名下资产、生成交接清单", &[("employee_id", "string"), ("last_day", "string"), ("reason", "string"), ("handover_to", "string")]),
    ("hr.promote", "hr", "晋升/涨薪，写入异动记录并按涨幅分派审批链", &[("employee_id", "string"), ("new_salary_cents", "integer"), ("new_position", "string"), ("effective_date", "string"), ("reason", "string")]),
    ("hr.approve", "hr", "人事异动单（入职/离职/晋升）逐级审批：通过转交下一级，末级通过才生效，驳回终止", &[("case_id", "string"), ("decision", "string"), ("comment", "string")]),
    ("hr.roster.query", "hr", "按部门、状态、关键词检索员工花名册（排除已离职）", &[("dept_id", "string"), ("status", "string"), ("keyword", "string")]),
    ("org.chart", "org", "返回组织架构树与各节点实时在编人数", &[]),
    ("finance.claim.create", "finance", "创建日常/商务/项目实施报销单，可带费用明细（发票/发生日期），预算占用校验并生成审批链", &[("type", "string"), ("applicant_id", "string"), ("amount_cents", "integer"), ("purpose", "string"), ("project", "string"), ("items", "array")]),
    ("finance.claim.approve", "finance", "审批通过/驳回（状态机校验），通过后进入付款队列、驳回释放预算", &[("claim_id", "string"), ("decision", "string"), ("comment", "string")]),
    ("finance.summary", "finance", "按类型/部门/月份汇总报销金额与预算执行率", &[("month", "string"), ("dept_id", "string"), ("group_by", "string")]),
    ("contract.create", "contract", "创建合同台账并发起法务/用印审批", &[("name", "string"), ("party", "string"), ("type", "string"), ("amount_cents", "integer"), ("sign_date", "string"), ("expire_date", "string")]),
    ("contract.approve", "contract", "合同法务/用印审批：逐级通过或驳回，末级通过后合同生效", &[("contract_id", "string"), ("decision", "string"), ("comment", "string")]),
    ("contract.query", "contract", "检索合同并输出 N 天内到期预警清单", &[("keyword", "string"), ("status", "string"), ("expiring_in_days", "integer")]),
    ("attendance.checkin", "attendance", "写入上班/下班打卡，判定迟到早退并计算工时", &[("employee_id", "string"), ("type", "string"), ("time", "string"), ("source", "string")]),
    ("attendance.makeup", "attendance", "提交补卡申请（同人同日仅一条待审批），进入主管审批", &[("employee_id", "string"), ("date", "string"), ("reason", "string"), ("clock_in", "string"), ("clock_out", "string")]),
    ("attendance.approve", "attendance", "补卡审批：通过后才补入考勤，驳回不写考勤", &[("request_id", "string"), ("decision", "string"), ("comment", "string")]),
    ("attendance.report", "attendance", "生成月度考勤汇总：出勤率、迟到、缺卡、外勤、请假", &[("month", "string"), ("dept_id", "string")]),
    ("asset.register", "asset", "登记新资产（SN 唯一校验）并生成资产编号与二维码", &[("category", "string"), ("model", "string"), ("serial_no", "string"), ("price_cents", "integer"), ("location", "string")]),
    ("asset.assign", "asset", "将资产领用给员工；目标被占用时自动改派同类在库资产", &[("asset_id", "string"), ("employee_id", "string")]),
    ("asset.return", "asset", "归还资产，状态回写为在库并释放责任人，写入资产履历", &[("asset_id", "string"), ("condition", "string")]),
    ("asset.query", "asset", "资产盘点：按类别/状态/领用人检索并汇总原值", &[("category", "string"), ("status", "string"), ("keyword", "string"), ("holder_id", "string")]),
];

pub fn spec_desc(name: &str) -> String {
    TOOL_SPECS
        .iter()
        .find(|s| s.0 == name)
        .map(|s| s.2.to_string())
        .unwrap_or_else(|| name.to_string())
}

/// 工具 → 业务域（用于回复里的「受影响模块」）。
pub fn spec_domain(name: &str) -> String {
    TOOL_SPECS
        .iter()
        .find(|s| s.0 == name)
        .map(|s| s.1.to_string())
        .unwrap_or_default()
}

const OPTIONAL_FIELDS: &[&str] = &[
    "comment", "project", "time", "source", "keyword", "dept_id", "status", "month",
    "group_by", "expiring_in_days", "expire_date", "sign_date", "effective_date",
    "entry_date", "new_position", "handover_to", "condition", "location", "serial_no",
    "holder_id", "last_day", "reason", "manager_id", "owner_id", "items",
];

/// 金额参数说明：明确单位是「分」，引导模型把「月薪 18000 元」换算成 1800000 分，
/// 而不是把元直接当分写入（薪资 / 金额差 100 倍的根因）。
const PARAM_DESCRIPTIONS: &[(&str, &str)] = &[
    ("salary_cents", "月薪（单位：分，1 元 = 100 分）"),
    ("new_salary_cents", "新月薪（单位：分，1 元 = 100 分）"),
    ("amount_cents", "金额（单位：分，1 元 = 100 分）"),
    ("price_cents", "采购价（单位：分，1 元 = 100 分）"),
    ("items", "费用明细数组，每项含 category 与 amount_cents（分）"),
];

fn param_desc(key: &str) -> &str {
    PARAM_DESCRIPTIONS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, d)| *d)
        .unwrap_or(key)
}

/// 数组参数的元素 Schema：目前只有报销明细一种数组，给出字段与金额单位，
/// 避免模型在明细里放错字段名、或把「元」直接当「分」写。
fn array_item_schema(key: &str) -> Value {
    if key == "items" {
        json!({
            "type": "object",
            "properties": {
                "category": { "type": "string", "description": "费用类型：transport/hotel/meal/office/other" },
                "amount_cents": { "type": "integer", "description": "金额（单位：分，1 元 = 100 分）" },
                "occurred_on": { "type": "string", "description": "发生日期 YYYY-MM-DD（可选）" },
                "invoice_no": { "type": "string", "description": "发票号（可选）" },
                "note": { "type": "string", "description": "备注（超标时必填）" }
            },
            "required": ["category", "amount_cents"]
        })
    } else {
        json!({ "type": "object" })
    }
}

pub fn tool_schema(name: &str) -> Value {
    let spec = TOOL_SPECS.iter().find(|s| s.0 == name);
    let mut props = Map::new();
    let mut required = Vec::new();
    if let Some((_, _, _, fields)) = spec {
        for (key, ty) in *fields {
            // 数组参数要带上 items 描述，否则模型不知道每行该给什么字段
            let prop = if *ty == "array" {
                json!({ "type": "array", "description": param_desc(key), "items": array_item_schema(key) })
            } else {
                json!({ "type": ty, "description": param_desc(key) })
            };
            props.insert((*key).to_string(), prop);
            if !OPTIONAL_FIELDS.contains(key) {
                required.push(*key);
            }
        }
    }
    json!({ "type": "object", "properties": props, "required": required })
}

pub fn all_specs() -> Vec<Value> {
    TOOL_SPECS
        .iter()
        .map(|(n, dom, d, _)| {
            json!({ "name": n, "domain": dom, "description": d, "parameters": tool_schema(n) })
        })
        .collect()
}

/// 按角色裁剪的能力清单：`allowed=false` 的工具前端置灰，后端仍会二次拦截。
pub fn specs_for(user: &SessionUser) -> Vec<Value> {
    TOOL_SPECS
        .iter()
        .map(|(n, dom, d, _)| {
            json!({
                "name": n, "domain": dom, "description": d,
                "parameters": tool_schema(n), "allowed": user.can(n)
            })
        })
        .collect()
}

macro_rules! oa_tool {
    ($ty:ident, $name:literal) => {
        pub struct $ty {
            pub state: AppState,
            pub user: SessionUser,
        }
        impl Tool for $ty {
            const NAME: &'static str = $name;
            type Args = Value;
            type Output = Value;
            type Error = ToolExecutionError;

            fn description(&self) -> String {
                spec_desc($name)
            }
            fn parameters(&self) -> Value {
                tool_schema($name)
            }
            async fn call(
                &self,
                _ctx: &mut ToolContext,
                args: Value,
            ) -> Result<Value, ToolExecutionError> {
                Ok(call(self.state.clone(), &self.user, $name, args).await)
            }
        }
    };
}

oa_tool!(HrOnboard, "hr.onboard");
oa_tool!(HrOffboard, "hr.offboard");
oa_tool!(HrPromote, "hr.promote");
oa_tool!(HrApprove, "hr.approve");
oa_tool!(HrRosterQuery, "hr.roster.query");
oa_tool!(OrgChart, "org.chart");
oa_tool!(ClaimCreate, "finance.claim.create");
oa_tool!(ClaimApprove, "finance.claim.approve");
oa_tool!(FinanceSummary, "finance.summary");
oa_tool!(ContractCreate, "contract.create");
oa_tool!(ContractApprove, "contract.approve");
oa_tool!(ContractQuery, "contract.query");
oa_tool!(AttendanceCheckin, "attendance.checkin");
oa_tool!(AttendanceMakeup, "attendance.makeup");
oa_tool!(AttendanceApprove, "attendance.approve");
oa_tool!(AttendanceReport, "attendance.report");
oa_tool!(AssetRegister, "asset.register");
oa_tool!(AssetAssign, "asset.assign");
oa_tool!(AssetReturn, "asset.return");
oa_tool!(AssetQuery, "asset.query");

/* ---------------- 参数归一化 ---------------- */

/// 依次尝试多个别名，返回第一个非空字符串（兼容前端 aliased() 生成的双份键）。
fn s(a: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(v) = a.get(*k) {
            match v {
                Value::String(x) if !x.is_empty() => return x.clone(),
                Value::Number(x) => return x.to_string(),
                _ => {}
            }
        }
    }
    String::new()
}

fn o(a: &Value, keys: &[&str]) -> Option<String> {
    let v = s(a, keys);
    if v.is_empty() { None } else { Some(v) }
}

fn n(a: &Value, keys: &[&str]) -> i64 {
    for k in keys {
        if let Some(v) = a.get(*k).and_then(Value::as_i64) {
            return v;
        }
        if let Some(v) = a.get(*k).and_then(Value::as_f64) {
            return v as i64;
        }
    }
    0
}

/// 金额：优先读 `<key>_cents`（分），否则读 `<key>`（元）×100。
fn cents(a: &Value, key: &str) -> i64 {
    let cents_key = format!("{key}_cents");
    if a.get(cents_key.as_str()).is_some() {
        return n(a, &[cents_key.as_str()]);
    }
    n(a, &[key]) * 100
}

/// 审批决定归一化：识别各种「驳回 / 拒绝」写法，其余一律视为「通过」。
///
/// LLM 会按工具描述里的中文「驳回」直接传 `decision: "驳回"`，或传 rejected /
/// decline / deny 等变体；若只判 `== "reject"`，这些会被全部误判成「通过」。
fn is_reject(decision: &str) -> bool {
    let d = decision.trim().to_ascii_lowercase();
    matches!(
        d.as_str(),
        "reject" | "rejected" | "decline" | "declined" | "deny" | "denied"
            | "驳回" | "拒绝" | "不通过" | "不同意"
    )
}

fn need(a: &Value, keys: &[&str], label: &str) -> Result<String> {
    let v = s(a, keys);
    if v.is_empty() {
        bail!("缺少必填参数 {label}")
    }
    Ok(v)
}

fn kb(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn year() -> String {
    clock::local_year().to_string()
}

fn today() -> String {
    clock::local_today()
}

fn now_hhmm() -> String {
    clock::local_hhmm()
}

fn month_now() -> String {
    clock::local_month()
}

async fn exists(db: &mut dyn Repo, sql: &str, id: &str) -> Result<bool> {
    Ok(!db.query_json(sql, params![id]).await?.is_empty())
}

async fn dept_of(db: &mut dyn Repo, emp: &str) -> Result<String> {
    Ok(db
        .query_json("SELECT dept_id FROM employees WHERE id=?1", params![emp])
        .await?
        .first()
        .and_then(|r| r["dept_id"].as_str())
        .unwrap_or("D01")
        .to_string())
}

/// 预算按部门优先、`ALL` 兜底：返回 (实际占用的 dept_id, total_cents, used_cents)。
/// 创建占用、驳回释放、汇总执行率三处共用同一口径，避免各写各的。
async fn budget_row(db: &mut dyn Repo, dept: &str) -> Result<(String, i64, i64)> {
    let year = year().parse::<i64>().unwrap_or(2026);
    let pick = |rows: Vec<Value>| {
        rows.first().map(|r| {
            (r["total_cents"].as_i64().unwrap_or(0), r["used_cents"].as_i64().unwrap_or(0))
        })
    };
    if let Some((t, u)) = pick(
        db.query_json("SELECT total_cents, used_cents FROM budgets WHERE year=?1 AND dept_id=?2", params![year, dept])
            .await?,
    ) {
        return Ok((dept.to_string(), t, u));
    }
    if let Some((t, u)) = pick(
        db.query_json("SELECT total_cents, used_cents FROM budgets WHERE year=?1 AND dept_id='ALL'", params![year])
            .await?,
    ) {
        return Ok(("ALL".to_string(), t, u));
    }
    Ok((dept.to_string(), 0, 0))
}

/// 员工号对应的直属上级（`employees.manager_id`）。
async fn manager_of(db: &mut dyn Repo, emp: &str) -> Option<String> {
    db.query_json("SELECT manager_id FROM employees WHERE id=?1", params![emp])
        .await
        .ok()?
        .first()
        .and_then(|r| r["manager_id"].as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// 部门负责人（`departments.leader_id`）。
async fn dept_leader(db: &mut dyn Repo, dept: &str) -> Option<String> {
    db.query_json("SELECT leader_id FROM departments WHERE id=?1", params![dept])
        .await
        .ok()?
        .first()
        .and_then(|r| r["leader_id"].as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
}

/// 角色标签 → 具体审批人（员工号）。按人员指定：
/// 直属主管取部门负责人，缺位或就是本人时沿 `manager_id` 上溯；
/// 部门总监取主管的上级；财务经理、总经理分别取财务部与总经办的负责人。
/// 结果去重并排除申请人本人，避免出现自己审自己。
async fn resolve_approvers(
    db: &mut dyn Repo,
    applicant: &str,
    dept: &str,
    labels: &[&str],
) -> Result<Vec<String>> {
    let supervisor = match dept_leader(db, dept).await.filter(|l| l != applicant) {
        Some(leader) => leader,
        None => manager_of(db, applicant)
            .await
            .filter(|m| m != applicant)
            .unwrap_or_else(|| "E01".to_string()),
    };
    let director = manager_of(db, &supervisor)
        .await
        .unwrap_or_else(|| "E01".to_string());
    let finance = dept_leader(db, "D05").await.unwrap_or_else(|| "E10".to_string());
    let boss = dept_leader(db, "D01").await.unwrap_or_else(|| "E01".to_string());

    let mut out: Vec<String> = Vec::with_capacity(labels.len());
    for label in labels {
        let who = match *label {
            "财务经理" => finance.clone(),
            "部门总监" => director.clone(),
            "总经理" => boss.clone(),
            _ => supervisor.clone(),
        };
        if who != applicant && !out.contains(&who) {
            out.push(who);
        }
    }
    // 申请人本身就是唯一候选时兜底到总经理，否则这张单永远没人能审
    if out.is_empty() {
        out.push(boss);
    }
    Ok(out)
}

/// 人事异动审批链 → 具体员工号（不再用「HRBP 冯雪」这类显示名当审批人）。
/// 入职：直属主管 → HRBP → 行政；离职：直属主管 → HRBP → 行政 → 财务；
/// 晋升由 `raise_chain` 单独决定（见 hr.promote），这里只兜底。
async fn hr_chain(db: &mut dyn Repo, emp: &str, dept: &str, kind: &str) -> Result<Vec<String>> {
    let hrbp = dept_leader(db, "D06").await.unwrap_or_else(|| "E12".to_string());
    let admin = dept_leader(db, "D07").await.unwrap_or_else(|| "E13".to_string());
    let finance = dept_leader(db, "D05").await.unwrap_or_else(|| "E10".to_string());
    let supervisor = resolve_approvers(db, emp, dept, &["直属主管"])
        .await?
        .into_iter()
        .next()
        .unwrap_or_else(|| "E01".to_string());
    let labels: &[&str] = match kind {
        "onboard" => &["直属主管", "HRBP", "行政"],
        "offboard" => &["直属主管", "HRBP", "行政", "财务"],
        _ => &["直属主管", "HRBP"],
    };
    let mut out = Vec::with_capacity(labels.len());
    for label in labels {
        let who = match *label {
            "HRBP" => hrbp.clone(),
            "行政" => admin.clone(),
            "财务" => finance.clone(),
            _ => supervisor.clone(),
        };
        if who != emp && !out.contains(&who) {
            out.push(who);
        }
    }
    if out.is_empty() {
        out.push("E01".to_string());
    }
    Ok(out)
}

/// 解析费用明细数组。金额兼容「元 / 分」两套量纲；非法行直接拒绝，不静默跳过，
/// 否则会出现「总额 3000、明细合计 2800」这种对不上账的单子。
///
/// 同时做**按费用类型的标准校验**（`domain::CLAIM_ITEM_STANDARDS`）：
/// 单条明细超过该类型标准时必须带说明，否则整单退回——超标不是禁止，但必须留痕。
fn parse_claim_items(a: &Value) -> Result<Vec<Value>> {
    let Some(rows) = a
        .get("items")
        .or_else(|| a.get("details"))
        .or_else(|| a.get("claim_items"))
        .and_then(|v| v.as_array())
    else {
        return Ok(Vec::new());
    };

    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let category = s(row, &["category", "type", "kind"]);
        if category.is_empty() {
            bail!(
                "第 {} 条费用明细缺少 category（可选 transport / hotel / meal / office / other）",
                i + 1
            );
        }
        if !domain::CLAIM_ITEM_CATEGORIES.contains(&category.as_str()) {
            bail!(
                "第 {} 条费用明细类别 {category} 不支持（可选 {}）",
                i + 1,
                domain::CLAIM_ITEM_CATEGORIES.join(" / ")
            );
        }
        let amount_cents = cents(row, "amount");
        if amount_cents <= 0 {
            bail!("第 {} 条费用明细金额必须大于 0", i + 1);
        }
        let note = s(row, &["note", "remark"]);
        let standard = domain::item_standard(&category);
        let over = standard.is_some_and(|cap| amount_cents > cap);
        if let (true, Some(cap)) = (over, standard) {
            if note.trim().is_empty() {
                bail!(
                    "第 {} 条明细（{} {:.2} 元）超出费用标准 {:.2} 元，请在备注中说明超标原因后再提交",
                    i + 1,
                    category,
                    amount_cents as f64 / 100.0,
                    cap as f64 / 100.0
                );
            }
        }
        out.push(json!({
            "category": category,
            "amount_cents": amount_cents,
            "occurred_on": o(row, &["occurred_on", "occurredOn", "date"]).unwrap_or_default(),
            "invoice_no": o(row, &["invoice_no", "invoiceNo"]).unwrap_or_default(),
            "note": note,
            "standard_cents": standard.unwrap_or(0),
            "over_standard": over,
        }));
    }
    Ok(out)
}

/// 写审批链。报销与合同存的是具体员工号；人事异动/补卡仍以角色标签占位，
/// 因此这里对两种写法都兼容（`approver` 只是 TEXT，不做外键约束）。
async fn add_flow<T: AsRef<str>>(
    db: &mut dyn Repo,
    entity: &str,
    id: &str,
    approvers: &[T],
) -> Result<()> {
    for (i, approver) in approvers.iter().enumerate() {
        let who = approver.as_ref();
        let fid = format!("AP-{entity}-{id}-{}", i + 1);
        db.execute(
            "INSERT INTO approval_flows(id,entity_type,entity_id,step,approver,status)
             VALUES(?1,?2,?3,?4,?5,'pending') ON CONFLICT (id) DO NOTHING",
            params![fid, entity, id, (i + 1) as i64, who],
        )
        .await?;
    }
    Ok(())
}

async fn notify(db: &mut dyn Repo, to: &str, title: &str, body: &str, kind: &str) -> Result<()> {
    let id = format!("NTF-{}-{}", chrono_stamp(), to);
    db.execute(
        "INSERT INTO notifications(id,recipient_id,title,body,kind) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT (id) DO NOTHING",
        params![id, to, title, body, kind],
    )
    .await?;
    Ok(())
}

/// 无外部依赖的单调递增后缀（同一毫秒内多次写入也不会撞主键）。
fn chrono_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{ms:x}")
}

/// 生成可扫描的二维码（内容 = 资产编号），返回内联 SVG 字符串。
/// 存进 `qr_code` 列，前端详情抽屉直接 `v-html` 渲染，无需额外图片服务。
fn qr_svg(content: &str) -> String {
    use qrcode::render::svg;
    let svg = qrcode::QrCode::new(content.as_bytes())
        .map(|c| c.render::<svg::Color>().build())
        .unwrap_or_default();
    // 去掉 `<?xml ...?>` 声明：前端 v-html 按 HTML 渲染，XML 声明会被当成处理指令
    svg.find("<svg").map(|i| svg[i..].to_string()).unwrap_or(svg)
}

async fn add_event(
    db: &mut dyn Repo,
    asset: &str,
    kind: &str,
    emp: Option<&str>,
    note: &str,
) -> Result<()> {
    let id = format!("AEV-{}-{asset}", chrono_stamp());
    db.execute(
        "INSERT INTO asset_events(id,asset_id,kind,employee_id,note) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT (id) DO NOTHING",
        params![id, asset, kind, emp, note],
    )
    .await?;
    Ok(())
}

/// 事务内的工具执行：业务返回 `ok:false` 以 Err 形式抛出，驱动事务回滚。
async fn run_tool_tx(tx: &mut dyn Repo, name: String, args: Value) -> Result<Value> {
    let out = run_sql(&name, &args, tx).await?;
    let ok = out.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        Ok(out)
    } else {
        Err(anyhow::Error::new(ToolOutcome(out)))
    }
}

/// 业务结果为 `ok:false` 时用它承载结果并触发回滚（`anyhow::Error` 需要 Error 类型）。
#[derive(Debug)]
struct ToolOutcome(Value);

impl std::fmt::Display for ToolOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tool reported failure")
    }
}

impl std::error::Error for ToolOutcome {}

/* ---------------- 统一调用入口 ---------------- */

/// 所有通道（Rig Agent / 本地意图引擎 / REST / Tauri / 测试）共用的唯一写入口。
pub async fn call(state: AppState, user: &SessionUser, name: &str, args: Value) -> Value {
    if !domain::TOOL_NAMES.contains(&name) {
        return json!({ "ok": false, "error": format!("unknown tool: {name}"), "msg": format!("未知工具：{name}") });
    }

    // RBAC 在写库之前拦截；被拒绝同样留审计，便于追责与安全审计。
    if !user.can(name) {
        let out = json!({
            "ok": false,
            "denied": true,
            "error": format!("角色 {} 无权调用 {name}", user.role),
            "msg": format!("权限不足：{} 角色不能执行 {}，请切换账号或提交审批", user.role, name)
        });
        let _ = state.store.audit(name, user.actor(), &args, &out).await;
        return out;
    }

    // 自助类工具的数据范围：普通员工只能以本人身份打卡、补卡、提报销。
    if user.role == "employee"
        && matches!(name, "attendance.checkin" | "attendance.makeup" | "finance.claim.create")
    {
        let keys: &[&str] = if name == "finance.claim.create" {
            &["applicant_id", "applicantId"]
        } else {
            &["employee_id", "employeeId"]
        };
        let target = s(&args, keys);
        if !target.is_empty() && target != user.employee_id {
            let out = json!({
                "ok": false,
                "denied": true,
                "error": format!("员工只能操作本人数据：{name} 目标 {target}"),
                "msg": "普通员工只能为本人打卡 / 补卡 / 提交报销，请改为本人后再操作"
            });
            let _ = state.store.audit(name, user.actor(), &args, &out).await;
            return out;
        }
    }

    // 多级审批：只有当前节点的指定审批人（管理员可代审）能操作，其他人连确认弹窗都不该看到
    let approval_target = match name {
        "finance.claim.approve" => Some(("claim", s(&args, &["claim_id", "claimId"]))),
        "contract.approve" => Some(("contract", s(&args, &["contract_id", "contractId"]))),
        "attendance.approve" => Some((
            "makeup",
            s(&args, &["request_id", "requestId", "makeup_id", "makeupId"]),
        )),
        "hr.approve" => Some(("hr_case", s(&args, &["case_id", "caseId"]))),
        _ => None,
    };
    if let Some((entity, id)) = approval_target {
        if !id.is_empty() {
            let store = state.store.clone();
            let node = store
                .query_json(
                    "SELECT approver FROM approval_flows
                     WHERE entity_type=?1 AND entity_id=?2 AND status='pending'
                     ORDER BY step ASC LIMIT 1",
                    params![entity, id],
                )
                .await
                .unwrap_or_default();
            if let Some(approver) = node.first().and_then(|r| r["approver"].as_str()) {
                if approver != user.employee_id && !user.is_admin() {
                    let out = json!({
                        "ok": false,
                        "denied": true,
                        "error": format!("{id} 当前审批节点是 {approver}，{} 无权审批", user.employee_id),
                        "msg": format!("该单当前应由 {approver} 审批，你不在这一级")
                    });
                    let _ = state.store.audit(name, user.actor(), &args, &out).await;
                    return out;
                }
            }
        }
    }

    // 人在回路：涨薪、离职、付款审批、合同签署必须人工确认。
    if domain::SENSITIVE_TOOLS.contains(&name) && !kb(&args, "confirmed") {
        let out = json!({
            "ok": false,
            "needs_confirmation": true,
            "tool": name,
            "message": "敏感操作需人工确认：请追加 confirmed=true 后重试（生产由 AgentHook 拦截转审批）",
            "msg": "敏感操作需人工确认"
        });
        let _ = state.store.audit(name, user.actor(), &args, &out).await;
        return out;
    }

    // 多步写入包在事务里：业务返回 ok:false 也回滚，避免半成品数据。
    let name_owned = name.to_string();
    let args_for_sql = args.clone();
    let result = state
        .store
        .transaction(move |tx| Box::pin(run_tool_tx(tx, name_owned, args_for_sql)))
        .await;
    let mut out = match result {
        Ok(v) => v,
        Err(e) => match e.downcast::<ToolOutcome>() {
            Ok(t) => t.0,
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        },
    };
    // 前端统一读 `msg` 提示；这里补齐别名，避免每个工具都手写两份。
    if let Value::Object(ref mut map) = out {
        let alias = map
            .get("msg")
            .cloned()
            .or_else(|| map.get("message").cloned())
            .or_else(|| map.get("error").cloned());
        if let Some(m) = alias {
            map.entry("msg".to_string()).or_insert(m);
        }
    }
    // 审计在事务外单独提交，确保失败调用也留痕。
    let _ = state.store.audit(name, user.actor(), &args, &out).await;
    out
}

/* ---------------- 19 个工具的真实业务实现 ---------------- */

async fn run_sql(name: &str, a: &Value, db: &mut dyn Repo) -> Result<Value> {
    match name {
        "hr.onboard" => {
            let person = need(a, &["name"], "name")?;
            let dept = o(a, &["dept_id", "deptId"]).unwrap_or_else(|| "D02".into());
            let position = o(a, &["position"]).unwrap_or_else(|| "员工".into());
            let salary = cents(a, "salary");
            let entry = o(a, &["entry_date", "entryDate"]).unwrap_or_else(|| "".into());
            // 默认入职日用 Rust 侧算：date('now') 是 SQLite 专有函数，Postgres 下会报错
            let entry = if entry.is_empty() { today() } else { entry };
            let manager = o(a, &["manager_id", "managerId"]);

            if !exists(db, "SELECT 1 FROM departments WHERE id=?1", &dept).await? {
                bail!("部门 {dept} 不存在");
            }
            let next = db.scalar_i64(
                "SELECT COALESCE(MAX(CAST(SUBSTR(id,2) AS INTEGER)),0)+1 FROM employees WHERE id LIKE 'E%'",
                params![],
            ).await?;
            let id = format!("E{next:02}");
            let no = format!("EMP-{}", 1000 + next);
            let email = format!("{}@yugong.com", format!("emp{next}").to_lowercase());
            db.execute(
                "INSERT INTO employees(id,no,name,dept_id,position,status,entry_date,salary_cents,manager_id,email)
                 VALUES(?1,?2,?3,?4,?5,'probation',?6,?7,?8,?9)",
                params![id, no, person, dept, position, entry, salary, manager, email],
            ).await?;

            let case_id = next_id(db, "hr_cases", &format!("HR-{}", year()), 3).await?;
            let steps = "offer → 资料收集 → 合同签署 → 设备领用 → 入职培训";
            db.execute(
                "INSERT INTO hr_cases(id,kind,employee_id,effective_date,reason,status,step_index,steps)
                 VALUES(?1,'onboard',?2,?3,?4,'running',1,?5)",
                params![case_id, id, entry, s(a, &["reason"]), steps],
            ).await?;
            let onboard_chain = hr_chain(db, &id, &dept, "onboard").await?;
            add_flow(db, "hr_case", &case_id, &onboard_chain).await?;
            let first = onboard_chain.first().cloned().unwrap_or_else(|| "E12".to_string());
            notify(db, &first, "入职流程待办", &format!("{person} 的合同签署与设备领用待确认"), "hr").await?;
            Ok(json!({
                "ok": true, "employee_id": id, "employee_no": no, "case_id": case_id,
                "salary_cents": salary, "steps": steps,
                "message": format!("已创建员工 {person}（{no}），入职流程 {case_id} 已启动")
            }))
        }

        "hr.offboard" => {
            let emp = need(a, &["employee_id", "employeeId"], "employee_id")?;
            if !exists(db, "SELECT 1 FROM employees WHERE id=?1", &emp).await? {
                bail!("员工 {emp} 不存在");
            }
            let last_day = o(a, &["last_day", "lastDay"]).unwrap_or_else(|| today());
            let reason = o(a, &["reason"]).unwrap_or_else(|| "—".into());
            let handover = o(a, &["handover_to"]);

            db.execute(
                "UPDATE employees SET status='leaving', account_frozen=1 WHERE id=?1",
                params![emp],
            ).await?;

            // 自动回收名下全部资产，并逐条写入资产履历。
            let held = db.query_json(
                "SELECT id, model FROM assets WHERE holder_id=?1",
                params![emp],
            ).await?;
            for r in &held {
                let aid = r["id"].as_str().unwrap_or("");
                db.execute(
                    "UPDATE assets SET status='instock', holder_id=NULL WHERE id=?1",
                    params![aid],
                ).await?;
                add_event(db, aid, "return", Some(&emp), "离职自动回收").await?;
            }

            let case_id = next_id(db, "hr_cases", &format!("HR-{}", year()), 3).await?;
            let steps = "申请 → 工作交接 → 资产归还 → 权限回收 → 薪资结算 → 归档";
            db.execute(
                "INSERT INTO hr_cases(id,kind,employee_id,effective_date,reason,status,step_index,steps)
                 VALUES(?1,'offboard',?2,?3,?4,'running',1,?5)",
                params![case_id, emp, last_day, reason, steps],
            ).await?;
            let dept = dept_of(db, &emp).await?;
            let offboard_chain = hr_chain(db, &emp, &dept, "offboard").await?;
            add_flow(db, "hr_case", &case_id, &offboard_chain).await?;
            // 未指定交接人时通知到直属主管，保证离职交接不会无声无息
            let handover_to = handover
                .or_else(|| offboard_chain.first().cloned())
                .unwrap_or_else(|| "E12".to_string());
            notify(db, &handover_to, "离职交接待办", &format!("请与 {emp} 完成工作交接"), "hr").await?;
            Ok(json!({
                "ok": true, "case_id": case_id, "account_frozen": true,
                "assets_returned": held.len(), "steps": steps,
                "message": format!("离职流程 {case_id} 已启动，账号已冻结，自动回收 {0} 项资产", held.len())
            }))
        }

        "hr.promote" => {
            let emp = need(a, &["employee_id", "employeeId"], "employee_id")?;
            let new_salary = cents(a, "new_salary");
            if new_salary <= 0 {
                bail!("new_salary 必须大于 0");
            }
            let old = db
                .query_json("SELECT salary_cents, name FROM employees WHERE id=?1", params![emp])
                .await?
                .first()
                .map(|r| (r["salary_cents"].as_i64().unwrap_or(0), r["name"].as_str().unwrap_or("").to_string()))
                .ok_or_else(|| anyhow::anyhow!("员工 {emp} 不存在"))?;
            let new_position = o(a, &["new_position", "newPosition"]);
            let effective = o(a, &["effective_date", "effectiveDate"]).unwrap_or_else(|| today());
            db.execute(
                "UPDATE employees SET salary_cents=?2, position=COALESCE(NULLIF(?3,''),position) WHERE id=?1",
                params![emp, new_salary, new_position],
            ).await?;
            let chain = domain::raise_chain(old.0, new_salary);
            let case_id = next_id(db, "hr_cases", &format!("HR-{}", year()), 3).await?;
            db.execute(
                "INSERT INTO hr_cases(id,kind,employee_id,effective_date,reason,status,step_index,steps)
                 VALUES(?1,'promote',?2,?3,?4,'running',2,?5)",
                params![case_id, emp, effective, s(a, &["reason"]), chain.join(" → ")],
            ).await?;
            // 把「直属主管 → 总经理 …」这类角色标签解析成具体员工号（去掉末尾的「生效」）
            let dept = dept_of(db, &emp).await?;
            let labels: Vec<&str> = chain.iter().filter(|l| **l != "生效").map(|l| *l).collect();
            let approvers = resolve_approvers(db, &emp, &dept, &labels).await?;
            add_flow(db, "hr_case", &case_id, &approvers).await?;
            Ok(json!({
                "ok": true, "case_id": case_id, "employee_name": old.1,
                "old_salary_cents": old.0, "new_salary_cents": new_salary,
                "delta_cents": new_salary - old.0, "chain": chain,
                "message": format!("{} 月薪 {:.2} → {:.2} 元，异动单 {} 进入审批",
                    old.1, old.0 as f64 / 100.0, new_salary as f64 / 100.0, case_id)
            }))
        }

        "hr.approve" => {
            let id = need(a, &["case_id", "caseId"], "case_id")?;
            let decision = o(a, &["decision"]).unwrap_or_else(|| "approve".into());
            let row = db
                .query_json("SELECT kind, employee_id, status FROM hr_cases WHERE id=?1", params![id])
                .await?
                .first()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("异动单 {id} 不存在"))?;
            let cur = row["status"].as_str().unwrap_or("");
            if cur != "running" {
                bail!("异动单 {id} 当前状态 {cur}，只有进行中的可审批");
            }
            let flow = db
                .query_json(
                    "SELECT id, step, approver FROM approval_flows
                     WHERE entity_type='hr_case' AND entity_id=?1 AND status='pending'
                     ORDER BY step ASC LIMIT 1",
                    params![id],
                )
                .await?;
            let Some(node) = flow.first() else {
                bail!("{id} 没有待审批节点，可能已被处理");
            };
            let flow_id = node["id"].as_str().unwrap_or_default().to_string();
            let step = node["step"].as_i64().unwrap_or(1);
            let approver = node["approver"].as_str().unwrap_or_default().to_string();
            let stamp = clock::local_datetime();
            let kind = row["kind"].as_str().unwrap_or("").to_string();
            let emp = row["employee_id"].as_str().unwrap_or("").to_string();

            if is_reject(&decision) {
                db.execute(
                    "UPDATE approval_flows SET status='rejected', comment=?2, acted_at=?3 WHERE id=?1",
                    params![flow_id, s(a, &["comment"]), stamp],
                )
                .await?;
                db.execute("UPDATE hr_cases SET status='rejected' WHERE id=?1", params![id]).await?;
                return Ok(json!({
                    "ok": true, "case_id": id, "status": "rejected", "step": step, "approver": approver,
                    "message": format!("异动单 {id} 已被第 {step} 级审批人驳回，流程终止")
                }));
            }

            db.execute(
                "UPDATE approval_flows SET status='done', comment=?2, acted_at=?3 WHERE id=?1",
                params![flow_id, s(a, &["comment"]), stamp],
            )
            .await?;

            let next = db
                .query_json(
                    "SELECT approver, step FROM approval_flows
                     WHERE entity_type='hr_case' AND entity_id=?1 AND status='pending'
                     ORDER BY step ASC LIMIT 1",
                    params![id],
                )
                .await?;
            if let Some(n) = next.first() {
                let next_approver = n["approver"].as_str().unwrap_or_default().to_string();
                let next_step = n["step"].as_i64().unwrap_or(step + 1);
                notify(
                    db,
                    &next_approver,
                    "人事异动待审批",
                    &format!("{id} 已通过第 {step} 级，待你审批"),
                    "hr",
                )
                .await?;
                return Ok(json!({
                    "ok": true, "case_id": id, "status": "running",
                    "step": step, "approver": approver,
                    "next_approver": next_approver, "next_step": next_step,
                    "message": format!("{id} 第 {step} 级审批已通过，转交第 {next_step} 级（{next_approver}）")
                }));
            }

            // 末级通过：异动单正式生效，离职才把状态从 leaving 落成 left
            db.execute("UPDATE hr_cases SET status='approved' WHERE id=?1", params![id]).await?;
            if kind == "offboard" {
                db.execute("UPDATE employees SET status='left' WHERE id=?1", params![emp]).await?;
            }
            Ok(json!({
                "ok": true, "case_id": id, "status": "approved",
                "step": step, "approver": approver, "kind": kind,
                "message": format!("异动单 {id} 全部审批完成")
            }))
        }

        "hr.roster.query" => {
            let dept = o(a, &["dept_id", "deptId"]);
            let status = o(a, &["status"]);
            let kw = o(a, &["keyword"]);
            // 未显式指定状态时排除已离职员工；指定状态时以该状态为准。
            let exclude_left: Option<&str> = if status.is_some() { None } else { Some("1") };
            let rows = db.query_json(
                "SELECT e.*, d.name AS dept_name FROM employees e
                 LEFT JOIN departments d ON d.id=e.dept_id
                 WHERE (?1 IS NULL OR e.status <> 'left')
                   AND (?2 IS NULL OR e.dept_id=?2)
                   AND (?3 IS NULL OR e.status=?3)
                   AND (?4 IS NULL OR e.name LIKE '%'||?4||'%' OR e.no LIKE '%'||?4||'%' OR e.position LIKE '%'||?4||'%')
                 ORDER BY e.id LIMIT 200",
                params![exclude_left, dept, status, kw],
            ).await?;
            Ok(json!({ "ok": true, "count": rows.len(), "rows": rows }))
        }

        "org.chart" => {
            let rows = db.query_json(
                "SELECT d.id, d.name, d.parent_id, d.leader_id, l.name AS leader_name,
                        (SELECT COUNT(*) FROM employees e WHERE e.dept_id=d.id AND e.status <> 'left') AS headcount
                 FROM departments d LEFT JOIN employees l ON l.id=d.leader_id ORDER BY d.id",
                params![],
            ).await?;
            let total = db.scalar_i64(
                "SELECT COUNT(*) FROM employees WHERE status <> 'left'",
                params![],
            ).await?;
            Ok(json!({
                "ok": true, "rows": rows, "total_departments": rows.len(),
                "total_headcount": total, "root": "总经办"
            }))
        }

        "finance.claim.create" => {
            let kind = domain::ClaimType::parse(&s(a, &["type", "kind"]))
                .ok_or_else(|| anyhow::anyhow!("type 必须是 daily/business/project"))?
                .as_str()
                .to_string();
            let applicant = need(a, &["applicant_id", "applicantId"], "applicant_id")?;
            if !exists(db, "SELECT 1 FROM employees WHERE id=?1", &applicant).await? {
                bail!("申请人 {applicant} 不存在");
            }
            // 明细优先：带明细时金额由明细合计决定，避免「总额与明细对不上」的脏数据
            let items = parse_claim_items(a)?;
            let declared = cents(a, "amount");
            let amount: i64 = if items.is_empty() {
                declared
            } else {
                items.iter().map(|it| it["amount_cents"].as_i64().unwrap_or(0)).sum()
            };
            if amount <= 0 {
                bail!("amount 必须大于 0");
            }
            let amount_mismatch = !items.is_empty() && declared > 0 && declared != amount;
            let dept = o(a, &["dept_id", "deptId"]).unwrap_or_else(|| "D01".into());
            let dept = if dept.is_empty() { dept_of(db, &applicant).await? } else { dept };
            let purpose = o(a, &["purpose"]).unwrap_or_else(|| "—".into());
            let project = o(a, &["project"]);

            // 预算按部门优先、ALL 兜底；配了预算就硬拦截：超额直接拒绝（事务回滚，不落库）
            let (budget_key, total, used) = budget_row(db, &dept).await?;
            let remaining = total - used;
            let has_budget = total > 0;
            let budget_ok = !has_budget || amount <= remaining;
            if !budget_ok {
                bail!(
                    "预算校验未通过：本年剩余额度 {:.2} 元，本次报销 {:.2} 元，超出 {:.2} 元，请削减金额或走人工特批",
                    remaining as f64 / 100.0,
                    amount as f64 / 100.0,
                    (amount - remaining) as f64 / 100.0,
                );
            }

            // 按人员指定审批人：角色标签先由金额/类型决定，再解析成具体员工号
            let labels = domain::approval_chain(amount, &kind);
            let approvers = resolve_approvers(db, &applicant, &dept, &labels).await?;
            let first = approvers.first().cloned().unwrap_or_else(|| "E01".to_string());

            let id = next_id(db, "claims", &format!("BX-{}", year()), 4).await?;
            db.execute(
                "INSERT INTO claims(id,kind,applicant_id,dept_id,amount_cents,purpose,project,status,submitted_at,approver_id,budget_ok)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,'pending',?8,?9,?10)",
                // 日期用 Rust 侧生成：SQLite 的 date('now') 在 Postgres 里不存在
                params![id, kind, applicant, dept, amount, purpose, project, today(), first, budget_ok as i64],
            ).await?;
            if budget_ok {
                db.execute(
                    "UPDATE budgets SET used_cents=used_cents+?1 WHERE year=?2 AND dept_id=?3",
                    params![amount, year().parse::<i64>().unwrap_or(2026), budget_key],
                ).await?;
            }
            // 费用明细：一条一行，id 用单号做前缀保证可读且唯一
            for (i, it) in items.iter().enumerate() {
                db.execute(
                    "INSERT INTO claim_items(id,claim_id,category,amount_cents,occurred_on,invoice_no,note)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        format!("{id}-{:02}", i + 1),
                        id,
                        it["category"].as_str().unwrap_or("other"),
                        it["amount_cents"].as_i64().unwrap_or(0),
                        it["occurred_on"].as_str().unwrap_or(""),
                        it["invoice_no"].as_str().unwrap_or(""),
                        it["note"].as_str().unwrap_or("")
                    ],
                )
                .await?;
            }
            add_flow(db, "claim", &id, &approvers).await?;
            notify(db, &first, "报销待审批", &format!("{id} 金额 {:.2} 元，待你审批", amount as f64 / 100.0), "approval").await?;
            // 超标明细单独回给前端/审批人，便于在详情里高亮；超标但已备注的照常入库
            let over_items: Vec<Value> = items
                .iter()
                .filter(|it| it["over_standard"].as_bool().unwrap_or(false))
                .cloned()
                .collect();
            Ok(json!({
                "ok": true, "claim_id": id, "kind": kind, "amount_cents": amount,
                "budget_ok": budget_ok, "budget_total_cents": total,
                "budget_used_cents": if budget_ok { used + amount } else { used },
                "budget_remaining_cents": total - used,
                "chain": labels, "approvers": approvers, "current_approver": first,
                "current_step": 1, "total_steps": labels.len(),
                "items_count": items.len(), "items": items, "items_amount_mismatch": amount_mismatch,
                "over_standard_count": over_items.len(), "over_standard_items": over_items,
                "message": format!("报销单 {id} 已创建（{:.2} 元{}），预算校验{}，审批链：{}（共 {} 级，当前由 {first} 审批）",
                    amount as f64 / 100.0,
                    if items.is_empty() { String::new() } else { format!("，{} 条明细", items.len()) },
                    "通过",
                    labels.join(" → "), labels.len())
            }))
        }

        "finance.claim.approve" => {
            let id = need(a, &["claim_id", "claimId"], "claim_id")?;
            let decision = o(a, &["decision"]).unwrap_or_else(|| "approve".into());
            let row = db
                .query_json(
                    "SELECT status, amount_cents, budget_ok, dept_id FROM claims WHERE id=?1",
                    params![id],
                )
                .await?
                .first()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("报销单 {id} 不存在"))?;
            let cur = row["status"].as_str().unwrap_or("draft");
            let from = domain::ClaimStatus::parse(cur)
                .ok_or_else(|| anyhow::anyhow!("未知状态 {cur}"))?;
            if from != domain::ClaimStatus::Pending {
                bail!("只有审批中的报销单可以审批，{id} 当前状态 {cur}");
            }

            // 当前待审节点 = step 最小的 pending 节点。多级审批逐级推进，一次只关一个节点。
            let flow = db
                .query_json(
                    "SELECT id, step, approver FROM approval_flows
                     WHERE entity_type='claim' AND entity_id=?1 AND status='pending'
                     ORDER BY step ASC LIMIT 1",
                    params![id],
                )
                .await?;
            let Some(node) = flow.first() else {
                bail!("{id} 没有待审批节点，可能已被处理");
            };
            let flow_id = node["id"].as_str().unwrap_or_default().to_string();
            let step = node["step"].as_i64().unwrap_or(1);
            let approver = node["approver"].as_str().unwrap_or_default().to_string();
            let stamp = clock::local_datetime();
            let amount_cents = row["amount_cents"].as_i64().unwrap_or(0);

            if is_reject(&decision) {
                db.execute(
                    "UPDATE approval_flows SET status='rejected', comment=?2, acted_at=?3 WHERE id=?1",
                    params![flow_id, s(a, &["comment"]), stamp],
                )
                .await?;
                db.execute("UPDATE claims SET status='rejected' WHERE id=?1", params![id]).await?;
                if row["budget_ok"].as_i64().unwrap_or(0) == 1 {
                    // 释放到创建时占用的那个预算行（部门优先、ALL 兜底），口径与创建一致
                    let dept_id = row["dept_id"].as_str().unwrap_or("ALL");
                    let (key, _, _) = budget_row(db, dept_id).await?;
                    db.execute(
                        "UPDATE budgets SET used_cents=MAX(0, used_cents-?1) WHERE year=?2 AND dept_id=?3",
                        params![amount_cents, year().parse::<i64>().unwrap_or(2026), key],
                    )
                    .await?;
                }
                return Ok(json!({
                    "ok": true, "claim_id": id, "status": "rejected",
                    "step": step, "approver": approver, "budget_released": true,
                    "message": format!("{id} 已被第 {step} 级审批人驳回，金额 {:.2} 元，已释放预算占用",
                        amount_cents as f64 / 100.0)
                }));
            }

            // 通过：只关掉当前节点，是否终结由「还有没有 pending 节点」决定
            db.execute(
                "UPDATE approval_flows SET status='done', comment=?2, acted_at=?3 WHERE id=?1",
                params![flow_id, s(a, &["comment"]), stamp],
            )
            .await?;

            let next = db
                .query_json(
                    "SELECT approver, step FROM approval_flows
                     WHERE entity_type='claim' AND entity_id=?1 AND status='pending'
                     ORDER BY step ASC LIMIT 1",
                    params![id],
                )
                .await?;

            if let Some(n) = next.first() {
                let next_approver = n["approver"].as_str().unwrap_or_default().to_string();
                let next_step = n["step"].as_i64().unwrap_or(step + 1);
                db.execute("UPDATE claims SET approver_id=?2 WHERE id=?1", params![id, next_approver])
                    .await?;
                notify(
                    db,
                    &next_approver,
                    "报销待审批",
                    &format!("{id} 已通过第 {step} 级，待你审批"),
                    "approval",
                )
                .await?;
                return Ok(json!({
                    "ok": true, "claim_id": id, "status": "pending",
                    "step": step, "approver": approver,
                    "next_approver": next_approver, "next_step": next_step,
                    "message": format!("{id} 第 {step} 级审批已通过，转交第 {next_step} 级（{next_approver}）继续审批")
                }));
            }

            db.execute("UPDATE claims SET status='approved' WHERE id=?1", params![id]).await?;
            let payment_id = next_id(db, "payments", "PAY", 4).await?;
            db.execute(
                "INSERT INTO payments(id,claim_id,amount_cents,status,scheduled_at)
                 VALUES(?1,?2,?3,'queued',?4)",
                params![payment_id, id, amount_cents, clock::date_offset(2)],
            )
            .await?;
            Ok(json!({
                "ok": true, "claim_id": id, "status": "approved",
                "payment_id": payment_id, "step": step,
                "message": format!("{id} 全部 {step} 级审批已完成，金额 {:.2} 元，已进入付款队列",
                    amount_cents as f64 / 100.0)
            }))
        }

        "finance.summary" => {
            let month = o(a, &["month"]).unwrap_or_else(|| month_now());
            let month = if month.is_empty() { month_now() } else { month };
            let by_kind = db.query_json(
                "SELECT kind, COUNT(*) AS cnt, SUM(amount_cents) AS total FROM claims
                 WHERE substr(submitted_at,1,7)=?1 GROUP BY kind",
                params![month],
            ).await?;
            let by_dept = db.query_json(
                "SELECT c.dept_id, d.name AS dept_name, COUNT(*) AS cnt, SUM(c.amount_cents) AS total
                 FROM claims c LEFT JOIN departments d ON d.id=c.dept_id
                 WHERE substr(c.submitted_at,1,7)=?1 GROUP BY c.dept_id",
                params![month],
            ).await?;
            let total = by_kind.iter().map(|r| r["total"].as_i64().unwrap_or(0)).sum::<i64>();
            // 预算执行率按传入部门算（没传就看全公司 ALL）
            let budget_dept = o(a, &["dept_id", "deptId"]).unwrap_or_else(|| "ALL".into());
            let (_, bt, bu) = budget_row(db, &budget_dept).await?;
            Ok(json!({
                "ok": true, "month": month, "total_cents": total,
                "by_kind": by_kind, "by_dept": by_dept,
                "budget_total_cents": bt, "budget_used_cents": bu,
                "budget_rate": if bt > 0 { (bu as f64 / bt as f64 * 10000.0).round() / 100.0 } else { 0.0 },
                "message": format!("{} 报销合计 {:.2} 元；预算执行率 {:.1}%",
                    month, total as f64 / 100.0,
                    if bt > 0 { bu as f64 / bt as f64 * 100.0 } else { 0.0 })
            }))
        }

        "contract.create" => {
            let name = need(a, &["name"], "name")?;
            let party = need(a, &["party"], "party")?;
            let kind = o(a, &["type", "kind"]).unwrap_or_else(|| "service".into());
            let amount = cents(a, "amount");
            let sign = o(a, &["sign_date", "signDate"]).unwrap_or_else(|| today());
            let sign = if sign.is_empty() { today() } else { sign };
            let expire = o(a, &["expire_date", "expireDate"]).unwrap_or_default();
            // 默认到期日放到 Rust 侧算：SQLite 的 date('now','+1 year') 在 Postgres 里不存在
            let expire = if expire.is_empty() { clock::date_offset(365) } else { expire };
            let owner = o(a, &["owner_id", "ownerId"]).unwrap_or_else(|| "E01".into());

            // 法务/用印没有独立岗位：条款与金额由财务经理审，用印由总经理签批。
            // 与报销一致，链上写具体员工号，并排除合同负责人自己审自己。
            let legal = dept_leader(db, "D05").await.unwrap_or_else(|| "E10".to_string());
            let seal = dept_leader(db, "D01").await.unwrap_or_else(|| "E01".to_string());
            let mut approvers: Vec<String> = Vec::with_capacity(2);
            for who in [legal, seal.clone()] {
                if who != owner && !approvers.contains(&who) {
                    approvers.push(who);
                }
            }
            // 负责人恰好占满两级候选时兜底到总经理，否则这张单没人能审
            if approvers.is_empty() {
                approvers.push(seal);
            }

            let id = next_id(db, "contracts", &format!("HT-{}", year()), 3).await?;
            db.execute(
                "INSERT INTO contracts(id,name,party,kind,amount_cents,sign_date,expire_date,status,owner_id)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,'review',?8)",
                params![id, name, party, kind, amount, sign, expire, owner],
            ).await?;
            add_flow(db, "contract", &id, &approvers).await?;
            let first = approvers.first().cloned().unwrap_or_else(|| "E01".to_string());
            notify(db, &first, "合同待审批", &format!("{id}「{name}」等待法务/用印审批"), "contract").await?;
            Ok(json!({
                "ok": true, "contract_id": id, "amount_cents": amount, "owner_id": owner,
                "approvers": approvers, "current_approver": first, "total_steps": approvers.len(),
                "message": format!("合同 {id}「{name}」已创建，进入法务/用印审批（{} 级，当前 {first}）",
                    approvers.len())
            }))
        }

        "contract.approve" => {
            let id = need(a, &["contract_id", "contractId"], "contract_id")?;
            let decision = o(a, &["decision"]).unwrap_or_else(|| "approve".into());
            let row = db
                .query_json(
                    "SELECT name, status, owner_id FROM contracts WHERE id=?1",
                    params![id],
                )
                .await?
                .first()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("合同 {id} 不存在"))?;
            let cur = row["status"].as_str().unwrap_or("");
            if cur != "review" {
                bail!("只有审批中的合同可以审批，{id} 当前状态 {cur}");
            }

            // 与报销同一套规则：待审节点 = step 最小的 pending 节点，一次只关一个
            let flow = db
                .query_json(
                    "SELECT id, step, approver FROM approval_flows
                     WHERE entity_type='contract' AND entity_id=?1 AND status='pending'
                     ORDER BY step ASC LIMIT 1",
                    params![id],
                )
                .await?;
            let Some(node) = flow.first() else {
                bail!("{id} 没有待审批节点，可能已被处理");
            };
            let flow_id = node["id"].as_str().unwrap_or_default().to_string();
            let step = node["step"].as_i64().unwrap_or(1);
            let approver = node["approver"].as_str().unwrap_or_default().to_string();
            let stamp = clock::local_datetime();
            let name = row["name"].as_str().unwrap_or("").to_string();
            let owner = row["owner_id"].as_str().unwrap_or("E01").to_string();

            if is_reject(&decision) {
                db.execute(
                    "UPDATE approval_flows SET status='rejected', comment=?2, acted_at=?3 WHERE id=?1",
                    params![flow_id, s(a, &["comment"]), stamp],
                )
                .await?;
                db.execute("UPDATE contracts SET status='rejected' WHERE id=?1", params![id]).await?;
                notify(
                    db,
                    &owner,
                    "合同已驳回",
                    &format!("{id}「{name}」被 {approver} 驳回"),
                    "contract",
                )
                .await?;
                return Ok(json!({
                    "ok": true, "contract_id": id, "status": "rejected",
                    "step": step, "approver": approver,
                    "message": format!("{id}「{name}」已被第 {step} 级审批人驳回")
                }));
            }

            db.execute(
                "UPDATE approval_flows SET status='done', comment=?2, acted_at=?3 WHERE id=?1",
                params![flow_id, s(a, &["comment"]), stamp],
            )
            .await?;

            let next = db
                .query_json(
                    "SELECT approver, step FROM approval_flows
                     WHERE entity_type='contract' AND entity_id=?1 AND status='pending'
                     ORDER BY step ASC LIMIT 1",
                    params![id],
                )
                .await?;
            if let Some(n) = next.first() {
                let next_approver = n["approver"].as_str().unwrap_or_default().to_string();
                let next_step = n["step"].as_i64().unwrap_or(step + 1);
                notify(
                    db,
                    &next_approver,
                    "合同待用印",
                    &format!("{id}「{name}」已通过第 {step} 级，待你审批"),
                    "contract",
                )
                .await?;
                return Ok(json!({
                    "ok": true, "contract_id": id, "status": "review",
                    "step": step, "approver": approver,
                    "next_approver": next_approver, "next_step": next_step,
                    "message": format!("{id} 第 {step} 级审批已通过，转交第 {next_step} 级（{next_approver}）继续审批")
                }));
            }

            db.execute("UPDATE contracts SET status='effective' WHERE id=?1", params![id]).await?;
            notify(
                db,
                &owner,
                "合同已生效",
                &format!("{id}「{name}」全部审批通过，合同已生效"),
                "contract",
            )
            .await?;
            Ok(json!({
                "ok": true, "contract_id": id, "status": "effective",
                "step": step, "approver": approver,
                "message": format!("{id}「{name}」全部 {step} 级审批已完成，合同已生效")
            }))
        }

        "contract.query" => {
            let kw = o(a, &["keyword"]);
            let status = o(a, &["status"]);
            let days = {
                let d = n(a, &["expiring_in_days", "expiringInDays"]);
                if d <= 0 { 30 } else { d }
            };
            let rows = db.query_json(
                "SELECT * FROM contracts
                 WHERE (?1 IS NULL OR status=?1)
                   AND (?2 IS NULL OR name LIKE '%'||?2||'%' OR party LIKE '%'||?2||'%' OR id LIKE '%'||?2||'%')
                 ORDER BY expire_date LIMIT 200",
                params![status, kw],
            ).await?;
            // 到期窗口用 Rust 侧算：date('now') / date('now','+N day') 是 SQLite 专有
            let (from, to) = (today(), clock::date_offset(days));
            let expiring = db.query_json(
                "SELECT * FROM contracts WHERE status <> 'archived'
                   AND expire_date BETWEEN ?1 AND ?2
                 ORDER BY expire_date",
                params![from, to],
            ).await?;
            Ok(json!({
                "ok": true, "count": rows.len(), "rows": rows,
                "expiring_in_days": days, "expiring_count": expiring.len(), "expiring": expiring,
                "message": format!("命中 {} 份合同，其中 {} 天内到期 {} 份", rows.len(), days, expiring.len())
            }))
        }

        "attendance.checkin" => {
            let emp = need(a, &["employee_id", "employeeId"], "employee_id")?;
            if !exists(db, "SELECT 1 FROM employees WHERE id=?1", &emp).await? {
                bail!("员工 {emp} 不存在");
            }
            let kind = if s(a, &["type"]) == "out" { "out" } else { "in" };
            let time = o(a, &["time"]).unwrap_or_else(|| now_hhmm());
            let time = if time.is_empty() { now_hhmm() } else { time };
            let source = o(a, &["source"]).unwrap_or_else(|| "agent".into());
            let date = today();

            let prior = db.query_json(
                "SELECT clock_in, clock_out FROM attendance WHERE employee_id=?1 AND work_date=?2",
                params![emp, date],
            ).await?;
            let (ci, co) = prior
                .first()
                .map(|r| (r["clock_in"].as_str().map(str::to_string), r["clock_out"].as_str().map(str::to_string)))
                .unwrap_or((None, None));
            let (ci, co) = if kind == "out" { (ci, Some(time.clone())) } else { (Some(time.clone()), co) };
            let hours = domain::work_hours(ci.as_deref(), co.as_deref());
            // 迟到优先：只要上班是迟到，即使下班正常 / 早退也记迟到，
            // 避免「打下班卡把上午的迟到冲掉」导致迟到统计失真（与 attendance.approve 同口径）
            let in_status = ci.as_deref().map(|t| domain::punch_status("in", t)).unwrap_or("normal");
            let out_status = co.as_deref().map(|t| domain::punch_status("out", t)).unwrap_or("normal");
            let final_status = if in_status != "normal" { in_status } else { out_status };
            let id = next_id(db, "attendance", "AT", 4).await?;
            db.execute(
                "INSERT INTO attendance(id,employee_id,work_date,clock_in,clock_out,status,source,hours)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(employee_id,work_date) DO UPDATE SET
                   clock_in=excluded.clock_in, clock_out=excluded.clock_out,
                   status=excluded.status, source=excluded.source, hours=excluded.hours",
                params![id, emp, date, ci, co, final_status, source, hours],
            ).await?;
            Ok(json!({
                "ok": true, "employee_id": emp, "work_date": date, "type": kind,
                "time": time, "status": final_status, "hours": hours,
                "clock_in": ci, "clock_out": co,
                "message": format!("{} {}卡 {}，状态 {}，工时 {:.1}h",
                    emp, if kind == "out" { "下班" } else { "上班" }, time, final_status, hours)
            }))
        }

        "attendance.makeup" => {
            let emp = need(a, &["employee_id", "employeeId"], "employee_id")?;
            if !exists(db, "SELECT 1 FROM employees WHERE id=?1", &emp).await? {
                bail!("员工 {emp} 不存在");
            }
            let date = o(a, &["date"]).unwrap_or_else(|| today());
            let date = if date.is_empty() { today() } else { date };
            // 补卡只能补历史/当天，补未来日期会让报表失真
            if date > today() {
                bail!("不能为未来日期 {date} 补卡");
            }
            let clock_in = o(a, &["clock_in", "clockIn", "time"]).unwrap_or_else(|| "09:00".into());
            let clock_out = o(a, &["clock_out", "clockOut"]).unwrap_or_else(|| "18:00".into());
            for (label, v) in [("上班", &clock_in), ("下班", &clock_out)] {
                if domain::parse_time(v).is_none() {
                    bail!("{label}时间 {v} 不是合法的 HH:MM");
                }
            }
            // 同人同日只留一条待审批，避免主管被重复申请刷屏、通过时互相覆盖
            let dup = db
                .query_json(
                    "SELECT id FROM makeup_requests
                      WHERE employee_id=?1 AND work_date=?2 AND status='pending'",
                    params![emp, date],
                )
                .await?;
            if let Some(existing) = dup.first().and_then(|r| r["id"].as_str()) {
                bail!("{emp} {date} 已有一条待审批的补卡申请（{existing}），请等主管审批后再提交");
            }
            let reason = o(a, &["reason"]).unwrap_or_else(|| "—".into());
            let dept = dept_of(db, &emp).await?;
            // 逐级：直属主管 → 部门总监（resolve_approvers 去重并排除本人，与报销同机制）
            let approvers = resolve_approvers(db, &emp, &dept, &["直属主管", "部门总监"]).await?;
            let approver = approvers.first().cloned().unwrap_or_else(|| "E01".to_string());

            let id = next_id(db, "makeup_requests", &format!("MK-{}", year()), 4).await?;
            db.execute(
                "INSERT INTO makeup_requests(id,employee_id,work_date,clock_in,clock_out,reason,status,approver_id)
                 VALUES(?1,?2,?3,?4,?5,?6,'pending',?7)",
                params![id, emp, date, clock_in, clock_out, reason, approver],
            )
            .await?;
            add_flow(db, "makeup", &id, &approvers).await?;
            notify(db, &approver, "补卡待审批", &format!("{emp} {date} 补卡：{reason}"), "attendance").await?;
            Ok(json!({
                "ok": true, "request_id": id, "employee_id": emp, "work_date": date,
                "clock_in": clock_in, "clock_out": clock_out, "reason": reason,
                "status": "pending", "approver": approver, "chain": approvers,
                "message": format!("{emp} {date} 补卡申请已提交（{reason}），审批链 {} 级（当前 {approver}），通过后才会补入考勤",
                    approvers.len())
            }))
        }

        "attendance.approve" => {
            let id = need(a, &["request_id", "requestId"], "request_id")?;
            let decision = o(a, &["decision"]).unwrap_or_else(|| "approve".into());
            let row = db
                .query_json(
                    "SELECT employee_id, work_date, clock_in, clock_out, status
                       FROM makeup_requests WHERE id=?1",
                    params![id],
                )
                .await?
                .first()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("补卡申请 {id} 不存在"))?;
            let status = row["status"].as_str().unwrap_or("");
            if status != "pending" {
                bail!("补卡申请 {id} 当前状态是 {status}，只有待审批的可以操作");
            }
            let flow = db
                .query_json(
                    "SELECT id, step, approver FROM approval_flows
                      WHERE entity_type='makeup' AND entity_id=?1 AND status='pending'
                      ORDER BY step ASC LIMIT 1",
                    params![id],
                )
                .await?;
            let Some(node) = flow.first() else {
                bail!("{id} 没有待审批节点，可能已被处理");
            };
            let flow_id = node["id"].as_str().unwrap_or_default().to_string();
            let step = node["step"].as_i64().unwrap_or(1);
            let approver = node["approver"].as_str().unwrap_or_default().to_string();
            let stamp = clock::local_datetime();
            let emp = row["employee_id"].as_str().unwrap_or("").to_string();
            let date = row["work_date"].as_str().unwrap_or("").to_string();
            let clock_in = row["clock_in"].as_str().unwrap_or("09:00").to_string();
            let clock_out = row["clock_out"].as_str().unwrap_or("18:00").to_string();

            if is_reject(&decision) {
                db.execute(
                    "UPDATE approval_flows SET status='rejected', comment=?2, acted_at=?3 WHERE id=?1",
                    params![flow_id, s(a, &["comment"]), stamp],
                )
                .await?;
                db.execute(
                    "UPDATE makeup_requests SET status='rejected', comment=?2, decided_at=?3 WHERE id=?1",
                    params![id, s(a, &["comment"]), stamp],
                )
                .await?;
                notify(db, &emp, "补卡已驳回", &format!("{date} 补卡被 {approver} 驳回"), "attendance").await?;
                return Ok(json!({
                    "ok": true, "request_id": id, "status": "rejected",
                    "employee_id": emp, "work_date": date, "step": step, "approver": approver,
                    "message": format!("补卡申请 {id}（{emp} {date}）已被驳回，未写入考勤")
                }));
            }

            db.execute(
                "UPDATE approval_flows SET status='done', comment=?2, acted_at=?3 WHERE id=?1",
                params![flow_id, s(a, &["comment"]), stamp],
            )
            .await?;

            // 多级：先看还有没有下一级待审节点，有则转交、暂不写考勤
            let next = db
                .query_json(
                    "SELECT approver, step FROM approval_flows
                     WHERE entity_type='makeup' AND entity_id=?1 AND status='pending'
                     ORDER BY step ASC LIMIT 1",
                    params![id],
                )
                .await?;
            if let Some(n) = next.first() {
                let next_approver = n["approver"].as_str().unwrap_or_default().to_string();
                let next_step = n["step"].as_i64().unwrap_or(step + 1);
                db.execute(
                    "UPDATE makeup_requests SET approver_id=?2 WHERE id=?1",
                    params![id, next_approver],
                )
                .await?;
                notify(
                    db,
                    &next_approver,
                    "补卡待审批",
                    &format!("{id}（{emp} {date}）已通过第 {step} 级，待你审批"),
                    "attendance",
                )
                .await?;
                return Ok(json!({
                    "ok": true, "request_id": id, "status": "pending",
                    "step": step, "approver": approver,
                    "next_approver": next_approver, "next_step": next_step,
                    "message": format!("{id} 第 {step} 级审批已通过，转交第 {next_step} 级（{next_approver}）")
                }));
            }

            // 末级通过后才补入考勤：迟到/早退与工时复用打卡同一套判定
            let status_in = domain::punch_status("in", &clock_in);
            let status_out = domain::punch_status("out", &clock_out);
            let att_status = if status_in != "normal" { status_in } else { status_out };
            let hours = domain::work_hours(Some(&clock_in), Some(&clock_out));
            let att_id = next_id(db, "attendance", "AT", 4).await?;
            db.execute(
                "INSERT INTO attendance(id,employee_id,work_date,clock_in,clock_out,status,source,hours)
                 VALUES(?1,?2,?3,?4,?5,?6,'makeup',?7)
                 ON CONFLICT(employee_id,work_date) DO UPDATE SET
                   clock_in=excluded.clock_in, clock_out=excluded.clock_out,
                   status=excluded.status, source='makeup', hours=excluded.hours",
                params![att_id, emp, date, clock_in, clock_out, att_status, hours],
            )
            .await?;
            db.execute(
                "UPDATE makeup_requests SET status='approved', comment=?2, decided_at=?3 WHERE id=?1",
                params![id, s(a, &["comment"]), stamp],
            )
            .await?;
            notify(
                db,
                &emp,
                "补卡已通过",
                &format!("{date} 补卡已由 {approver} 通过，考勤已更新"),
                "attendance",
            )
            .await?;
            Ok(json!({
                "ok": true, "request_id": id, "status": "approved",
                "employee_id": emp, "work_date": date, "attendance_id": att_id,
                "clock_in": clock_in, "clock_out": clock_out,
                "attendance_status": att_status, "hours": hours,
                "step": step, "approver": approver,
                "message": format!("补卡申请 {id}（{emp} {date}）已通过，考勤已补入（{att_status}，{hours:.1}h）")
            }))
        }

        "attendance.report" => {
            let month = o(a, &["month"]).unwrap_or_else(|| month_now());
            let month = if month.is_empty() { month_now() } else { month };
            let dept = o(a, &["dept_id", "deptId"]);
            let rows = db.query_json(
                "SELECT e.id AS employee_id, e.name, e.dept_id, d.name AS dept_name,
                        COUNT(a.id) AS days,
                        SUM(CASE WHEN a.status='normal' THEN 1 ELSE 0 END) AS normal,
                        SUM(CASE WHEN a.status='late' THEN 1 ELSE 0 END) AS late,
                        SUM(CASE WHEN a.status='early' THEN 1 ELSE 0 END) AS early,
                        SUM(CASE WHEN a.status='missing' THEN 1 ELSE 0 END) AS missing,
                        SUM(CASE WHEN a.status='field' THEN 1 ELSE 0 END) AS field,
                        SUM(CASE WHEN a.status='leave' THEN 1 ELSE 0 END) AS leave_count,
                        SUM(a.hours) AS hours_raw
                   FROM attendance a JOIN employees e ON e.id=a.employee_id
                   LEFT JOIN departments d ON d.id=e.dept_id
                  WHERE substr(a.work_date,1,7)=?1 AND (?2 IS NULL OR e.dept_id=?2)
                  GROUP BY e.id, e.name, e.dept_id, d.name
                  ORDER BY late DESC, days DESC, e.id",
                params![month, dept],
            ).await?;
            let mut out: Vec<Value> = Vec::with_capacity(rows.len());
            let mut total = 0i64;
            let mut good = 0i64;
            for row in rows {
                let days = row["days"].as_i64().unwrap_or(0);
                let normal = row["normal"].as_i64().unwrap_or(0);
                let field = row["field"].as_i64().unwrap_or(0);
                total += days;
                good += normal + field;
                let rate = if days > 0 {
                    ((normal + field) as f64 / days as f64 * 1000.0).round() / 10.0
                } else {
                    0.0
                };
                let hours = (row["hours_raw"].as_f64().unwrap_or(0.0) * 10.0).round() / 10.0;
                let mut obj = row;
                if let Some(map) = obj.as_object_mut() {
                    // SQLite/Postgres 的 ROUND 语义不一致，四舍五入统一在 Rust 侧做
                    map.insert("hours".into(), json!(hours));
                    map.insert("attendance_rate".into(), json!(rate));
                    map.remove("hours_raw");
                }
                out.push(obj);
            }
            let rate = if total > 0 {
                (good as f64 / total as f64 * 1000.0).round() / 10.0
            } else {
                0.0
            };
            let pending = db
                .scalar_i64(
                    "SELECT COUNT(*) FROM makeup_requests
                      WHERE status='pending' AND substr(work_date,1,7)=?1",
                    params![month],
                )
                .await?;
            Ok(json!({
                "ok": true, "month": month, "total": total, "attendance_rate": rate,
                "rows": out, "employees": out.len(), "pending_makeups": pending,
                "message": format!("{} 共 {} 人 {} 条打卡，出勤率 {:.1}%，待审批补卡 {} 条",
                    month, out.len(), total, rate, pending)
            }))
        }

        "asset.register" => {
            let category = o(a, &["category"]).unwrap_or_else(|| "other".into());
            let model = need(a, &["model"], "model")?;
            let sn = o(a, &["serial_no", "sn"]).unwrap_or_else(|| "-".into());
            if sn != "-" && exists(db, "SELECT 1 FROM assets WHERE serial_no=?1", &sn).await? {
                bail!("资产序列号 {sn} 已存在");
            }
            let price = cents(a, "price");
            let location = o(a, &["location"]).unwrap_or_else(|| "行政库房".into());
            let id = next_id(db, "assets", "AST", 4).await?;
            // 二维码内容 = 资产编号，SVG 直接落库，前端详情抽屉内联渲染
            let qr = qr_svg(&id);
            db.execute(
                "INSERT INTO assets(id,category,model,serial_no,status,holder_id,location,price_cents,qr_code,purchased_at)
                 VALUES(?1,?2,?3,?4,'instock',NULL,?5,?6,?7,?8)",
                params![id, category, model, sn, location, price, qr, today()],
            ).await?;
            add_event(db, &id, "register", None, "采购入库").await?;
            Ok(json!({
                "ok": true, "asset_id": id, "qr_code": qr, "qr_content": id, "serial_no": sn,
                "price_cents": price,
                "message": format!("资产 {id} 已入库：{model}（SN {sn}），二维码已生成")
            }))
        }

        "asset.assign" => {
            let asset = need(a, &["asset_id", "assetId"], "asset_id")?;
            let emp = need(a, &["employee_id", "employeeId"], "employee_id")?;
            if !exists(db, "SELECT 1 FROM employees WHERE id=?1", &emp).await? {
                bail!("领用人 {emp} 不存在");
            }
            let found = db.query_json(
                "SELECT id, category, status, model FROM assets WHERE id=?1 OR serial_no=?1",
                params![asset],
            ).await?;
            let first = found
                .first()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("资产 {asset} 不存在"))?;
            let mut target = first["id"].as_str().unwrap_or("").to_string();
            let mut reassigned = false;
            if first["status"].as_str() != Some("instock") {
                let category = first["category"].as_str().unwrap_or("other");
                let alt = db.query_json(
                    "SELECT id FROM assets WHERE category=?1 AND status='instock' LIMIT 1",
                    params![category],
                ).await?;
                target = alt
                    .first()
                    .and_then(|r| r["id"].as_str())
                    .ok_or_else(|| anyhow::anyhow!("目标资产已占用且无同类在库资产，请先归还"))?
                    .to_string();
                reassigned = true;
            }
            db.execute(
                "UPDATE assets SET status='inuse', holder_id=?2 WHERE id=?1",
                params![target, emp],
            ).await?;
            add_event(db, &target, "assign", Some(&emp), if reassigned { "自动改派领用" } else { "领用" }).await?;
            Ok(json!({
                "ok": true, "asset_id": target, "employee_id": emp, "reassigned": reassigned,
                "message": format!("{}{} 已领用给 {emp}",
                    if reassigned { "目标资产已占用，已自动改派：" } else { "" }, target)
            }))
        }

        "asset.return" => {
            let asset = need(a, &["asset_id", "assetId"], "asset_id")?;
            let found = db.query_json(
                "SELECT id, holder_id FROM assets WHERE id=?1 OR serial_no=?1",
                params![asset],
            ).await?;
            let row = found
                .first()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("资产 {asset} 不存在"))?;
            let id = row["id"].as_str().unwrap_or("").to_string();
            let holder = row["holder_id"].as_str().map(str::to_string);
            db.execute(
                "UPDATE assets SET status='instock', holder_id=NULL WHERE id=?1",
                params![id],
            ).await?;
            add_event(db, &id, "return", holder.as_deref(), &format!("归还，成色：{}", s(a, &["condition"]))).await?;
            Ok(json!({
                "ok": true, "asset_id": id, "previous_holder": holder,
                "message": format!("{id} 已归还入库，责任人已释放")
            }))
        }

        "asset.query" => {
            let category = o(a, &["category"]);
            let status = o(a, &["status"]);
            let kw = o(a, &["keyword"]);
            let holder = o(a, &["holder_id", "holderId"]);
            let rows = db.query_json(
                "SELECT a.*, e.name AS holder_name FROM assets a
                 LEFT JOIN employees e ON e.id=a.holder_id
                 WHERE (?1 IS NULL OR a.category=?1) AND (?2 IS NULL OR a.status=?2)
                   AND (?3 IS NULL OR a.holder_id=?3)
                   AND (?4 IS NULL OR a.model LIKE '%'||?4||'%' OR a.id LIKE '%'||?4||'%' OR a.serial_no LIKE '%'||?4||'%')
                 ORDER BY a.id LIMIT 200",
                params![category, status, holder, kw],
            ).await?;
            let total_value: i64 = rows.iter().map(|r| r["price_cents"].as_i64().unwrap_or(0)).sum();
            let by_category = db.query_json(
                "SELECT category, COUNT(*) AS cnt, SUM(price_cents) AS value FROM assets GROUP BY category",
                params![],
            ).await?;
            Ok(json!({
                "ok": true, "count": rows.len(), "rows": rows,
                "total_value_cents": total_value, "by_category": by_category,
                "message": format!("盘点命中 {} 项资产，原值合计 {:.2} 元", rows.len(), total_value as f64 / 100.0)
            }))
        }

        _ => bail!("unknown tool: {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn arg_normalization_handles_frontend_aliases() {
        let a = json!({ "employeeId": "E03", "amount": 3280, "asset_id": "AST-0003" });
        assert_eq!(s(&a, &["employee_id", "employeeId"]), "E03");
        assert_eq!(cents(&a, "amount"), 328000);
        let b = json!({ "amount_cents": 5000, "amount": 50 });
        assert_eq!(cents(&b, "amount"), 5000);
        assert_eq!(s(&a, &["asset_id"]), "AST-0003");
    }

    #[test]
    fn decision_normalization_recognizes_reject_variants() {
        // LLM 可能传中文「驳回」或 rejected/decline 等变体，只有这些才算驳回，其余按通过
        for d in ["reject", "REJECT", "Rejected", "rejected", "decline", "declined", "deny", "denied", "驳回", "拒绝", "不通过", "不同意"] {
            assert!(is_reject(d), "{d} 应被识别为驳回");
        }
        for d in ["approve", "通过", "同意", "approved", "ok", ""] {
            assert!(!is_reject(d), "{d} 不应被识别为驳回");
        }
    }

    #[test]
    fn specs_cover_every_registered_tool() {
        assert_eq!(TOOL_SPECS.len(), 20);
        for (name, _, _, _) in TOOL_SPECS {
            assert!(domain::TOOL_NAMES.contains(name), "{name} missing from TOOL_NAMES");
            let schema = tool_schema(name);
            assert_eq!(schema["type"], "object");
        }
    }

    #[tokio::test]
    async fn rbac_blocks_direct_tool_calls_and_audits() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, employee) = app
            .store
            .login("lina", crate::auth::DEFAULT_PASSWORD)
            .await
            .unwrap();
        let out = call(
            app.clone(),
            &employee,
            "asset.register",
            json!({ "model": "ThinkPad X1 Carbon", "serial_no": "RBAC-TEST-1" }),
        )
        .await;
        assert_eq!(out["ok"], false);
        assert_eq!(out["denied"], true);
        // 被拒绝的调用同样写审计
        let rows = app.store.audit_events(10, 0).await.unwrap();
        assert!(rows.iter().any(|r| r["tool"] == "asset.register"
            && r["actor"] == "E03"
            && r["result"].as_str().unwrap_or("").contains("denied")));
    }

    #[tokio::test]
    async fn employee_cannot_operate_on_behalf_of_others() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, employee) = app
            .store
            .login("lina", crate::auth::DEFAULT_PASSWORD)
            .await
            .unwrap();
        let out = call(
            app.clone(),
            &employee,
            "attendance.checkin",
            json!({ "employee_id": "E02", "type": "in", "time": "09:30" }),
        )
        .await;
        assert_eq!(out["denied"], true);

        let own = call(
            app,
            &employee,
            "attendance.checkin",
            json!({ "employee_id": "E03", "type": "in", "time": "09:30" }),
        )
        .await;
        assert_eq!(own["ok"], true);
    }

    /// 1.5 万元报销 → 两级审批（直属主管 E02 → 财务经理 E10），逐级推进而不是一次批完。
    #[tokio::test]
    async fn claim_approval_walks_chain_step_by_step() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, admin) = app.store.login("evan", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let (_, employee) = app.store.login("lina", crate::auth::DEFAULT_PASSWORD).await.unwrap();

        let created = call(
            app.clone(),
            &employee,
            "finance.claim.create",
            json!({
                "applicant_id": "E03", "dept_id": "D02", "type": "daily",
                "amount": 15000, "purpose": "客户拜访差旅"
            }),
        )
        .await;
        assert_eq!(created["ok"], true, "{created}");
        assert_eq!(created["total_steps"], 2);
        assert_eq!(created["approvers"], json!(["E02", "E10"]));
        assert_eq!(created["current_approver"], "E02");
        let id = created["claim_id"].as_str().unwrap().to_string();

        // 申请人自己不在审批链上，连门禁都过不去
        let denied = call(
            app.clone(),
            &employee,
            "finance.claim.approve",
            json!({ "claim_id": id, "decision": "approve", "confirmed": true }),
        )
        .await;
        assert_eq!(denied["denied"], true, "{denied}");

        // 第一级通过后单子仍在审批中，只是转交下一级
        let step1 = call(
            app.clone(),
            &admin,
            "finance.claim.approve",
            json!({ "claim_id": id, "decision": "approve", "comment": "主管同意", "confirmed": true }),
        )
        .await;
        assert_eq!(step1["ok"], true, "{step1}");
        assert_eq!(step1["status"], "pending");
        assert_eq!(step1["next_step"], 2);
        assert_eq!(step1["next_approver"], "E10");

        // 第二级通过才算完成，并生成付款单
        let step2 = call(
            app.clone(),
            &admin,
            "finance.claim.approve",
            json!({ "claim_id": id, "decision": "approve", "comment": "财务同意", "confirmed": true }),
        )
        .await;
        assert_eq!(step2["status"], "approved", "{step2}");
        assert!(step2["payment_id"].as_str().unwrap_or("").starts_with("PAY-"));

        // 留痕：两个节点都 done，且记下意见与审批时间
        let flows = app
            .store
            .query_json(
                "SELECT step,approver,status,comment,acted_at FROM approval_flows
                 WHERE entity_id=?1 ORDER BY step",
                params![id],
            )
            .await
            .unwrap();
        assert_eq!(flows.len(), 2);
        assert!(flows.iter().all(|f| f["status"] == "done"));
        assert_eq!(flows[0]["comment"], "主管同意");
        assert!(flows.iter().all(|f| f["acted_at"].as_str().unwrap_or("").len() >= 10));

        // 全部批完后不能再审
        let again = call(
            app,
            &admin,
            "finance.claim.approve",
            json!({ "claim_id": id, "confirmed": true }),
        )
        .await;
        assert_eq!(again["ok"], false, "{again}");
    }

    /// 驳回要终止整条链并释放已占用的预算。
    #[tokio::test]
    async fn claim_reject_stops_chain_and_releases_budget() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, admin) = app.store.login("evan", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let (_, employee) = app.store.login("lina", crate::auth::DEFAULT_PASSWORD).await.unwrap();

        let created = call(
            app.clone(),
            &employee,
            "finance.claim.create",
            json!({
                "applicant_id": "E03", "dept_id": "D02", "type": "daily",
                "amount": 8000, "purpose": "办公用品采购"
            }),
        )
        .await;
        let id = created["claim_id"].as_str().unwrap().to_string();
        assert_eq!(created["budget_ok"], true, "{created}");

        let rejected = call(
            app.clone(),
            &admin,
            "finance.claim.approve",
            json!({ "claim_id": id, "decision": "reject", "comment": "缺少发票", "confirmed": true }),
        )
        .await;
        assert_eq!(rejected["status"], "rejected", "{rejected}");
        assert_eq!(rejected["budget_released"], true);

        let rows = app
            .store
            .query_json("SELECT status FROM claims WHERE id=?1", params![id])
            .await
            .unwrap();
        assert_eq!(rows[0]["status"], "rejected");

        // 驳回后链路终止，不能再审
        let again = call(
            app,
            &admin,
            "finance.claim.approve",
            json!({ "claim_id": id, "confirmed": true }),
        )
        .await;
        assert_eq!(again["ok"], false, "{again}");
    }

    /// 申请人本人就是部门主管时，审批链要上溯到他自己的上级，不能自己审自己。
    #[tokio::test]
    async fn applicant_is_never_his_own_approver() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, manager) = app.store.login("chenli", crate::auth::DEFAULT_PASSWORD).await.unwrap();

        let created = call(
            app,
            &manager,
            "finance.claim.create",
            json!({
                "applicant_id": "E02", "dept_id": "D02", "type": "daily",
                "amount": 15000, "purpose": "部门团建"
            }),
        )
        .await;
        assert_eq!(created["ok"], true, "{created}");
        let approvers = created["approvers"].as_array().unwrap();
        assert!(!approvers.iter().any(|v| v == "E02"), "{created}");
        assert_eq!(created["current_approver"], "E01");
    }

    /// 报销总额以费用明细合计为准，明细逐条落库并带发票与发生日期。
    #[tokio::test]
    async fn claim_items_drive_total_and_are_persisted() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, employee) = app.store.login("lina", crate::auth::DEFAULT_PASSWORD).await.unwrap();

        let created = call(
            app.clone(),
            &employee,
            "finance.claim.create",
            json!({
                "applicant_id": "E03", "dept_id": "D02", "type": "daily",
                "purpose": "客户拜访差旅",
                // 声明的总额故意写错，应被明细合计（620+380=1000 元）覆盖
                "amount": 999,
                "items": [
                    { "category": "transport", "amount": 620, "occurred_on": "2026-11-10", "invoice_no": "INV-001", "note": "高铁" },
                    { "category": "hotel", "amount": 380, "occurred_on": "2026-11-10", "invoice_no": "INV-002" }
                ]
            }),
        )
        .await;
        assert_eq!(created["ok"], true, "{created}");
        assert_eq!(created["amount_cents"], 100000);
        assert_eq!(created["items_count"], 2);
        assert_eq!(created["items_amount_mismatch"], true);

        let id = created["claim_id"].as_str().unwrap().to_string();
        let rows = app
            .store
            .query_json(
                "SELECT id,category,amount_cents,invoice_no FROM claim_items WHERE claim_id=?1 ORDER BY id",
                params![id],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["category"], "transport");
        assert_eq!(rows[0]["amount_cents"], 62000);
        assert_eq!(rows[1]["invoice_no"], "INV-002");
        assert!(rows[0]["id"].as_str().unwrap().starts_with(&id));
    }

    /// 明细缺类别或金额为 0 时直接拒绝，且事务回滚不留下半张单。
    #[tokio::test]
    async fn invalid_claim_items_are_rejected() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, employee) = app.store.login("lina", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let before = app
            .store
            .query_json("SELECT COUNT(*) AS n FROM claims", &[])
            .await
            .unwrap();

        let no_category = call(
            app.clone(),
            &employee,
            "finance.claim.create",
            json!({
                "applicant_id": "E03", "dept_id": "D02", "type": "daily",
                "amount": 100, "items": [{ "amount": 100 }]
            }),
        )
        .await;
        assert_eq!(no_category["ok"], false, "{no_category}");

        let zero_amount = call(
            app.clone(),
            &employee,
            "finance.claim.create",
            json!({
                "applicant_id": "E03", "dept_id": "D02", "type": "daily",
                "amount": 0, "items": [{ "category": "meal", "amount": 0 }]
            }),
        )
        .await;
        assert_eq!(zero_amount["ok"], false, "{zero_amount}");

        // 不在费用类型白名单里的类别直接拒绝（否则标准校验会形同虚设）
        let unknown_category = call(
            app.clone(),
            &employee,
            "finance.claim.create",
            json!({
                "applicant_id": "E03", "dept_id": "D02", "type": "daily",
                "amount": 120, "items": [{ "category": "taxi", "amount": 120 }]
            }),
        )
        .await;
        assert_eq!(unknown_category["ok"], false, "{unknown_category}");

        let after = app
            .store
            .query_json("SELECT COUNT(*) AS n FROM claims", &[])
            .await
            .unwrap();
        assert_eq!(before[0]["n"], after[0]["n"]);
    }

    /// 配置了预算就要硬拦截：超额报销直接拒绝、不落库，而不是只提示「人工复核」。
    /// 本年 ALL 预算剩余 35.74 万（120 万 − 84.26 万），申请 100 万必然超额。
    #[tokio::test]
    async fn over_budget_claim_is_rejected() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, employee) = app.store.login("lina", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let before = app
            .store
            .query_json("SELECT COUNT(*) AS n FROM claims", &[])
            .await
            .unwrap();

        let res = call(
            app.clone(),
            &employee,
            "finance.claim.create",
            json!({
                "applicant_id": "E03", "dept_id": "D02", "type": "daily",
                "amount_cents": 100_000_000, "purpose": "超额测试"
            }),
        )
        .await;
        assert_eq!(res["ok"], false, "{res}");
        assert!(
            res["error"].as_str().unwrap_or_default().contains("预算"),
            "错误应指明是预算问题：{res}"
        );

        let after = app
            .store
            .query_json("SELECT COUNT(*) AS n FROM claims", &[])
            .await
            .unwrap();
        assert_eq!(before[0]["n"], after[0]["n"], "超额报销不应落库");
    }

    /// 合同审批链存具体员工号：法务（财务经理 E10）→ 用印（总经理 E01），
    /// 逐级流转，末级通过才生效；不是审批人的同级角色也过不了门禁。
    #[tokio::test]
    async fn contract_approval_walks_legal_then_seal() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, admin) = app.store.login("evan", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let (_, manager) = app.store.login("chenli", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let (_, finance) = app.store.login("wuhao", crate::auth::DEFAULT_PASSWORD).await.unwrap();

        let created = call(
            app.clone(),
            &manager,
            "contract.create",
            json!({
                "name": "服务器采购合同", "party": "联想（北京）有限公司",
                "type": "purchase", "amount": 480000, "owner_id": "E02",
                "confirmed": true
            }),
        )
        .await;
        assert_eq!(created["ok"], true, "{created}");
        assert_eq!(created["approvers"], json!(["E10", "E01"]));
        assert_eq!(created["current_approver"], "E10");
        let id = created["contract_id"].as_str().unwrap().to_string();

        // 负责人自己（有 contract.approve 权限）也不在当前节点，必须拒绝
        let denied = call(
            app.clone(),
            &manager,
            "contract.approve",
            json!({ "contract_id": id, "decision": "approve", "confirmed": true }),
        )
        .await;
        assert_eq!(denied["denied"], true, "{denied}");

        let step1 = call(
            app.clone(),
            &finance,
            "contract.approve",
            json!({ "contract_id": id, "decision": "approve", "comment": "条款与金额无异议", "confirmed": true }),
        )
        .await;
        assert_eq!(step1["ok"], true, "{step1}");
        assert_eq!(step1["status"], "review");
        assert_eq!(step1["next_approver"], "E01");

        let step2 = call(
            app.clone(),
            &admin,
            "contract.approve",
            json!({ "contract_id": id, "decision": "approve", "comment": "同意用印", "confirmed": true }),
        )
        .await;
        assert_eq!(step2["status"], "effective", "{step2}");

        let flows = app
            .store
            .query_json(
                "SELECT step,approver,status,comment,acted_at FROM approval_flows
                 WHERE entity_type='contract' AND entity_id=?1 ORDER BY step",
                params![id],
            )
            .await
            .unwrap();
        assert_eq!(flows.len(), 2);
        assert!(flows.iter().all(|f| f["status"] == "done"));
        assert_eq!(flows[0]["approver"], "E10");
        assert_eq!(flows[1]["approver"], "E01");
        assert_eq!(flows[1]["comment"], "同意用印");
        assert!(flows.iter().all(|f| f["acted_at"].as_str().unwrap_or("").len() >= 10));

        let contracts = app
            .store
            .query_json("SELECT status FROM contracts WHERE id=?1", params![id])
            .await
            .unwrap();
        assert_eq!(contracts[0]["status"], "effective");
    }

    /// 驳回终止整条链，合同转为 rejected 后不能再审。
    #[tokio::test]
    async fn contract_reject_terminates_chain() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, admin) = app.store.login("evan", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let (_, manager) = app.store.login("chenli", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let (_, finance) = app.store.login("wuhao", crate::auth::DEFAULT_PASSWORD).await.unwrap();

        let created = call(
            app.clone(),
            &manager,
            "contract.create",
            json!({
                "name": "咨询服务合同", "party": "蓝海信息", "type": "service",
                "amount": 120000, "owner_id": "E02", "confirmed": true
            }),
        )
        .await;
        let id = created["contract_id"].as_str().unwrap().to_string();

        let rejected = call(
            app.clone(),
            &finance,
            "contract.approve",
            json!({ "contract_id": id, "decision": "reject", "comment": "缺少违约条款", "confirmed": true }),
        )
        .await;
        assert_eq!(rejected["status"], "rejected", "{rejected}");

        let again = call(
            app,
            &admin,
            "contract.approve",
            json!({ "contract_id": id, "decision": "approve", "confirmed": true }),
        )
        .await;
        assert_eq!(again["ok"], false, "{again}");
    }

    /// 费用标准：超标明细必须带说明，否则整单拒收且不落库；带说明则入库并回传超标清单。
    #[tokio::test]
    async fn over_standard_items_require_a_note() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, employee) = app.store.login("lina", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let before = app
            .store
            .query_json("SELECT COUNT(*) AS n FROM claims", &[])
            .await
            .unwrap();

        // 住宿标准 5000 元/次，6000 元且无备注 → 拒绝
        let blocked = call(
            app.clone(),
            &employee,
            "finance.claim.create",
            json!({
                "applicant_id": "E03", "dept_id": "D02", "type": "daily",
                "purpose": "客户拜访",
                "items": [{ "category": "hotel", "amount": 6000 }]
            }),
        )
        .await;
        assert_eq!(blocked["ok"], false, "{blocked}");
        assert!(
            blocked["error"].as_str().unwrap_or("").contains("费用标准"),
            "{blocked}"
        );

        let after = app
            .store
            .query_json("SELECT COUNT(*) AS n FROM claims", &[])
            .await
            .unwrap();
        assert_eq!(before[0]["n"], after[0]["n"], "超标未备注不应留下报销单");

        // 补上说明后照常入库，并把超标明细单独回传
        let created = call(
            app.clone(),
            &employee,
            "finance.claim.create",
            json!({
                "applicant_id": "E03", "dept_id": "D02", "type": "daily",
                "purpose": "客户拜访",
                "items": [
                    { "category": "hotel", "amount": 6000, "note": "客户指定酒店，超标部分自付" },
                    { "category": "meal", "amount": 200 }
                ]
            }),
        )
        .await;
        assert_eq!(created["ok"], true, "{created}");
        assert_eq!(created["over_standard_count"], 1);
        assert_eq!(created["over_standard_items"][0]["category"], "hotel");
        assert_eq!(created["items"][0]["standard_cents"], 500000);
        assert_eq!(created["items"][1]["over_standard"], false);

        // 台账也带标准与超标标记，审批人能在详情里直接看到
        let items = app.store.table_rows(&employee, "claim_items").await.unwrap();
        let hotel = items
            .iter()
            .find(|r| r["category"] == "hotel")
            .unwrap_or_else(|| panic!("no hotel item in {items:?}"));
        assert_eq!(hotel["standard_cents"], 500000);
        assert_eq!(hotel["over_standard"], true);
    }

    /// 补卡：申请只写请求与审批流，主管通过后才补入考勤；驳回不留考勤数据。
    #[tokio::test]
    async fn makeup_is_written_to_attendance_only_after_approval() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, manager) = app.store.login("chenli", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let (_, employee) = app.store.login("lina", crate::auth::DEFAULT_PASSWORD).await.unwrap();

        // 李娜（E03）补 11-10 的卡，审批人是直属主管 E02
        let submitted = call(
            app.clone(),
            &employee,
            "attendance.makeup",
            json!({
                "employee_id": "E03", "date": "2026-11-10",
                "clock_in": "09:05", "clock_out": "18:30", "reason": "忘记打卡"
            }),
        )
        .await;
        assert_eq!(submitted["ok"], true, "{submitted}");
        assert_eq!(submitted["approver"], "E02");
        let id = submitted["request_id"].as_str().unwrap().to_string();

        // 通过前看不到考勤记录
        let before = app
            .store
            .query_json(
                "SELECT COUNT(*) AS n FROM attendance WHERE employee_id='E03' AND work_date='2026-11-10'",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(before[0]["n"], 0);

        // 同人同日重复提交被拒
        let dup = call(
            app.clone(),
            &employee,
            "attendance.makeup",
            json!({ "employee_id": "E03", "date": "2026-11-10", "reason": "再提交一次" }),
        )
        .await;
        assert_eq!(dup["ok"], false, "{dup}");

        // 申请人自己不在审批节点上
        let denied = call(
            app.clone(),
            &employee,
            "attendance.approve",
            json!({ "request_id": id, "decision": "approve", "confirmed": true }),
        )
        .await;
        assert_eq!(denied["denied"], true, "{denied}");

        // 逐级：直属主管 E02 是第 1 级，通过后转交第 2 级（E02 的上级 E01）
        let first = call(
            app.clone(),
            &manager,
            "attendance.approve",
            json!({ "request_id": id, "decision": "approve", "comment": "核实无误", "confirmed": true }),
        )
        .await;
        assert_eq!(first["status"], "pending", "{first}");
        assert_eq!(first["next_approver"], "E01", "{first}");

        let (_, admin) = app.store.login("evan", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let approved = call(
            app.clone(),
            &admin,
            "attendance.approve",
            json!({ "request_id": id, "decision": "approve", "comment": "同意", "confirmed": true }),
        )
        .await;
        assert_eq!(approved["status"], "approved", "{approved}");
        assert_eq!(approved["attendance_status"], "late"); // 09:05 晚于 09:00
        assert_eq!(approved["hours"], 8.4);

        let att = app
            .store
            .query_json(
                "SELECT clock_in, clock_out, status, source, hours FROM attendance
                  WHERE employee_id='E03' AND work_date='2026-11-10'",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(att.len(), 1, "{att:?}");
        assert_eq!(att[0]["clock_in"], "09:05");
        assert_eq!(att[0]["source"], "makeup");
        assert_eq!(att[0]["status"], "late");

        // 已审批的不能再审
        let again = call(
            app,
            &manager,
            "attendance.approve",
            json!({ "request_id": id, "confirmed": true }),
        )
        .await;
        assert_eq!(again["ok"], false, "{again}");

        // 驳回场景：另一天补卡被拒后不写考勤
        let app2 = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, manager2) = app2.store.login("chenli", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let (_, employee2) = app2.store.login("lina", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let req = call(
            app2.clone(),
            &employee2,
            "attendance.makeup",
            json!({ "employee_id": "E03", "date": "2026-11-09", "reason": "外出" }),
        )
        .await;
        let rid = req["request_id"].as_str().unwrap().to_string();
        let rejected = call(
            app2.clone(),
            &manager2,
            "attendance.approve",
            json!({ "request_id": rid, "decision": "reject", "comment": "当天无外出记录", "confirmed": true }),
        )
        .await;
        assert_eq!(rejected["status"], "rejected", "{rejected}");
        let rows = app2
            .store
            .query_json("SELECT COUNT(*) AS n FROM attendance WHERE work_date='2026-11-09'", &[])
            .await
            .unwrap();
        assert_eq!(rows[0]["n"], 0, "驳回不应写考勤");
    }

    /// 演示数据里的合同审批链也必须是员工号，否则前端「待你审批」永远匹配不到人。
    #[tokio::test]
    async fn seeded_contract_chain_is_person_based() {
        let db = Store::open_in_memory().await.unwrap();
        let rows = db
            .query_json(
                "SELECT approver FROM approval_flows
                 WHERE entity_type='contract' AND entity_id='HT-2026-013' ORDER BY step",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0]["approver"], "E10");
        assert_eq!(rows[1]["approver"], "E01");
    }

    /// 人事异动审批链是真实可推进的：入职单逐级流转（直属主管 → HRBP → 行政），末级通过才 approved。
    #[tokio::test]
    async fn hr_case_approval_walks_chain() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, admin) = app.store.login("evan", crate::auth::DEFAULT_PASSWORD).await.unwrap();

        let created = call(
            app.clone(),
            &admin,
            "hr.onboard",
            json!({ "name": "测试入职", "dept_id": "D02", "position": "工程师", "salary_cents": 1500000 }),
        )
        .await;
        assert_eq!(created["ok"], true, "{created}");
        let case_id = created["case_id"].as_str().unwrap().to_string();

        // 第 1 级：D02 负责人 E02 → 转 HRBP E12
        let s1 = call(
            app.clone(),
            &admin,
            "hr.approve",
            json!({ "case_id": case_id, "decision": "approve", "confirmed": true }),
        )
        .await;
        assert_eq!(s1["status"], "running", "{s1}");
        assert_eq!(s1["next_approver"], "E12", "{s1}");

        // 第 2 级：HRBP E12 → 转行政 E13
        let s2 = call(
            app.clone(),
            &admin,
            "hr.approve",
            json!({ "case_id": case_id, "decision": "approve", "confirmed": true }),
        )
        .await;
        assert_eq!(s2["status"], "running", "{s2}");
        assert_eq!(s2["next_approver"], "E13", "{s2}");

        // 第 3 级：行政 E13 → 完成
        let s3 = call(
            app.clone(),
            &admin,
            "hr.approve",
            json!({ "case_id": case_id, "decision": "approve", "confirmed": true }),
        )
        .await;
        assert_eq!(s3["status"], "approved", "{s3}");
    }
}
