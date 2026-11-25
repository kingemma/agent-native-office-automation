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
use anyhow::Context;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};

#[derive(Clone)]
struct WebState {
    app: AppState,
    /// 附件落盘目录（`OA_FILES`），与 Tauri 端同一套 files 逻辑，只是根目录不同
    files: PathBuf,
    /// 登录失败计数：账号 → (失败次数, 首次失败时间)，用于简单限流
    login_fails: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
}

/// 同一账号 10 分钟内失败 10 次后暂时拒绝，避免在线暴力破解。
const LOGIN_MAX_FAILS: u32 = 10;
const LOGIN_FAIL_WINDOW: Duration = Duration::from_secs(10 * 60);
/// 计数表上限，超出时清理过期条目，避免被大量随机用户名撑爆内存。
const LOGIN_FAIL_TRACK_MAX: usize = 2_000;

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
    let key = req.username.trim().to_lowercase();
    {
        let guard = st.login_fails.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((fails, first)) = guard.get(&key) {
            if *fails >= LOGIN_MAX_FAILS && first.elapsed() < LOGIN_FAIL_WINDOW {
                return Err((
                    StatusCode::TOO_MANY_REQUESTS,
                    "登录失败次数过多，请 10 分钟后再试".into(),
                ));
            }
        }
    }

    let result = st.app.store.login(&req.username, &req.password).await;
    let mut guard = st.login_fails.lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok((token, user)) => {
            guard.remove(&key);
            Ok(Json(auth::login_payload(token, user)))
        }
        Err(e) => {
            if guard.len() >= LOGIN_FAIL_TRACK_MAX {
                guard.retain(|_, (_, first)| first.elapsed() < LOGIN_FAIL_WINDOW);
            }
            let entry = guard.entry(key).or_insert((0, Instant::now()));
            if entry.1.elapsed() >= LOGIN_FAIL_WINDOW {
                *entry = (0, Instant::now());
            }
            entry.0 += 1;
            Err((StatusCode::UNAUTHORIZED, e.to_string()))
        }
    }
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
    /// 本轮之前的对话，服务端不再自己保存会话状态
    #[serde(default)]
    history: Vec<agent::ChatTurn>,
    /// 覆盖已存配置的模型名；空串表示沿用配置
    #[serde(default)]
    model: String,
}

async fn chat(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<MessageReq>,
) -> Result<Json<agent::AgentReply>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let reply = agent::run_stream_with_history(
        st.app,
        &user,
        &req.message,
        &req.history,
        &req.model,
        agent::Thinker::silent(),
    )
    .await
    .map_err(err500)?;
    Ok(Json(reply))
}

/// 对话页的模型下拉数据源：列出当前端点可用的模型，任意登录用户可读。
///
/// 只暴露模型名，不含 Key；端点不实现 `/models` 时退回「只用已存模型」，
/// 不因此把对话页卡住。
async fn agent_models(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let configured = agent::llm::effective(&st.app.store).await.map_err(err500)?;
    let Some(cfg) = configured.config else {
        return Ok(Json(json!({ "models": [], "current": "", "mode": "local" })));
    };
    // 用户上次选的模型优先，没选过才回落到全局配置
    let current = agent::llm::user_model(&st.app.store, &user.employee_id)
        .await
        .unwrap_or(None)
        .unwrap_or_else(|| cfg.model.clone());
    let (models, error) = match agent::llm::list_models(&st.app.store, "", "").await {
        Ok(list) => {
            let mut chat: Vec<String> = list
                .into_iter()
                .filter(|m| agent::llm::is_chat_model(m))
                .collect();
            // 过滤后为空说明端点没提供对话模型，至少保证已配置的那个能选
            if chat.is_empty() {
                chat.push(cfg.model.clone());
            }
            (chat, String::new())
        }
        Err(e) => (vec![cfg.model.clone()], format!("{e:#}")),
    };
    Ok(Json(json!({ "models": models, "current": current, "mode": "llm", "error": error })))
}

/// 记住当前用户选的对话模型（个人偏好，跨设备生效）
#[derive(Deserialize, Default)]
struct AgentModelReq {
    #[serde(default)]
    model: String,
}

async fn set_agent_model(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<AgentModelReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let model = req.model.trim();
    if model.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "模型名不能为空".to_string()));
    }
    agent::llm::set_user_model(&st.app.store, &user.employee_id, model)
        .await
        .map_err(err500)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct StreamQuery {
    q: String,
}

