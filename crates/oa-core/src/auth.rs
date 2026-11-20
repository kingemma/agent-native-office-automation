//! 认证与 RBAC（角色访问控制）。
//!
//! 设计要点：
//! - 账号与员工一一对应（`users.employee_id → employees.id`），审计里的 actor 永远是真实员工号；
//! - 密码用随机盐 + 多轮 SHA-256 摘要存储，库里不出现明文；
//! - 会话表 `sessions` 有 7 天有效期，登录时顺带清理过期会话；
//! - 角色 → 工具权限矩阵是唯一事实来源：REST、Tauri、Agent（本地/LLM）共用 `SessionUser::can`。
//!
//! 默认演示账号见 `SEED_USERS`，密码统一 `DEFAULT_PASSWORD`（首次登录后可在设置中修改，
//! 修改能力由 `change_password` 提供）。

use crate::{clock, params};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::domain;

/// 全部角色（admin 隐式拥有所有权限，其余按 `can_use` 矩阵判定）。
pub const ROLES: &[&str] = &["admin", "hr", "finance", "manager", "employee"];

pub const DEFAULT_PASSWORD: &str = "oa123456";

/// (员工号, 登录名, 显示名, 角色)
pub const SEED_USERS: &[(&str, &str, &str, &str)] = &[
    ("E01", "evan", "Evan", "admin"),
    ("E02", "chenli", "陈立", "manager"),
    ("E03", "lina", "李娜", "employee"),
    ("E10", "wuhao", "吴昊", "finance"),
    ("E12", "fengxue", "冯雪", "hr"),
];

/// 已认证的会话主体。所有写操作与审计都以它为唯一身份来源，前端传入的 actor 一律忽略。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionUser {
    pub employee_id: String,
    pub username: String,
    pub display_name: String,
    pub role: String,
    /// 该角色可调用的工具名列表（admin 为全部工具）
    pub permissions: Vec<String>,
}

impl SessionUser {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }

    /// 是否允许调用某个工具。权限判定只认服务端会话，不认请求体里的 actor。
    pub fn can(&self, tool: &str) -> bool {
        self.is_admin() || can_use(&self.role, tool)
    }

    /// 审计用的操作者标识（员工号）。
    pub fn actor(&self) -> &str {
        &self.employee_id
    }
}

/// 角色 → 工具权限矩阵。
pub fn can_use(role: &str, tool: &str) -> bool {
    if role == "admin" {
        return true;
    }
    let allowed: &[&str] = match tool {
        "hr.onboard" | "hr.offboard" | "hr.promote" => &["hr"],
        "hr.roster.query" | "org.chart" => &["hr", "finance", "manager", "employee"],
        "finance.claim.create" => &["hr", "finance", "manager", "employee"],
        "finance.claim.approve" | "finance.summary" => &["finance", "manager"],
        "contract.create" | "contract.approve" => &["finance", "manager"],
        "contract.query" => &["hr", "finance", "manager", "employee"],
        "attendance.checkin" | "attendance.makeup" => &["hr", "finance", "manager", "employee"],
        "attendance.approve" => &["hr", "manager"],
        "attendance.report" => &["hr", "manager"],
        "asset.register" => &["hr"],
        "asset.assign" | "asset.return" => &["hr", "manager"],
        "asset.query" => &["hr", "finance", "manager", "employee"],
        _ => &[],
    };
    allowed.contains(&role)
}

pub fn permissions_for(role: &str) -> Vec<String> {
    domain::TOOL_NAMES
        .iter()
        .filter(|t| role == "admin" || can_use(role, t))
        .map(|t| (*t).to_string())
        .collect()
}

/* ---------------- 密码 / 令牌 ---------------- */

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

/// 随机盐：不引入 rand 依赖，用高精度时钟 + 原子计数 + 进程号派生，再摘要拉平分布。
fn new_salt(seed: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let raw = format!("{seed}:{nanos}:{n}:{}", std::process::id());
    hex(&Sha256::digest(raw.as_bytes()))[..32].to_string()
}

/// 1024 轮 SHA-256：单次摘要对离线爆破太弱，多轮显著抬高成本且无需额外依赖。
fn hash_password(salt: &str, password: &str) -> String {
    let mut digest = Sha256::digest(format!("{salt}:{password}").as_bytes()).to_vec();
    for _ in 0..1023 {
        let mut h = Sha256::new();
        h.update(&digest);
        h.update(salt.as_bytes());
        digest = h.finalize().to_vec();
    }
    hex(&digest)
}

fn new_token() -> String {
    new_salt("session")
}

/* ---------------- Store 上的认证方法 ---------------- */

