//! 测试基础设施：共享一个 testcontainers Postgres，每个测试用独立 schema 隔离。
//!
//! Docker Desktop 必须正在运行；容器只启动一次，测试之间靠 `search_path` 隔离，
//! 因此不会因为并发执行而互相污染数据。

use crate::store::Store;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use tokio::sync::OnceCell;
use uuid::Uuid;

struct PgFixture {
    base_url: String,
    _container: ContainerAsync<Postgres>,
}

static PG: OnceCell<PgFixture> = OnceCell::const_new();

/// 返回共享容器的基础连接串（数据库 `postgres`）。
async fn base_url() -> &'static str {
    let fixture = PG
        .get_or_init(|| async {
            let container = Postgres::default()
                .with_host_auth()
                .with_tag("16-alpine")
                .start()
                .await
                .expect("启动 Postgres 测试容器失败：请确认 Docker Desktop 正在运行");
            let host = container.get_host().await.expect("读取 Docker host 失败");
            let port = container
                .get_host_port_ipv4(5432)
                .await
                .expect("读取 Postgres 映射端口失败");
            PgFixture {
                base_url: format!("postgres://postgres@{host}:{port}/postgres"),
                _container: container,
            }
        })
        .await;
    &fixture.base_url
}

async fn isolated_store(seeded: bool) -> Store {
    let base = base_url().await;
    let schema = format!("t_{}", Uuid::new_v4().simple());

    let admin = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect(base)
        .await
        .expect("连接测试 Postgres 失败");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("创建测试 schema 失败");
    admin.close().await;

    let options: PgConnectOptions = base.parse().expect("解析测试连接串失败");
    let options = options.options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options)
        .await
        .expect("连接测试 schema 失败");
    let store = Store(pool);
    if seeded {
        store.init().await.expect("初始化测试数据失败");
    } else {
        store.apply_schema().await.expect("创建测试表失败");
    }
    store
}

/// 建表并灌入演示数据，替代原来的 `Store::open_in_memory()`。
pub async fn test_store() -> Store {
    isolated_store(true).await
}

/// 只建表不灌业务数据，替代原来的 `Store::empty_in_memory()`。
pub async fn empty_store() -> Store {
    isolated_store(false).await
}