/// 把 Agent 的执行过程包成 SSE：思考步骤边执行边推，最后一帧是完整回复或错误。
fn agent_sse(
    app: AppState,
    user: SessionUser,
    message: String,
    history: Vec<agent::ChatTurn>,
    model: String,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<agent::AgentEvent>();
    tokio::spawn(async move {
        let result = agent::run_stream_with_history(
            app,
            &user,
            &message,
            &history,
            &model,
            agent::Thinker::streaming(tx.clone()),
        )
        .await;
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
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// 流式对话（GET 兼容口）：单轮，不带历史，仅用于调试与 EventSource 类客户端。
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
    Ok(agent_sse(st.app, user, message, Vec::new(), String::new()))
}

/// 流式对话（POST 主口）：请求体带上最近若干轮历史，模型才能接着上文回答。
/// 用 POST 是因为历史放不进 query string，长度与转义都不可控。
async fn agent_stream_post(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<MessageReq>,
) -> Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let message = req.message.trim().to_string();
    if message.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "指令不能为空".to_string()));
    }
    Ok(agent_sse(st.app, user, message, req.history, req.model))
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
    let user = require_user(&st, &headers).await?;
    let mut m = st.app.store.metrics().await.map_err(err500)?;
    // 人力成本与预算只对财务/经理/管理员开放；其他角色保留人数、待办等公共指标
    if !matches!(user.role.as_str(), "admin" | "finance" | "manager") {
        if let Value::Object(map) = &mut m {
            for key in ["payroll_cents", "budget_total_cents", "budget_used_cents"] {
                map.insert(key.into(), json!(0));
            }
            map.insert("trend".into(), json!([]));
            map.insert("trend_months".into(), json!([]));
        }
    }
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
    let user = require_user(&st, &headers).await?;
    // 审计日志含工具名/操作者/入参/结果，与 tables(audit_events) 同口径，仅管理员可读
    require_admin(&user)?;
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

#[derive(Deserialize)]
struct AttendanceSiteReq {
    #[serde(default)]
    name: String,
    latitude: f64,
    longitude: f64,
    #[serde(default)]
    radius_m: Option<i64>,
    #[serde(default)]
    address: String,
}

#[derive(Deserialize)]
struct AttendanceSettingsReq {
    #[serde(default)]
    sites: Vec<AttendanceSiteReq>,
}

/// 打卡点数量上限：避免一次提交过大的配置。
const MAX_ATTENDANCE_SITES: usize = 50;

/// 打卡点配置：登录用户可读（前端展示范围说明），仅管理员可改。
async fn get_attendance_settings(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let _ = require_user(&st, &headers).await?;
    let sites = st.app.store.attendance_sites().await.map_err(err500)?;
    Ok(Json(json!({
        "sites": sites,
        "default_radius_m": domain::DEFAULT_ATTENDANCE_RADIUS_M,
    })))
}

/// 整体覆盖打卡点列表；每个点独立校验半径，传空数组表示不做位置限制。
async fn save_attendance_settings(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<AttendanceSettingsReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;

    if req.sites.len() > MAX_ATTENDANCE_SITES {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("打卡点最多 {MAX_ATTENDANCE_SITES} 个"),
        ));
    }

    let mut sites = Vec::with_capacity(req.sites.len());
    for (index, item) in req.sites.into_iter().enumerate() {
        let no = index + 1;
        if !(-90.0..=90.0).contains(&item.latitude) || !(-180.0..=180.0).contains(&item.longitude)
        {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("第 {no} 个打卡点经纬度超出合法范围"),
            ));
        }
        let radius_m = item.radius_m.unwrap_or(domain::DEFAULT_ATTENDANCE_RADIUS_M);
        if !(50..=100_000).contains(&radius_m) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("第 {no} 个打卡点半径需在 50 ~ 100000 米之间"),
            ));
        }
        let name = item.name.trim().to_string();
        if name.chars().count() > 50 {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("第 {no} 个打卡点名称过长（最多 50 字）"),
            ));
        }
        let address = item.address.trim().to_string();
        if address.chars().count() > 200 {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("第 {no} 个打卡点地址过长（最多 200 字）"),
            ));
        }
        sites.push(domain::AttendanceSite {
            name,
            latitude: item.latitude,
            longitude: item.longitude,
            radius_m,
            address,
        });
    }

    st.app
        .store
        .set_attendance_sites(&sites)
        .await
        .map_err(err500)?;
    Ok(Json(json!({ "ok": true, "sites": sites })))
}

#[derive(Deserialize)]
struct AttendancePolicyReq {
    work_start: String,
    work_end: String,
    lunch_minutes: i64,
}

