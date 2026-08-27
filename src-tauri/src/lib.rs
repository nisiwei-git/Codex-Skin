mod authoring;
mod catalog;
mod cdp;
mod compiler;
mod dreamskin;
mod error;
mod models;
mod paths;
mod platform;
mod protocol;
mod repository;
mod updater;

pub fn verify_dreamskin(path: &str, platform: &str) -> Result<(), String> {
    dreamskin::verify_for_platform(path, platform)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn catalog_index(root: &str, name: Option<&str>) -> Result<usize, String> {
    authoring::index(root, name).map_err(|error| error.to_string())
}
pub fn catalog_validate(root: &str) -> Result<usize, String> {
    authoring::validate(root).map_err(|error| error.to_string())
}
pub fn catalog_pack(root: &str, output: &str) -> Result<usize, String> {
    authoring::pack(root, output).map_err(|error| error.to_string())
}

use base64::Engine;
use models::{AppState, SOURCES, SyncResult};
use tauri::{Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

struct ActivationQueue(std::sync::Mutex<Vec<String>>);

#[tauri::command]
fn get_app_state() -> error::Result<AppState> {
    let settings = repository::load_settings();
    Ok(AppState {
        themes: catalog::load()?,
        sources: SOURCES.to_vec(),
        selected_source_id: settings.source_id,
    })
}

#[tauri::command]
async fn sync_catalog(source_id: String) -> error::Result<SyncResult> {
    repository::sync(&source_id).await
}

#[tauri::command]
async fn download_theme(theme_id: String) -> error::Result<String> {
    repository::ensure_theme(&theme_id).await?;
    Ok(format!("{} 已下载", catalog::find(&theme_id)?.name))
}

#[tauri::command]
fn set_theme_subscription(theme_id: String, subscribed: bool) -> error::Result<()> {
    repository::set_subscription(&theme_id, subscribed)
}

#[tauri::command]
fn delete_theme(theme_id: String) -> error::Result<String> {
    let name = catalog::find(&theme_id)?.name;
    if repository::delete_theme(&theme_id)? {
        Ok(format!("{name} 已从本地删除"))
    } else {
        Ok(format!("{name} 没有可删除的本地文件"))
    }
}

#[tauri::command]
async fn sync_subscribed_themes() -> error::Result<usize> {
    repository::sync_subscriptions().await
}

#[tauri::command]
async fn import_local(path: String) -> error::Result<dreamskin::ImportResult> {
    tauri::async_runtime::spawn_blocking(move || dreamskin::import_local(&path))
        .await
        .map_err(|error| error::AppError::Message(format!("主题导入任务失败：{error}")))?
}

#[tauri::command]
async fn install_uri(
    uri: String,
    source_id: Option<String>,
) -> error::Result<dreamskin::ImportResult> {
    protocol::install_uri(&uri, source_id.as_deref()).await
}

#[tauri::command]
fn pending_activations(queue: tauri::State<'_, ActivationQueue>) -> Vec<String> {
    std::mem::take(&mut *queue.0.lock().unwrap())
}

#[tauri::command]
async fn read_preview(path: String) -> error::Result<String> {
    let requested = if let Some(theme_id) = path.strip_prefix("remote://") {
        repository::ensure_preview(theme_id).await?.canonicalize()?
    } else {
        std::path::PathBuf::from(&path).canonicalize()?
    };
    let allowed = [paths::cache_root()?, paths::installed_root()?]
        .into_iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| requested.starts_with(root));
    if !allowed {
        return Err(error::AppError::Message(
            "拒绝读取主题目录之外的图片。".into(),
        ));
    }
    let mime = match requested
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "avif" => "image/avif",
        _ => return Err(error::AppError::Message("不支持的预览图片格式。".into())),
    };
    let bytes = std::fs::read(requested)?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err(error::AppError::Message("预览图片超过 20 MB 限制。".into()));
    }
    Ok(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

#[tauri::command]
async fn theme_runtime_ready() -> bool {
    cdp::is_ready().await
}

#[tauri::command]
async fn apply_theme(theme_id: String) -> error::Result<String> {
    repository::ensure_theme(&theme_id).await?;
    let payload = compiler::compile(&theme_id)?;
    let mut outcome = cdp::InjectionOutcome::default();
    for attempt in 0..3 {
        outcome = cdp::inject(&payload, std::time::Duration::from_secs(15)).await?;
        if outcome.applied > 0 {
            break;
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        }
    }
    if outcome.applied == 0 {
        let message = if outcome.candidates == 0 {
            "已连接 Codex 主题端口，但主窗口尚未完成初始化，请稍后重试。"
        } else {
            "已连接 Codex 主题端口，但当前 Codex 页面结构暂不受支持，请更新 Codex-Skin。"
        };
        return Err(error::AppError::Message(message.into()));
    }
    Ok(format!("{} 已应用", catalog::find(&theme_id)?.name))
}
#[tauri::command]
async fn restart_and_apply(theme_id: String) -> error::Result<String> {
    repository::ensure_theme(&theme_id).await?;
    let payload = compiler::compile(&theme_id)?;
    platform::restart_and_inject(&payload, std::time::Duration::from_secs(90)).await?;
    Ok(format!("{} 已应用", catalog::find(&theme_id)?.name))
}
#[tauri::command]
async fn rollback_theme() -> error::Result<String> {
    // A fresh Codex process has no theme injection to remove. In particular,
    // macOS does not expose the local CDP endpoint until Codex has been started
    // in theme mode, so restoring defaults must be a safe no-op in that state.
    if !cdp::is_ready().await {
        return Ok("当前已经是默认主题".into());
    }
    let mut last_error = None;
    for attempt in 0..3 {
        match cdp::remove(std::time::Duration::from_secs(15)).await {
            Ok(0) => return Ok("当前已经是默认主题".into()),
            Ok(_) => return Ok("已恢复默认主题".into()),
            Err(error) => last_error = Some(error.to_string()),
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(350)).await;
        }
    }
    if !cdp::is_ready().await {
        return Ok("当前已经是默认主题".into());
    }
    Err(error::AppError::Message(format!(
        "恢复默认主题失败，请关闭并重新打开 Codex 后重试：{}",
        last_error.unwrap_or_else(|| "本机主题端口暂时不可用".into())
    )))
}

