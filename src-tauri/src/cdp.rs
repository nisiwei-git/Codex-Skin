use crate::error::{AppError, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use url::Url;

pub const DEBUG_PORT: u16 = 9229;
const MAX_MESSAGE: usize = 4 * 1024 * 1024;
const LIVE_STYLE_ID: &str = "codex-theme-store-live-style";
const ACTIVE_KEY: &str = "__codexThemeStoreActiveInjection";
const SCRIPT_KEY: &str = "__codexThemeStoreNewDocumentScript";
const CLEANUP: &str = "(() => { try { localStorage.removeItem('__codexThemeStoreActiveInjection'); localStorage.removeItem('__codexThemeStoreNewDocumentScript'); } catch {} globalThis.__codexThemeStore?.dispose?.(); globalThis.__codexThemeStore?.observer?.disconnect(); globalThis.__codexThemeStore?.resizeHandler && window.removeEventListener('resize',globalThis.__codexThemeStore.resizeHandler); delete globalThis.__codexThemeStore; document.querySelectorAll('.codex-theme-sidebar-logo,#codex-theme-effect-ambient,#codex-theme-effect-overlay,#codex-theme-composer-accent').forEach(node=>node.remove()); document.getElementById('codex-theme-store-live-style')?.remove(); document.getElementById('codex-theme-home')?.remove(); document.documentElement.classList.remove('codex-theme-native'); document.documentElement.removeAttribute('data-codex-theme-id'); document.documentElement.style.removeProperty('--codex-theme-background-size'); setTimeout(()=>location.reload(),0); return true; })()";

#[derive(Clone, Debug)]
pub struct Payload {
    pub css: String,
    pub javascript: String,
    pub theme_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InjectionOutcome {
    pub applied: usize,
    pub candidates: usize,
}

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

pub async fn is_ready() -> bool {
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    else {
        return false;
    };
    let Ok(response) = client
        .get(format!("http://127.0.0.1:{DEBUG_PORT}/json/version"))
        .send()
        .await
    else {
        return false;
    };
    let Ok(value) = response.json::<Value>().await else {
        return false;
    };
    value
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .is_some_and(allowed_socket)
}

pub fn allowed_socket(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.scheme() != "ws"
        || url.port_or_known_default() != Some(DEBUG_PORT)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    if !matches!(url.host_str(), Some("127.0.0.1" | "localhost")) {
        return false;
    }
    let segments: Vec<_> = url
        .path_segments()
        .map(Iterator::collect)
        .unwrap_or_default();
    segments.len() == 3
        && segments[0] == "devtools"
        && matches!(segments[1], "page" | "browser")
        && !segments[2].is_empty()
        && segments[2].len() <= 200
        && segments[2]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

pub async fn inject(payload: &Payload, timeout: std::time::Duration) -> Result<InjectionOutcome> {
    tokio::time::timeout(timeout, inject_inner(payload))
        .await
        .map_err(|_| AppError::Message("CDP 注入超时。".into()))?
}

async fn inject_inner(payload: &Payload) -> Result<InjectionOutcome> {
    let injection_id = uuid::Uuid::new_v4().simple().to_string();
    let theme = theme_expression(payload, &injection_id);
    let id_json = serde_json::to_string(&injection_id)?;
    let active_json = serde_json::to_string(ACTIVE_KEY)?;
    let activation = format!(
        "(() => {{ try {{ localStorage.setItem({active_json}, {id_json}); if (localStorage.getItem({active_json}) !== {id_json}) return false; }} catch {{ return false; }} return {theme}; }})()"
    );
    let new_document = format!(
        "(() => {{ if (window.top !== window) return; const applySavedTheme=()=>{{ try {{ if(localStorage.getItem({active_json})!=={id_json}) return; }} catch {{ return; }} {theme}; }}; if(document.documentElement) applySavedTheme(); else document.addEventListener('DOMContentLoaded',applySavedTheme,{{once:true}}); }})()"
    );
    let expected_theme = serde_json::to_string(&payload.theme_id)?;
    let verification = format!(
        "(() => {{ let active=false; try {{ active=localStorage.getItem({active_json})==={id_json}; }} catch {{}} const style=document.getElementById({}); const store=globalThis.__codexThemeStore; const root=document.documentElement; const actualTheme=root?.dataset?.codexThemeId||''; return active && style?.dataset.codexThemeStoreInjectionId==={id_json} && !!store && store.injectionId==={id_json} && store.surfaceDecorated===true && root?.classList.contains('codex-theme-native') && actualTheme.length>0 && ({expected_theme}===null || actualTheme==={expected_theme}); }})()",
        serde_json::to_string(LIVE_STYLE_ID)?
    );
    let targets = targets().await?;
    let candidates = targets.len();
    let mut applied = 0;
    for target in targets {
        if inject_target(&target, &activation, &new_document, &verification)
            .await
            .unwrap_or(false)
        {
            applied += 1;
        }
    }
    Ok(InjectionOutcome {
        applied,
        candidates,
    })
}

pub async fn remove(timeout: std::time::Duration) -> Result<usize> {
    tokio::time::timeout(timeout, remove_inner())
        .await
        .map_err(|_| AppError::Message("CDP 回滚超时。".into()))?
}

async fn remove_inner() -> Result<usize> {
    let mut successes = 0;
    for target in targets().await? {
        if remove_target(&target, CLEANUP).await.unwrap_or(false) {
            successes += 1;
        }
    }
    Ok(successes)
}

fn theme_expression(payload: &Payload, injection_id: &str) -> String {
    format!(
        "(() => {{ let style=document.getElementById({style_id}); if(!style){{ style=document.createElement('style'); style.id={style_id}; const root=document.head||document.documentElement; if(!root)return false; root.appendChild(style); }} style.dataset.codexThemeStoreInjectionId={injection}; style.textContent={css}; {javascript} if(!globalThis.__codexThemeStore)return false; globalThis.__codexThemeStore.injectionId={injection}; return true; }})()",
        style_id = serde_json::to_string(LIVE_STYLE_ID).unwrap(),
        injection = serde_json::to_string(injection_id).unwrap(),
        css = serde_json::to_string(&payload.css).unwrap(),
        javascript = payload.javascript
    )
}

async fn targets() -> Result<Vec<String>> {
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(3))
        .build()?
        .get(format!("http://127.0.0.1:{DEBUG_PORT}/json/list"))
        .send()
        .await?
        .error_for_status()?;
    let values: Vec<Value> = response.json().await?;
    let mut result = Vec::new();
    for value in values {
        if value.get("type").and_then(Value::as_str) != Some("page") {
            continue;
        }
        let url = value.get("url").and_then(Value::as_str).unwrap_or("");
        let title = value.get("title").and_then(Value::as_str).unwrap_or("");
        let app_root = is_main_codex_page(url, title);
        let app_page = url.to_ascii_lowercase().starts_with("app:");
        let codex = url.to_ascii_lowercase().contains("codex")
            || title.to_ascii_lowercase().contains("codex");
        let priority = match (app_root, app_page, codex) {
            (true, _, _) => 0,
            // Auxiliary app pages such as avatar-overlay must never receive a
            // persistent theme script even though their title is also Codex.
            (false, true, _) => continue,
            (false, false, true) => 2,
            _ => continue,
        };
        if let Some(socket) = value
            .get("webSocketDebuggerUrl")
            .and_then(Value::as_str)
            .filter(|value| allowed_socket(value))
        {
            result.push((priority, socket.to_owned()));
        }
    }
    result.sort();
    result.dedup_by(|left, right| left.1 == right.1);
    Ok(result.into_iter().map(|item| item.1).collect())
}

fn is_main_codex_page(url: &str, title: &str) -> bool {
    let Ok(url) = Url::parse(url) else {
        return false;
    };
    url.scheme() == "app"
        && url.path().eq_ignore_ascii_case("/index.html")
        && url.query().is_none()
        && title.eq_ignore_ascii_case("codex")
}

async fn inject_target(
    url: &str,
    activation: &str,
    new_document: &str,
    verification: &str,
) -> Result<bool> {
    let (mut socket, _) = connect_async(url)
        .await
        .map_err(|error| AppError::Message(format!("CDP WebSocket 连接失败：{error}")))?;
    if !successful(&send(&mut socket, 1, "Page.enable", json!({})).await?) {
        return Ok(false);
    }
    // Codex uses CSS modules for parts of the main surface, so the generated
    // suffix changes between releases. Keep the stable semantic fragments and
    // retain the legacy selectors for older desktop builds.
    let main_surface = serde_json::to_string(crate::compiler::MAIN_SURFACE_SELECTOR)?;
    let probe = format!(
        "!!document.body && !!document.querySelector('.app-shell-left-panel, aside.app-shell-left-panel') && !!document.querySelector({main_surface}) && (!!document.querySelector('.composer-surface-chrome, [role=main]') || !!document.querySelector('[contenteditable=\"true\"][role=\"textbox\"]'))"
    );
    if !is_true(&evaluate(&mut socket, 99, &probe).await?) {
        return Ok(false);
    }
    let previous = evaluate(&mut socket, 2, "(() => { try { return localStorage.getItem('__codexThemeStoreNewDocumentScript') || ''; } catch { return ''; } })()").await?;
    if let Some(identifier) = evaluation_value(&previous)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        remove_script(&mut socket, identifier, 3).await?;
    }
    let added = send(
        &mut socket,
        4,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source":new_document}),
    )
    .await?;
    let Some(identifier) = added
        .get("result")
        .and_then(|v| v.get("identifier"))
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return Ok(false);
    };
    let persist = format!(
        "(() => {{ try {{ localStorage.setItem({},{}) ; return true; }} catch {{ return false; }} }})()",
        serde_json::to_string(SCRIPT_KEY)?,
        serde_json::to_string(&identifier)?
    );
    if !is_true(&evaluate(&mut socket, 5, &persist).await?) {
        remove_script(&mut socket, &identifier, 6).await?;
        return Ok(false);
    }
    if !successful_evaluation(&evaluate(&mut socket, 7, activation).await?) {
        remove_script(&mut socket, &identifier, 8).await?;
        return Ok(false);
    }
    for attempt in 0..5 {
        if is_true(&evaluate(&mut socket, 10 + attempt, verification).await?) {
            return Ok(true);
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    remove_script(&mut socket, &identifier, 20).await?;
    Ok(false)
}

async fn remove_target(url: &str, cleanup: &str) -> Result<bool> {
    let (mut socket, _) = connect_async(url)
        .await
        .map_err(|error| AppError::Message(format!("CDP WebSocket 连接失败：{error}")))?;
    if !successful(&send(&mut socket, 1, "Page.enable", json!({})).await?) {
        return Ok(false);
    }
    let previous = evaluate(&mut socket, 2, "(() => { try { return localStorage.getItem('__codexThemeStoreNewDocumentScript') || ''; } catch { return ''; } })()").await?;
    if let Some(identifier) = evaluation_value(&previous)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        remove_script(&mut socket, identifier, 3).await?;
    }
    Ok(is_true(&evaluate(&mut socket, 4, cleanup).await?))
}

async fn remove_script(socket: &mut Socket, identifier: &str, id: i32) -> Result<()> {
    let _ = send(
        socket,
        id,
        "Page.removeScriptToEvaluateOnNewDocument",
        json!({"identifier":identifier}),
    )
    .await?;
    Ok(())
}
async fn evaluate(socket: &mut Socket, id: i32, expression: &str) -> Result<Value> {
    send(
        socket,
        id,
        "Runtime.evaluate",
        json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),
    )
    .await
}

