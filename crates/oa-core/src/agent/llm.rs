//! LLM 提供商配置（管理员可改，落 `settings` 表）
//!
//! DeepSeek / Qwen(DashScope) / vLLM / Ollama / OpenAI 都提供 OpenAI 兼容接口，
//! 因此只需要「base_url + model + api_key」三要素，共用同一条 rig 客户端路径。
//!
//! 优先级：`settings` 表（管理员在界面上配置）> 环境变量
//! （`OPENAI_API_KEY` / `OPENAI_BASE_URL` / `OA_MODEL`）> 无配置（本地意图引擎）。

use crate::store::Store;
use anyhow::{anyhow, Context, Result};
use rig::providers::openai::{OpenAI, OpenAIConfig, Route};
use rig::tool::ToolContext;
use rig::AgentBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 配置在 settings 表里的键名（值为一段 JSON）
pub const SETTING_KEY: &str = "llm.config";

/// 落库的配置。`api_key` 只在这里出现，对外一律走 [`mask_key`]。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: String,
}

/// 前端下拉用的提供商预设
pub struct ProviderPreset {
    pub code: &'static str,
    pub label: &'static str,
    pub base_url: &'static str,
    pub model: &'static str,
    /// 云端服务必须填 Key；本地 vLLM / Ollama 不校验
    pub needs_key: bool,
}

pub const PROVIDERS: &[ProviderPreset] = &[
    ProviderPreset {
        code: "deepseek",
        label: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        model: "deepseek-chat",
        needs_key: true,
    },
    ProviderPreset {
        code: "qwen",
        label: "Qwen · DashScope",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        model: "qwen-plus",
        needs_key: true,
    },
    ProviderPreset {
        code: "vllm",
        label: "vLLM · 自建",
        base_url: "http://127.0.0.1:8000/v1",
        model: "Qwen2.5-7B-Instruct",
        needs_key: false,
    },
    ProviderPreset {
        code: "ollama",
        label: "Ollama · 本地",
        base_url: "http://127.0.0.1:11434/v1",
        model: "qwen2.5:7b",
        needs_key: false,
    },
    ProviderPreset {
        code: "openai",
        label: "OpenAI",
        base_url: "https://api.openai.com/v1",
        model: "gpt-4o-mini",
        needs_key: true,
    },
];

pub fn preset(code: &str) -> Option<&'static ProviderPreset> {
    let want = code.trim().to_ascii_lowercase();
    PROVIDERS.iter().find(|p| p.code == want)
}

/// 当前配置的来源，前端据此提示「来自界面 / 来自环境变量 / 未配置」
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Db,
    Env,
    None,
}

impl Source {
    pub fn code(self) -> &'static str {
        match self {
            Source::Db => "db",
            Source::Env => "env",
            Source::None => "none",
        }
    }
}

/// 解析结果：`config` 为 None 表示没有可用模型，只能走本地意图引擎
pub struct Effective {
    pub config: Option<LlmConfig>,
    pub source: Source,
}

/// 掩码展示：`sk-abcdefgh1234` → `sk-ab…1234`；空串原样返回
pub fn mask_key(key: &str) -> String {
    let chars: Vec<char> = key.trim().chars().collect();
    if chars.is_empty() {
        return String::new();
    }
    if chars.len() <= 10 {
        return "…".to_string();
    }
    let head: String = chars[..5].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// 读取落库配置（不含环境变量回退）
async fn stored_config(store: &Store) -> Result<Option<LlmConfig>> {
    let Some(raw) = store.setting(SETTING_KEY).await? else {
        return Ok(None);
    };
    let Ok(cfg) = serde_json::from_str::<LlmConfig>(&raw) else {
        return Ok(None);
    };
    if cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(cfg))
}