impl crate::store::Store {
    /// 幂等写入演示账号：已有任意账号就跳过，避免覆盖用户改过的密码。
    pub async fn ensure_auth_seed(&self) -> Result<usize> {
        let existing = self.scalar_i64("SELECT COUNT(*) FROM users", &[]).await?;
        if existing > 0 {
            return Ok(0);
        }
        for (emp, username, display, role) in SEED_USERS {
            let salt = new_salt(username);
            let hash = hash_password(&salt, DEFAULT_PASSWORD);
            self.execute(
                "INSERT INTO users(id,employee_id,username,display_name,password_hash,salt,role,active)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,1) ON CONFLICT DO NOTHING",
                params![
                    format!("U-{emp}"),
                    emp,
                    username,
                    display,
                    hash,
                    salt,
                    role
                ],
            )
            .await?;
        }
        Ok(SEED_USERS.len())
    }

    /// 登录：校验账号密码，签发 7 天会话令牌。
    pub async fn login(&self, username: &str, password: &str) -> Result<(String, SessionUser)> {
        let key = username.trim();
        if key.is_empty() || password.is_empty() {
            bail!("请输入用户名与密码");
        }
        let row = self
            .query_json(
                "SELECT employee_id, username, display_name, password_hash, salt, role, active
                 FROM users WHERE lower(username)=lower(?1) OR employee_id=?1",
                params![key],
            )
            .await?
            .into_iter()
            .next();
        let Some(row) = row else {
            bail!("用户名或密码错误");
        };
        if row["active"].as_i64().unwrap_or(0) != 1 {
            bail!("账号已停用，请联系管理员");
        }
        let salt = row["salt"].as_str().unwrap_or("");
        let stored = row["password_hash"].as_str().unwrap_or("");
        if hash_password(salt, password) != stored {
            bail!("用户名或密码错误");
        }

        // 离职冻结：offboard 只把 employees.account_frozen 置 1，这里必须一并校验，
        // 否则已离职员工仍能用旧密码重新登录
        let employee_id = row["employee_id"].as_str().unwrap_or("").to_string();
        let frozen = self
            .query_json(
                "SELECT account_frozen FROM employees WHERE id=?1",
                params![employee_id],
            )
            .await?
            .first()
            .and_then(|r| r["account_frozen"].as_i64())
            .unwrap_or(0);
        if frozen != 0 {
            bail!("账号已停用（员工已离职），请联系管理员");
        }

        let role = row["role"].as_str().unwrap_or("employee").to_string();
        let user = SessionUser {
            employee_id,
            username: row["username"].as_str().unwrap_or("").to_string(),
            display_name: row["display_name"].as_str().unwrap_or("").to_string(),
            permissions: permissions_for(&role),
            role,
        };

        self.execute(
            "DELETE FROM sessions WHERE expires_at <= ?1",
            params![clock::utc_now()],
        )
        .await?;
        let token = new_token();
        self.execute(
            "INSERT INTO sessions(token,user_id,expires_at) VALUES(?1,?2,?3)",
            params![token, user.employee_id, clock::utc_after_days(7)],
        )
        .await?;
        Ok((token, user))
    }

    /// 令牌 → 会话主体；过期或停用返回 None。
    pub async fn resolve_session(&self, token: &str) -> Result<Option<SessionUser>> {
        if token.trim().is_empty() {
            return Ok(None);
        }
        let row = self
            .query_json(
                "SELECT u.employee_id, u.username, u.display_name, u.role
                 FROM sessions s
                 JOIN users u ON u.employee_id=s.user_id
                 LEFT JOIN employees e ON e.id=u.employee_id
                 WHERE s.token=?1 AND s.expires_at > ?2 AND u.active=1
                   AND COALESCE(e.account_frozen,0)=0",
                params![token, clock::utc_now()],
            )
            .await?
            .into_iter()
            .next();
        Ok(row.map(|r| {
            let role = r["role"].as_str().unwrap_or("employee").to_string();
            SessionUser {
                employee_id: r["employee_id"].as_str().unwrap_or("").to_string(),
                username: r["username"].as_str().unwrap_or("").to_string(),
                display_name: r["display_name"].as_str().unwrap_or("").to_string(),
                permissions: permissions_for(&role),
                role,
            }
        }))
    }

    pub async fn logout(&self, token: &str) -> Result<()> {
        self.execute("DELETE FROM sessions WHERE token=?1", params![token])
            .await?;
        Ok(())
    }

