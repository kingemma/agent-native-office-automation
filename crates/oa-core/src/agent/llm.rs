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

/// 多模型目录的键名：管理员可配置多个模型，用户在对话页按名称选择
pub const CATALOG_KEY: &str = "llm.catalog";

/// 落库的配置。`api_key` 只在这里出现，对外一律走 [`mask_key`]。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LlmConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: String,
}

/// 目录里的一条模型。
///
/// `name` 是给用户看的名称（对话页下拉显示的也是它），`model` 是真实模型 id：
/// 分开两个字段才能做到「同一端点两个 Key」「不同端点同名模型」。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub name: String,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub api_key: String,
}

impl CatalogEntry {
    /// 条目 → 单条配置（端点与 Key 都跟着条目走，不与其他条目混用）
    pub fn config(&self) -> LlmConfig {
        LlmConfig {
            provider: self.provider.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: self.api_key.clone(),
        }
    }
}

/// 管理员维护的模型目录：`default` 指向某个条目的 `name`
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub entries: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 默认条目：`default` 指向失效时退到第一条，避免改名后对话页没有默认
    pub fn default_entry(&self) -> Option<&CatalogEntry> {
        let want = self.default.trim();
        self.entries
            .iter()
            .find(|e| e.name.trim() == want)
            .or_else(|| self.entries.first())
    }

    pub fn find(&self, name: &str) -> Option<&CatalogEntry> {
        let want = name.trim();
        self.entries.iter().find(|e| e.name.trim() == want)
    }
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

/// 每个用户记住的对话模型（settings 键前缀，避免新增表）
fn user_model_key(employee_id: &str) -> String {
    format!("agent_model:{employee_id}")
}

/// 读用户上次选的模型；没选过返回 `None`，调用方回落到全局配置
pub async fn user_model(store: &Store, employee_id: &str) -> Result<Option<String>> {
    let value = store.setting(&user_model_key(employee_id)).await?;
    Ok(value.filter(|v| !v.trim().is_empty()))
}

/// 记住用户选的模型（跨设备生效，因为存在服务端）
pub async fn set_user_model(store: &Store, employee_id: &str, model: &str) -> Result<()> {
    store.set_setting(&user_model_key(employee_id), model.trim()).await
}

/// 端点返回的清单里混着嵌入 / 重排 / 语音模型，它们不能对话，
/// 出现在「对话用哪个模型」的下拉里只会误导。按名字做一层保守过滤。
pub fn is_chat_model(name: &str) -> bool {
    const NON_CHAT: [&str; 4] = ["embed", "rerank", "bge", "whisper"];
    let lower = name.to_lowercase();
    !NON_CHAT.iter().any(|k| lower.contains(k))
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

/// 读取落库的多模型目录（不含旧配置合成）
async fn stored_catalog(store: &Store) -> Result<Option<Catalog>> {
    let Some(raw) = store.setting(CATALOG_KEY).await? else {
        return Ok(None);
    };
    let Ok(cat) = serde_json::from_str::<Catalog>(&raw) else {
        return Ok(None);
    };
    if cat.entries.is_empty() {
        return Ok(None);
    }
    Ok(Some(cat))
}

/// 生效的模型目录。
///
/// 旧版本只有单条 `llm.config`，这里合成一条（`name` 取模型名）返回，**不写库**：
/// 管理员在界面上保存一次目录后才会真正落成多条目结构。
pub async fn catalog(store: &Store) -> Result<Catalog> {
    if let Some(cat) = stored_catalog(store).await? {
        return Ok(cat);
    }
    let Some(cfg) = stored_config(store).await? else {
        return Ok(Catalog::default());
    };
    Ok(Catalog {
        default: cfg.model.clone(),
        entries: vec![CatalogEntry {
            name: cfg.model.clone(),
            provider: cfg.provider,
            base_url: cfg.base_url,
            model: cfg.model,
            api_key: cfg.api_key,
        }],
    })
}

/// 按名称取整条配置；名称为空或不存在时返回 `None`，由调用方决定回落策略
pub async fn resolve(store: &Store, name: &str) -> Result<Option<LlmConfig>> {
    if name.trim().is_empty() {
        return Ok(None);
    }
    Ok(catalog(store).await?.find(name).map(CatalogEntry::config))
}

/// 目录里的默认模型配置；目录为空时返回 `None`
pub async fn default_config(store: &Store) -> Result<Option<LlmConfig>> {
    Ok(catalog(store).await?.default_entry().map(CatalogEntry::config))
}

/// 校验并归一化待保存的目录。
///
/// 抽成纯函数是为了能脱离数据库做单测：Key 空串沿用同名旧条目的 Key
/// （界面上只有掩码，不可能回传明文）。
fn normalize_catalog(input: Catalog, previous: &Catalog) -> Result<Catalog> {
    let mut entries: Vec<CatalogEntry> = Vec::with_capacity(input.entries.len());
    for raw in input.entries {
        let name = raw.name.trim().to_string();
        if name.is_empty() {
            return Err(anyhow!("模型名称不能为空"));
        }
        if entries.iter().any(|e| e.name.eq_ignore_ascii_case(&name)) {
            return Err(anyhow!("模型名称重复：{name}"));
        }
        let base = raw.base_url.trim().to_string();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(anyhow!("「{name}」的 Base URL 必须以 http:// 或 https:// 开头"));
        }
        let model = raw.model.trim().to_string();
        if model.is_empty() {
            return Err(anyhow!("「{name}」的模型名不能为空"));
        }
        let code = {
            let c = raw.provider.trim().to_ascii_lowercase();
            if c.is_empty() { "openai".to_string() } else { c }
        };
        let needs_key = preset(&code).map(|p| p.needs_key).unwrap_or(true);
        let key = if raw.api_key.trim().is_empty() {
            previous.find(&name).map(|e| e.api_key.clone()).unwrap_or_default()
        } else {
            raw.api_key.trim().to_string()
        };
        if key.is_empty() && needs_key {
            return Err(anyhow!("「{name}」这个提供商需要 API Key"));
        }
        entries.push(CatalogEntry { name, provider: code, base_url: base, model, api_key: key });
    }

    let want = input.default.trim();
    let default = if entries.is_empty() {
        String::new()
    } else if want.is_empty() {
        entries[0].name.clone()
    } else if entries.iter().any(|e| e.name == want) {
        want.to_string()
    } else {
        return Err(anyhow!("默认模型「{want}」不在模型列表中"));
    };

    Ok(Catalog { default, entries })
}

