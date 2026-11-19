//! 离线确定性意图引擎（Agent 的本地执行模式）。
//!
//! 存在的意义：**功能完整性不依赖外部 LLM 密钥**。没有 OPENAI_API_KEY 时，
//! 自然语言仍可解析成执行计划并驱动全部 19 个工具；配置密钥后切
//! `OA_AGENT_MODE=llm` 由 Rig + 真实模型接管，工具与业务代码零改动。
//!
//! 与 tools::TOOL_SPECS 共用同一套工具契约，支持中英双语与多步编排。

use super::tools;
use crate::auth::SessionUser;
use crate::db::Repo;
use crate::{clock, domain, AppState};
use anyhow::Result;
use regex::Regex;
use crate::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub tool: String,
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub tool: String,
    pub args: Value,
    pub result: Value,
    pub ok: bool,
}

/// 一步「思考」：阶段 + 人类可读说明。
///
/// 流式接口边执行边推给前端；非流式接口把它和回复一起返回，
/// 保证两种调用方式看到的是同一份推理记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkStep {
    /// intent / plan / tool / guard / route / done
    pub stage: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReply {
    pub reply: String,
    /// local / llm / llm-fallback
    pub mode: String,
    pub trace: Vec<TraceEntry>,
    pub plan: Vec<PlanStep>,
    /// 本次的思考过程（阶段 + 说明），前端在对话气泡里折叠展示
    #[serde(default)]
    pub thinking: Vec<ThinkStep>,
    /// 被敏感操作门禁拦下的那一步：前端确认后带 confirmed=true 直接重试此工具。
    #[serde(default)]
    pub pending: Option<PlanStep>,
    /// 本次实际写入的业务域（hr / finance / contract / attendance / asset）
    #[serde(default)]
    pub affected: Vec<String>,
    #[serde(default)]
    pub needs_confirmation: bool,
}

impl AgentReply {
    pub fn new(reply: impl Into<String>, mode: &str) -> Self {
        Self {
            reply: reply.into(),
            mode: mode.to_string(),
            trace: vec![],
            plan: vec![],
            thinking: vec![],
            pending: None,
            affected: vec![],
            needs_confirmation: false,
        }
    }
}

fn has_han(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

fn cap<'a>(re: &Regex, text: &'a str, idx: usize) -> Option<&'a str> {
    re.captures(text).and_then(|c| c.get(idx)).map(|m| m.as_str())
}

/// 金额 → 分。优先带单位的数字（48 万 / 3280 元 / 480000 CNY），
/// 其次「金额|amount|salary|月薪|价」后面的数字，避免把日期误当金额。
pub fn money_cents(text: &str) -> i64 {
    let with_unit = Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*(万元|万|元|CNY|RMB|yuan)").unwrap();
    if let Some(c) = with_unit.captures(text) {
        let v: f64 = c[1].parse().unwrap_or(0.0);
        let unit = c[2].to_ascii_lowercase();
        let yuan = if unit.contains('万') { v * 10_000.0 } else { v };
        return (yuan * 100.0).round() as i64;
    }
    let labelled = Regex::new(r"(?i)(?:金额|amount|salary|月薪|采购价|price|value)[^\d]{0,12}(\d+(?:\.\d+)?)")
        .unwrap();
    if let Some(c) = labelled.captures(text) {
        let v: f64 = c[1].parse().unwrap_or(0.0);
        return (v * 100.0).round() as i64;
    }
    // 兜底取 5 位以上数字（4 位通常是年份，不当作金额）
    let any = Regex::new(r"\b(\d{5,9})\b").unwrap();
    cap(&any, text, 1)
        .and_then(|x| x.parse::<f64>().ok())
        .map(|v| (v * 100.0).round() as i64)
        .unwrap_or(0)
}

pub fn dept_of(text: &str) -> Option<&'static str> {
    let pairs: &[(&str, &str)] = &[
        ("研发", "D02"), ("R&D", "D02"), ("技术", "D02"), ("engineering", "D02"),
        ("产品", "D03"), ("product", "D03"),
        ("市场", "D04"), ("销售", "D04"), ("sales", "D04"), ("marketing", "D04"),
        ("财务", "D05"), ("finance", "D05"),
        ("人力", "D06"), ("HR", "D06"), ("human resources", "D06"),
        ("行政", "D07"), ("admin", "D07"),
        ("实施", "D08"), ("项目", "D08"), ("delivery", "D08"), ("project", "D08"),
        ("总经办", "D01"), ("executive", "D01"),
    ];
    let lower = text.to_ascii_lowercase();
    pairs
        .iter()
        .find(|(k, _)| lower.contains(&k.to_ascii_lowercase()))
        .map(|(_, v)| *v)
}

