//! 存储层 · Postgres（服务端唯一数据库）
//!
//! 设计要点：
//! - 15 张业务表（含附件元数据）+ users/sessions + counters，覆盖 6 大域与认证审计；
//! - `open_uri` 自动建表、灌入幂等 seed、初始化演示账号；
//! - 金额统一「分」，日期统一 `YYYY-MM-DD`；业务日期在 Rust 侧按本地时区计算，
//!   SQL 不再出现 `date('now')/strftime()`；
//! - 单号通过 `counters` 表原子取号（`UPDATE ... RETURNING`），多实例并发不撞号；
//! - 写多步操作走 `Store::transaction`。

use crate::auth::SessionUser;
use crate::db::{execute_pg, fetch_pg, BoxFuture, Params, Repo, Tx};
use crate::params;
use crate::clock;
use crate::domain;
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use std::sync::OnceLock;
use std::time::Duration;

/* ---------------- DDL ---------------- */



/// Postgres DDL。
pub const SCHEMA_PG: &str = r#"
CREATE TABLE IF NOT EXISTS departments (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT, leader_id TEXT
);
CREATE TABLE IF NOT EXISTS employees (
  id TEXT PRIMARY KEY, no TEXT UNIQUE NOT NULL, name TEXT NOT NULL,
  dept_id TEXT NOT NULL, position TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'probation',
  entry_date TEXT NOT NULL, salary_cents BIGINT NOT NULL DEFAULT 0,
  manager_id TEXT, email TEXT, account_frozen BIGINT NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS hr_cases (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, employee_id TEXT NOT NULL,
  effective_date TEXT, reason TEXT, status TEXT NOT NULL DEFAULT 'running',
  step_index BIGINT NOT NULL DEFAULT 0, steps TEXT,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS claims (
  id TEXT PRIMARY KEY, kind TEXT NOT NULL, applicant_id TEXT NOT NULL,
  dept_id TEXT, amount_cents BIGINT NOT NULL, purpose TEXT, project TEXT,
  status TEXT NOT NULL DEFAULT 'pending', submitted_at TEXT NOT NULL,
  approver_id TEXT, budget_ok BIGINT NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS contracts (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, party TEXT, kind TEXT,
  amount_cents BIGINT NOT NULL DEFAULT 0, sign_date TEXT, expire_date TEXT,
  status TEXT NOT NULL DEFAULT 'review', owner_id TEXT,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS attendance (
  id TEXT PRIMARY KEY, employee_id TEXT NOT NULL, work_date TEXT NOT NULL,
  clock_in TEXT, clock_out TEXT, status TEXT, source TEXT,
  hours DOUBLE PRECISION NOT NULL DEFAULT 0,
  UNIQUE(employee_id, work_date)
);
CREATE TABLE IF NOT EXISTS makeup_requests (
  id TEXT PRIMARY KEY, employee_id TEXT NOT NULL, work_date TEXT NOT NULL,
  clock_in TEXT, clock_out TEXT, reason TEXT,
  status TEXT NOT NULL DEFAULT 'pending', approver_id TEXT, comment TEXT, decided_at TEXT,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS assets (
  id TEXT PRIMARY KEY, category TEXT NOT NULL, model TEXT, serial_no TEXT UNIQUE,
  status TEXT NOT NULL DEFAULT 'instock', holder_id TEXT, location TEXT,
  price_cents BIGINT NOT NULL DEFAULT 0, qr_code TEXT, purchased_at TEXT
);
CREATE TABLE IF NOT EXISTS budgets (
  year BIGINT NOT NULL, dept_id TEXT NOT NULL, total_cents BIGINT NOT NULL,
  used_cents BIGINT NOT NULL DEFAULT 0, PRIMARY KEY(year, dept_id)
);
CREATE TABLE IF NOT EXISTS approval_flows (
  id TEXT PRIMARY KEY, entity_type TEXT NOT NULL, entity_id TEXT NOT NULL,
  step BIGINT NOT NULL, approver TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'pending',
  comment TEXT, acted_at TEXT,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS payments (
  id TEXT PRIMARY KEY, claim_id TEXT NOT NULL, amount_cents BIGINT NOT NULL,
  status TEXT NOT NULL DEFAULT 'queued', scheduled_at TEXT, paid_at TEXT
);
CREATE TABLE IF NOT EXISTS claim_items (
  id TEXT PRIMARY KEY, claim_id TEXT NOT NULL, category TEXT NOT NULL,
  amount_cents BIGINT NOT NULL, occurred_on TEXT, invoice_no TEXT, note TEXT,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS attachments (
  id TEXT PRIMARY KEY, entity_type TEXT NOT NULL, entity_id TEXT NOT NULL,
  file_name TEXT NOT NULL, mime TEXT NOT NULL, size_bytes BIGINT NOT NULL,
  storage_path TEXT NOT NULL, uploaded_by TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS asset_events (
  id TEXT PRIMARY KEY, asset_id TEXT NOT NULL, kind TEXT NOT NULL,
  employee_id TEXT, note TEXT,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS notifications (
  id TEXT PRIMARY KEY, recipient_id TEXT NOT NULL, title TEXT NOT NULL,
  body TEXT, kind TEXT, is_read BIGINT NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS audit_events (
  id BIGSERIAL PRIMARY KEY, tool TEXT NOT NULL, actor TEXT NOT NULL,
  payload TEXT, result TEXT,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS users (
  id TEXT PRIMARY KEY, employee_id TEXT UNIQUE NOT NULL, username TEXT UNIQUE NOT NULL,
  display_name TEXT NOT NULL, password_hash TEXT NOT NULL, salt TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'employee', active BIGINT NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
CREATE TABLE IF NOT EXISTS sessions (
  token TEXT PRIMARY KEY, user_id TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS')),
  expires_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS counters (
  name TEXT PRIMARY KEY, value BIGINT NOT NULL DEFAULT 0
);
-- 运行时配置（目前是 LLM 提供商），与业务数据分开存放，重置演示数据时保留
CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY, value TEXT NOT NULL
);
-- 自定义字段库：全局定义，按业务实体归类；label 存 {"zh","en"} JSON，options 存下拉项 JSON 数组
CREATE TABLE IF NOT EXISTS custom_fields (
  id TEXT PRIMARY KEY, entity_type TEXT NOT NULL, label TEXT NOT NULL,
  field_type TEXT NOT NULL, options TEXT,
  required BIGINT NOT NULL DEFAULT 0, sort_order BIGINT NOT NULL DEFAULT 0,
  enabled BIGINT NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT (to_char(now() AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS'))
);
-- 自定义字段值：EAV 模式，避免给每张业务表加列；一条记录一个字段一行
CREATE TABLE IF NOT EXISTS custom_field_values (
  entity_type TEXT NOT NULL, entity_id TEXT NOT NULL, field_id TEXT NOT NULL,
  value TEXT, PRIMARY KEY(entity_type, entity_id, field_id)
);
CREATE INDEX IF NOT EXISTS idx_emp_dept ON employees(dept_id);
CREATE INDEX IF NOT EXISTS idx_emp_status ON employees(status);
CREATE INDEX IF NOT EXISTS idx_claims_status ON claims(status);
CREATE INDEX IF NOT EXISTS idx_claims_date ON claims(submitted_at);
CREATE INDEX IF NOT EXISTS idx_contract_expire ON contracts(expire_date);
CREATE INDEX IF NOT EXISTS idx_att_date ON attendance(work_date);
CREATE INDEX IF NOT EXISTS idx_makeup_emp ON makeup_requests(employee_id, work_date);
CREATE INDEX IF NOT EXISTS idx_asset_status ON assets(status);
CREATE INDEX IF NOT EXISTS idx_attach_entity ON attachments(entity_type, entity_id);
CREATE INDEX IF NOT EXISTS idx_audit_tool ON audit_events(tool);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_expire ON sessions(expires_at);
"#;

/// 费用明细 seed 单独一份：`claim_items` 是后加的表，老库已有数据时整段
/// `ensure_seed` 会被跳过，需要单独补一次，否则演示单上只有总额没有明细。
const CLAIM_ITEM_SEED: &str = r#"
INSERT OR IGNORE INTO claim_items(id,claim_id,category,amount_cents,occurred_on,invoice_no,note) VALUES
 ('BX-2026-0048-01','BX-2026-0048','transport',168000,date('now','-4 day'),'INV-2026-0451','上海往返高铁二等座'),
 ('BX-2026-0048-02','BX-2026-0048','hotel',160000,date('now','-3 day'),'INV-2026-0452','上海出差住宿 1 晚'),
 ('BX-2026-0047-01','BX-2026-0047','transport',460000,date('now','-5 day'),'INV-2026-0447','深圳往返机票'),
 ('BX-2026-0047-02','BX-2026-0047','hotel',540000,date('now','-4 day'),'INV-2026-0448','项目现场住宿 3 晚'),
 ('BX-2026-0047-03','BX-2026-0047','office',260000,date('now','-4 day'),'INV-2026-0449','现场布线耗材'),
 ('BX-2026-0046-01','BX-2026-0046','office',48600,date('now','-5 day'),'INV-2026-0446','打印耗材与纸张');
"#;

/// 存量库补列清单：(表, 列, 列定义)。新增列时同步登记到这里，
/// `init()` 会先探测再 ALTER，老库无需删库重建。
const COLUMN_MIGRATIONS: &[(&str, &str, &str)] = &[
    // 审批时间：多级审批逐级流转后，时间线需要区分「节点创建」与「实际审批」
    ("approval_flows", "acted_at", "TEXT"),
];

/// 数据迁移：多级审批改造前，`approval_flows.approver` 存的是「角色 姓名」，
/// 改造后统一存员工号（按人员指定审批人）。UPDATE 匹配不到即无操作，可重复执行。
const DATA_MIGRATIONS: &[&str] = &[
    "UPDATE approval_flows SET approver='E10' WHERE approver='财务经理 吴昊'",
    "UPDATE approval_flows SET approver='E01' WHERE approver='部门总监 韩磊'",
    "UPDATE approval_flows SET approver='E06' WHERE approver='直属主管 刘洋'",
    "UPDATE approval_flows SET approver='E12' WHERE approver='HRBP 冯雪'",
    // 合同审批改为按人员指定：法务→财务经理 E10，用印→总经理 E01
    "UPDATE approval_flows SET approver='E10' WHERE approver='法务'",
    "UPDATE approval_flows SET approver='E01' WHERE approver='用印'",
    // 老库只有法务节点，补齐用印节点；只在合同仍为 review 且缺第 2 级时插入，可重复执行
    "INSERT INTO approval_flows(id,entity_type,entity_id,step,approver,status)
     SELECT 'AP-contract-'||f.entity_id||'-2','contract',f.entity_id,2,'E01','pending'
       FROM approval_flows f JOIN contracts c ON c.id=f.entity_id
      WHERE f.entity_type='contract' AND f.step=1 AND f.status='pending' AND c.status='review'
        AND NOT EXISTS (SELECT 1 FROM approval_flows g
                         WHERE g.entity_type='contract' AND g.entity_id=f.entity_id AND g.step=2)",
    // 演示合同负责人改为会计郑楠，避免审批链第一级正好是负责人本人
    "UPDATE contracts SET owner_id='E11' WHERE id='HT-2026-013' AND owner_id='E10'",
];

/// 演示/验收数据模板：日期占位符在运行时按本地时区渲染，两种数据库共用。
const SEED_TEMPLATE: &str = r#"
INSERT OR IGNORE INTO departments(id,name,parent_id,leader_id) VALUES
 ('D01','总经办',NULL,'E01'),('D02','研发中心','D01','E02'),
 ('D03','产品部','D01','E06'),('D04','市场销售部','D01','E08'),
 ('D05','财务部','D01','E10'),('D06','人力资源部','D01','E12'),
 ('D07','行政部','D01','E13'),('D08','项目实施部','D01','E14');

INSERT OR IGNORE INTO employees(id,no,name,dept_id,position,status,entry_date,salary_cents,manager_id,email) VALUES
 ('E01','EMP-1001','周明远','D01','总经理','active','2018-03-01',4800000,NULL,'zhoumy@yugong.com'),
 ('E02','EMP-1002','陈立','D02','研发总监','active','2019-06-15',3800000,'E01','chenli@yugong.com'),
 ('E03','EMP-1003','李娜','D02','后端工程师','active','2021-04-12',2400000,'E02','lina@yugong.com'),
 ('E04','EMP-1004','张伟','D02','前端工程师','probation','2026-10-20',1800000,'E02','zhangwei@yugong.com'),
 ('E05','EMP-1005','王强','D02','测试工程师','active','2022-08-01',2000000,'E02','wangqiang@yugong.com'),
 ('E06','EMP-1006','刘洋','D03','产品经理','active','2021-09-06',2600000,'E01','liuyang@yugong.com'),
 ('E07','EMP-1007','赵敏','D03','产品专员','active','2023-02-13',1600000,'E06','zhaomin@yugong.com'),
 ('E08','EMP-1008','陈静','D04','销售经理','active','2020-11-23',2800000,'E01','chenjing@yugong.com'),
 ('E09','EMP-1009','孙悦','D04','销售代表','active','2024-03-04',1400000,'E08','sunyue@yugong.com'),
 ('E10','EMP-1010','吴昊','D05','财务经理','active','2019-07-08',3000000,'E01','wuhao@yugong.com'),
 ('E11','EMP-1011','郑楠','D05','会计','active','2022-05-16',1500000,'E10','zhengnan@yugong.com'),
 ('E12','EMP-1012','冯雪','D06','HRBP','active','2021-01-11',1900000,'E01','fengxue@yugong.com'),
 ('E13','EMP-1013','蒋涛','D07','行政主管','active','2020-09-01',1700000,'E01','jiangtao@yugong.com'),
 ('E14','EMP-1014','韩磊','D08','实施工程师','active','2022-10-10',2100000,'E01','hanlei@yugong.com'),
 ('E15','EMP-1015','沈琪','D08','实施顾问','active','2023-06-19',1800000,'E14','shenqi@yugong.com'),
 ('E16','EMP-1016','何嘉','D02','架构师','active','2020-02-17',3600000,'E02','hejia@yugong.com');

INSERT OR IGNORE INTO hr_cases(id,kind,employee_id,effective_date,reason,status,step_index,steps) VALUES
 ('HR-2026-031','onboard','E04','2026-10-20','研发中心前端岗位补员','running',3,'offer → 资料收集 → 合同签署 → 设备领用 → 入职培训'),
 ('HR-2026-028','promote','E03','2026-10-01','年度绩效 A，晋升高级工程师并调薪','done',4,'直属主管 → HRBP → 部门总监 → 总经理 → 生效'),
 ('HR-2026-025','offboard','E09','2026-09-30','个人原因离职（已完成交接）','done',5,'申请 → 交接 → 资产归还 → 权限回收 → 薪资结算 → 归档');

INSERT OR IGNORE INTO claims(id,kind,applicant_id,dept_id,amount_cents,purpose,project,status,submitted_at,approver_id) VALUES
 ('BX-2026-0048','business','E08','D04',328000,'上海客户拜访差旅（高铁+住宿）',NULL,'pending',date('now','-2 day'),'E10'),
 ('BX-2026-0047','project','E14','D08',1260000,'恒信科技项目现场实施差旅与耗材','恒信科技数字化平台','pending',date('now','-3 day'),'E10'),
 ('BX-2026-0046','daily','E07','D03',48600,'办公用品与打印耗材',NULL,'pending',date('now','-4 day'),'E06'),
 ('BX-2026-0045','daily','E11','D05',120000,'财务软件年度订阅分摊',NULL,'approved',date('now','-6 day'),'E10'),
 ('BX-2026-0044','project','E15','D08',850000,'客户现场服务器机柜及布线','恒信科技数字化平台','paid',date('now','-9 day'),'E10'),
 ('BX-2026-0043','business','E09','D04',216000,'行业峰会客户招待',NULL,'rejected',date('now','-12 day'),'E10'),
 ('BX-2026-0042','daily','E05','D02',76000,'测试机配件采购',NULL,'paid',date('now','-14 day'),'E10'),
 ('BX-2026-0041','project','E16','D02',1980000,'研发测试云资源包','云原生中台','paid',date('now','-18 day'),'E10');

INSERT OR IGNORE INTO contracts(id,name,party,kind,amount_cents,sign_date,expire_date,status,owner_id) VALUES
 ('HT-2026-018','恒信科技数字化平台建设合同','恒信科技有限公司','sales',186000000,date('now','-240 day'),date('now','+120 day'),'effective','E08'),
 ('HT-2026-017','办公区服务器采购合同','联想（北京）有限公司','purchase',48000000,date('now','-180 day'),date('now','+20 day'),'expiring','E13'),
 ('HT-2026-016','云资源年度服务框架协议','阿里云计算有限公司','service',36000000,date('now','-290 day'),date('now','+70 day'),'effective','E02'),
 ('HT-2026-015','张伟劳动合同','张伟','labor',0,date('now','-22 day'),date('now','+1000 day'),'effective','E12'),
 ('HT-2026-014','客户数据保密协议','恒信科技有限公司','nda',0,date('now','-250 day'),date('now','+17 day'),'expiring','E01'),
 ('HT-2026-013','SaaS 订阅服务合同','金蝶软件','service',12000000,date('now','-160 day'),date('now','+200 day'),'review','E11'),
 ('HT-2025-042','旧版 OA 运维合同','蓝海信息','service',8800000,date('now','-660 day'),date('now','-300 day'),'archived','E13');

INSERT OR IGNORE INTO attendance(id,employee_id,work_date,clock_in,clock_out,status,source,hours) VALUES
 ('AT-0001','E03',date('now'),'08:52','18:12','normal','device',8.3),
 ('AT-0002','E04',date('now'),'09:16','18:30','late','mobile',8.2),
 ('AT-0003','E05',date('now'),'08:41','18:05','normal','device',8.4),
 ('AT-0004','E06',date('now'),'08:58','18:20','normal','desktop',8.4),
 ('AT-0005','E07',date('now'),NULL,'17:40','missing','device',0),
 ('AT-0006','E08',date('now'),NULL,NULL,'field','mobile',8),
 ('AT-0007','E10',date('now'),'08:47','18:02','normal','device',8.2),
 ('AT-0008','E11',date('now'),'08:55','17:35','early','device',7.7),
 ('AT-0009','E12',date('now'),'09:02','18:22','normal','mobile',8.3),
 ('AT-0010','E14',date('now'),NULL,NULL,'field','mobile',8),
 ('AT-0011','E15',date('now'),NULL,NULL,'leave','mobile',0),
 ('AT-0012','E16',date('now'),'08:36','18:41','normal','device',9.1),
 ('AT-0013','E03',date('now','-3 day'),'08:49','18:08','normal','device',8.3),
 ('AT-0014','E05',date('now','-3 day'),'09:22','18:15','late','mobile',7.9);

INSERT OR IGNORE INTO makeup_requests(id,employee_id,work_date,clock_in,clock_out,reason,status,approver_id) VALUES
 ('MK-2026-0001','E03',date('now','-1 day'),'09:00','18:00','客户现场演示，忘记打卡','pending','E02');

INSERT OR IGNORE INTO assets(id,category,model,serial_no,status,holder_id,location,price_cents,qr_code,purchased_at) VALUES
 ('AST-0001','laptop','ThinkPad X1 Carbon Gen12','PF4A2B9C','inuse','E02','研发中心 A 区',1480000,'OA-AST-0001','2025-04-12'),
 ('AST-0002','laptop','MacBook Pro 14 M4','C02ZQ8K1','inuse','E16','研发中心 A 区',1899900,'OA-AST-0002','2025-08-03'),
 ('AST-0003','laptop','Dell Latitude 7450','DL7450X21','instock',NULL,'行政库房 3 号柜',960000,'OA-AST-0003','2026-02-18'),
 ('AST-0004','desktop','OptiPlex 7020 工作站','DP7020-773','inuse','E03','研发中心 B 区',1120000,'OA-AST-0004','2024-11-20'),
 ('AST-0005','printer','HP LaserJet MFP M528','HP528-00931','inuse',NULL,'3F 公共打印区',860000,'OA-AST-0005','2024-06-05'),
 ('AST-0006','printer','Canon imageCLASS MF643C','CN643-1182','repair',NULL,'维修中 · 供应商',420000,'OA-AST-0006','2023-09-14'),
 ('AST-0007','usb','SanDisk 加密 U 盘 64GB','SD-64-0221','inuse','E14','项目实施部',26000,'OA-AST-0007','2025-01-08'),
 ('AST-0008','usb','Kingston DataTraveler 32GB','KT-32-0917','instock',NULL,'行政库房 1 号柜',8900,'OA-AST-0008','2025-03-22'),
 ('AST-0009','disc','归档光盘 · 2025 财务凭证','DISC-2025-FIN','instock',NULL,'档案室 B-12',3500,'OA-AST-0009','2026-01-15'),
 ('AST-0010','disc','归档光盘 · 客户交付镜像','DISC-2026-HX','inuse','E15','档案室 B-12',3500,'OA-AST-0010','2026-04-02'),
 ('AST-0011','monitor','Dell UltraSharp U2723QE','U2723-4408','inuse','E06','产品部',390000,'OA-AST-0011','2025-05-19'),
 ('AST-0012','projector','Epson CB-FH06','EP-FH06-221','instock',NULL,'5F 大会议室',520000,'OA-AST-0012','2024-02-27');

INSERT OR IGNORE INTO budgets(year,dept_id,total_cents,used_cents) VALUES
 (2026,'ALL',120000000,84260000),
 (2026,'D02',30000000,22100000),(2026,'D04',22000000,14600000),
 (2026,'D08',32000000,21300000),(2026,'D03',12000000,5200000),
 (2026,'D05',8000000,3100000),(2026,'D01',16000000,8900000);

INSERT OR IGNORE INTO approval_flows(id,entity_type,entity_id,step,approver,status) VALUES
 ('AP-0001','claim','BX-2026-0048',1,'E10','pending'),
 ('AP-0002','claim','BX-2026-0047',1,'E10','pending'),
 ('AP-0003','claim','BX-2026-0047',2,'E01','pending'),
 ('AP-0004','claim','BX-2026-0046',1,'E06','pending'),
 ('AP-0005','hr_case','HR-2026-031',4,'E12','pending'),
 ('AP-0006','contract','HT-2026-013',1,'E10','pending'),
 ('AP-0007','contract','HT-2026-013',2,'E01','pending'),
 ('AP-0008','makeup','MK-2026-0001',1,'E02','pending');

INSERT OR IGNORE INTO payments(id,claim_id,amount_cents,status,scheduled_at,paid_at) VALUES
 ('PAY-0001','BX-2026-0045',120000,'queued',date('now','+2 day'),NULL),
 ('PAY-0002','BX-2026-0044',850000,'paid',date('now','-5 day'),date('now','-5 day')),
 ('PAY-0003','BX-2026-0042',76000,'paid',date('now','-10 day'),date('now','-10 day')),
 ('PAY-0004','BX-2026-0041',1980000,'paid',date('now','-15 day'),date('now','-15 day'));

INSERT OR IGNORE INTO asset_events(id,asset_id,kind,employee_id,note) VALUES
 ('AEV-0001','AST-0003','register',NULL,'采购入库'),
 ('AEV-0002','AST-0007','assign','E14','项目实施部领用'),
 ('AEV-0003','AST-0010','assign','E15','客户交付镜像归档领用');

INSERT OR IGNORE INTO notifications(id,recipient_id,title,body,kind) VALUES
 ('NTF-0001','E10','待审批：商务报销 BX-2026-0048','陈静提交的客户拜访差旅报销待处理','approval'),
 ('NTF-0002','E12','入职流程待办','张伟的设备领用与入职培训待确认','hr'),
 ('NTF-0003','E01','合同到期预警','客户数据保密协议将在 17 天后到期','contract');

INSERT OR IGNORE INTO audit_events(tool,actor,payload,result) VALUES
 ('hr.onboard','Agent','{"name":"张伟"}','{"ok":true}'),
 ('finance.claim.create','Agent','{"amount_cents":328000}','{"ok":true}'),
 ('asset.return','Agent','{"asset_id":"AST-0002"}','{"ok":true}');
"#;

const RESET_SQL: &str = r#"
DELETE FROM audit_events; DELETE FROM notifications; DELETE FROM asset_events;
DELETE FROM payments; DELETE FROM approval_flows; DELETE FROM budgets; DELETE FROM attachments;
DELETE FROM assets; DELETE FROM makeup_requests; DELETE FROM attendance; DELETE FROM contracts;
DELETE FROM claims; DELETE FROM hr_cases; DELETE FROM employees; DELETE FROM departments;
DELETE FROM counters;
"#;

/* ---------------- Store ---------------- */

#[derive(Clone)]
pub struct Store(pub(crate) PgPool);

impl Store {
    /// 统一入口：只接受 Postgres 连接串（`postgres://` / `postgresql://`）。
    pub async fn open_uri(uri: &str) -> Result<Self> {
        let uri = uri.trim();
        if !uri.starts_with("postgres://") && !uri.starts_with("postgresql://") {
            anyhow::bail!("OA_DB 必须是 postgres:// 或 postgresql:// 连接串，收到: {uri}");
        }
        Self::open_postgres(uri).await
    }

    pub async fn open_postgres(url: &str) -> Result<Self> {
        let size: u32 = std::env::var("OA_PG_POOL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let pool = PgPoolOptions::new()
            .max_connections(size)
            .acquire_timeout(Duration::from_secs(10))
            .connect(url)
            .await
            .context("connect postgres")?;
        let store = Self(pool);
        store.init().await?;
        Ok(store)
    }

    pub(crate) async fn apply_schema(&self) -> Result<()> {
        sqlx::raw_sql(SCHEMA_PG).execute(&self.0).await?;
        Ok(())
    }

    async fn run_batch(&self, sql: &str) -> Result<()> {
        sqlx::raw_sql(sql).execute(&self.0).await?;
        Ok(())
    }

    /// 列是否存在：查 information_schema。
    async fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        let sql = "SELECT COUNT(*) FROM information_schema.columns \
                   WHERE table_name=?1 AND column_name=?2";
        let db = self.clone();
        Ok(db.scalar_i64(sql, params![table, column]).await? > 0)
    }

    /// 存量库升级：先补列再跑数据迁移，全部可重复执行。
    /// `CREATE TABLE IF NOT EXISTS` 不会修改已存在的表，老库缺新列会在查询时直接报错，
    /// 所以先探测再 ALTER。
    async fn migrate_columns(&self) -> Result<()> {
        for (table, column, ddl) in COLUMN_MIGRATIONS {
            if self.has_column(table, column).await? {
                continue;
            }
            self.run_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"))
                .await
                .with_context(|| format!("补列失败：{table}.{column}"))?;
        }
        for sql in DATA_MIGRATIONS {
            self.run_batch(sql)
                .await
                .with_context(|| format!("数据迁移失败：{sql}"))?;
        }
        Ok(())
    }

    /// 建表 → 补列 → 幂等 seed → 演示账号。多实例同时启动时依赖 `IF NOT EXISTS` /
    /// `ON CONFLICT DO NOTHING` 保证幂等，不会互相覆盖。
    pub(crate) async fn init(&self) -> Result<()> {
        self.apply_schema().await?;
        self.migrate_columns().await?;
        self.ensure_seed().await?;
        self.ensure_auth_seed().await?;
        self.ensure_claim_items().await?;
        Ok(())
    }

    /// 只为「明细表是后加的、老库已经有其它业务数据」的场景补一次演示明细。
    async fn ensure_claim_items(&self) -> Result<usize> {
        if self.scalar_i64("SELECT COUNT(*) FROM claim_items", &[]).await? > 0 {
            return Ok(0);
        }
        self.run_batch(&render_seed(CLAIM_ITEM_SEED)).await?;
        Ok(1)
    }

    pub async fn is_seeded(&self) -> Result<bool> {
        Ok(self
            .scalar_i64("SELECT COUNT(*) FROM employees", &[])
            .await?
            > 0)
    }

    pub async fn ensure_seed(&self) -> Result<usize> {
        if self.is_seeded().await? {
            return Ok(0);
        }
        self.run_batch(&seed_sql()).await?;
        Ok(self
            .scalar_i64("SELECT COUNT(*) FROM employees", &[])
            .await?
            .max(0) as usize)
    }

    /// 一键重置演示数据；账号与会话保留，计数表清空后按新数据重新初始化。
    pub async fn reset_seed(&self) -> Result<()> {
        self.run_batch(RESET_SQL).await?;
        self.run_batch(&seed_sql()).await?;
        Ok(())
    }

    /* ---------------- 运行时配置 ---------------- */

    /// 读单个配置项；不存在返回 `None`（区别于空字符串）。
    pub async fn setting(&self, key: &str) -> Result<Option<String>> {
        let rows = self
            .query_json("SELECT value FROM settings WHERE key = ?1", params![key])
            .await?;
        Ok(rows
            .first()
            .and_then(|r| r["value"].as_str())
            .map(str::to_string))
    }

    /// 写配置项（不存在则插入），依赖 Postgres 的 `ON CONFLICT` 语法。
    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .await?;
        Ok(())
    }

    /// 删除配置项（回落到环境变量或默认行为）。
    pub async fn remove_setting(&self, key: &str) -> Result<()> {
        self.execute("DELETE FROM settings WHERE key = ?1", params![key])
            .await?;
        Ok(())
    }

    /* ---------------- 基础执行器（自动提交） ---------------- */

    pub async fn execute(&self, sql: &str, params: Params<'_>) -> Result<usize> {
        let n = execute_pg(&self.0, sql, params).await?;
        Ok(n as usize)
    }

    pub async fn scalar_i64(&self, sql: &str, params: Params<'_>) -> Result<i64> {
        let rows = fetch_pg(&self.0, sql, params).await?;
        rows.first()
            .and_then(|r| r.as_object())
            .and_then(|o| o.values().next())
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("scalar query returned no value"))
    }

    pub async fn query_json(&self, sql: &str, params: Params<'_>) -> Result<Vec<Value>> {
        fetch_pg(&self.0, sql, params).await
    }

    pub async fn query_all(&self, sql: &str) -> Result<Vec<Value>> {
        self.query_json(sql, &[]).await
    }

    /// 事务边界：闭包内的所有写操作共享同一事务，返回 Err 自动回滚。
    pub async fn transaction<T, F>(&self, f: F) -> Result<T>
    where
        F: for<'a> FnOnce(&'a mut dyn Repo) -> BoxFuture<'a, Result<T>>,
    {
        let mut tx = Tx::new(self.0.begin().await?);
        match f(&mut tx).await {
            Ok(v) => {
                tx.commit().await?;
                Ok(v)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }

    /* ---------------- 指标 ---------------- */

    /// 工作台 KPI：一次查询拿全指标；日期全部由 Rust 计算后绑定。
    pub async fn metrics(&self) -> Result<Value> {
        let today = clock::local_today();
        let in30 = clock::date_offset(30);
        let year = clock::local_year();

        let active = self
            .scalar_i64(
                "SELECT COUNT(*) FROM employees WHERE status IN ('active','probation')",
                &[],
            )
            .await?;
        let pending = self
            .scalar_i64("SELECT COUNT(*) FROM claims WHERE status='pending'", &[])
            .await?;
        let expiring = self
            .scalar_i64(
                "SELECT COUNT(*) FROM contracts WHERE status <> 'archived'
                   AND expire_date BETWEEN ?1 AND ?2",
                params![today, in30],
            )
            .await?;
        let asset_value = self
            .scalar_i64(
                "SELECT CAST(COALESCE(SUM(price_cents),0) AS BIGINT) FROM assets
                 WHERE status <> 'scrapped'",
                &[],
            )
            .await?;
        let payroll_cents = self
            .scalar_i64(
                "SELECT CAST(COALESCE(SUM(salary_cents),0) AS BIGINT) FROM employees
                 WHERE status IN ('active','probation')",
                &[],
            )
            .await?;
        let today_total = self
            .scalar_i64("SELECT COUNT(*) FROM attendance WHERE work_date=?1", params![today])
            .await?;
        let today_good = self
            .scalar_i64(
                "SELECT COUNT(*) FROM attendance WHERE work_date=?1
                   AND status IN ('normal','field')",
                params![today],
            )
            .await?;
        let attendance_rate = if today_total > 0 {
            (today_good as f64 / today_total as f64 * 1000.0).round() / 10.0
        } else {
            0.0
        };
        let budget_total = self
            .scalar_i64(
                "SELECT COALESCE(total_cents,0) FROM budgets WHERE year=?1 AND dept_id='ALL'",
                params![year],
            )
            .await?;
        let budget_used = self
            .scalar_i64(
                "SELECT COALESCE(used_cents,0) FROM budgets WHERE year=?1 AND dept_id='ALL'",
                params![year],
            )
            .await?;

        // 近 6 个月费用趋势：SQL 只负责分组，月份骨架与补 0 在 Rust 侧完成
        let labels = clock::month_labels(6);
        let start = format!("{}-01", labels.first().cloned().unwrap_or_default());
        let rows = self
            .query_json(
                "SELECT substr(submitted_at,1,7) AS month,
                        CAST(COALESCE(SUM(amount_cents),0) AS BIGINT) AS total_cents
                 FROM claims WHERE submitted_at >= ?1
                 GROUP BY substr(submitted_at,1,7)",
                params![start],
            )
            .await?;
        let trend: Vec<i64> = labels
            .iter()
            .map(|m| {
                rows.iter()
                    .find(|r| r["month"].as_str() == Some(m.as_str()))
                    .and_then(|r| r["total_cents"].as_i64())
                    .unwrap_or(0)
            })
            .collect();

        Ok(json!({
            "active_employees": active,
            "pending_claims": pending,
            "expiring_contracts": expiring,
            "asset_value_cents": asset_value,
            "payroll_cents": payroll_cents,
            "attendance_rate": attendance_rate,
            "budget_total_cents": budget_total,
            "budget_used_cents": budget_used,
            "trend": trend,
            "trend_months": labels,
            "generated_at": clock::local_datetime(),
        }))
    }

    /* ---------------- 台账（按角色行级裁剪） ---------------- */

    pub async fn table_rows(&self, user: &SessionUser, name: &str) -> Result<Vec<Value>> {
        let role = user.role.as_str();
        let admin = role == "admin";
        let privileged = admin || matches!(role, "hr" | "finance" | "manager");
        let finance_scope = admin || matches!(role, "finance" | "manager");
        match name {
            "employees" => self.query_all("SELECT * FROM employees ORDER BY id LIMIT 500").await,
            "departments" => self.query_all("SELECT * FROM departments ORDER BY id LIMIT 500").await,
            "hr_cases" if privileged => {
                self.query_all("SELECT * FROM hr_cases ORDER BY id DESC LIMIT 500").await
            }
            "hr_cases" => Ok(vec![]),
            "claims" if finance_scope => {
                self.query_all("SELECT * FROM claims ORDER BY id DESC LIMIT 500").await
            }
            "claims" => {
                self.query_json(
                    "SELECT * FROM claims WHERE applicant_id=?1 ORDER BY id DESC LIMIT 500",
                    params![user.employee_id],
                )
                .await
            }
            "claim_items" if finance_scope => {
                let rows = self.query_all("SELECT * FROM claim_items ORDER BY id LIMIT 500").await?;
                Ok(decorate_claim_items(rows))
            }
            // 普通员工只能看到自己名下报销单的明细（靠 claim_id 关联裁剪，不信任前端）
            "claim_items" => {
                let rows = self
                    .query_json(
                        "SELECT ci.* FROM claim_items ci JOIN claims c ON c.id=ci.claim_id
                         WHERE c.applicant_id=?1 ORDER BY ci.id LIMIT 500",
                        params![user.employee_id],
                    )
                    .await?;
                Ok(decorate_claim_items(rows))
            }
            "contracts" if finance_scope => {
                self.query_all("SELECT * FROM contracts ORDER BY id DESC LIMIT 500").await
            }
            "contracts" => Ok(vec![]),
            "attendance" if privileged => {
                self.query_all("SELECT * FROM attendance ORDER BY work_date DESC, id DESC LIMIT 500")
                    .await
            }
            "attendance" => {
                self.query_json(
                    "SELECT * FROM attendance WHERE employee_id=?1
                     ORDER BY work_date DESC, id DESC LIMIT 500",
                    params![user.employee_id],
                )
                .await
            }
            "makeup_requests" if privileged => {
                self.query_all(
                    "SELECT * FROM makeup_requests ORDER BY status='pending' DESC, work_date DESC LIMIT 500",
                )
                .await
            }
            "makeup_requests" => {
                self.query_json(
                    "SELECT * FROM makeup_requests WHERE employee_id=?1
                     ORDER BY work_date DESC, id DESC LIMIT 500",
                    params![user.employee_id],
                )
                .await
            }
            "assets" => self.query_all("SELECT * FROM assets ORDER BY id LIMIT 500").await,
            "asset_events" if privileged => {
                self.query_all("SELECT * FROM asset_events ORDER BY created_at DESC, id LIMIT 500")
                    .await
            }
            "asset_events" => Ok(vec![]),
            "budgets" if finance_scope => {
                self.query_all("SELECT * FROM budgets ORDER BY year DESC, dept_id LIMIT 500").await
            }
            "budgets" => Ok(vec![]),
            "payments" if finance_scope => {
                self.query_all("SELECT * FROM payments ORDER BY id DESC LIMIT 500").await
            }
            "payments" => Ok(vec![]),
            "approval_flows" if privileged => {
                self.query_all(
                    "SELECT * FROM approval_flows ORDER BY created_at DESC, id LIMIT 500",
                )
                .await
            }
            // 普通员工看不到别人的审批链，但自己单据（报销 / 补卡）的进度要能看到
            "approval_flows" => {
                self.query_json(
                    "SELECT f.* FROM approval_flows f
                     WHERE (f.entity_type='claim'
                            AND f.entity_id IN (SELECT id FROM claims WHERE applicant_id=?1))
                        OR (f.entity_type='makeup'
                            AND f.entity_id IN (SELECT id FROM makeup_requests WHERE employee_id=?1))
                     ORDER BY f.created_at DESC, f.id LIMIT 500",
                    params![user.employee_id],
                )
                .await
            }
            "notifications" => {
                self.query_json(
                    "SELECT * FROM notifications WHERE recipient_id=?1
                     ORDER BY created_at DESC, id DESC LIMIT 500",
                    params![user.employee_id],
                )
                .await
            }
            "audit_events" if admin => {
                self.query_all("SELECT * FROM audit_events ORDER BY id DESC LIMIT 500").await
            }
            "audit_events" => Ok(vec![]),
            // 字段定义全员可读（创建表单要渲染），未启用的也返回以便管理员在设置页恢复
            "custom_fields" => {
                self.query_all("SELECT * FROM custom_fields ORDER BY entity_type, sort_order, id LIMIT 500")
                    .await
            }
            // 字段值跟随各业务表的数据范围：管理员/对应业务角色看全部，其余只看本人相关记录
            "custom_field_values" if privileged => {
                self.query_all("SELECT * FROM custom_field_values ORDER BY entity_type, entity_id, field_id LIMIT 500")
                    .await
            }
            "custom_field_values" => {
                self.query_json(
                    "SELECT v.* FROM custom_field_values v
                     WHERE (v.entity_type='employee' AND v.entity_id=?1)
                        OR (v.entity_type='claim'
                            AND v.entity_id IN (SELECT id FROM claims WHERE applicant_id=?1))
                        OR (v.entity_type='contract'
                            AND v.entity_id IN (SELECT id FROM contracts WHERE owner_id=?1))
                        -- 资产台账全员可见，其字段值同样放开
                        OR v.entity_type='asset'
                     ORDER BY v.entity_type, v.entity_id, v.field_id LIMIT 500",
                    params![user.employee_id],
                )
                .await
            }
            _ => Err(anyhow!("unknown table: {name}")),
        }
    }

    /* ---------------- 审计 ---------------- */

    pub async fn audit_events(&self, limit: i64, offset: i64) -> Result<Vec<Value>> {
        self.query_json(
            "SELECT id, tool, actor, payload, result, created_at FROM audit_events
             ORDER BY id DESC LIMIT ?1 OFFSET ?2",
            params![limit.clamp(1, 500), offset.max(0)],
        )
        .await
    }

    pub async fn last_audit_id(&self) -> Result<i64> {
        self.scalar_i64("SELECT COALESCE(MAX(id),0) FROM audit_events", &[])
            .await
    }

    pub async fn audit_since(&self, id: i64) -> Result<Vec<Value>> {
        self.query_json(
            "SELECT id, tool, actor, payload, result, created_at FROM audit_events
             WHERE id > ?1 ORDER BY id",
            params![id],
        )
        .await
    }

    pub async fn audit(
        &self,
        tool: &str,
        actor: &str,
        payload: &Value,
        result: &Value,
    ) -> Result<()> {
        self.execute(
            "INSERT INTO audit_events(tool, actor, payload, result) VALUES (?1, ?2, ?3, ?4)",
            params![tool, actor, payload.to_string(), result.to_string()],
        )
        .await?;
        Ok(())
    }
}

/// 台账行补充「费用标准 / 是否超标」：标准是代码里的策略常量，不落业务表，
/// 调整标准无需迁移历史数据；单条是否超标由金额与常量实时判定，审批留痕靠明细备注。
fn decorate_claim_items(mut rows: Vec<Value>) -> Vec<Value> {
    for row in &mut rows {
        let Some(obj) = row.as_object_mut() else {
            continue;
        };
        let category = obj
            .get("category")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let amount = obj.get("amount_cents").and_then(Value::as_i64).unwrap_or(0);
        let standard = domain::item_standard(&category);
        obj.insert("standard_cents".to_string(), Value::from(standard.unwrap_or(0)));
        obj.insert(
            "over_standard".to_string(),
            Value::Bool(standard.is_some_and(|cap| amount > cap)),
        );
    }
    rows
}

#[async_trait::async_trait]
impl Repo for Store {
    async fn execute(&mut self, sql: &str, params: Params<'_>) -> Result<usize> {
        Store::execute(self, sql, params).await
    }
    async fn scalar_i64(&mut self, sql: &str, params: Params<'_>) -> Result<i64> {
        Store::scalar_i64(self, sql, params).await
    }
    async fn query_json(&mut self, sql: &str, params: Params<'_>) -> Result<Vec<Value>> {
        Store::query_json(self, sql, params).await
    }
    async fn query_all(&mut self, sql: &str) -> Result<Vec<Value>> {
        Store::query_all(self, sql).await
    }
}

/* ---------------- seed 渲染 ---------------- */

/// 把模板里的 `date('now'[, '±N day'])` 渲染成本地日期字面量，
/// 并把 `INSERT OR IGNORE` 统一改写为两种数据库都支持的 `ON CONFLICT DO NOTHING`。
fn seed_sql() -> String {
    render_seed(SEED_TEMPLATE)
}

fn render_seed(template: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"date\('now'(?:,\s*'([+-]\d+) day')?\)").expect("seed date regex")
    });
    let rendered = re.replace_all(template, |caps: &regex::Captures| {
        let days = caps
            .get(1)
            .map(|m| m.as_str().parse::<i64>().unwrap_or(0))
            .unwrap_or(0);
        format!("'{}'", clock::date_offset(days))
    });
    let mut out = String::with_capacity(rendered.len() + 512);
    for stmt in rendered.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        let stmt = stmt.replacen("INSERT OR IGNORE INTO", "INSERT INTO", 1);
        out.push_str(&stmt);
        if stmt.starts_with("INSERT") {
            out.push_str(" ON CONFLICT DO NOTHING");
        }
        out.push_str(";\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain;

    #[tokio::test]
    async fn schema_and_seed_are_complete() {
        let db = crate::test_support::test_store().await;
        assert!(db.is_seeded().await.unwrap());
        for t in domain::TABLE_NAMES {
            let n = db
                .scalar_i64(&format!("SELECT COUNT(*) FROM {t}"), &[])
                .await
                .unwrap_or(-1);
            assert!(n > 0, "table {t} should be seeded, got {n}");
        }
    }

    #[tokio::test]
    async fn seed_is_idempotent() {
        let db = crate::test_support::test_store().await;
        let before = db
            .scalar_i64("SELECT COUNT(*) FROM employees", &[])
            .await
            .unwrap();
        assert_eq!(db.ensure_seed().await.unwrap(), 0);
        assert_eq!(
            db.scalar_i64("SELECT COUNT(*) FROM employees", &[])
                .await
                .unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn reset_restores_seed() {
        let db = crate::test_support::test_store().await;
        db.execute("DELETE FROM employees WHERE id='E01'", &[])
            .await
            .unwrap();
        assert_eq!(
            db.scalar_i64("SELECT COUNT(*) FROM employees", &[])
                .await
                .unwrap(),
            15
        );
        db.reset_seed().await.unwrap();
        assert_eq!(
            db.scalar_i64("SELECT COUNT(*) FROM employees", &[])
                .await
                .unwrap(),
            16
        );
    }

    #[tokio::test]
    async fn table_rows_respect_role_scope() {
        let db = crate::test_support::test_store().await;
        let (_, admin) = db.login("evan", crate::auth::DEFAULT_PASSWORD).await.unwrap();
        let (_, employee) = db.login("lina", crate::auth::DEFAULT_PASSWORD).await.unwrap();

        let all_claims = db.table_rows(&admin, "claims").await.unwrap();
        let own_claims = db.table_rows(&employee, "claims").await.unwrap();
        assert!(all_claims.len() > own_claims.len());
        assert!(own_claims.iter().all(|r| r["applicant_id"] == "E03"));

        assert!(db.table_rows(&employee, "contracts").await.unwrap().is_empty());
        assert!(db.table_rows(&employee, "audit_events").await.unwrap().is_empty());
        assert!(!db.table_rows(&admin, "audit_events").await.unwrap().is_empty());
        assert!(!db.table_rows(&employee, "employees").await.unwrap().is_empty());
        assert!(db
            .table_rows(&employee, "notifications")
            .await
            .unwrap()
            .iter()
            .all(|r| r["recipient_id"] == "E03"));
        // 补卡申请：员工只看自己的，管理者能看到全部待审批
        assert!(db
            .table_rows(&employee, "makeup_requests")
            .await
            .unwrap()
            .iter()
            .all(|r| r["employee_id"] == "E03"));
        let all_makeups = db.table_rows(&admin, "makeup_requests").await.unwrap();
        assert!(all_makeups.iter().any(|r| r["status"] == "pending"));
        // 员工的审批链可见范围包含本人补卡
        let own_flows = db.table_rows(&employee, "approval_flows").await.unwrap();
        assert!(own_flows.iter().any(|f| f["entity_type"] == "makeup"));
        assert!(db.table_rows(&admin, "no_such_table").await.is_err());
    }

    #[tokio::test]
    async fn transaction_rolls_back() {
        let db = crate::test_support::empty_store().await;
        let result: Result<()> = db
            .transaction(|tx| {
                Box::pin(async move {
                    tx.execute("INSERT INTO departments(id,name) VALUES('X','Tmp')", &[])
                        .await?;
                    Err(anyhow!("force rollback"))
                })
            })
            .await;
        assert!(result.is_err());
        assert_eq!(
            db.scalar_i64("SELECT COUNT(*) FROM departments", &[])
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn next_id_is_atomic_under_concurrency() {
        let db = crate::test_support::test_store().await;
        let mut handles = Vec::new();
        for _ in 0..8 {
            let db = db.clone();
            handles.push(tokio::spawn(async move {
                let mut conn = db;
                crate::db::next_id(&mut conn, "claims", "BX-TEST", 4)
                    .await
                    .unwrap()
            }));
        }
        let mut ids = Vec::new();
        for h in handles {
            ids.push(h.await.unwrap());
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 8, "duplicate ids: {ids:?}");
    }
}