/// 考勤时间策略：登录用户可读（含自己是否免考勤），仅管理员可改。
async fn get_attendance_policy(
    State(st): State<WebState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let policy = st.app.store.attendance_policy().await.map_err(err500)?;
    let exempt = st
        .app
        .store
        .is_attendance_exempt(&user.employee_id)
        .await
        .map_err(err500)?;
    Ok(Json(json!({ "policy": policy, "exempt_self": exempt })))
}

async fn save_attendance_policy(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<AttendancePolicyReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;
    let Some(start) = domain::parse_time(&req.work_start) else {
        return Err((StatusCode::BAD_REQUEST, "上班时间必须是 HH:MM".into()));
    };
    let Some(end) = domain::parse_time(&req.work_end) else {
        return Err((StatusCode::BAD_REQUEST, "下班时间必须是 HH:MM".into()));
    };
    if start >= end {
        return Err((StatusCode::BAD_REQUEST, "下班时间必须晚于上班时间".into()));
    }
    if !(0..=240).contains(&req.lunch_minutes) {
        return Err((StatusCode::BAD_REQUEST, "午休时长需在 0 ~ 240 分钟之间".into()));
    }
    let policy = domain::AttendancePolicy {
        work_start: req.work_start,
        work_end: req.work_end,
        lunch_minutes: req.lunch_minutes,
    };
    st.app
        .store
        .set_attendance_policy(&policy)
        .await
        .map_err(err500)?;
    Ok(Json(json!({ "ok": true, "policy": policy })))
}

#[derive(Deserialize)]
struct MakeupQuotaReq {
    quota: i64,
    #[serde(default)]
    employee_id: Option<String>,
}

/// 补卡配额：登录用户可读（默认值 + 自己的已用/剩余），管理员可看全部覆盖。
async fn get_makeup_quota(
    State(st): State<WebState>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    let default_quota = st.app.store.makeup_quota_default().await.map_err(err500)?;
    let overrides = st
        .app
        .store
        .query_all("SELECT employee_id, makeup_quota FROM attendance_quotas ORDER BY employee_id")
        .await
        .map_err(err500)?;
    let overrides: Vec<Value> = if user.is_admin() || matches!(user.role.as_str(), "hr" | "manager") {
        overrides
    } else {
        overrides
            .into_iter()
            .filter(|r| r["employee_id"].as_str() == Some(user.employee_id.as_str()))
            .collect()
    };
    let target = q
        .get("employee_id")
        .cloned()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| user.employee_id.clone());
    let quota = st.app.store.makeup_quota_of(&target).await.map_err(err500)?;
    let used = st.app.store.makeup_used_this_month(&target).await.map_err(err500)?;
    Ok(Json(json!({
        "default_quota": default_quota,
        "overrides": overrides,
        "employee_id": target,
        "quota": quota,
        "used": used,
        "remaining": (quota - used).max(0),
    })))
}

async fn save_makeup_quota(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<MakeupQuotaReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;
    if req.quota < 0 {
        return Err((StatusCode::BAD_REQUEST, "补卡次数不能为负数".into()));
    }
    match req.employee_id.filter(|s| !s.trim().is_empty()) {
        Some(emp) => {
            st.app
                .store
                .set_makeup_quota_of(&emp, req.quota)
                .await
                .map_err(err500)?;
            Ok(Json(json!({
                "ok": true, "scope": "employee", "employee_id": emp, "quota": req.quota
            })))
        }
        None => {
            st.app
                .store
                .set_makeup_quota_default(req.quota)
                .await
                .map_err(err500)?;
            Ok(Json(json!({ "ok": true, "scope": "global", "quota": req.quota })))
        }
    }
}

/// 测试连接：默认用表单当前值（留空回落到已存配置），
/// 这样「改了 Base URL 还没保存就点测试」测到的是新地址。
#[derive(Deserialize, Default)]
struct LlmProbeReq {
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    api_key: String,
}

async fn test_llm_settings(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<LlmProbeReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;
    // `{e:#}` 展开 anyhow 因果链，管理员能看到是连接被拒还是 401
    let reply = agent::llm::probe(&st.app.store, &req.base_url, &req.model, &req.api_key)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    Ok(Json(json!({ "ok": true, "reply": reply })))
}

/// 列出端点上的可用模型（`GET {base_url}/models`），给「模型名」下拉用。
#[derive(Deserialize, Default)]
struct LlmModelsReq {
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    api_key: String,
}

