//! 领域模型、业务规则、状态机与功能验收矩阵。
//!
//! 金额一律用「分」(i64) 存储；前端表单同时提交 `amount`(元) 与 `amount_cents`(分)，
//! 由 tools.rs 的 `cents()` 统一归一化。所有平台共用本模块，无 IO。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmployeeStatus {
    Active,
    Probation,
    Leaving,
    Left,
}

impl EmployeeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Probation => "probation",
            Self::Leaving => "leaving",
            Self::Left => "left",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "probation" => Some(Self::Probation),
            "leaving" => Some(Self::Leaving),
            "left" => Some(Self::Left),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Employee {
    pub id: String,
    pub no: String,
    pub name: String,
    pub dept_id: String,
    pub position: String,
    pub status: EmployeeStatus,
    pub entry_date: String,
    pub salary_cents: i64,
    pub manager_id: Option<String>,
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimType {
    Daily,
    Business,
    Project,
}

impl ClaimType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Business => "business",
            Self::Project => "project",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "daily" | "日常" => Some(Self::Daily),
            "business" | "商务" => Some(Self::Business),
            "project" | "项目实施" => Some(Self::Project),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Draft,
    Pending,
    Approved,
    Rejected,
    Paid,
}

impl ClaimStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Paid => "paid",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "draft" => Some(Self::Draft),
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "rejected" => Some(Self::Rejected),
            "paid" => Some(Self::Paid),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claim {
    pub id: String,
    pub kind: ClaimType,
    pub applicant_id: String,
    pub dept_id: String,
    pub amount_cents: i64,
    pub purpose: String,
    pub project: Option<String>,
    pub status: ClaimStatus,
    pub submitted_at: String,
    pub approver_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contract {
    pub id: String,
    pub name: String,
    pub party: String,
    pub kind: String,
    pub amount_cents: i64,
    pub sign_date: String,
    pub expire_date: String,
    pub status: String,
    pub owner_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttendanceRecord {
    pub id: String,
    pub employee_id: String,
    pub work_date: String,
    pub clock_in: Option<String>,
    pub clock_out: Option<String>,
    pub status: String,
    pub source: String,
    pub hours: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub category: String,
    pub model: String,
    pub serial_no: String,
    pub status: String,
    pub holder_id: Option<String>,
    pub location: String,
    pub price_cents: i64,
    pub qr_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HrCase {
    pub id: String,
    pub kind: String,
    pub employee_id: String,
    pub effective_date: String,
    pub reason: String,
    pub status: String,
    pub step_index: i64,
    pub steps: String,
}

/// 报销单状态机：拒绝任何非法跃迁（例如 draft → paid、approved → pending）。
///
/// `Pending → Draft` 是发起人撤回（审批中可收回重编辑），
/// `Rejected → Pending` 是驳回后修改重提。
pub fn can_transition(from: &ClaimStatus, to: &ClaimStatus) -> bool {
    use ClaimStatus::*;
    matches!(
        (from, to),
        (Draft, Pending)
            | (Pending, Draft)
            | (Pending, Approved)
            | (Pending, Rejected)
            | (Approved, Paid)
            | (Rejected, Pending)
    )
}

/// 合同状态机。
pub fn can_transition_contract(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("review", "effective")
            | ("review", "rejected")
            | ("effective", "expiring")
            | ("expiring", "archived")
            | ("effective", "archived")
    )
}

/// 资产状态机（repair 只能由 inuse/instock 进入）。
pub fn can_transition_asset(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("instock", "inuse")
            | ("inuse", "instock")
            | ("instock", "repair")
            | ("inuse", "repair")
            | ("repair", "instock")
            | ("instock", "scrapped")
            | ("inuse", "scrapped")
    )
}

pub const WORK_START_MIN: i64 = 9 * 60;
pub const WORK_END_MIN: i64 = 17 * 60 + 30;
pub const LUNCH_MIN: i64 = 60;

/// "HH:MM" → 距零点分钟数。
pub fn parse_time(hhmm: &str) -> Option<i64> {
    let mut it = hhmm.trim().split(':');
    let h = it.next()?.parse::<i64>().ok()?;
    let m = it.next()?.parse::<i64>().ok()?;
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
        return None;
    }
    Some(h * 60 + m)
}

/// 打卡异常判定：晚于 09:00 上班记 late，早于 17:30 下班记 early。
pub fn punch_status(kind: &str, time: &str) -> &'static str {
    let Some(min) = parse_time(time) else {
        return "normal";
    };
    match kind {
        "out" if min < WORK_END_MIN => "early",
        "in" if min > WORK_START_MIN => "late",
        _ => "normal",
    }
}

/// 计算工时（扣除 1 小时午休，保留 1 位小数，下限 0）。
pub fn work_hours(clock_in: Option<&str>, clock_out: Option<&str>) -> f64 {
    let (Some(a), Some(b)) = (clock_in.and_then(parse_time), clock_out.and_then(parse_time)) else {
        return 0.0;
    };
    let raw = (b - a - LUNCH_MIN) as f64 / 60.0;
    (raw.max(0.0) * 10.0).round() / 10.0
}