/// 保存模型目录（仅管理员）。
///
/// 同时把默认条目写回单条 `llm.config`：`effective()` / `probe()` / `list_models()`
/// 这些按「一份配置」工作的老路径不用改，仍然指向默认模型。
pub async fn save_catalog(store: &Store, input: Catalog) -> Result<Value> {
    let previous = catalog(store).await?;
    let cat = normalize_catalog(input, &previous)?;
    if cat.is_empty() {
        store.remove_setting(CATALOG_KEY).await?;
        store.remove_setting(SETTING_KEY).await?;
        return describe(store).await;
    }
    let default = cat.default_entry().ok_or_else(|| anyhow!("模型列表不能为空"))?;
    let cfg = serde_json::to_string(&default.config()).context("序列化 LLM 配置失败")?;
    let raw = serde_json::to_string(&cat).context("序列化模型目录失败")?;
    store.set_setting(SETTING_KEY, &cfg).await?;
    store.set_setting(CATALOG_KEY, &raw).await?;
    describe(store).await
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

    let cat = catalog(store).await?;
    let default_name = cat.default_entry().map(|e| e.name.clone()).unwrap_or_default();
    let models: Vec<Value> = cat
        .entries
        .iter()
        .map(|e| {
            json!({
                "name": e.name,
                "provider": e.provider,
                "base_url": e.base_url,
                "model": e.model,
                "has_key": !e.api_key.trim().is_empty(),
                "key_hint": mask_key(&e.api_key),
                "is_default": e.name == default_name,
            })
        })
        .collect();

    Ok(json!({
        "provider": provider,
        "base_url": base_url,
        "model": model,
        "has_key": has_key,
        "key_hint": key_hint,
        "source": eff.source.code(),
        "mode": if eff.config.is_some() { "llm" } else { "local" },
        "providers": providers,
        "models": models,
        "default_model": default_name,
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
    // 单条保存 = 退回单模型模式：目录同步收敛成这一条，避免两者说法不一致
    let cat = Catalog {
        default: cfg.model.clone(),
        entries: vec![CatalogEntry {
            name: cfg.model.clone(),
            provider: cfg.provider.clone(),
            base_url: cfg.base_url.clone(),
            model: cfg.model.clone(),
            api_key: cfg.api_key.clone(),
        }],
    };
    let raw_cat = serde_json::to_string(&cat).context("序列化模型目录失败")?;
    store.set_setting(CATALOG_KEY, &raw_cat).await?;
    describe(store).await
}

/// 清除落库配置，回到环境变量 / 本地引擎
pub async fn clear(store: &Store) -> Result<Value> {
    store.remove_setting(SETTING_KEY).await?;
    store.remove_setting(CATALOG_KEY).await?;
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
/// `base_url` / `api_key` 留空时回落到已存配置：管理员改 Base URL 时不必重输 Key，
/// 对话页想列出「当前端点有哪些模型」也可以直接空参调用。
pub async fn list_models(store: &Store, base_url: &str, api_key: &str) -> Result<Vec<String>> {
    let stored = stored_config(store).await?;
    let base = if base_url.trim().is_empty() {
        stored.as_ref().map(|c| c.base_url.clone()).unwrap_or_default()
    } else {
        base_url.trim().to_string()
    };
    let base = base.trim_end_matches('/').to_string();
    if base.is_empty() {
        return Err(anyhow!("请先填写 Base URL"));
    }
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(anyhow!("Base URL 必须以 http:// 或 https:// 开头"));
    }
    let key = if api_key.trim().is_empty() {
        stored.map(|c| c.api_key).unwrap_or_default()
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
    fn non_chat_models_stay_out_of_the_selector() {
        for name in [
            "text-embedding-3-large",
            "qwen3-embedding:4b",
            "bge-m3:567m",
            "bge-reranker-v2-m3",
            "whisper-large-v3",
            "BAAI/bge-large-zh",
        ] {
            assert!(!is_chat_model(name), "{name} 不应出现在对话模型下拉里");
        }
        for name in ["qwen3.5:9b", "deepseek-chat", "gpt-4o-mini", "Qwen2.5-7B-Instruct"] {
            assert!(is_chat_model(name), "{name} 是对话模型");
        }
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
        let db = crate::test_support::test_store().await;
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

    fn entry(name: &str, model: &str, key: &str) -> CatalogEntry {
        CatalogEntry {
            name: name.to_string(),
            provider: "deepseek".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            model: model.to_string(),
            api_key: key.to_string(),
        }
    }

    #[test]
    fn normalize_catalog_defaults_to_first_and_keeps_blank_key() {
        let previous = Catalog {
            default: "快速版".to_string(),
            entries: vec![entry("快速版", "deepseek-chat", "sk-old-key-0001")],
        };
        // 只改显示名以外的东西、Key 留空：Key 要沿用同名旧条目
        let input = Catalog {
            default: String::new(),
            entries: vec![entry("快速版", "deepseek-chat", ""), entry("深度版", "deepseek-reasoner", "sk-new-key-0002")],
        };
        let cat = normalize_catalog(input, &previous).unwrap();
        assert_eq!(cat.default, "快速版");
        assert_eq!(cat.entries[0].api_key, "sk-old-key-0001");
        assert_eq!(cat.entries[1].api_key, "sk-new-key-0002");
    }

    #[test]
    fn normalize_catalog_rejects_duplicate_name_and_unknown_default() {
        let dup = Catalog {
            default: String::new(),
            entries: vec![entry("同名", "a", "sk-1-000000"), entry("同名", "b", "sk-2-000000")],
        };
        assert!(normalize_catalog(dup, &Catalog::default()).is_err());

        let bad_default = Catalog {
            default: "不存在的名字".to_string(),
            entries: vec![entry("快速版", "deepseek-chat", "sk-1-000000")],
        };
        assert!(normalize_catalog(bad_default, &Catalog::default()).is_err());
    }

    #[test]
    fn catalog_entry_keeps_its_own_endpoint_and_key() {
        let mut slow = entry("内网 Qwen", "qwen2.5:7b", "sk-local");
        slow.base_url = "http://10.0.0.9:11434/v1".to_string();
        let cat = Catalog { default: "快速版".to_string(), entries: vec![entry("快速版", "deepseek-chat", "sk-cloud"), slow] };

        let picked = cat.find("内网 Qwen").unwrap().config();
        assert_eq!(picked.base_url, "http://10.0.0.9:11434/v1");
        assert_eq!(picked.api_key, "sk-local");
        assert_eq!(cat.default_entry().unwrap().name, "快速版");
        // 名字失效时退默认条目，而不是拼出一个「默认端点 + 别的模型名」的混合配置
        assert_eq!(cat.default_entry().unwrap().model, "deepseek-chat");
    }
}