/// 从文本中解析员工：优先显式 ID，其次匹配花名册里的中文/英文姓名。
pub async fn employee_of(db: &mut dyn Repo, text: &str) -> Option<String> {
    let id_re = Regex::new(r"\bE\d{2}\b").unwrap();
    if let Some(id) = cap(&id_re, text, 0) {
        return Some(id.to_string());
    }
    let rows = db
        .query_json("SELECT id, name FROM employees", params![])
        .await
        .unwrap_or_default();
    for r in rows {
        let name = r["name"].as_str().unwrap_or("");
        if !name.is_empty() && text.contains(name) {
            return Some(r["id"].as_str().unwrap_or("").to_string());
        }
    }
    None
}

pub fn date_of(text: &str) -> String {
    let iso = Regex::new(r"(\d{4})-(\d{1,2})-(\d{1,2})").unwrap();
    if let Some(c) = iso.captures(text) {
        return format!("{:0>4}-{:0>2}-{:0>2}", &c[1], &c[2], &c[3]);
    }
    let cn = Regex::new(r"(\d{1,2})\s*月\s*(\d{1,2})\s*日").unwrap();
    if let Some(c) = cn.captures(text) {
        let y = chrono_year();
        return format!("{y}-{:0>2}-{:0>2}", &c[1], &c[2]);
    }
    let en = Regex::new(r"(?i)\b(Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+(\d{1,2})\b")
        .unwrap();
    if let Some(c) = en.captures(text) {
        let m = match c[1].to_ascii_lowercase().as_str() {
            "jan" => 1, "feb" => 2, "mar" => 3, "apr" => 4, "may" => 5, "jun" => 6,
            "jul" => 7, "aug" => 8, "sep" => 9, "oct" => 10, "nov" => 11, _ => 12,
        };
        return format!("{}-{:0>2}-{:0>2}", chrono_year(), m, &c[2]);
    }
    String::new()
}

fn chrono_year() -> i64 {
    clock::local_year()
}

pub fn asset_id_of(text: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\b(AST-?\d{3,4})\b").unwrap();
    let s = cap(&re, text, 1)?.to_ascii_uppercase();
    if s.starts_with("AST-") {
        Some(s)
    } else {
        Some(format!("AST-{}", &s[3..]))
    }
}

pub fn claim_id_of(text: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\bBX-\d{4}-\d{4}\b").unwrap();
    cap(&re, text, 0).map(str::to_uppercase)
}

pub fn contract_id_of(text: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\bHT-\d{4}-\d{3}\b").unwrap();
    cap(&re, text, 0).map(str::to_uppercase)
}

pub fn makeup_id_of(text: &str) -> Option<String> {
    let re = Regex::new(r"(?i)\bMK-\d{4}-\d{4}\b").unwrap();
    cap(&re, text, 0).map(str::to_uppercase)
}

pub fn category_of(text: &str) -> &'static str {
    let t = text.to_ascii_lowercase();
    if t.contains("笔记本") || t.contains("laptop") || t.contains("thinkpad") || t.contains("macbook") {
        "laptop"
    } else if t.contains("台式") || t.contains("工作站") || t.contains("desktop") || t.contains("optiplex") {
        "desktop"
    } else if t.contains("打印机") || t.contains("printer") {
        "printer"
    } else if t.contains("u 盘") || t.contains("u盘") || t.contains("优盘") || t.contains("usb") {
        "usb"
    } else if t.contains("光盘") || t.contains("disc") || t.contains(" cd") || t.contains("dvd") {
        "disc"
    } else if t.contains("显示器") || t.contains("monitor") {
        "monitor"
    } else if t.contains("投影") || t.contains("projector") {
        "projector"
    } else {
        "other"
    }
}

fn person_name(text: &str) -> String {
    let zh = Regex::new(r"(?:为|给|帮)?\s*([\p{Han}]{2,4})\s*(?:办理|申请|登记|入职)").unwrap();
    if let Some(n) = cap(&zh, text, 1) {
        return n.to_string();
    }
    let en = Regex::new(r"(?i)\bonboard\s+([A-Z][a-z]+(?:\s+[A-Z][a-z]+)?)").unwrap();
    if let Some(n) = cap(&en, text, 1) {
        return n.to_string();
    }
    "新员工".to_string()
}

