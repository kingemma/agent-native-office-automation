//! Tauri 2.0 客户端外壳（Windows / macOS / Linux / Android / iOS）。
//!
//! 桌面与移动端**不再是数据宿主**：没有本地 SQLite、没有 invoke 业务命令，
//! 所有业务一律走 `oa-server` 的 REST API（服务端地址在登录页配置，局域网优先、外网兜底）。
//! 这里只保留窗口/系统集成，保证四个平台跑的是同一份前端产物与同一套接口。

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        // 更新包由 release 流水线用 TAURI_SIGNING_PRIVATE_KEY 签名，
        // 公钥和清单地址写在 tauri.conf.json 的 plugins.updater 里
        .plugin(tauri_plugin_updater::Builder::new().build())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