async fn send(socket: &mut Socket, id: i32, method: &str, params: Value) -> Result<Value> {
    socket
        .send(Message::Text(
            serde_json::to_string(&json!({"id":id,"method":method,"params":params}))?.into(),
        ))
        .await
        .map_err(|error| AppError::Message(format!("CDP 发送失败：{error}")))?;
    while let Some(message) = socket.next().await {
        let message =
            message.map_err(|error| AppError::Message(format!("CDP 接收失败：{error}")))?;
        let bytes = match message {
            Message::Text(value) => value.as_bytes().to_vec(),
            Message::Binary(value) => value.to_vec(),
            Message::Close(_) => return Err(AppError::Message("CDP 连接已关闭。".into())),
            _ => continue,
        };
        if bytes.len() > MAX_MESSAGE {
            return Err(AppError::Message("CDP 响应超过 4 MB 安全限制。".into()));
        }
        let value: Value = serde_json::from_slice(&bytes)?;
        if value.get("id").and_then(Value::as_i64) == Some(id as i64) {
            return Ok(value);
        }
    }
    Err(AppError::Message("CDP 未返回响应。".into()))
}

fn successful(value: &Value) -> bool {
    value.get("error").is_none()
}
fn successful_evaluation(value: &Value) -> bool {
    successful(value)
        && value.get("result").is_some_and(|result| {
            result.get("exceptionDetails").is_none() && result.get("result").is_some()
        })
}
fn evaluation_value(value: &Value) -> Option<&Value> {
    value.get("result")?.get("result")?.get("value")
}
fn is_true(value: &Value) -> bool {
    successful_evaluation(value) && evaluation_value(value).and_then(Value::as_bool) == Some(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_removes_orphaned_theme_nodes() {
        assert!(CLEANUP.contains(".codex-theme-sidebar-logo"));
        assert!(CLEANUP.contains("#codex-theme-effect-overlay"));
        assert!(CLEANUP.contains("querySelectorAll"));
    }
    #[test]
    fn only_loopback_cdp_urls_are_allowed() {
        assert!(allowed_socket("ws://127.0.0.1:9229/devtools/page/abc-123"));
        assert!(allowed_socket(
            "ws://localhost:9229/devtools/browser/abc_123"
        ));
        assert!(!allowed_socket("ws://0.0.0.0:9229/devtools/page/abc"));
        assert!(!allowed_socket("ws://127.0.0.1:9230/devtools/page/abc"));
        assert!(!allowed_socket(
            "ws://user@127.0.0.1:9229/devtools/page/abc"
        ));
        assert!(!allowed_socket("ws://127.0.0.1:9229/devtools/page/abc?x=1"));
    }

    #[test]
    fn identifies_only_the_main_codex_app_page() {
        assert!(is_main_codex_page("app://-/index.html", "Codex"));
        assert!(is_main_codex_page("app:///index.html", "codex"));
        assert!(!is_main_codex_page(
            "app://-/index.html?initialRoute=%2Favatar-overlay",
            "Codex"
        ));
        assert!(!is_main_codex_page("https://chatgpt.com/", "Codex"));
    }
}
