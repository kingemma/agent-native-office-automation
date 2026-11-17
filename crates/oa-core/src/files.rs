//! 附件 / 发票图片：统一上传通道（HTTP 与 Tauri 共用同一份校验、落盘与元数据逻辑）。
//!
//! 设计要点：
//! - 文件落磁盘（`OA_FILES`，默认 `oa-files/`），库里只存相对路径与元数据，避免把二进制塞进库；
//! - 上传/读取统一走 JSON + base64：浏览器与桌面端同一份契约，不需要各写一套 multipart / 本地路径；
//! - 大小与类型白名单在服务端校验，前端只负责提示，绕不过去；
//! - 可见性跟业务单据一致：报销单看本人或财务范围，合同看财务范围（与台账行级裁剪同源）。

use crate::auth::SessionUser;
use crate::db::next_id;
use crate::params;
use crate::store::Store;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 单文件上限：10 MB（base64 后约 13.3 MB，服务端 body 上限据此放宽）。
pub const MAX_BYTES: usize = 10 * 1024 * 1024;

/// 允许挂附件的业务单据类型。
pub const ENTITY_TYPES: &[&str] = &["claim", "contract"];

/// 允许的附件类型：发票/合同扫描件都是图片或 PDF。
pub const ALLOWED_MIME: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "application/pdf",
];

/// 附件根目录：`OA_FILES` 优先，默认工作目录下的 `oa-files/`。
pub fn default_root() -> PathBuf {
    std::env::var("OA_FILES")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("oa-files"))
}

/* ---------------- base64（不引入额外依赖） ---------------- */

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b1 = chunk[0] as u32;
        let b2 = *chunk.get(1).unwrap_or(&0) as u32;
        let b3 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b1 << 16) | (b2 << 8) | b3;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

pub fn b64_decode(text: &str) -> Result<Vec<u8>> {
    let clean: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let end = clean.iter().position(|b| *b == b'=').unwrap_or(clean.len());
    let mut out = Vec::with_capacity(end / 4 * 3 + 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for &b in &clean[..end] {
        let v = B64
            .iter()
            .position(|c| *c == b)
            .ok_or_else(|| anyhow!("上传内容不是合法的 base64"))? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if out.is_empty() {
        bail!("上传内容为空");
    }
    Ok(out)
}

/* ---------------- 文件名校验 ---------------- */

fn mime_of(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "pdf" => "application/pdf",
        _ => "",
    }
}

/// 前端可能给 `image/png; charset=...` 或空串（部分移动端），这里统一归一化。
fn resolve_mime(mime: &str, name: &str) -> String {
    let cleaned = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if cleaned.is_empty() {
        mime_of(name).to_string()
    } else {
        cleaned
    }
}

/// 只留文件名本身：去掉路径分隔符与控制字符，避免目录穿越写库外文件。
fn safe_name(name: &str) -> String {
    let base = name.rsplit(|c| c == '/' || c == '\\').next().unwrap_or(name).trim();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '*' | '?' | '"' | '<' | '>' | '|' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let out: String = cleaned.trim_matches('.').trim().chars().take(80).collect();
    if out.is_empty() {
        "attachment".to_string()
    } else {
        out
    }
}

/* ---------------- 权限 ---------------- */

/// 与台账行级裁剪同一套口径：报销/合同的管理范围是 admin / finance / manager。
fn finance_scope(user: &SessionUser) -> bool {
    user.is_admin() || matches!(user.role.as_str(), "finance" | "manager")
}

async fn owner_of(store: &Store, entity_type: &str, entity_id: &str) -> Result<Option<String>> {
    let sql = match entity_type {
        "claim" => "SELECT applicant_id AS owner FROM claims WHERE id=?1",
        "contract" => "SELECT owner_id AS owner FROM contracts WHERE id=?1",
        _ => bail!("不支持的附件归属：{entity_type}（可用 claim / contract）"),
    };
    Ok(store
        .query_json(sql, params![entity_id])
        .await?
        .first()
        .and_then(|r| r["owner"].as_str())
        .map(str::to_string))
}

fn can_access(user: &SessionUser, entity_type: &str, owner: &str) -> bool {
    match entity_type {
        // 本人单据可传可看；财务/经理范围可管理全部报销单附件
        "claim" => finance_scope(user) || owner == user.employee_id,
        // 合同只对能建合同的人开放（与 contract.create 的 RBAC 一致）
        _ => user.can("contract.create"),
    }
}

/* ---------------- 上传 / 列表 / 读取 / 删除 ---------------- */

pub async fn save(
    store: &Store,
    root: &Path,
    user: &SessionUser,
    entity_type: &str,
    entity_id: &str,
    file_name: &str,
    mime: &str,
    data_base64: &str,
) -> Result<Value> {
    let entity_type = entity_type.trim();
    let entity_id = entity_id.trim();
    if !ENTITY_TYPES.contains(&entity_type) {
        bail!("不支持的附件归属：{entity_type}（可用 claim / contract）");
    }
    if entity_id.is_empty() {
        bail!("缺少附件归属单据 id");
    }
    let owner = owner_of(store, entity_type, entity_id)
        .await?
        .ok_or_else(|| anyhow!("附件归属单据 {entity_id} 不存在"))?;
    if !can_access(user, entity_type, &owner) {
        bail!("无权为 {entity_id} 上传附件");
    }

    let bytes = b64_decode(data_base64)?;
    if bytes.len() > MAX_BYTES {
        bail!("附件超过 {} MB 上限", MAX_BYTES / 1024 / 1024);
    }
    let file_name = safe_name(file_name);
    let mime = resolve_mime(mime, &file_name);
    if !ALLOWED_MIME.contains(&mime.as_str()) {
        bail!("不支持的附件类型 {mime}，仅支持 PNG / JPG / WEBP / GIF / PDF");
    }

    let id = next_id(&mut store.clone(), "attachments", "FL", 4).await?;
    let rel = format!("{}/{id}-{file_name}", crate::clock::local_month());
    let path = root.join(&rel);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("创建附件目录失败：{}", dir.display()))?;
    }
    std::fs::write(&path, &bytes)
        .with_context(|| format!("写入附件失败：{}", path.display()))?;

    let size = bytes.len() as i64;
    let inserted = store
        .execute(
            "INSERT INTO attachments(id,entity_type,entity_id,file_name,mime,size_bytes,storage_path,uploaded_by)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id, entity_type, entity_id, file_name, mime, size, rel, user.employee_id],
        )
        .await;
    if let Err(e) = inserted {
        // 元数据没落库就不要留孤儿文件
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }

    Ok(json!({
        "ok": true, "id": id, "entity_type": entity_type, "entity_id": entity_id,
        "file_name": file_name, "mime": mime, "size_bytes": size,
        "uploaded_by": user.employee_id,
        "message": format!("附件「{file_name}」已上传")
    }))
}

