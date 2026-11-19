//! oa-server · Axum HTTP 服务（服务器 / 局域网部署）
//! 桌面与移动端走 Tauri invoke 直调 oa-core，无需经过本服务。
//!
//! 鉴权：除 `/api/health` 与 `/api/auth/login` 外，所有业务端点都要求
//! `Authorization: Bearer <token>`；操作者身份与角色只从会话解析，请求体里的 actor 不被信任。

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use oa_core::auth::SessionUser;
use oa_core::{agent, auth, domain, files, store::Store, AppState};
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::path::PathBuf;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tower_http::cors::{Any, CorsLayer};

#[derive(Clone)]
struct WebState {
    app: AppState,
    /// 附件落盘目录（`OA_FILES`），与 Tauri 端同一套 files 逻辑，只是根目录不同
    files: PathBuf,
}

type ApiError = (StatusCode, String);

fn err500(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn require_admin(user: &SessionUser) -> Result<(), ApiError> {
    if user.is_admin() {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            format!("需要管理员权限，当前角色为 {}", user.role),
        ))
    }
}

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

async fn require_user(st: &WebState, headers: &HeaderMap) -> Result<SessionUser, ApiError> {
    let token = bearer(headers)
        .ok_or((StatusCode::UNAUTHORIZED, "未登录：缺少 Bearer 令牌".to_string()))?;
    st.app
        .store
        .resolve_session(&token)
        .await
        .map_err(err500)?
        .ok_or((StatusCode::UNAUTHORIZED, "会话已失效，请重新登录".to_string()))
}

/* ---------------- 健康检查 / 认证 ---------------- */

async fn health() -> Json<Value> {
    // instance 便于在负载均衡后确认请求落到哪个副本
    let instance = std::env::var("OA_INSTANCE").unwrap_or_else(|_| "local".to_string());
    Json(json!({
        "ok": true, "service": "yugong-oa", "version": env!("CARGO_PKG_VERSION"),
        "instance": instance
    }))
}

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

async fn login(
    State(st): State<WebState>,
    Json(req): Json<LoginReq>,
) -> Result<Json<Value>, ApiError> {
    let (token, user) = st
        .app
        .store
        .login(&req.username, &req.password)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;
    Ok(Json(auth::login_payload(token, user)))
}

async fn logout(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let token = bearer(&headers).ok_or((StatusCode::UNAUTHORIZED, "未登录".to_string()))?;
    st.app.store.logout(&token).await.map_err(err500)?;
    Ok(Json(json!({ "ok": true })))
}

async fn me(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<SessionUser>, ApiError> {
    let user = require_user(&st, &headers).await?;
    Ok(Json(user))
}

#[derive(Deserialize)]
struct PasswordReq {
    old_password: String,
    new_password: String,
}

async fn change_password(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<PasswordReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    st.app
        .store
        .change_password(&user.employee_id, &req.old_password, &req.new_password)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({ "ok": true })))
}

/* ---------------- 账号管理（管理员） ---------------- */

async fn users(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;
    let rows = st.app.store.user_accounts().await.map_err(err500)?;
    Ok(Json(json!({ "users": rows })))
}

#[derive(Deserialize)]
struct ActiveReq {
    active: bool,
}

async fn set_user_active(
    State(st): State<WebState>,
    headers: HeaderMap,
    Path(employee_id): Path<String>,
    Json(req): Json<ActiveReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;
    if employee_id == user.employee_id {
        return Err((StatusCode::BAD_REQUEST, "不能停用当前登录账号".to_string()));
    }
    st.app
        .store
        .set_user_active(&employee_id, req.active)
        .await
        .map_err(err500)?;
    Ok(Json(json!({ "ok": true, "employee_id": employee_id, "active": req.active })))
}

/* ---------------- Agent ---------------- */

#[derive(Deserialize)]
struct MessageReq {
    message: String,
}

async fn chat(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<MessageReq>,
) -> Result<Json<agent::AgentReply>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let reply = agent::run(st.app, &user, &req.message).await.map_err(err500)?;
    Ok(Json(reply))
}

#[derive(Deserialize)]
struct StreamQuery {
    q: String,
}

