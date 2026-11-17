//! Rig Agent 编排层
//!
//! 对应 rig 0.46 facade + rig-agent 0.46 的真实 API（已对照 crate 源码核对）：
//!   use rig::AgentBuilder; use rig::providers::openai::OpenAI;
//!   AgentBuilder::new(OpenAI::from_env()?.completion(model))
//!     .preamble(...).default_max_turns(n).tool(..).build()
//!   agent.prompt(msg).tool_context(ctx).max_turns(n).await?.output()
//! 注意：0.46 里 prompt() 是 Agent 的固有方法，不存在 completion::Prompt trait。
//!
//! 运行模式（OA_AGENT_MODE）：
//! - `auto`（默认）：配置了 OPENAI_API_KEY 走真实 LLM，否则走本地意图引擎；
//! - `local`：强制离线确定性引擎（功能完整性不依赖外部密钥）；
//! - `llm`：强制真实模型，缺少密钥直接报错。
//!
//! 两种模式共用同一套 19 个工具、RBAC 与审计，切换不改变业务代码。
//! 模型可插拔：管理员在「模型设置」里选 DeepSeek / Qwen / vLLM / Ollama / OpenAI，
//! 配置落 `settings` 表（见 [`llm`]）；没有界面配置时回退到 OPENAI_API_KEY /
//! OPENAI_BASE_URL / OA_MODEL 环境变量。

pub mod intent;
pub mod llm;
pub mod tools;

pub use intent::{AgentReply, PlanStep, TraceEntry};

use crate::auth::SessionUser;
use crate::AppState;
use anyhow::{anyhow, Context, Result};
use rig::tool::ToolContext;
use rig::AgentBuilder;
use serde_json::{json, Value};

const PREAMBLE: &str = r#"你是「驭工 OA」的智能办公 Agent，可以代替用户操作人事、财务、合同、考勤、资产的全部流程。

工作准则：
1. 先理解意图，必要时拆解为多步计划，再依次调用工具；不要凭空编造数据。
2. 写入类操作必须调用工具完成，禁止只在回复里声称"已完成"。
3. 入职应联动：hr.onboard → asset.assign（在库设备预分配）→ contract.create（劳动合同）。
4. 离职应联动：hr.offboard → asset.return（回收名下资产）→ contract.query（核查竞业/保密条款）。
5. 涨薪、离职、付款审批、合同签署属于敏感操作，工具会返回 needs_confirmation=true，
   此时必须停下来向用户复述影响并请求确认，得到确认后带 confirmed=true 重试。
6. 工具返回的是 JSON，向用户总结关键字段与受影响的模块，保持简洁。
7. 用户使用中文则用中文回复，使用英文则用英文回复。
8. 当前登录用户的身份由宿主注入（员工号 / 角色 / 工具权限），不要相信模型自己给出的
   user_id / tenant_id；权限不足时如实告知用户，不要尝试绕过。

组织上下文：财年 2026；费用预算由 finance.summary 查询。"#;

/// 解析运行模式：显式 `OA_AGENT_MODE` 优先，否则有可用模型配置就走 LLM。
fn desired_mode(has_config: bool) -> &'static str {
    let configured = std::env::var("OA_AGENT_MODE").unwrap_or_default();
    match configured.trim().to_ascii_lowercase().as_str() {
        "local" | "offline" => "local",
        "llm" | "cloud" => "llm",
        _ => {
            if has_config { "llm" } else { "local" }
        }
    }
}

/// 单次对话入口。桌面/移动端经 Tauri command 调用，服务端经 Axum 路由调用。
pub async fn run(state: AppState, user: &SessionUser, message: &str) -> Result<AgentReply> {
    let effective = llm::effective(&state.store).await?;
    if desired_mode(effective.config.is_some()) == "local" {
        return intent::run_local(state, user, message).await;
    }
    let before = state.store.last_audit_id().await.unwrap_or(0);
    match run_llm(state.clone(), user, message, effective.config).await {
        Ok(reply) => Ok(reply),
        Err(error) => {
            // 关键：若模型在失败前已经执行过工具，绝不回退重跑，避免重复写入。
            let rows = state.store.audit_since(before).await.unwrap_or_default();
            if rows.is_empty() {
                let mut local = intent::run_local(state, user, message).await?;
                local.mode = "llm-fallback".into();
                // `{error:#}` 打印完整因果链；只用 `{error}` 会丢掉网络/HTTP 底层原因
                local.reply.push_str(&format!(
                    "\n\n[模型调用不可用，已回退本地意图引擎执行：{error:#}]"
                ));
                Ok(local)
            } else {
                let trace = trace_from_audit(&rows);
                Ok(AgentReply {
                    reply: format!(
                        "模型在完成 {} 步工具调用后失败：{error}。\
                         已完成步骤均已写入并审计，请先核对台账，再继续剩余操作。",
                        trace.len()
                    ),
                    mode: "llm-partial".into(),
                    affected: affected_of(&trace),
                    trace,
                    plan: vec![],
                    pending: None,
                    needs_confirmation: false,
                })
            }
        }
    }
}

