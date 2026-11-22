//! oa-core · 纯 Rust 领域模型（无平台依赖，桌面/移动/服务端共用）

pub mod agent;
pub mod auth;
pub mod clock;
pub mod db;
pub mod domain;
pub mod files;
pub mod store;

#[cfg(test)]
pub(crate) mod test_support;

pub use auth::SessionUser;

/// 所有 Tool 共享的宿主状态。`Store` 内部是 sqlx 连接池（Clone = 共享池），
/// 不再需要 `Mutex`：并发请求由连接池调度，事务通过 `Store::transaction` 显式开启。
#[derive(Clone)]
pub struct AppState {
    pub store: store::Store,
}

impl AppState {
    pub fn new(store: store::Store) -> Self {
        Self { store }
    }
}
