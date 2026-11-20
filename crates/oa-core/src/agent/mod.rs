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

pub use intent::{AgentReply, PlanStep, ThinkStep, TraceEntry};

use crate::auth::SessionUser;
use crate::AppState;
use anyhow::{anyhow, Result};
use chrono::{Datelike, Local};
use futures::StreamExt;
use rig::agent::MultiTurnStreamItem;
use rig::completion::Message;
use rig::streaming::{Item, StreamEvent};
use rig::tool::ToolContext;
use rig::AgentBuilder;
use serde::{Deserialize, Serialize};
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

/// 「今天是几号、周几」提示。
///
/// 模型自身没有系统时钟，不注入基准日期时遇到「下周一」只能回头问用户，
/// 而用户已经在上一轮说过「下周入职」——把日期给足，模型直接换算成 YYYY-MM-DD。
fn today_hint() -> String {
    const WEEKDAYS: [&str; 7] =
        ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
    let now = Local::now();
    let weekday = WEEKDAYS[now.weekday().num_days_from_monday() as usize];
    format!(
        "今天是 {}（{}）。用户说「下周」「下周一」「月底」这类相对日期时，\
         按这个基准换算成具体日期（YYYY-MM-DD）再调用工具，不要反问用户。",
        now.format("%Y-%m-%d"),
        weekday
    )
}

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

/// SSE 一帧：思考步骤 / 最终回复 / 错误，三者互斥
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Think { stage: String, text: String },
    /// 正文增量：模型每吐一小段就推一帧，前端追加到气泡里
    Delta { text: String },
    Reply { reply: AgentReply },
    Error { message: String },
}

/// 思考收集器：一边把步骤实时推给流式订阅者，一边留一份放进最终回复。
pub struct Thinker {
    sink: Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>,
    steps: Vec<ThinkStep>,
}

impl Thinker {
    /// 非流式调用：只收集，不推送
    pub fn silent() -> Self {
        Self { sink: None, steps: Vec::new() }
    }

    pub fn streaming(tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>) -> Self {
        Self { sink: Some(tx), steps: Vec::new() }
    }

    pub fn say(&mut self, stage: &str, text: impl Into<String>) {
        let step = ThinkStep { stage: stage.to_string(), text: text.into() };
        if let Some(tx) = &self.sink {
            // 订阅端断开（用户关页面）不该让执行中断，忽略发送失败
            let _ = tx.send(AgentEvent::Think {
                stage: step.stage.clone(),
                text: step.text.clone(),
            });
        }
        self.steps.push(step);
    }

    /// 正文增量：只推给流式订阅者，不进入思考列表（最终正文由 Reply 帧给出）
    pub fn delta(&self, text: impl Into<String>) {
        if let Some(tx) = &self.sink {
            let _ = tx.send(AgentEvent::Delta { text: text.into() });
        }
    }

    pub fn snapshot(&self) -> Vec<ThinkStep> {
        self.steps.clone()
    }

    pub fn steps(self) -> Vec<ThinkStep> {
        self.steps
    }
}

/// 一轮历史对话。
///
/// 只回传角色与纯文本：工具调用的原始 JSON 不回灌给模型，既避免上下文膨胀，
/// 也避免上一轮的工具结果被模型当成新一轮的事实重复使用。
#[derive(Debug, Clone, Deserialize)]
pub struct ChatTurn {
    /// `user` 或 `agent`（其余值按 assistant 处理）
    pub role: String,
    pub text: String,
}

/// 最多带回的历史轮数：再多对追问帮助有限，只会推高 token 与首字延迟。
const MAX_HISTORY_TURNS: usize = 20;
/// 单轮历史正文上限（按字符截断），长报告不需要全文进上下文。
const MAX_HISTORY_CHARS: usize = 2000;

/// 单次对话入口（非流式）。服务端经 Axum 路由调用。
pub async fn run(state: AppState, user: &SessionUser, message: &str) -> Result<AgentReply> {
    run_with_history(state, user, message, &[], "", Thinker::silent()).await
}