pub async fn list(
    store: &Store,
    user: &SessionUser,
    entity_type: &str,
    entity_id: &str,
) -> Result<Vec<Value>> {
    let owner = owner_of(store, entity_type, entity_id)
        .await?
        .ok_or_else(|| anyhow!("单据 {entity_id} 不存在"))?;
    if !can_access(user, entity_type, &owner) {
        bail!("无权查看 {entity_id} 的附件");
    }
    store
        .query_json(
            "SELECT id, entity_type, entity_id, file_name, mime, size_bytes, uploaded_by, created_at
               FROM attachments WHERE entity_type=?1 AND entity_id=?2
              ORDER BY created_at DESC, id DESC LIMIT 200",
            params![entity_type, entity_id],
        )
        .await
}

/// 读取附件内容：返回 data URL，前端可直接塞进 `<img src>` / `<a href>`，两个通道行为一致。
pub async fn read(store: &Store, root: &Path, user: &SessionUser, id: &str) -> Result<Value> {
    let row = store
        .query_json(
            "SELECT id, entity_type, entity_id, file_name, mime, size_bytes, storage_path
               FROM attachments WHERE id=?1",
            params![id],
        )
        .await?
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("附件 {id} 不存在"))?;

    let entity_type = row["entity_type"].as_str().unwrap_or("").to_string();
    let entity_id = row["entity_id"].as_str().unwrap_or("").to_string();
    let owner = owner_of(store, &entity_type, &entity_id).await?.unwrap_or_default();
    if !can_access(user, &entity_type, &owner) {
        bail!("无权查看该附件");
    }

    let rel = row["storage_path"].as_str().unwrap_or("");
    let path = root.join(rel);
    let bytes = std::fs::read(&path)
        .with_context(|| format!("附件文件缺失：{}", path.display()))?;
    let mime = row["mime"].as_str().unwrap_or("application/octet-stream").to_string();

    Ok(json!({
        "id": row["id"], "file_name": row["file_name"], "mime": mime,
        "size_bytes": row["size_bytes"], "size": bytes.len(),
        "data_url": format!("data:{mime};base64,{}", b64_encode(&bytes)),
    }))
}

