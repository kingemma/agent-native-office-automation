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

/// 默认上班时间 09:00；管理员可在设置里改。
pub const WORK_START_MIN: i64 = 9 * 60;
/// 默认下班时间 18:00；管理员可在设置里改。
pub const WORK_END_MIN: i64 = 18 * 60;
/// 默认午休 60 分钟。
pub const LUNCH_MIN: i64 = 60;

/// 考勤时间策略：上下班时间与午休时长，存 `settings.attendance.policy`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttendancePolicy {
    pub work_start: String,
    pub work_end: String,
    pub lunch_minutes: i64,
}

impl Default for AttendancePolicy {
    fn default() -> Self {
        Self {
            work_start: "09:00".to_string(),
            work_end: "18:00".to_string(),
            lunch_minutes: LUNCH_MIN,
        }
    }
}

impl AttendancePolicy {
    pub fn start_min(&self) -> i64 {
        parse_time(&self.work_start).unwrap_or(WORK_START_MIN)
    }
    pub fn end_min(&self) -> i64 {
        parse_time(&self.work_end).unwrap_or(WORK_END_MIN)
    }
}

/// 免考勤的适用范围。
pub const EXEMPT_SCOPES: &[&str] = &["employee", "department"];

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

/// 打卡异常判定（使用显式上下班时间）：晚于上班记 late，早于下班记 early。
pub fn punch_status_with(kind: &str, time: &str, start_min: i64, end_min: i64) -> &'static str {
    let Some(min) = parse_time(time) else {
        return "normal";
    };
    match kind {
        "out" if min < end_min => "early",
        "in" if min > start_min => "late",
        _ => "normal",
    }
}

/// 打卡异常判定（默认 09:00 / 18:00）。
pub fn punch_status(kind: &str, time: &str) -> &'static str {
    punch_status_with(kind, time, WORK_START_MIN, WORK_END_MIN)
}

/// 计算工时（扣除指定午休分钟数，保留 1 位小数，下限 0）。
pub fn work_hours_with(
    clock_in: Option<&str>,
    clock_out: Option<&str>,
    lunch_min: i64,
) -> f64 {
    let (Some(a), Some(b)) = (clock_in.and_then(parse_time), clock_out.and_then(parse_time)) else {
        return 0.0;
    };
    let raw = (b - a - lunch_min) as f64 / 60.0;
    (raw.max(0.0) * 10.0).round() / 10.0
}

/// 计算工时（默认午休 60 分钟）。
pub fn work_hours(clock_in: Option<&str>, clock_out: Option<&str>) -> f64 {
    work_hours_with(clock_in, clock_out, LUNCH_MIN)
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
    "hr.approve",
    "finance.claim.approve",
    "contract.create",
    "contract.approve",
    "attendance.approve",
    "attendance.leave.approve",
    "attendance.overtime.approve",
];

/// 请假类型（参考钉钉默认口径）。前端按 key 做 i18n，落库存 key。
pub const LEAVE_TYPES: &[&str] = &[
    "annual",       // 年假
    "personal",     // 事假
    "sick",         // 病假
    "compensatory", // 调休
    "marriage",     // 婚假
    "maternity",    // 产假
    "paternity",    // 陪产假
    "bereavement",  // 丧假
    "lactation",    // 哺乳假
    "work_injury",  // 工伤假
    "travel",       // 路途假
    "other",        // 其他
];

/// 需要扣减额度的假期类型：年假与调休。
pub const LEAVE_BALANCE_TYPES: &[&str] = &["annual", "compensatory"];

/// 请假审批链：≤3 天为主管 + 总监；>3 天追加 HRBP。
pub fn leave_chain(days: i64) -> Vec<&'static str> {
    if days > 3 {
        vec!["直属主管", "HRBP", "部门总监"]
    } else {
        vec!["直属主管", "部门总监"]
    }
}