/// 生效配置：数据库优先，其次环境变量
pub async fn effective(store: &Store) -> Result<Effective> {
    if let Some(config) = stored_config(store).await? {
        return Ok(Effective { config: Some(config), source: Source::Db });
    }

    let key = std::env::var("OPENAI_API_KEY").unwrap_or_default();
    if key.trim().is_empty() {
        return Ok(Effective { config: None, source: Source::None });
    }
    let fallback = preset("openai").expect("openai preset");
    let base_url = std::env::var("OPENAI_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| fallback.base_url.to_string());
    let model = std::env::var("OA_MODEL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| fallback.model.to_string());
    Ok(Effective {
        config: Some(LlmConfig {
            provider: fallback.code.to_string(),
            base_url,
            model,
            api_key: key,
        }),
        source: Source::Env,
    })
}

/// 对外视图：**永不返回明文 Key**
pub async fn describe(store: &Store) -> Result<Value> {
    let eff = effective(store).await?;
    let providers: Vec<Value> = PROVIDERS
        .iter()
        .map(|p| {
            json!({
                "code": p.code, "label": p.label,
                "base_url": p.base_url, "model": p.model, "needs_key": p.needs_key,
            })
        })
        .collect();

    let (provider, base_url, model, has_key, key_hint) = match &eff.config {
        Some(c) => (
            c.provider.clone(),
            c.base_url.clone(),
            c.model.clone(),
            !c.api_key.trim().is_empty(),
            mask_key(&c.api_key),
        ),
        None => (String::new(), String::new(), String::new(), false, String::new()),
    };

    Ok(json!({
        "provider": provider,
        "base_url": base_url,
        "model": model,
        "has_key": has_key,
        "key_hint": key_hint,
        "source": eff.source.code(),
        "mode": if eff.config.is_some() { "llm" } else { "local" },
        "providers": providers,
    }))
}

/// 保存配置。`api_key` 传空字符串表示沿用已存的 Key（界面只拿到掩码，不能回传明文）。
pub async fn save(
    store: &Store,
    provider: &str,
    base_url: &str,
    model: &str,
    api_key: &str,
) -> Result<Value> {
    let code = provider.trim().to_ascii_lowercase();
    let base = base_url.trim();
    let mdl = model.trim();

    if base.is_empty() || mdl.is_empty() {
        return Err(anyhow!("Base URL 与模型名不能为空"));
    }
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(anyhow!("Base URL 必须以 http:// 或 https:// 开头"));
    }
    let needs_key = preset(&code).map(|p| p.needs_key).unwrap_or(true);
    let previous = stored_config(store).await?.map(|c| c.api_key).unwrap_or_default();
    let key = if api_key.trim().is_empty() { previous } else { api_key.trim().to_string() };
    if key.is_empty() && needs_key {
        return Err(anyhow!("该提供商需要 API Key"));
    }

    let cfg = LlmConfig {
        provider: if code.is_empty() { "openai".to_string() } else { code },
        base_url: base.to_string(),
        model: mdl.to_string(),
        api_key: key,
    };
    let raw = serde_json::to_string(&cfg).context("序列化 LLM 配置失败")?;
    store.set_setting(SETTING_KEY, &raw).await?;
    describe(store).await
}

/// 清除落库配置，回到环境变量 / 本地引擎
pub async fn clear(store: &Store) -> Result<Value> {
    store.remove_setting(SETTING_KEY).await?;
    describe(store).await
}

/// 用 OpenAI 兼容协议构造客户端；本地服务不需要 Key，但 rig 要求非空占位
///
/// 必须显式指定 `Route::Chat`：rig 的 OpenAI dialect 默认走 Responses API
/// （`POST /v1/responses`），而 DeepSeek / Qwen / Ollama / vLLM 只实现了
/// `/v1/chat/completions`（见 rig-core `providers/openai/wire.rs` 的
/// `Quirks::completion_route` 注释）。不改这一行，换任何兼容端点都会连接失败。
pub fn build_client(cfg: &LlmConfig) -> Result<OpenAI> {
    let key = if cfg.api_key.trim().is_empty() {
        "EMPTY".to_string()
    } else {
        cfg.api_key.trim().to_string()
    };
    Ok(OpenAIConfig::new(key)
        .with_base_url(cfg.base_url.trim())
        .with_route(Route::Chat)
        .client())
}

/// 截断超长文本：错误信息只留一屏看得完的部分
fn brief(text: &str, max: usize) -> String {
    let t = text.trim();
    let mut out: String = t.chars().take(max).collect();
    if t.chars().count() > max {
        out.push('…');
    }
    out
}

/// 列出端点上的可用模型（OpenAI 兼容 `GET {base_url}/models`）。
///
/// 管理员往往只填了 Base URL 就想知道该选哪个模型名，所以 `api_key` 留空时
/// 沿用已存 Key——这样改 Base URL 不必先把 Key 重新输一遍。
pub async fn list_models(store: &Store, base_url: &str, api_key: &str) -> Result<Vec<String>> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(anyhow!("请先填写 Base URL"));
    }
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(anyhow!("Base URL 必须以 http:// 或 https:// 开头"));
    }
    let key = if api_key.trim().is_empty() {
        stored_config(store).await?.map(|c| c.api_key).unwrap_or_default()
    } else {
        api_key.trim().to_string()
    };

    let url = format!("{base}/models");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("创建 HTTP 客户端失败")?;
    let mut req = client.get(&url);
    if !key.is_empty() {
        req = req.bearer_auth(&key);
    }
    let res = req.send().await.with_context(|| format!("请求 {url} 失败"))?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("{url} 返回 {status}：{}", brief(&body, 200)));
    }
    let payload: Value = serde_json::from_str(&body)
        .with_context(|| format!("{url} 返回的不是合法 JSON：{}", brief(&body, 120)))?;

    // 标准 OpenAI 把列表放在 data[].id；少数兼容实现直接给顶层数组或 models[]
    let rows = payload
        .get("data")
        .or_else(|| payload.get("models"))
        .and_then(Value::as_array)
        .or_else(|| payload.as_array())
        .cloned()
        .unwrap_or_default();
    let mut models: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            r.get("id")
                .or_else(|| r.get("name"))
                .or_else(|| r.get("model"))
                .and_then(Value::as_str)
                .or_else(|| r.as_str())
                .map(str::to_string)
        })
        .filter(|m| !m.trim().is_empty())
        .collect();
    models.sort();
    models.dedup();
    Ok(models)
}