#[tauri::command]
async fn check_app_update() -> error::Result<Option<updater::UpdateInfo>> {
    updater::check().await
}

#[tauri::command]
async fn install_app_update(app: tauri::AppHandle, version: String) -> error::Result<String> {
    updater::install(app, version).await
}

pub fn run() {
    tauri::Builder::default()
        .manage(ActivationQueue(std::sync::Mutex::new(Vec::new())))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_single_instance::init(|app, argv, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
            app.state::<ActivationQueue>().0.lock().unwrap().extend(
                argv.iter()
                    .filter(|value| {
                        value.starts_with("dreamskin:")
                            || value.to_ascii_lowercase().ends_with(".dreamskin")
                    })
                    .cloned(),
            );
            let _ = app.emit("external-activation", argv);
        }))
        .setup(|app| {
            let initial: Vec<_> = std::env::args()
                .skip(1)
                .filter(|value| {
                    value.starts_with("dreamskin:")
                        || value.to_ascii_lowercase().ends_with(".dreamskin")
                })
                .collect();
            app.state::<ActivationQueue>()
                .0
                .lock()
                .unwrap()
                .extend(initial);
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                let values: Vec<_> = event.urls().iter().map(ToString::to_string).collect();
                handle
                    .state::<ActivationQueue>()
                    .0
                    .lock()
                    .unwrap()
                    .extend(values.clone());
                let _ = handle.emit("external-activation", values);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            sync_catalog,
            download_theme,
            set_theme_subscription,
            delete_theme,
            sync_subscribed_themes,
            import_local,
            install_uri,
            pending_activations,
            read_preview,
            theme_runtime_ready,
            apply_theme,
            restart_and_apply,
            rollback_theme,
            check_app_update,
            install_app_update
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Codex-Skin");
}