/// 流式对话：把 Agent 的思考步骤按 SSE 逐条推给前端。
///
/// 用 GET + query 而不是 POST，是为了让浏览器 `EventSource` 也能接；
/// 但本项目登录态走 `Authorization: Bearer`（EventSource 不支持自定义头），
/// 因此前端实际用 `fetch` + `ReadableStream` 读这个流。
async fn agent_stream(
    State(st): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<StreamQuery>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let message = query.q.trim().to_string();
    if message.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "指令不能为空".to_string()));
    }

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<agent::AgentEvent>();
    let app = st.app.clone();
    tokio::spawn(async move {
        let result =
            agent::run_stream(app, &user, &message, agent::Thinker::streaming(tx.clone())).await;
        // 收尾帧：成功给完整回复，失败给错误文本，前端据此结束本轮
        let last = match result {
            Ok(reply) => agent::AgentEvent::Reply { reply },
            Err(e) => agent::AgentEvent::Error { message: format!("{e:#}") },
        };
        let _ = tx.send(last);
    });

    let stream = UnboundedReceiverStream::new(rx).map(|event| {
        let data = serde_json::to_string(&event)
            .unwrap_or_else(|_| r#"{"type":"error","message":"事件序列化失败"}"#.to_string());
        Ok(Event::default().data(data))
    });
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

async fn plan(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<MessageReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let steps = agent::plan(&st.app, &user, &req.message).await.map_err(err500)?;
    let reply = if steps.is_empty() {
        "未识别到可执行流程，输入「帮助」查看全部能力。".to_string()
    } else {
        format!("计划执行 {} 步，确认后由 Agent 落库", steps.len())
    };
    Ok(Json(json!({ "steps": steps, "reply": reply })))
}

/* ---------------- 工具 ---------------- */

async fn list_tools(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    Ok(Json(json!({ "tools": agent::tools::specs_for(&user) })))
}

#[derive(Deserialize)]
struct CallReq {
    name: String,
    #[serde(default)]
    args: Value,
}

async fn call_tool(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<CallReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let out = agent::tools::call(st.app, &user, &req.name, req.args).await;
    Ok(Json(out))
}

/* ---------------- 附件（统一上传通道：HTTP 与 Tauri 共用 oa-core::files） ---------------- */

#[derive(Deserialize)]
struct UploadFileReq {
    entity_type: String,
    entity_id: String,
    file_name: String,
    /// 前端可能不传，服务端按扩展名兜底
    #[serde(default)]
    mime: String,
    /// 文件内容（base64，不带 data: 前缀）
    data_base64: String,
}

async fn upload_file(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<UploadFileReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let out = files::save(
        &st.app.store,
        &st.files,
        &user,
        &req.entity_type,
        &req.entity_id,
        &req.file_name,
        &req.mime,
        &req.data_base64,
    )
    .await
    .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct AttachmentQuery {
    entity_type: String,
    entity_id: String,
}

async fn list_files(
    State(st): State<WebState>,
    headers: HeaderMap,
    Query(q): Query<AttachmentQuery>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let rows = files::list(&st.app.store, &user, &q.entity_type, &q.entity_id)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({ "rows": rows })))
}

async fn read_file(
    State(st): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let out = files::read(&st.app.store, &st.files, &user, &id)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(out))
}

async fn delete_file(
    State(st): State<WebState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let out = files::delete(&st.app.store, &st.files, &user, &id)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(out))
}

/* ---------------- 台账 / 指标 / 审计 / 重置 ---------------- */

async fn table(
    State(st): State<WebState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let rows = st
        .app
        .store
        .table_rows(&user, &name)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    Ok(Json(json!({ "rows": rows })))
}

async fn metrics(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let _user = require_user(&st, &headers).await?;
    let m = st.app.store.metrics().await.map_err(err500)?;
    Ok(Json(m))
}

#[derive(Deserialize)]
struct PageQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    offset: Option<i64>,
}