/// 区间内的工作日列表（含起止、排除周六日，按本地日期字符串返回）。
/// 法定节假日没有内置数据，报表口径以「周一至周五」为准。
pub fn workdays_in(start: &str, end: &str) -> Vec<String> {
    use chrono::{Datelike, NaiveDate};
    let (Ok(start), Ok(end)) = (
        NaiveDate::parse_from_str(start.trim(), "%Y-%m-%d"),
        NaiveDate::parse_from_str(end.trim(), "%Y-%m-%d"),
    ) else {
        return Vec::new();
    };
    if end < start {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = start;
    while cur <= end {
        if cur.weekday().num_days_from_monday() < 5 {
            out.push(cur.format("%Y-%m-%d").to_string());
        }
        let Some(next) = cur.succ_opt() else { break };
        cur = next;
    }
    out
}

/// 工作日天数；区间非法或没有工作日时返回 0。
pub fn workdays_between(start: &str, end: &str) -> i64 {
    workdays_in(start, end).len() as i64
}

/// 加班申请默认开始时间。
pub const OVERTIME_DEFAULT_START: &str = "18:00";

/// 加班转调休折算标准：8 小时 = 1 天。
pub const OVERTIME_HOURS_PER_DAY: f64 = 8.0;

/// 每月补卡默认次数；管理员可按员工单独覆盖。
pub const DEFAULT_MAKEUP_QUOTA: i64 = 3;

/// 加班时长（小时，保留 1 位小数）。
/// `end <= start` 视为跨天（如 18:00 ~ 02:00 = 8h）；相等或超过 16 小时返回 `None`。
pub fn overtime_hours(start: &str, end: &str) -> Option<f64> {
    let a = parse_time(start)?;
    let b = parse_time(end)?;
    let mins = if b > a {
        b - a
    } else if b < a {
        b + 24 * 60 - a
    } else {
        return None;
    };
    if mins > 16 * 60 {
        return None;
    }
    Some((mins as f64 / 60.0 * 10.0).round() / 10.0)
}

/// 加班审批链：≤4 小时直属主管；>4 小时追加部门总监。
pub fn overtime_chain(hours: f64) -> Vec<&'static str> {
    if hours > 4.0 {
        vec!["直属主管", "部门总监"]
    } else {
        vec!["直属主管"]
    }
}

pub const TOOL_NAMES: &[&str] = &[
    "hr.onboard",
    "hr.offboard",
    "hr.promote",
    "hr.approve",
    "hr.roster.query",
    "hr.leave.balance.set",
    "hr.makeup.quota",
    "hr.attendance.exempt",
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
    "attendance.leave",
    "attendance.leave.approve",
    "attendance.leave.cancel",
    "attendance.overtime",
    "attendance.overtime.approve",
    "attendance.overtime.cancel",
    "asset.register",
    "asset.assign",
    "asset.return",
    "asset.query",
    "custom.field.define",
    "custom.field.list",
    "custom.field.remove",
];

pub const TABLE_NAMES: &[&str] = &[
    "departments",
    "employees",
    "hr_cases",
    "claims",
    "claim_items",
    "contracts",
    "attendance",
    "attendance_quotas",
    "attendance_exemptions",
    "makeup_requests",
    "leave_requests",
    "leave_balances",
    "leave_balance_logs",
    "overtime_requests",
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
    ("AG-01", "agent", "32 个工具全部注册且可调用"),
    ("AG-02", "agent", "自然语言意图解析（中英双语）"),
    ("AG-03", "agent", "多步编排：入职联动资产/合同，离职联动回收"),
    ("AG-04", "agent", "敏感操作人工确认门禁"),
    ("AG-05", "agent", "每次调用全量审计（工具/参数/操作者）"),
    ("AG-06", "agent", "无 LLM 密钥时本地意图引擎仍可驱动全部流程"),
    ("CF-01", "custom", "全局自定义字段库：管理员定义字段，四类单据创建时填写"),
    ("CF-02", "custom", "字段值 EAV 落库，随记录详情展示、不污染固定列"),
    ("PL-01", "platform", "五端统一 REST（浏览器/Windows/macOS/Linux/移动端同一套接口）"),
    ("PL-02", "platform", "Postgres 统一数据源 + 幂等 seed + 一键重置"),
    ("PL-03", "platform", "工作台 KPI 与审计查询"),
];

/// 管理员配置的打卡点：员工必须落在任一点自己的 `radius_m` 范围内才能打卡。
///
/// 半径按点独立配置：总部大楼可以给 2km，厂区可以给 10km。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttendanceSite {
    /// 打卡点名称，用于设置页与超标提示；旧版单点配置没有该字段，反序列化时留空。
    #[serde(default)]
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub radius_m: i64,
    pub address: String,
}

impl AttendanceSite {
    /// 展示名：优先名称，其次地址，都没有时回落到「打卡点」。
    pub fn display_name(&self) -> &str {
        let name = self.name.trim();
        if !name.is_empty() {
            return name;
        }
        let address = self.address.trim();
        if !address.is_empty() {
            return address;
        }
        "打卡点"
    }
}