async fn first_instock_asset(db: &mut dyn Repo, category: &str) -> Option<String> {
    db.query_json(
        "SELECT id FROM assets WHERE status='instock' AND category=?1 ORDER BY id LIMIT 1",
        params![category],
    )
    .await
    .ok()?
    .first()
    .and_then(|r| r["id"].as_str())
    .map(str::to_string)
}

fn reason_of(text: &str) -> String {
    let re = Regex::new(r"(?:理由|原因|reason)[：:，,\s]*(.{2,40})").unwrap();
    cap(&re, text, 1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Agent 发起".to_string())
}

/// 自然语言 → 执行计划（不写库）。敏感工具不自动附带 confirmed，由调用方决定。
/// 默认操作者 E03（命令行/单测场景）；HTTP/Tauri 走 `plan_for` 传入真实登录员工。
pub async fn plan(db: &mut dyn Repo, message: &str) -> Result<Vec<PlanStep>> {
    plan_for(db, message, "E03").await
}

pub async fn plan_for(
    db: &mut dyn Repo,
    message: &str,
    actor_employee: &str,
) -> Result<Vec<PlanStep>> {
    let t = message.trim();
    let lower = t.to_ascii_lowercase();
    let step = |tool: &str, args: Value| PlanStep { tool: tool.to_string(), args };
    let money = money_cents(t);
    let date = date_of(t);
    let dept = dept_of(t);
    let emp = employee_of(db, t).await;
    let actor_emp = match emp.clone() {
        Some(e) => e,
        None if actor_employee.trim().is_empty() => "E03".to_string(),
        None => actor_employee.trim().to_string(),
    };

    // 1. 帮助 / 能力清单
    if lower.contains("help") || lower.contains("帮助") || lower == "?" || lower == "？" {
        return Ok(vec![]);
    }

    // 2. 审批（补卡 / 合同 / 报销共用「审批」字样，按关键字依次分流）
    if lower.contains("审批") || lower.contains("approve") || lower.contains("reject") || lower.contains("驳回") {
        if t.contains("补卡") || lower.contains("makeup") {
            let mid = match makeup_id_of(t) {
                Some(id) => Some(id),
                None => db
                    .query_json(
                        "SELECT id FROM makeup_requests WHERE status='pending'
                         ORDER BY created_at DESC, id DESC LIMIT 1",
                        params![],
                    )
                    .await
                    .ok()
                    .and_then(|rows| {
                        rows.first()
                            .and_then(|r| r["id"].as_str())
                            .map(str::to_string)
                    }),
            };
            if let Some(mid) = mid {
                let reject = lower.contains("驳回") || lower.contains("reject");
                return Ok(vec![step(
                    "attendance.approve",
                    json!({ "request_id": mid, "decision": if reject { "reject" } else { "approve" } }),
                )]);
            }
        }
        if t.contains("合同") || lower.contains("contract") {
            let cid = match contract_id_of(t) {
                Some(id) => Some(id),
                None => db
                    .query_json(
                        "SELECT id FROM contracts WHERE status='review'
                         ORDER BY sign_date DESC LIMIT 1",
                        params![],
                    )
                    .await
                    .ok()
                    .and_then(|rows| {
                        rows.first()
                            .and_then(|r| r["id"].as_str())
                            .map(str::to_string)
                    }),
            };
            if let Some(cid) = cid {
                let reject = lower.contains("驳回") || lower.contains("reject");
                return Ok(vec![step(
                    "contract.approve",
                    json!({ "contract_id": cid, "decision": if reject { "reject" } else { "approve" } }),
                )]);
            }
        }
        let id = match claim_id_of(t) {
            Some(id) => Some(id),
            None => db
                .query_json(
                    "SELECT id FROM claims WHERE status='pending' ORDER BY submitted_at DESC LIMIT 1",
                    params![],
                )
                .await
                .ok()
                .and_then(|rows| {
                    rows.first()
                        .and_then(|r| r["id"].as_str())
                        .map(str::to_string)
                }),
        };
        if let Some(id) = id {
            let reject = lower.contains("驳回") || lower.contains("reject");
            return Ok(vec![step(
                "finance.claim.approve",
                json!({ "claim_id": id, "decision": if reject { "reject" } else { "approve" } }),
            )]);
        }
    }

    // 3. 离职（工具内部自动冻结账号并回收资产）
    if t.contains("离职") || lower.contains("offboard") || lower.contains("resign") || lower.contains("terminate") {
        if let Some(e) = emp.clone() {
            return Ok(vec![step(
                "hr.offboard",
                json!({
                    "employee_id": e, "last_day": if date.is_empty() { Value::Null } else { json!(date) },
                    "reason": reason_of(t)
                }),
            )]);
        }
    }

    // 4. 入职：三步跨模块编排（建档 → 设备领用 → 劳动合同）
    if t.contains("入职") || lower.contains("onboard") || lower.contains("new hire") {
        let name = person_name(t);
        let position = if lower.contains("前端") || lower.contains("frontend") {
            "前端工程师"
        } else if lower.contains("后端") || lower.contains("backend") {
            "后端工程师"
        } else if lower.contains("engineer") {
            "Engineer"
        } else {
            "员工"
        };
        let salary = if money > 0 { money } else { 1_800_000 };
        let assigned = first_instock_asset(db, "laptop").await;
        let mut steps = vec![step(
            "hr.onboard",
            json!({
                "name": name, "dept_id": dept.unwrap_or("D02"),
                "position": position, "salary_cents": salary,
                "entry_date": if date.is_empty() { Value::Null } else { json!(date) }
            }),
        )];
        if let Some(asset) = assigned {
            steps.push(step(
                "asset.assign",
                json!({ "asset_id": asset, "employee_id": "$0.employee_id" }),
            ));
        }
        steps.push(step(
            "contract.create",
            json!({
                "name": format!("{name} 劳动合同"), "party": name,
                "type": "labor", "amount_cents": 0,
                "sign_date": if date.is_empty() { Value::Null } else { json!(date) }
            }),
        ));
        return Ok(steps);
    }

    // 5. 晋升 / 涨薪
    if t.contains("涨薪") || t.contains("晋升") || lower.contains("promote") || lower.contains("raise") {
        if let Some(e) = emp.clone() {
            return Ok(vec![step(
                "hr.promote",
                json!({
                    "employee_id": e, "new_salary_cents": if money > 0 { money } else { 0 },
                    "effective_date": if date.is_empty() { Value::Null } else { json!(date) },
                    "reason": reason_of(t), "new_position": ""
                }),
            )]);
        }
    }

    // 6. 组织架构
    if t.contains("组织架构") || t.contains("组织") || lower.contains("org chart") || lower.contains("organization") {
        return Ok(vec![step("org.chart", json!({}))]);
    }

    // 7. 花名册
    if t.contains("花名册") || t.contains("在职员工") || t.contains("人员名单")
        || lower.contains("roster") || lower.contains("list") && lower.contains("employee")
    {
        return Ok(vec![step("hr.roster.query", json!({ "dept_id": dept }))]);
    }

    // 8. 合同到期 / 合同查询
    if (t.contains("合同") && (t.contains("到期") || t.contains("查询") || t.contains("检索")))
        || lower.contains("expiring") || lower.contains("list contracts") || lower.contains("search contracts")
    {
        return Ok(vec![step("contract.query", json!({ "expiring_in_days": 30 }))]);
    }

    // 9. 新建合同
    if (t.contains("新建合同") || t.contains("创建合同") || t.contains("签订合同")
        || lower.contains("create") && lower.contains("contract")
        || lower.contains("new contract") || lower.contains("sign a contract"))
        && !lower.contains("expiring")
    {
        let party = if t.contains("恒信") { "恒信科技有限公司" } else { "待指定合作方" };
        return Ok(vec![step(
            "contract.create",
            json!({
                "name": if lower.contains("server") || t.contains("服务器") { "服务器采购合同" } else { "新签合同" },
                "party": party, "type": "purchase", "amount_cents": money,
                "sign_date": if date.is_empty() { Value::Null } else { json!(date) }
            }),
        )]);
    }

    // 10. 补卡
    if t.contains("补卡") || lower.contains("makeup") || lower.contains("make-up punch") {
        return Ok(vec![step(
            "attendance.makeup",
            json!({ "employee_id": actor_emp, "date": if date.is_empty() { Value::Null } else { json!(date) }, "reason": reason_of(t) }),
        )]);
    }

    // 11. 考勤报表
    if (t.contains("考勤") && (t.contains("报表") || t.contains("统计") || t.contains("汇总")))
        || lower.contains("attendance report") || lower.contains("attendance summary")
    {
        let m = Regex::new(r"(\d{1,2})\s*月").unwrap();
        let month = cap(&m, t, 1)
            .map(|x| format!("{}-{:0>2}", chrono_year(), x))
            .unwrap_or_default();
        return Ok(vec![step("attendance.report", json!({ "month": month }))]);
    }

    // 12. 打卡（兼容「打卡」「上班卡」「下班卡」「打卡上班」等口语说法）
    if t.contains("打卡")
        || t.contains("上班卡")
        || t.contains("下班卡")
        || lower.contains("clock in")
        || lower.contains("clock out")
        || lower.contains("clock me in")
        || lower.contains("clock me out")
        || lower.contains("check in")
        || lower.contains("check-in")
        || lower.contains("check out")
        || lower.contains("punch")
    {
        let out = t.contains("下班")
            || lower.contains("clock out")
            || lower.contains("clock me out")
            || lower.contains("check out")
            || lower.contains("punch out");
        return Ok(vec![step(
            "attendance.checkin",
            json!({ "employee_id": actor_emp, "type": if out { "out" } else { "in" }, "source": "agent" }),
        )]);
    }

    // 13. 资产归还
    if t.contains("归还") || t.contains("退回") || lower.contains("return") {
        if let Some(asset) = asset_id_of(t) {
            return Ok(vec![step("asset.return", json!({ "asset_id": asset }))]);
        }
    }

    // 14. 资产领用
    if t.contains("领用") || t.contains("发放") || lower.contains("assign") {
        if let Some(asset) = asset_id_of(t) {
            return Ok(vec![step(
                "asset.assign",
                json!({ "asset_id": asset, "employee_id": emp.unwrap_or_else(|| "E06".to_string()) }),
            )]);
        }
    }

    // 15. 资产登记
    if t.contains("登记") || t.contains("入库") || lower.contains("register") || lower.contains("add asset") {
        let sn_re = Regex::new(r"(?i)\bSN[:：\s]*([A-Za-z0-9-]{4,})\b").unwrap();
        let sn = cap(&sn_re, t, 1).unwrap_or("");
        return Ok(vec![step(
            "asset.register",
            json!({
                "category": category_of(t), "model": "ThinkPad X1 Carbon",
                "serial_no": sn, "price_cents": money, "location": "行政库房"
            }),
        )]);
    }

    // 16. 资产盘点
    if t.contains("盘点") || t.contains("资产查询") || lower.contains("inventory") || lower.contains("list assets") {
        let cat = category_of(t);
        return Ok(vec![step(
            "asset.query",
            json!({
                "category": if cat == "other" { Value::Null } else { json!(cat) },
                "status": if t.contains("在库") || lower.contains("in-stock") { json!("instock") } else { Value::Null }
            }),
        )]);
    }

    // 17. 报销创建（排除「统计…报销金额」这类汇总型指令，避免误建单）
    if (t.contains("报销") || lower.contains("claim") || lower.contains("expense") || lower.contains("reimburse"))
        && !(t.contains("统计") || t.contains("汇总") || lower.contains("summary")
            || lower.contains("summarize") || lower.contains("cost report"))
    {
        let kind = if t.contains("项目实施") || t.contains("项目现场") || lower.contains("project delivery") || lower.contains("project claim") {
            "project"
        } else if t.contains("商务") || t.contains("客户") || lower.contains("business") || lower.contains("client") {
            "business"
        } else {
            "daily"
        };
        return Ok(vec![step(
            "finance.claim.create",
            json!({
                "type": kind, "applicant_id": emp.unwrap_or_else(|| actor_emp.clone()),
                "amount_cents": money, "purpose": "Agent 发起的报销申请",
                "project": if kind == "project" { json!("恒信科技数字化平台") } else { Value::Null }
            }),
        )]);
    }

    // 18. 费用统计 / 总览
    if t.contains("统计") || t.contains("汇总") || t.contains("总览") || lower.contains("summary")
        || lower.contains("summarize") || lower.contains("cost report")
    {
        return Ok(vec![step("finance.summary", json!({}))]);
    }

    Ok(vec![])
}

/// 解析 `$<step_index>.<field>` 占位符，实现跨步骤参数传递。
fn resolve_args(args: &Value, results: &[Value]) -> Value {
    let re = Regex::new(r"^\$(\d+)\.([A-Za-z_]+)$").unwrap();
    let mut out = Map::new();
    if let Value::Object(obj) = args {
        for (k, v) in obj {
            let nv = match v {
                Value::String(s) if s.starts_with('$') => {
                    if let Some(c) = re.captures(s) {
                        let i: usize = c[1].parse().unwrap_or(usize::MAX);
                        results
                            .get(i)
                            .and_then(|r| r.get(&c[2]).cloned())
                            .unwrap_or_else(|| v.clone())
                    } else {
                        v.clone()
                    }
                }
                _ => v.clone(),
            };
            if !nv.is_null() {
                out.insert(k.clone(), nv);
            }
        }
    }
    Value::Object(out)
}

pub fn help_text(en: bool) -> String {
    if en {
        format!(
            "I can operate all OA workflows. {} registered tools: {}. Try: \"Onboard Xu Zhiyuan as Frontend Engineer in R&D Center, salary 18000\", \"Approve claim BX-2026-0048\", \"Approve contract HT-2026-013\", \"List contracts expiring soon\", \"Inventory in-stock laptops\".",
            domain::TOOL_NAMES.len(),
            domain::TOOL_NAMES.join(", ")
        )
    } else {
        format!(
            "我可以代你操作人事、财务、合同、考勤、资产的全部流程，已注册 {} 个工具：{}。\n例如：「为许知远办理入职，研发中心 / 前端工程师，月薪 18000」「审批通过 BX-2026-0048」「审批合同 HT-2026-013」「查询即将到期的合同」「盘点在库的电脑类资产」。",
            domain::TOOL_NAMES.len(),
            domain::TOOL_NAMES.join("、")
        )
    }
}

/// 按字符截断，用于思考日志里展示原始输入而不刷屏
fn brief(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let head: String = flat.chars().take(max).collect();
    format!("{head}…")
}

/// 本地模式：执行计划 → 逐步调用工具 → 汇总回复与轨迹。
/// 身份来自 `SessionUser`，工具内部的 RBAC 与审计都使用真实员工号。
pub async fn run_local(state: AppState, user: &SessionUser, message: &str) -> Result<AgentReply> {
    run_local_stream(state, user, message, super::Thinker::silent()).await
}

/// 同 [`run_local`]，但把每一步决策实时推给 `thinker`（流式接口用）。
pub async fn run_local_stream(
    state: AppState,
    user: &SessionUser,
    message: &str,
    mut thinker: super::Thinker,
) -> Result<AgentReply> {
    // 复制一份连接池句柄（Clone 只复制 Arc），避免与后续事务争用
    let mut store = state.store.clone();
    thinker.say("intent", format!("解析指令：{}", brief(message, 60)));
    let steps = plan_for(&mut store, message, &user.employee_id).await?;
    let en = !has_han(message);
    if steps.is_empty() {
        thinker.say("intent", "未命中任何业务流程，返回能力清单");
        let mut out = AgentReply::new(help_text(en), "local");
        out.reply.push_str(if en {
            "\n\nCurrent user: {display} ({role}); permissions are enforced server-side."
        } else {
            "\n\n当前账号：{display}（{role}），工具权限由服务端会话实时校验。"
        }
        .replace("{display}", &user.display_name)
        .replace("{role}", &user.role)
        .as_str());
        out.thinking = thinker.steps();
        return Ok(out);
    }
    let plan_line = steps.iter().map(|s| s.tool.as_str()).collect::<Vec<_>>().join(" → ");
    thinker.say("plan", format!("规划 {} 步：{}", steps.len(), plan_line));

    let mut results: Vec<Value> = Vec::new();
    let mut trace: Vec<TraceEntry> = Vec::new();
    let mut pending: Option<PlanStep> = None;
    let mut affected: Vec<String> = Vec::new();

    let total = steps.len();
    for (index, step) in steps.iter().enumerate() {
        let args = resolve_args(&step.args, &results);
        thinker.say(
            "tool",
            format!(
                "第 {}/{} 步：调用 `{}`，参数 {}",
                index + 1, total, step.tool,
                brief(&args.to_string(), 120)
            ),
        );
        let result = tools::call(state.clone(), user, &step.tool, args.clone()).await;
        let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let outcome = result
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| result.get("error").and_then(Value::as_str))
            .unwrap_or(if ok { "ok" } else { "failed" });
        thinker.say(
            "tool",
            format!("`{}` {}：{}", step.tool, if ok { "成功" } else { "失败" }, brief(outcome, 100)),
        );
        if ok {
            let domain = tools::spec_domain(&step.tool);
            if !domain.is_empty() && !affected.contains(&domain) {
                affected.push(domain);
            }
        }
        if result
            .get("needs_confirmation")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            pending = Some(PlanStep { tool: step.tool.clone(), args: args.clone() });
            thinker.say("guard", format!("`{}` 属于敏感操作，暂停等待用户确认", step.tool));
        }
        trace.push(TraceEntry {
            tool: step.tool.clone(),
            args,
            result: result.clone(),
            ok,
        });
        results.push(result);
        if pending.is_some() || !ok {
            break;
        }
    }

    let messages: Vec<String> = results
        .iter()
        .filter_map(|r| r.get("message").and_then(Value::as_str).map(str::to_string))
        .collect();
    let last = results.last().cloned().unwrap_or(json!({}));
    let denied = last.get("denied").and_then(Value::as_bool).unwrap_or(false);
    let mut reply = if let Some(p) = &pending {
        if en {
            format!(
                "Step `{}` is a sensitive change and needs your confirmation before it is written. Confirm to execute it.",
                p.tool
            )
        } else {
            format!("`{}` 属于敏感操作，需要你确认后才会写入数据库。确认即执行。", p.tool)
        }
    } else if denied {
        let msg = last
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("permission denied");
        if en {
            format!("Permission denied, nothing was written: {msg}")
        } else {
            format!("权限不足，操作未执行：{msg}")
        }
    } else if messages.is_empty() {
        if en {
            "No matching workflow found. Type 'help' for capabilities.".into()
        } else {
            "未识别到可执行流程，输入「帮助」查看全部能力。".into()
        }
    } else {
        let head = if en {
            format!("Executed {} step(s):\n", trace.len())
        } else {
            format!("已完成 {} 步操作：\n", trace.len())
        };
        head + &messages
            .iter()
            .enumerate()
            .map(|(i, m)| format!("{}. {}", i + 1, m))
            .collect::<Vec<_>>()
            .join("\n")
    };
    if !trace.is_empty() && !denied {
        reply.push_str(if en {
            "\n\nAll changes were written to the database and audited."
        } else {
            "\n\n以上变更均已写入数据库并记录审计日志。"
        });
    }

    thinker.say(
        "done",
        match (pending.is_some(), denied) {
            (true, _) => format!("执行 {} 步后暂停，等待确认", trace.len()),
            (_, true) => format!("执行 {} 步，最后一步被权限拒绝", trace.len()),
            _ => format!("执行 {} 步，变更已写入并审计", trace.len()),
        },
    );
    let thinking = thinker.steps();

    Ok(AgentReply {
        reply,
        mode: "local".into(),
        trace,
        plan: steps,
        thinking,
        needs_confirmation: pending.is_some(),
        pending,
        affected,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn money_parsing() {
        assert_eq!(money_cents("金额 3280 元"), 328000);
        // 48 万 = 480,000 元 = 48,000,000 分，与下面 480000 CNY 等价
        assert_eq!(money_cents("金额 48 万"), 48_000_000);
        assert_eq!(money_cents("value 480000 CNY"), 48_000_000);
        assert_eq!(money_cents("salary 18000"), 1_800_000);
    }

    #[test]
    fn date_and_ids() {
        assert_eq!(date_of("2026-11-20 签署"), "2026-11-20");
        assert_eq!(date_of("11月18日报到"), "2026-11-18");
        assert_eq!(date_of("Nov 18 onboard"), "2026-11-18");
        assert_eq!(claim_id_of("审批通过 BX-2026-0048"), Some("BX-2026-0048".into()));
        assert_eq!(asset_id_of("把 AST-0008 领用给刘洋"), Some("AST-0008".into()));
    }

    #[tokio::test]
    async fn plan_covers_all_domains_in_both_languages() {
        let mut db = Store::open_in_memory().await.unwrap();
        let cases = [
            ("为许知远办理入职，研发中心 / 前端工程师，11月18日报到，月薪 18000", "hr.onboard"),
            ("给李娜涨薪到 26000，12月1日生效", "hr.promote"),
            ("王强将于 11月30日离职", "hr.offboard"),
            ("查询研发中心在职员工花名册", "hr.roster.query"),
            ("查询即将到期的合同", "contract.query"),
            ("新建合同：与恒信科技签订服务器采购合同，金额 48 万", "contract.create"),
            ("提交一笔商务报销：申请人陈静，金额 3280 元", "finance.claim.create"),
            ("审批通过 BX-2026-0048", "finance.claim.approve"),
            ("审批合同 HT-2026-013 的法务节点", "contract.approve"),
            ("审批通过补卡申请 MK-2026-0001", "attendance.approve"),
            ("统计本月各部门报销金额", "finance.summary"),
            ("帮我打上班卡", "attendance.checkin"),
            ("赵敏 11月7日补卡", "attendance.makeup"),
            ("生成 11 月考勤报表", "attendance.report"),
            ("登记一台 ThinkPad X1 Carbon 笔记本，SN: PF4A9921，采购价 12800 元", "asset.register"),
            ("把 AST-0008 领用给刘洋", "asset.assign"),
            ("归还资产 AST-0007", "asset.return"),
            ("盘点在库的电脑类资产", "asset.query"),
            ("Onboard Xu Zhiyuan as Frontend Engineer in R&D Center, salary 18000", "hr.onboard"),
            ("Approve claim BX-2026-0048", "finance.claim.approve"),
            ("Approve contract HT-2026-013", "contract.approve"),
            ("Approve punch makeup MK-2026-0001", "attendance.approve"),
            ("List contracts expiring soon", "contract.query"),
            ("Clock me in for work", "attendance.checkin"),
            ("Register a ThinkPad X1 Carbon laptop, SN PF4A9921, price 12800 CNY", "asset.register"),
        ];
        for (cmd, expected) in cases {
            let steps = plan(&mut db, cmd).await.unwrap();
            assert!(!steps.is_empty(), "no plan for: {cmd}");
            assert_eq!(steps[0].tool, expected, "wrong tool for: {cmd}");
        }
        assert_eq!(steps_for(&mut db, "组织架构").await[0].tool, "org.chart");
    }

    async fn steps_for(db: &mut dyn Repo, s: &str) -> Vec<PlanStep> {
        plan(db, s).await.unwrap()
    }

    #[tokio::test]
    async fn onboarding_is_multi_step_with_cross_module_ref() {
        let mut db = Store::open_in_memory().await.unwrap();
        let steps = plan(&mut db, "为许知远办理入职，研发中心前端工程师，月薪 18000")
            .await
            .unwrap();
        assert_eq!(steps.len(), 3, "onboarding should chain 3 tools");
        assert_eq!(steps[0].tool, "hr.onboard");
        assert_eq!(steps[1].tool, "asset.assign");
        assert_eq!(steps[1].args["employee_id"], "$0.employee_id");
        assert_eq!(steps[2].tool, "contract.create");
        let resolved = resolve_args(&steps[1].args, &[json!({ "employee_id": "E17" })]);
        assert_eq!(resolved["employee_id"], "E17");
    }

    #[tokio::test]
    async fn help_has_no_steps() {
        let mut db = Store::open_in_memory().await.unwrap();
        assert!(plan(&mut db, "帮助").await.unwrap().is_empty());
        assert!(help_text(false).contains(&domain::TOOL_NAMES.len().to_string()));
    }

    #[tokio::test]
    async fn run_local_executes_multi_step_and_stops_for_confirmation() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, admin) = app
            .store
            .login("evan", crate::auth::DEFAULT_PASSWORD)
            .await
            .unwrap();
        // 入职编排：建档 → 设备领用 → 劳动合同
        let reply = run_local(
            app.clone(),
            &admin,
            "为许知远办理入职，研发中心 / 前端工程师，11月18日报到，月薪 18000",
        )
        .await
        .unwrap();
        assert_eq!(reply.mode, "local");
        assert!(reply.trace.len() >= 2, "expected multi-step trace, got {:?}", reply.trace);
        assert!(reply.affected.contains(&"hr".to_string()));
        // 第三步 contract.create 属敏感操作，应停在确认门禁而不是静默签署
        assert_eq!(reply.pending.as_ref().map(|p| p.tool.as_str()), Some("contract.create"));

        // 敏感操作：涨薪必须先停在确认门禁，且不写库
        let blocked = run_local(app.clone(), &admin, "给李娜涨薪到 26000").await.unwrap();
        assert!(blocked.needs_confirmation);
        let pending = blocked.pending.expect("pending step");
        assert_eq!(pending.tool, "hr.promote");
        let salary = app
            .store
            .query_json("SELECT salary_cents FROM employees WHERE id='E03'", params![])
            .await
            .unwrap();
        assert_eq!(salary[0]["salary_cents"].as_i64().unwrap(), 2_400_000);
    }

    #[tokio::test]
    async fn run_local_reports_permission_denied_without_writing() {
        let app = AppState::new(Store::open_in_memory().await.unwrap());
        let (_, employee) = app
            .store
            .login("lina", crate::auth::DEFAULT_PASSWORD)
            .await
            .unwrap();
        let reply = run_local(
            app.clone(),
            &employee,
            "为许知远办理入职，研发中心前端工程师，月薪 18000",
        )
        .await
        .unwrap();
        assert!(reply.trace.first().map(|t| !t.ok).unwrap_or(false));
        assert!(reply.reply.contains("权限不足"));
        let n = app
            .store
            .scalar_i64("SELECT COUNT(*) FROM employees WHERE name='许知远'", params![])
            .await
            .unwrap();
        assert_eq!(n, 0, "denied onboarding must not create employees");
    }
}