async fn audit(
    State(st): State<WebState>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Result<Json<Value>, ApiError> {
    let _user = require_user(&st, &headers).await?;
    let rows = st
        .app
        .store
        .audit_events(q.limit.unwrap_or(100), q.offset.unwrap_or(0))
        .await
        .map_err(err500)?;
    Ok(Json(json!({ "events": rows })))
}

async fn reset_seed(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;
    st.app.store.reset_seed().await.map_err(err500)?;
    // 记录已随 RESET_SQL 清掉，落盘文件也要一起清，避免孤儿
    files::purge(&st.files);
    Ok(Json(json!({ "ok": true, "message": "演示数据已重置（账号与会话保留）" })))
}

/* ---------------- 运行时配置（仅管理员） ---------------- */

#[derive(Deserialize)]
struct LlmSettingsReq {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    model: String,
    /// 空字符串表示沿用已存 Key（前端只拿到掩码，不回传明文）
    #[serde(default)]
    api_key: String,
    /// true = 清除配置，回落到环境变量 / 本地引擎
    #[serde(default)]
    clear: bool,
}

async fn get_llm_settings(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;
    let view = agent::llm::describe(&st.app.store).await.map_err(err500)?;
    Ok(Json(view))
}

async fn save_llm_settings(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<LlmSettingsReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;
    let view = if req.clear {
        agent::llm::clear(&st.app.store).await
    } else {
        agent::llm::save(
            &st.app.store,
            &req.provider,
            &req.base_url,
            &req.model,
            &req.api_key,
        )
        .await
    };
    view.map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))
}

async fn test_llm_settings(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;
    // `{e:#}` 展开 anyhow 因果链，管理员能看到是连接被拒还是 401
    let reply = agent::llm::probe(&st.app.store)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    Ok(Json(json!({ "ok": true, "reply": reply })))
}

async fn features() -> Json<Value> {
    let rows: Vec<Value> = domain::FEATURES
        .iter()
        .map(|(id, dom, text)| json!({ "id": id, "domain": dom, "text": text }))
        .collect();
    Json(json!({ "features": rows, "tools": domain::TOOL_NAMES }))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // SQLite 用于单机；多租户部署改为 sqlx::PgPool + 同样的 SCHEMA_SQL。
    let db = std::env::var("OA_DB").unwrap_or_else(|_| "oa.db".to_string());
    // 附件根目录：默认 oa-files/，与库文件同机；容器部署时挂卷到 OA_FILES 即可
    let files_dir = files::default_root();
    std::fs::create_dir_all(&files_dir)?;
    let state = WebState {
        app: AppState::new(Store::open_uri(&db).await?),
        files: files_dir,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me))
        .route("/api/auth/change-password", post(change_password))
        .route("/api/users", get(users))
        .route("/api/users/{employee_id}/active", post(set_user_active))
        .route("/api/agent/chat", post(chat))
        .route("/api/agent/stream", get(agent_stream))
        .route("/api/agent/plan", post(plan))
        .route("/api/tools", get(list_tools))
        .route("/api/tools/call", post(call_tool))
        .route("/api/files", get(list_files).post(upload_file))
        .route("/api/files/{id}", get(read_file).delete(delete_file))
        .route("/api/tables/{name}", get(table))
        .route("/api/metrics", get(metrics))
        .route("/api/audit", get(audit))
        .route("/api/seed/reset", post(reset_seed))
        .route(
            "/api/settings/llm",
            get(get_llm_settings).post(save_llm_settings),
        )
        .route("/api/settings/llm/test", post(test_llm_settings))
        .route("/api/features", get(features))
        // base64 会把体积放大 4/3，10 MB 附件需要 ~14 MB body
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(cors)
        .with_state(state);

    // 默认监听所有网卡：桌面/移动端需要经局域网 IP 访问，仅绑 127.0.0.1 会连不上。
    // 只允许本机访问时显式设置 OA_ADDR=127.0.0.1:8787。
    let addr = std::env::var("OA_ADDR").unwrap_or_else(|_| "0.0.0.0:8787".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("oa-server listening on http://{addr}");
    println!("LAN clients: http://<this-host-ip>:8787 （Windows 防火墙需放行）");
    println!("demo account: evan / {}", auth::DEFAULT_PASSWORD);
    axum::serve(listener, app).await?;
    Ok(())
}
