//! 数据库抽象层：统一走 Postgres。
//!
//! 设计：
//! - SQL 统一用 `?1` / `?` 占位，执行前重写为 Postgres 的 `$1`，业务 SQL 只写一份；
//! - 参数统一用 `serde_json::Value`（`params!` 宏），绑定层转成 PG 原生类型；
//! - 查询结果统一转 `serde_json::Value`，避免为每张表各写一遍 row mapper；
//! - 写多步操作通过 `Store::transaction` + `Repo` 对象；
//! - 业务单号用 `counters` 表原子取号（`UPDATE ... RETURNING`），避免 `MAX(id)+1` 撞号。

use anyhow::{anyhow, Result};
use serde_json::{Map, Value};
use sqlx::postgres::{PgArguments, PgRow};
use sqlx::query::Query;
use sqlx::{Column, Executor, Postgres, Row, Transaction};
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

pub type Params<'a> = &'a [Value];
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 参数宏：`params![a, b]` → `&[Value]`，自动借用，兼容 `&str/String/i64/f64/bool/Option<T>`。
#[macro_export]
macro_rules! params {
    () => { &[] as &[serde_json::Value] };
    ($($x:expr),+ $(,)?) => {
        &[$(serde_json::json!(&$x)),+] as &[serde_json::Value]
    };
}

/// 仓储接口：`Store`（连接池，自动提交）与 `Tx`（事务）都实现它。
/// 方法取 `&mut self`：事务对象内部需要可变连接，统一后调用点无需区分。
#[async_trait::async_trait]
pub trait Repo: Send + Sync {
    async fn execute(&mut self, sql: &str, params: Params<'_>) -> Result<usize>;
    async fn scalar_i64(&mut self, sql: &str, params: Params<'_>) -> Result<i64>;
    async fn query_json(&mut self, sql: &str, params: Params<'_>) -> Result<Vec<Value>>;

    async fn query_all(&mut self, sql: &str) -> Result<Vec<Value>> {
        self.query_json(sql, &[]).await
    }
}

/* ---------------- 方言 ---------------- */

fn placeholder_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\?(\d*)").expect("placeholder regex"))
}

/// 业务 SQL 统一用 `?N`，执行前重写为 Postgres 的 `$N`。
pub fn normalize_sql(sql: &str) -> Cow<'_, str> {
    let mut seq = 0usize;
    placeholder_re().replace_all(sql, |caps: &regex::Captures| {
        let n = match caps.get(1).map(|m| m.as_str()).filter(|s| !s.is_empty()) {
            Some(idx) => idx.parse::<usize>().unwrap_or(1),
            None => {
                seq += 1;
                seq
            }
        };
        format!("${n}")
    })
}

/* ---------------- 参数绑定 ---------------- */

fn bind_pg<'q>(
    mut q: Query<'q, Postgres, PgArguments>,
    params: Params<'q>,
) -> Query<'q, Postgres, PgArguments> {
    for p in params {
        q = match p {
            Value::Null => q.bind(Option::<String>::None),
            Value::Bool(b) => q.bind(*b),
            Value::Number(num) => match num.as_i64() {
                Some(i) => q.bind(i),
                None => q.bind(num.as_f64().unwrap_or(0.0)),
            },
            Value::String(s) => q.bind(s.as_str()),
            other => q.bind(other.to_string()),
        };
    }
    q
}

/* ---------------- 行 → JSON ---------------- */