async fn list_llm_models(
    State(st): State<WebState>,
    headers: HeaderMap,
    Json(req): Json<LlmModelsReq>,
) -> Result<Json<Value>, ApiError> {
    let user = require_user(&st, &headers).await?;
    require_admin(&user)?;
    let models = agent::llm::list_models(&st.app.store, &req.base_url, &req.api_key)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    Ok(Json(json!({ "models": models })))
}

async fn features() -> Json<Value> {
    let rows: Vec<Value> = domain::FEATURES
        .iter()
        .map(|(id, dom, text)| json!({ "id": id, "domain": dom, "text": text }))
        .collect();
    Json(json!({ "features": rows, "tools": domain::TOOL_NAMES }))
}

/// CORS：默认放开（鉴权用 Bearer 令牌而非 Cookie，跨站不会自动携带）。
///
/// 设置 `OA_CORS_ORIGINS`（逗号分隔）后收敛为白名单，例如
/// `OA_CORS_ORIGINS=https://oa.example.com,tauri://localhost`。
fn cors_layer() -> CorsLayer {
    let list = std::env::var("OA_CORS_ORIGINS")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let Some(list) = list else {
        return CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);
    };
    let origins: Vec<axum::http::HeaderValue> = list
        .split(',')
        .filter_map(|o| o.trim().parse().ok())
        .collect();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods(Any)
        .allow_headers(Any)
}

/// 读环境变量，未设置时用默认值；空字符串视为「已设置」，便于空密码等场景。
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 服务端唯一数据源是 Postgres。
    // 优先 `OA_DB`（连接串，兼容存量部署）；未设置时用分解参数，
    // 密码含 @ : / ? # 等字符也无需 URL 编码（连接串会把这些字符当语法）。
    let store = match std::env::var("OA_DB").ok().filter(|v| !v.trim().is_empty()) {
        Some(url) => Store::open_uri(&url).await?,
        None => {
            let port: u16 = env_or("OA_DB_PORT", "5432")
                .parse()
                .context("OA_DB_PORT 必须是 1-65535 的端口号")?;
            Store::open_parts(
                &env_or("OA_DB_HOST", "127.0.0.1"),
                port,
                &env_or("OA_DB_NAME", "oa"),
                &env_or("OA_DB_USER", "oa"),
                &env_or("OA_DB_PASSWORD", "oa"),
            )
            .await?
        }
    };
    // 附件根目录：默认 oa-files/；容器部署时挂卷到 OA_FILES 即可
    let files_dir = files::default_root();
    std::fs::create_dir_all(&files_dir)?;
    let state = WebState {
        app: AppState::new(store),
        files: files_dir,
        login_fails: Arc::new(Mutex::new(HashMap::new())),
    };

    let cors = cors_layer();

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/me", get(me))
        .route("/api/auth/change-password", post(change_password))
        .route("/api/users", get(users))
        .route("/api/users/{employee_id}/active", post(set_user_active))
        .route("/api/agent/chat", post(chat))
        .route("/api/agent/stream", get(agent_stream).post(agent_stream_post))
        .route("/api/agent/models", get(agent_models))
        .route("/api/agent/model", post(set_agent_model))
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
        .route(
            "/api/settings/attendance",
            get(get_attendance_settings).post(save_attendance_settings),
        )
        .route(
            "/api/settings/attendance-policy",
            get(get_attendance_policy).post(save_attendance_policy),
        )
        .route(
            "/api/settings/makeup-quota",
            get(get_makeup_quota).post(save_makeup_quota),
        )
        .route("/api/settings/llm/test", post(test_llm_settings))
        .route("/api/settings/llm/models", post(list_llm_models))
        .route("/api/features", get(features))
        // base64 会把体积放大 4/3，10 MB 附件需要 ~14 MB body
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .layer(cors)
        .with_state(state);

    // 静态前端：容器里把 Vite 的 dist 拷到 OA_WEB_ROOT；未匹配的非 API 路径回 index.html，
    // 支持前端路由刷新。本机开发未设该变量时不挂载，行为与原来完全一致。
    let app = if let Some(root) = std::env::var("OA_WEB_ROOT").ok().filter(|v| !v.trim().is_empty()) {
        let index = std::path::Path::new(&root).join("index.html");
        if !index.exists() {
            return Err(anyhow::anyhow!("OA_WEB_ROOT={root} 下找不到 index.html"));
        }
        let spa = ServeDir::new(&root).fallback(ServeFile::new(index));
        app.fallback_service(spa)
    } else {
        app
    };

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