/// 打卡范围默认值（米）。
pub const DEFAULT_ATTENDANCE_RADIUS_M: i64 = 2000;

/// 两点球面距离（米）。Haversine 在办公场景（公里级）足够精确，且不引入地理库依赖。
pub fn haversine_meters(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS_M: f64 = 6_371_000.0;
    let (phi1, phi2) = (lat1.to_radians(), lat2.to_radians());
    let dphi = (lat2 - lat1).to_radians();
    let dlambda = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (dlambda / 2.0).sin().powi(2);
    EARTH_RADIUS_M * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
}

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
        // 默认下班 18:00：17:35 仍算早退，18:00 起算正常
        assert_eq!(punch_status("out", "17:20"), "early");
        assert_eq!(punch_status("out", "17:35"), "early");
        assert_eq!(punch_status("out", "18:00"), "normal");
        assert_eq!(punch_status("out", "18:12"), "normal");
        assert_eq!(punch_status("in", "bad"), "normal");
        assert_eq!(parse_time("25:00"), None);
        // 可配置时间：10:00 上班、19:00 下班
        assert_eq!(punch_status_with("in", "09:30", 600, 1140), "normal");
        assert_eq!(punch_status_with("out", "18:30", 600, 1140), "early");
        assert_eq!(punch_status_with("in", "10:30", 600, 1140), "late");
        assert_eq!(AttendancePolicy::default().end_min(), 18 * 60);
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
        assert_eq!(TOOL_NAMES.len(), 32);
        assert_eq!(TABLE_NAMES.len(), 21);
        assert!(FEATURES.len() >= 25);
        let mut sorted = TOOL_NAMES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 32, "tool names must be unique");
    }

    #[test]
    fn leave_workdays_skip_weekends() {
        // 2026-11-23 周一 ~ 2026-11-27 周五 = 5 天
        assert_eq!(workdays_between("2026-11-23", "2026-11-27"), 5);
        // 2026-11-21 周六 ~ 2026-11-22 周日 = 0 天
        assert_eq!(workdays_between("2026-11-21", "2026-11-22"), 0);
        // 跨周：周五 ~ 下周一 = 2 天
        assert_eq!(workdays_between("2026-11-20", "2026-11-23"), 2);
        // 非法区间
        assert_eq!(workdays_between("2026-11-27", "2026-11-23"), 0);
        assert_eq!(workdays_in("2026-11-23", "2026-11-24").len(), 2);
    }

    #[test]
    fn leave_chain_adds_hrbp_beyond_three_days() {
        assert_eq!(leave_chain(3), vec!["直属主管", "部门总监"]);
        assert_eq!(leave_chain(4), vec!["直属主管", "HRBP", "部门总监"]);
    }

    #[test]
    fn overtime_hours_handles_cross_midnight() {
        assert_eq!(overtime_hours("18:00", "21:00"), Some(3.0));
        // 跨天：22:00 ~ 次日 02:00 = 4 小时
        assert_eq!(overtime_hours("22:00", "02:00"), Some(4.0));
        // 起止相同与超过 16 小时都视为非法
        assert_eq!(overtime_hours("18:00", "18:00"), None);
        assert_eq!(overtime_hours("18:00", "12:00"), None);
        assert_eq!(OVERTIME_DEFAULT_START, "18:00");
        assert_eq!(OVERTIME_HOURS_PER_DAY, 8.0);
        assert_eq!(DEFAULT_MAKEUP_QUOTA, 3);
    }

    #[test]
    fn overtime_chain_adds_director_beyond_four_hours() {
        assert_eq!(overtime_chain(4.0), vec!["直属主管"]);
        assert_eq!(overtime_chain(4.5), vec!["直属主管", "部门总监"]);
    }

    #[test]
    fn haversine_matches_known_short_distance() {
        // 纬度差 0.001° ≈ 111 米，办公场景的量级足够用
        let d = haversine_meters(31.2304, 121.4737, 31.2314, 121.4737);
        assert!((d - 111.2).abs() < 2.0, "unexpected distance: {d}");
        // 同一点距离为 0
        assert!(haversine_meters(31.2304, 121.4737, 31.2304, 121.4737) < 0.001);
    }

    #[test]
    fn default_attendance_radius_is_two_km() {
        assert_eq!(DEFAULT_ATTENDANCE_RADIUS_M, 2000);
    }
}