    /// 修改密码：需校验旧密码，成功后吊销该账号的全部会话。
    pub async fn change_password(&self, employee_id: &str, old: &str, new: &str) -> Result<()> {
        if new.len() < 6 {
            bail!("新密码至少 6 位");
        }
        let row = self
            .query_json(
                "SELECT password_hash, salt FROM users WHERE employee_id=?1",
                params![employee_id],
            )
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("账号不存在"))?;
        let salt = row["salt"].as_str().unwrap_or("");
        if hash_password(salt, old) != row["password_hash"].as_str().unwrap_or("") {
            bail!("原密码不正确");
        }
        let new_salt = new_salt(employee_id);
        let hash = hash_password(&new_salt, new);
        self.execute(
            "UPDATE users SET password_hash=?2, salt=?3 WHERE employee_id=?1",
            params![employee_id, hash, new_salt],
        )
        .await?;
        self.execute("DELETE FROM sessions WHERE user_id=?1", params![employee_id])
            .await?;
        Ok(())
    }

    /// 账号台账（不含密码摘要与盐），供管理员查看。
    pub async fn user_accounts(&self) -> Result<Vec<Value>> {
        self.query_json(
            "SELECT u.employee_id, u.username, u.display_name, u.role, u.active,
                    (SELECT COUNT(*) FROM sessions s WHERE s.user_id=u.employee_id
                       AND s.expires_at > ?1) AS active_sessions
             FROM users u ORDER BY u.employee_id",
            params![clock::utc_now()],
        )
        .await
    }

    /// 管理员启用/停用账号。
    pub async fn set_user_active(&self, employee_id: &str, active: bool) -> Result<()> {
        let n = self
            .execute(
                "UPDATE users SET active=?2 WHERE employee_id=?1",
                params![employee_id, active as i64],
            )
            .await?;
        if n == 0 {
            bail!("账号 {employee_id} 不存在");
        }
        if !active {
            self.execute("DELETE FROM sessions WHERE user_id=?1", params![employee_id])
                .await?;
        }
        Ok(())
    }
}

/// 登录响应（REST / Tauri 同构）。
pub fn login_payload(token: String, user: SessionUser) -> Value {
    json!({ "token": token, "user": user })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[tokio::test]
    async fn seed_accounts_can_login_and_permissions_apply() {
        let db = Store::open_in_memory().await.unwrap();
        let (token, admin) = db.login("evan", DEFAULT_PASSWORD).await.unwrap();
        assert_eq!(admin.role, "admin");
        assert_eq!(admin.permissions.len(), domain::TOOL_NAMES.len());
        assert!(db.resolve_session(&token).await.unwrap().is_some());

        let (_, finance) = db.login("wuhao", DEFAULT_PASSWORD).await.unwrap();
        assert!(finance.can("finance.claim.approve"));
        assert!(!finance.can("hr.onboard"));

        let (_, employee) = db.login("lina", DEFAULT_PASSWORD).await.unwrap();
        assert!(employee.can("finance.claim.create"));
        assert!(!employee.can("finance.claim.approve"));
        assert!(!employee.can("asset.register"));
    }

    #[tokio::test]
    async fn wrong_password_is_rejected() {
        let db = Store::open_in_memory().await.unwrap();
        assert!(db.login("evan", "bad-password").await.is_err());
        assert!(db.login("nobody", DEFAULT_PASSWORD).await.is_err());
    }

    /// 离职冻结（employees.account_frozen=1）后：不能再登录，旧会话令牌也立即失效。
    #[tokio::test]
    async fn offboard_frozen_account_cannot_login_or_resume_session() {
        let db = Store::open_in_memory().await.unwrap();
        let (token, employee) = db.login("lina", DEFAULT_PASSWORD).await.unwrap();
        assert_eq!(employee.employee_id, "E03");
        assert!(db.resolve_session(&token).await.unwrap().is_some());

        // 模拟 hr.offboard 的副作用：只置 account_frozen，不碰 users.active
        db.execute("UPDATE employees SET account_frozen=1 WHERE id=?1", params!["E03"])
            .await
            .unwrap();

        assert!(db.login("lina", DEFAULT_PASSWORD).await.is_err(), "冻结后不应再允许登录");
        assert!(db.resolve_session(&token).await.unwrap().is_none(), "冻结后旧会话应立即失效");
    }

    #[tokio::test]
    async fn logout_invalidates_token() {
        let db = Store::open_in_memory().await.unwrap();
        let (token, _) = db.login("evan", DEFAULT_PASSWORD).await.unwrap();
        db.logout(&token).await.unwrap();
        assert!(db.resolve_session(&token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn change_password_revokes_sessions() {
        let db = Store::open_in_memory().await.unwrap();
        let (token, user) = db.login("lina", DEFAULT_PASSWORD).await.unwrap();
        db.change_password(&user.employee_id, DEFAULT_PASSWORD, "newpass123")
            .await
            .unwrap();
        assert!(db.resolve_session(&token).await.unwrap().is_none());
        assert!(db.login("lina", "newpass123").await.is_ok());
        assert!(db.login("lina", DEFAULT_PASSWORD).await.is_err());
    }

    #[test]
    fn rbac_matrix_covers_every_tool() {
        for role in ROLES {
            let perms = permissions_for(role);
            if *role == "admin" {
                assert_eq!(perms.len(), domain::TOOL_NAMES.len());
            } else {
                assert!(!perms.is_empty(), "role {role} should own some tools");
                assert!(perms.len() < domain::TOOL_NAMES.len());
            }
        }
    }
}