/// 按金额与业务类型分派审批链（分单位）。
///
/// 返回的是**角色标签序列**，只决定有几级、每级是什么角色；
/// 具体审批人由 `tools::resolve_approvers` 按部门主管 / 上级链路解析成员工号。
pub fn approval_chain(cents: i64, kind: &str) -> Vec<&'static str> {
    let mut chain = vec!["直属主管"];
    if kind == "project" || cents >= 500_000 {
        // ≥ 5,000 元
        chain.push("财务经理");
    }
    if cents >= 2_000_000 {
        // ≥ 20,000 元
        chain.push("部门总监");
    }
    if cents >= 10_000_000 {
        // ≥ 100,000 元
        chain.push("总经理");
    }
    chain
}

/// 调薪涨幅超过 20% 需总经理终审。
pub fn raise_chain(old_cents: i64, new_cents: i64) -> Vec<&'static str> {
    let mut chain = vec!["直属主管", "HRBP", "部门总监"];
    if old_cents > 0 && (new_cents - old_cents) * 100 / old_cents >= 20 {
        chain.push("总经理");
    }
    chain.push("生效");
    chain
}

/// 分 → 千分位金额字符串（例：1234567 → "12,345.67"）。
pub fn format_cents(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let v = cents.unsigned_abs();
    let int = v / 100;
    let dec = v % 100;
    let raw = int.to_string();
    let mut grouped = String::new();
    for (i, ch) in raw.chars().enumerate() {
        if i > 0 && (raw.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}{grouped}.{dec:02}")
}

/// 敏感操作清单：Agent 与表单都必须经人工二次确认。
pub const SENSITIVE_TOOLS: &[&str] = &[
    "hr.offboard",
    "hr.promote",
    "finance.claim.approve",
    "contract.create",
    "contract.approve",
    "attendance.approve",
];

pub const TOOL_NAMES: &[&str] = &[
    "hr.onboard",
    "hr.offboard",
    "hr.promote",
    "hr.roster.query",
    "org.chart",
    "finance.claim.create",
    "finance.claim.approve",
    "finance.summary",
    "contract.create",
    "contract.approve",
    "contract.query",
    "attendance.checkin",
    "attendance.makeup",
    "attendance.approve",
    "attendance.report",
    "asset.register",
    "asset.assign",
    "asset.return",
    "asset.query",
];

pub const TABLE_NAMES: &[&str] = &[
    "departments",
    "employees",
    "hr_cases",
    "claims",
    "claim_items",
    "contracts",
    "attendance",
    "makeup_requests",
    "assets",
    "budgets",
    "approval_flows",
    "payments",
    "asset_events",
    "notifications",
    "audit_events",
];

/// 报销明细的费用类型。用英文 key 便于前端 i18n，落库时存 key 本身。
pub const CLAIM_ITEM_CATEGORIES: &[&str] = &["transport", "hotel", "meal", "office", "other"];

/// 费用标准（分）：按费用类型给**单条明细**定上限，避免只拿汇总总额做校验。
/// 住宿/交通按一次差旅多晚/多段合计，餐饮按人次；超标必须填写说明（见 `parse_claim_items`）。
pub const CLAIM_ITEM_STANDARDS: &[(&str, i64)] = &[
    ("transport", 500_000), // 交通 5000 元/次（含往返、多段）
    ("hotel", 500_000),     // 住宿 5000 元/次（多晚合计）
    ("meal", 100_000),      // 餐饮 1000 元/人次
    ("office", 300_000),    // 办公用品 3000 元/次
    ("other", 500_000),     // 其他 5000 元/次
];

/// 某费用类型的标准（分）；未知类型返回 None（不参与校验）。
pub fn item_standard(category: &str) -> Option<i64> {
    CLAIM_ITEM_STANDARDS
        .iter()
        .find(|(c, _)| *c == category)
        .map(|(_, cap)| *cap)
}

/// 单条明细是否超标（未知类型不判超标）。
pub fn item_over_standard(category: &str, amount_cents: i64) -> bool {
    item_standard(category).is_some_and(|cap| amount_cents > cap)
}

/// 功能完整性验收矩阵：(能力编号, 业务域, 验收点)。
/// 测试与 /api/features 都读取此常量，保证「声明的能力」与「被测的能力」是同一份。
pub const FEATURES: &[(&str, &str, &str)] = &[
    ("HR-01", "hr", "入职建档 + 工号唯一 + 入职流程与审批链"),
    ("HR-02", "hr", "离职冻结账号 + 自动回收名下资产 + 交接通知"),
    ("HR-03", "hr", "晋升/涨薪写异动单 + 按涨幅分派审批链"),
    ("HR-04", "hr", "花名册按部门/状态/关键词检索且排除已离职"),
    ("HR-05", "org", "组织架构树 + 各节点实时在编人数"),
    ("FIN-01", "finance", "三类报销创建 + 金额校验 + 审批链"),
    ("FIN-02", "finance", "预算占用与超额拦截，驳回释放预算"),
    ("FIN-03", "finance", "审批状态机拒绝非法跃迁，通过后进入付款队列"),
    ("FIN-04", "finance", "按类型/部门/月份汇总与预算执行率"),
    ("FIN-05", "finance", "费用类型标准校验：超标明细必须填写说明才可提交"),
    ("CT-01", "contract", "合同台账创建 + 法务/用印审批"),
    ("CT-02", "contract", "关键词检索 + N 天内到期预警"),
    ("CT-03", "contract", "法务/用印两级审批按具体审批人逐级流转并留痕"),
    ("AT-01", "attendance", "上下班打卡 + 迟到早退判定 + 工时计算"),
    ("AT-02", "attendance", "补卡申请按主管逐级审批，通过后才补入考勤"),
    ("AT-03", "attendance", "月度报表按人汇总：出勤率/迟到/早退/缺卡/外勤/请假/工时"),
    ("AT-04", "attendance", "同人同日只允许一条待审批补卡，防止重复补卡"),
    ("AS-01", "asset", "资产登记（SN 唯一 + 二维码）"),
    ("AS-02", "asset", "领用占用自动改派 + 责任人写入"),
    ("AS-03", "asset", "归还释放责任人 + 状态回写"),
    ("AS-04", "asset", "盘点按类别/状态检索 + 原值合计"),
    ("AG-01", "agent", "19 个工具全部注册且可调用"),
    ("AG-02", "agent", "自然语言意图解析（中英双语）"),
    ("AG-03", "agent", "多步编排：入职联动资产/合同，离职联动回收"),
    ("AG-04", "agent", "敏感操作人工确认门禁"),
    ("AG-05", "agent", "每次调用全量审计（工具/参数/操作者）"),
    ("AG-06", "agent", "无 LLM 密钥时本地意图引擎仍可驱动全部流程"),
    ("PL-01", "platform", "REST 与 Tauri 双通道命令对齐"),
    ("PL-02", "platform", "SQLite 离线库 + 幂等 seed + 一键重置"),
    ("PL-03", "platform", "工作台 KPI 与审计查询"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_state_machine_rejects_illegal_jumps() {
        assert!(can_transition(&ClaimStatus::Pending, &ClaimStatus::Approved));
        assert!(can_transition(&ClaimStatus::Approved, &ClaimStatus::Paid));
        assert!(!can_transition(&ClaimStatus::Draft, &ClaimStatus::Paid));
        assert!(!can_transition(&ClaimStatus::Rejected, &ClaimStatus::Approved));
        assert!(!can_transition(&ClaimStatus::Paid, &ClaimStatus::Pending));
    }

    #[test]
    fn punch_rules() {
        assert_eq!(punch_status("in", "08:52"), "normal");
        assert_eq!(punch_status("in", "09:16"), "late");
        // 规则是「早于 17:30 下班记 early」，17:35 已在下班时间之后，应判 normal
        assert_eq!(punch_status("out", "17:20"), "early");
        assert_eq!(punch_status("out", "17:35"), "normal");
        assert_eq!(punch_status("out", "18:12"), "normal");
        assert_eq!(punch_status("in", "bad"), "normal");
        assert_eq!(parse_time("25:00"), None);
    }

    #[test]
    fn work_hours_deducts_lunch() {
        assert_eq!(work_hours(Some("08:52"), Some("18:12")), 8.3);
        assert_eq!(work_hours(Some("09:00"), None), 0.0);
    }

    #[test]
    fn approval_chain_scales_with_amount() {
        assert_eq!(approval_chain(48_600, "daily"), vec!["直属主管"]);
        assert!(approval_chain(1_260_000, "business").contains(&"财务经理"));
        assert!(approval_chain(48_000_000, "purchase").contains(&"总经理"));
        assert_eq!(raise_chain(24_000_00, 26_000_00).last(), Some(&"生效"));
        assert!(raise_chain(10_000_00, 20_000_00).contains(&"总经理"));
    }

    #[test]
    fn expense_standards_cover_every_category() {
        for category in CLAIM_ITEM_CATEGORIES {
            assert!(item_standard(category).is_some(), "missing standard for {category}");
        }
        assert!(item_over_standard("hotel", 500_001));
        assert!(!item_over_standard("hotel", 500_000));
        // 未知类型不参与标准校验，避免自定义类别被误拦
        assert!(!item_over_standard("unknown", 99_999_999));
    }

    #[test]
    fn asset_and_contract_state_machines() {
        assert!(can_transition_asset("instock", "inuse"));
        assert!(!can_transition_asset("repair", "inuse"));
        assert!(can_transition_contract("review", "effective"));
        assert!(!can_transition_contract("archived", "effective"));
    }

    #[test]
    fn matrix_and_tool_list_are_consistent() {
        assert_eq!(TOOL_NAMES.len(), 19);
        assert_eq!(TABLE_NAMES.len(), 15);
        assert!(FEATURES.len() >= 25);
        let mut sorted = TOOL_NAMES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 19, "tool names must be unique");
    }
}