/// 流式入口（不带历史），保留给只取单轮结果的调用方。
pub async fn run_stream(
    state: AppState,
    user: &SessionUser,
    message: &str,
    thinker: Thinker,
) -> Result<AgentReply> {
    run_with_history(state, user, message, &[], "", thinker).await
}

/// 带历史的流式入口：`history` 是本轮之前已经发生的对话，按时间正序。
///
/// 没有它，每一轮都是全新提示词，模型会反复追问用户已经回答过的信息——
/// 这正是「补充完信息后又要求提供姓名/职位/月薪」的根因。
pub async fn run_stream_with_history(
    state: AppState,
    user: &SessionUser,
    message: &str,
    history: &[ChatTurn],
    model: &str,
    thinker: Thinker,
) -> Result<AgentReply> {
    run_with_history(state, user, message, history, model, thinker).await
}

async fn run_with_history(
    state: AppState,
    user: &SessionUser,
    message: &str,
    history: &[ChatTurn],
    model: &str,
    mut thinker: Thinker,
) -> Result<AgentReply> {
    let effective = llm::effective(&state.store).await?;
    if desired_mode(effective.config.is_some()) == "local" {
        thinker.say("route", "没有可用模型配置，走本地意图引擎");
        return intent::run_local_stream(state, user, message, thinker).await;
    }
    // 对话页可以临时换模型：Base URL / Key 仍用已存配置，只覆盖 model 名
    let label = effective
        .config
        .as_ref()
        .map(|c| {
            let model = if model.trim().is_empty() { &c.model } else { model.trim() };
            format!("{}/{}", c.provider, model)
        })
        .unwrap_or_else(|| "llm".to_string());
    thinker.say("route", format!("使用模型 {label} 推理并调用工具"));
    if !history.is_empty() {
        thinker.say("llm", format!("携带最近 {} 轮对话作为上下文", history.len()));
    }

    let before = state.store.last_audit_id().await.unwrap_or(0);
    match run_llm(
        state.clone(),
        user,
        message,
        history,
        model,
        effective.config,
        &mut thinker,
    )
    .await
    {
        Ok(mut reply) => {
            reply.thinking = thinker.snapshot();
            Ok(reply)
        }
        Err(error) => {
            // 关键：若模型在失败前已经执行过工具，绝不回退重跑，避免重复写入。
            let rows = state.store.audit_since(before).await.unwrap_or_default();
            if rows.is_empty() {
                thinker.say("route", "模型调用失败，回退本地意图引擎执行");
                let mut local = intent::run_local_stream(state, user, message, thinker).await?;
                local.mode = "llm-fallback".into();
                // `{error:#}` 打印完整因果链；只用 `{error}` 会丢掉网络/HTTP 底层原因
                local.reply.push_str(&format!(
                    "\n\n[模型调用不可用，已回退本地意图引擎执行：{error:#}]"
                ));
                Ok(local)
            } else {
                let trace = trace_from_audit(&rows);
                thinker.say("done", format!("模型在 {} 步工具调用后失败", trace.len()));
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
                    thinking: thinker.snapshot(),
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
    history: &[ChatTurn],
    model: &str,
    config: Option<llm::LlmConfig>,
    thinker: &mut Thinker,
) -> Result<AgentReply> {
    let stored = config.ok_or_else(|| {
        anyhow!(
            "缺少 LLM 配置：请在「模型设置」里选择提供商并填写 API Key，\
             或设置 OPENAI_API_KEY 环境变量（也可用 OA_AGENT_MODE=local 走离线引擎）"
        )
    })?;
    // 只覆盖模型名：请求里没指定（空串）就用已存配置的那个
    let cfg = if model.trim().is_empty() {
        stored
    } else {
        llm::LlmConfig { model: model.trim().to_string(), ..stored }
    };
    let client = llm::build_client(&cfg)?;

    let before = state.store.last_audit_id().await?;

    let agent = AgentBuilder::new(client.completion(cfg.model.clone()))
        .preamble(format!("{PREAMBLE}\n\n{}", today_hint()))
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
        .tool(tools::ContractApprove { state: state.clone(), user: user.clone() })
        .tool(tools::ContractQuery { state: state.clone(), user: user.clone() })
        .tool(tools::AttendanceCheckin { state: state.clone(), user: user.clone() })
        .tool(tools::AttendanceMakeup { state: state.clone(), user: user.clone() })
        .tool(tools::AttendanceApprove { state: state.clone(), user: user.clone() })
        .tool(tools::AttendanceReport { state: state.clone(), user: user.clone() })
        .tool(tools::AssetRegister { state: state.clone(), user: user.clone() })
        .tool(tools::AssetAssign { state: state.clone(), user: user.clone() })
        .tool(tools::AssetReturn { state: state.clone(), user: user.clone() })
        .tool(tools::AssetQuery { state: state.clone(), user: user.clone() })
        .build();

    // 模型无法伪造身份：工具实例已绑定宿主注入的 SessionUser。
    thinker.say(
        "llm",
        format!("已提交给 {}，单轮最多 10 次工具调用", cfg.model),
    );

    // 历史按时间正序转成 rig 消息；超长只留最近 N 轮，空正文直接丢弃（占位气泡、被取消的回复）
    let recent = &history[history.len().saturating_sub(MAX_HISTORY_TURNS)..];
    let history_msgs: Vec<Message> = recent
        .iter()
        .filter_map(|turn| {
            let text = turn.text.trim();
            if text.is_empty() {
                return None;
            }
            let text: String = text.chars().take(MAX_HISTORY_CHARS).collect();
            Some(if turn.role == "user" {
                Message::user(text)
            } else {
                Message::assistant(text)
            })
        })
        .collect();

    // 走流式：文本增量边生成边推给前端，工具调用轮次由 rig 的 drive_agent 内部驱动，
    // 与阻塞版共用同一套工具执行和审计逻辑，只是多了 delta 事件。
    let runner = agent
        .prompt(message)
        .tool_context(ToolContext::new())
        .max_turns(10);
    let mut stream = if history_msgs.is_empty() {
        runner.stream()
    } else {
        runner.history(history_msgs).stream()
    };

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut final_text: Option<String> = None;

    while let Some(item) = stream.next().await {
        match item.map_err(|e| anyhow!("模型流中断：{e}"))? {
            MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Text {
                text: delta, ..
            })) => {
                if !delta.is_empty() {
                    text.push_str(&delta);
                    thinker.delta(delta);
                }
            }
            // 推理模型的内部思考：攒起来最后作为一条思考步骤展示，避免逐 token 刷屏
            MultiTurnStreamItem::StreamAssistantItem(Item::Event(StreamEvent::Reasoning {
                text: chunk,
                ..
            })) => reasoning.push_str(&chunk),
            MultiTurnStreamItem::ToolCall { tool_call } => {
                thinker.say("tool", format!("模型请求调用 `{}`", tool_call.function.name));
            }
            MultiTurnStreamItem::ToolResult { tool_result } => {
                thinker.say("tool", format!("`{}` 已返回结果", tool_result.name));
            }
            MultiTurnStreamItem::FinalResponse(response) => final_text = Some(response.output()),
            _ => {}
        }
    }

    if !reasoning.trim().is_empty() {
        thinker.say(
            "llm",
            format!("模型内部推理：{}", intent::brief(reasoning.trim(), 200)),
        );
    }

    let reply_text = final_text.filter(|t| !t.trim().is_empty()).unwrap_or(text);
    if reply_text.trim().is_empty() {
        return Err(anyhow!("模型没有返回任何内容（流已结束但没有文本，也没有 FinalResponse）"));
    }

    let rows = state.store.audit_since(before).await?;
    let trace = trace_from_audit(&rows);
    thinker.say("done", format!("模型完成推理，共 {} 次工具调用", trace.len()));
    Ok(AgentReply {
        reply: reply_text,
        mode: "llm".into(),
        affected: affected_of(&trace),
        trace,
        plan: vec![],
        thinking: thinker.snapshot(),
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