/// 连通性自检：用「表单当前值」发一次最小对话，失败信息直接回给管理员。
///
/// 入参留空时回落到已存配置，因此「只改了 Base URL 还没保存就点测试」也能测到新地址。
pub async fn probe(
    store: &Store,
    base_url: &str,
    model: &str,
    api_key: &str,
) -> Result<String> {
    let stored = effective(store).await?.config;
    let pick = |given: &str, saved: Option<&str>| {
        if given.trim().is_empty() {
            saved.unwrap_or("").trim().to_string()
        } else {
            given.trim().to_string()
        }
    };
    let cfg = LlmConfig {
        provider: stored
            .as_ref()
            .map(|c| c.provider.clone())
            .unwrap_or_else(|| "custom".to_string()),
        base_url: pick(base_url, stored.as_ref().map(|c| c.base_url.as_str())),
        model: pick(model, stored.as_ref().map(|c| c.model.as_str())),
        api_key: pick(api_key, stored.as_ref().map(|c| c.api_key.as_str())),
    };
    if cfg.base_url.is_empty() || cfg.model.is_empty() {
        return Err(anyhow!("尚未配置模型：请选择提供商并填写 Base URL、模型名与 API Key"));
    }

    let client = build_client(&cfg)?;
    let agent = AgentBuilder::new(client.completion(cfg.model.clone()))
        .preamble("你是连通性自检助手，只做最短回复。")
        .default_max_turns(1)
        .build();
    let text = agent
        .prompt("请只回复两个字符：ok")
        .tool_context(ToolContext::new())
        .max_turns(1)
        .await
        .with_context(|| format!("模型调用失败（{} {}）", cfg.provider, cfg.model))?
        .output();
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    #[test]
    fn presets_cover_the_four_required_providers() {
        for code in ["deepseek", "qwen", "vllm", "ollama"] {
            let p = preset(code).unwrap_or_else(|| panic!("missing preset {code}"));
            assert!(p.base_url.starts_with("http"));
            assert!(!p.model.is_empty());
        }
        assert!(!preset("vllm").unwrap().needs_key);
        assert!(preset("deepseek").unwrap().needs_key);
        assert!(preset("DEEPSEEK").is_some(), "provider code 应大小写不敏感");
    }

    #[test]
    fn mask_key_never_leaks_the_middle() {
        assert_eq!(mask_key(""), "");
        assert_eq!(mask_key("short"), "…");
        let masked = mask_key("sk-1234567890abcdef");
        assert!(masked.starts_with("sk-12"));
        assert!(masked.ends_with("cdef"));
        assert!(!masked.contains("67890"));
    }

    #[tokio::test]
    async fn settings_roundtrip_and_precedence() {
        let db = Store::open_in_memory().await.unwrap();
        assert!(db.setting(SETTING_KEY).await.unwrap().is_none());

        // 落库后 describe 只回掩码，且来源是 db
        save(&db, "ollama", "http://127.0.0.1:11434/v1", "qwen2.5:7b", "sk-test-1234567890")
            .await
            .unwrap();
        let view = describe(&db).await.unwrap();
        assert_eq!(view["source"], "db");
        assert_eq!(view["mode"], "llm");
        assert_eq!(view["has_key"], true);
        assert!(!view["key_hint"].as_str().unwrap().contains("1234567890"));

        // 不改 Key 也能保存（空字符串沿用旧值）
        save(&db, "vllm", "http://127.0.0.1:8000/v1", "Qwen2.5-7B-Instruct", "").await.unwrap();
        let cfg = effective(&db).await.unwrap().config.unwrap();
        assert_eq!(cfg.provider, "vllm");
        assert_eq!(cfg.api_key, "sk-test-1234567890");

        // 清空后回到 env / none
        clear(&db).await.unwrap();
        assert!(db.setting(SETTING_KEY).await.unwrap().is_none());
    }
}