pub async fn delete(store: &Store, root: &Path, user: &SessionUser, id: &str) -> Result<Value> {
    let row = store
        .query_json(
            "SELECT entity_type, entity_id, storage_path, uploaded_by FROM attachments WHERE id=?1",
            params![id],
        )
        .await?
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("附件 {id} 不存在"))?;
    let entity_type = row["entity_type"].as_str().unwrap_or("");
    let entity_id = row["entity_id"].as_str().unwrap_or("");
    let owner = owner_of(store, entity_type, entity_id).await?.unwrap_or_default();
    let uploader = row["uploaded_by"].as_str().unwrap_or("");
    if uploader != user.employee_id && !user.is_admin() {
        bail!("只有上传者或管理员可以删除附件");
    }
    if !can_access(user, entity_type, &owner) {
        bail!("无权删除该附件");
    }

    store
        .execute("DELETE FROM attachments WHERE id=?1", params![id])
        .await?;
    if let Some(rel) = row["storage_path"].as_str() {
        let _ = std::fs::remove_file(root.join(rel));
    }
    Ok(json!({ "ok": true, "id": id, "message": "附件已删除" }))
}

/// 重置演示数据时清空附件目录（记录由 `RESET_SQL` 一并删除）。
pub fn purge(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let ok = if path.is_dir() {
            std::fs::remove_dir_all(&path).is_ok()
        } else {
            std::fs::remove_file(&path).is_ok()
        };
        if ok {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::DEFAULT_PASSWORD;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("oa-files-{tag}-{nanos}-{n}"))
    }

    #[test]
    fn base64_roundtrip_and_rejects_garbage() {
        for len in [1usize, 2, 3, 4, 5, 64] {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&data)).unwrap(), data);
        }
        assert!(b64_decode("!!!").is_err());
        assert!(b64_decode("").is_err());
    }

    #[test]
    fn file_names_are_sanitized() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("发票 2026:11.png"), "发票 2026_11.png");
        assert_eq!(safe_name("   "), "attachment");
    }

    #[tokio::test]
    async fn upload_list_read_delete_roundtrip() {
        let db = Store::open_in_memory().await.unwrap();
        let (_, admin) = db.login("evan", DEFAULT_PASSWORD).await.unwrap();
        let root = temp_root("ok");
        let payload = b64_encode(b"\x89PNG\r\n\x1a\n demo-invoice");

        let saved = save(&db, &root, &admin, "claim", "BX-2026-0048", "发票 1.png", "image/png", &payload)
            .await
            .unwrap();
        assert_eq!(saved["ok"], true, "{saved}");
        let id = saved["id"].as_str().unwrap().to_string();
        assert!(id.starts_with("FL-"));

        let rows = list(&db, &admin, "claim", "BX-2026-0048").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["file_name"], "发票 1.png");
        assert_eq!(rows[0]["uploaded_by"], "E01");

        let got = read(&db, &root, &admin, &id).await.unwrap();
        assert!(got["data_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert_eq!(b64_decode(got["data_url"].as_str().unwrap().split(',').nth(1).unwrap()).unwrap(),
            b"\x89PNG\r\n\x1a\n demo-invoice");

        delete(&db, &root, &admin, &id).await.unwrap();
        assert!(list(&db, &admin, "claim", "BX-2026-0048").await.unwrap().is_empty());
        assert!(read(&db, &root, &admin, &id).await.is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn scope_and_type_validation_are_enforced() {
        let db = Store::open_in_memory().await.unwrap();
        let (_, employee) = db.login("lina", DEFAULT_PASSWORD).await.unwrap();
        let (_, finance) = db.login("wuhao", DEFAULT_PASSWORD).await.unwrap();
        let root = temp_root("scope");
        let payload = b64_encode(b"demo");

        // 普通员工不能给别人的报销单或合同挂附件
        assert!(save(&db, &root, &employee, "claim", "BX-2026-0048", "a.png", "image/png", &payload)
            .await
            .is_err());
        assert!(save(&db, &root, &employee, "contract", "HT-2026-013", "a.png", "image/png", &payload)
            .await
            .is_err());
        // 财务可以给报销单传；类型与归属必须合法
        assert!(save(&db, &root, &finance, "claim", "BX-2026-0048", "a.exe", "application/x-msdownload", &payload)
            .await
            .is_err());
        assert!(save(&db, &root, &finance, "claim", "BX-404", "a.png", "image/png", &payload)
            .await
            .is_err());
        assert!(save(&db, &root, &finance, "claim", "BX-2026-0048", "a.pdf", "application/pdf", &payload)
            .await
            .is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }
}