fn pg_cell(row: &PgRow, i: usize) -> Value {
    if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(i) {
        return Value::from(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(i) {
        return Value::from(v as i64);
    }
    if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(i) {
        return Value::from(v);
    }
    if let Ok(Some(v)) = row.try_get::<Option<String>, _>(i) {
        return Value::String(v);
    }
    Value::Null
}

fn to_object(names: &[String], cells: impl Iterator<Item = Value>) -> Value {
    let mut obj = Map::new();
    for (name, value) in names.iter().zip(cells) {
        obj.insert(name.clone(), value);
    }
    Value::Object(obj)
}

/* ---------------- Postgres 执行器 ---------------- */

pub async fn fetch_pg<'c, E>(exec: E, sql: &str, params: Params<'_>) -> Result<Vec<Value>>
where
    E: Executor<'c, Database = Postgres>,
{
    let rewritten = normalize_sql(sql);
    let q = bind_pg(sqlx::query(rewritten.as_ref()), params);
    let rows: Vec<PgRow> = q
        .fetch_all(exec)
        .await
        .map_err(|e| anyhow!("postgres query failed for `{sql}`: {e:#}"))?;
    Ok(rows
        .iter()
        .map(|r| {
            let names: Vec<String> = r.columns().iter().map(|c| c.name().to_string()).collect();
            to_object(&names, (0..names.len()).map(|i| pg_cell(r, i)))
        })
        .collect())
}

pub async fn execute_pg<'c, E>(exec: E, sql: &str, params: Params<'_>) -> Result<u64>
where
    E: Executor<'c, Database = Postgres>,
{
    let rewritten = normalize_sql(sql);
    let q = bind_pg(sqlx::query(rewritten.as_ref()), params);
    Ok(q.execute(exec)
        .await
        .map_err(|e| anyhow!("postgres execute failed for `{sql}`: {e:#}"))?
        .rows_affected())
}

/* ---------------- 事务对象 ---------------- */

/// 事务句柄。`Store::transaction` 创建，交由闭包内的 `&mut dyn Repo` 使用。
/// 保留 newtype 包装，业务代码只依赖 `&mut dyn Repo`，不感知具体数据库。
pub struct Tx(Transaction<'static, Postgres>);

impl Tx {
    pub(crate) fn new(tx: Transaction<'static, Postgres>) -> Self {
        Self(tx)
    }

    pub async fn commit(self) -> Result<()> {
        self.0.commit().await?;
        Ok(())
    }

    pub async fn rollback(self) -> Result<()> {
        self.0.rollback().await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Repo for Tx {
    async fn execute(&mut self, sql: &str, params: Params<'_>) -> Result<usize> {
        let n = execute_pg(&mut *self.0, sql, params).await?;
        Ok(n as usize)
    }

    async fn scalar_i64(&mut self, sql: &str, params: Params<'_>) -> Result<i64> {
        let rows = fetch_pg(&mut *self.0, sql, params).await?;
        rows.first()
            .and_then(|r| r.as_object())
            .and_then(|o| o.values().next())
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("scalar query returned no value"))
    }

    async fn query_json(&mut self, sql: &str, params: Params<'_>) -> Result<Vec<Value>> {
        fetch_pg(&mut *self.0, sql, params).await
    }
}

/* ---------------- 原子取号 ---------------- */

/// `counters` 表原子自增：跨实例安全，替代 `MAX(id)+1`。
pub async fn next_seq(repo: &mut dyn Repo, name: &str, seed_sql: &str) -> Result<i64> {
    // `WHERE true` 明确分隔 `INSERT ... SELECT` 与 upsert 子句
    let insert = format!(
        "INSERT INTO counters(name, value) SELECT ?1, COALESCE(({seed_sql}), 0) \
         WHERE true ON CONFLICT(name) DO NOTHING"
    );
    repo.execute(&insert, crate::params![name]).await?;
    let rows = repo
        .query_json(
            "UPDATE counters SET value = value + 1 WHERE name = ?1 RETURNING value",
            crate::params![name],
        )
        .await?;
    rows.first()
        .and_then(|r| r["value"].as_i64())
        .ok_or_else(|| anyhow!("counter {name} not initialized"))
}

pub async fn next_id(
    repo: &mut dyn Repo,
    table: &str,
    prefix: &str,
    width: usize,
) -> Result<String> {
    // table/prefix 均来自代码内常量（含年份），不接受外部输入
    // RIGHT 取末尾数字段：Postgres 的 SUBSTR 不支持负数起点
    let seed = format!(
        "SELECT COALESCE(MAX(CAST(RIGHT(id, {width}) AS BIGINT)),0) \
         FROM {table} WHERE id LIKE '{prefix}%'"
    );
    let n = next_seq(repo, prefix, &seed).await?;
    Ok(format!("{prefix}-{:0width$}", n, width = width))
}