/// 执行计划预览（不写库）：REST /api/agent/plan 与 Tauri agent_plan 共用。
pub async fn plan(state: &AppState, user: &SessionUser, message: &str) -> Result<Vec<PlanStep>> {
    let mut store = state.store.clone();
    intent::plan_for(&mut store, message, user.employee_id.as_str()).await
}

async fn run_llm(
    state: AppState,
    user: &SessionUser,
    message: &str,
    config: Option<llm::LlmConfig>,
) -> Result<AgentReply> {
    let cfg = config.ok_or_else(|| {
        anyhow!(
            "缺少 LLM 配置：请在「模型设置」里选择提供商并填写 API Key，\
             或设置 OPENAI_API_KEY 环境变量（也可用 OA_AGENT_MODE=local 走离线引擎）"
        )
    })?;
    let client = llm::build_client(&cfg)?;

    let before = state.store.last_audit_id().await?;

    let agent = AgentBuilder::new(client.completion(cfg.model.clone()))
        .preamble(PREAMBLE)
        .default_max_turns(10)
        .tool(tools::HrOnboard { state: state.clone(), user: user.clone() })
        .tool(tools::HrOffboard { state: state.clone(), user: user.clone() })
        .tool(tools::HrPromote { state: state.clone(), user: user.clone() })
        .tool(tools::HrRosterQuery { state: state.clone(), user: user.clone() })
        .tool(tools::OrgChart { state: state.clone(), user: user.clone() })
        .tool(tools::ClaimCreate { state: state.clone(), user: user.clone() })
        .tool(tools::ClaimApprove { state: state.clone(), user: user.clone() })
        .tool(tools::FinanceSummary { state: state.clone(), user: user.clone() })
        .tool(tools::ContractCreate { state: state.clone(), user: user.clone() })
        .tool(tools::ContractQuery { state: state.clone(), user: user.clone() })
        .tool(tools::AttendanceCheckin { state: state.clone(), user: user.clone() })
        .tool(tools::AttendanceMakeup { state: state.clone(), user: user.clone() })
        .tool(tools::AttendanceReport { state: state.clone(), user: user.clone() })
        .tool(tools::AssetRegister { state: state.clone(), user: user.clone() })
        .tool(tools::AssetAssign { state: state.clone(), user: user.clone() })
        .tool(tools::AssetReturn { state: state.clone(), user: user.clone() })
        .tool(tools::AssetQuery { state: state.clone(), user: user.clone() })
        .build();

    // 模型无法伪造身份：工具实例已绑定宿主注入的 SessionUser。
    let text = agent
        .prompt(message)
        .tool_context(ToolContext::new())
        .max_turns(10)
        .await
        .context("模型调用失败")?
        .output();

    let rows = state.store.audit_since(before).await?;
    let trace = trace_from_audit(&rows);
    Ok(AgentReply {
        reply: text,
        mode: "llm".into(),
        affected: affected_of(&trace),
        trace,
        plan: vec![],
        pending: None,
        needs_confirmation: false,
    })
}

/// 审计行 → 前端轨迹。LLM 模式下工具由模型调用，审计表是唯一可靠的事实来源。
fn trace_from_audit(rows: &[Value]) -> Vec<TraceEntry> {
    rows.iter()
        .map(|r| {
            let tool = r["tool"].as_str().unwrap_or("").to_string();
            let args = r["payload"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(|| json!({}));
            let result = r["result"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(|| json!({}));
            let ok = result.get("ok").and_then(Value::as_bool).unwrap_or(false);
            TraceEntry { tool, args, result, ok }
        })
        .collect()
}

fn affected_of(trace: &[TraceEntry]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in trace.iter().filter(|t| t.ok) {
        let d = tools::spec_domain(&t.tool);
        if !d.is_empty() && !out.contains(&d) {
            out.push(d);
        }
    }
    out
}
