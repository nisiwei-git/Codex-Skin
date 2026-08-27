use crate::{
    error::{AppError, Result},
    models::{RepositorySettings, SOURCES, SyncResult},
    paths,
};
use atomicwrites::{AllowOverwrite, AtomicFile};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

const MAX_FILE: u64 = 20 * 1024 * 1024;
const MAX_FEED: u64 = 5 * 1024 * 1024;
const UPSTREAM_ROOT: &str = "https://raw.githubusercontent.com/lixiaobaivv/Codex-Skin-Store/main/";
const FEED_NAME: &str = "desktop-catalog-v2.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteResource {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteTheme {
    pub id: String,
    pub version: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub variant: String,
    pub manifest: RemoteResource,
    pub preview: RemoteResource,
    pub assets: Vec<RemoteResource>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteCatalog {
    pub schema_version: u64,
    pub name: String,
    pub revision: String,
    pub themes: Vec<RemoteTheme>,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncState {
    source_id: String,
    etag: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ThemeLibraryState {
    pub downloaded_versions: BTreeMap<String, String>,
    pub subscriptions: BTreeSet<String>,
}

pub fn load_settings() -> RepositorySettings {
    paths::settings_path()
        .ok()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub async fn sync(preferred: &str) -> Result<SyncResult> {
    let preferred = SOURCES
        .iter()
        .find(|source| source.id == preferred)
        .unwrap_or(&SOURCES[0]);
    let mut candidates = vec![preferred];
    for source in &SOURCES {
        if !candidates.iter().any(|item| item.id == source.id) {
            candidates.push(source);
        }
    }
    let mut last_error = None;
    for source in candidates {
        match sync_feed(source.id, source.prefix).await {
            Ok(catalog) => {
                let settings = serde_json::json!({"sourceId": source.id});
                let settings_path = paths::settings_path()?;
                fs::create_dir_all(settings_path.parent().unwrap())?;
                write_atomic(&settings_path, &serde_json::to_vec_pretty(&settings)?)?;
                return Ok(SyncResult {
                    theme_count: catalog.themes.len(),
                    source_id: source.id.into(),
                    source_name: source.name.into(),
                });
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(AppError::Message(format!(
        "所有主题同步线路均失败：{}",
        last_error.map(|e| e.to_string()).unwrap_or_default()
    )))
}

async fn sync_feed(source_id: &str, prefix: &str) -> Result<RemoteCatalog> {
    let cached = load_remote_catalog()?;
    let state = load_sync_state();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()?;
    let mut request = client
        .get(format!("{prefix}{UPSTREAM_ROOT}{FEED_NAME}"))
        .header("User-Agent", "Codex-Skin/2");
    if state.source_id == source_id
        && let Some(etag) = state.etag.as_deref()
    {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request.send().await?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return cached
            .ok_or_else(|| AppError::Message("服务器返回未修改，但本地没有目录缓存。".into()));
    }
    let response = response.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|value| value > MAX_FEED)
    {
        return Err(AppError::Message("远程主题目录超过 5 MB 限制。".into()));
    }
    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = download_response(response, MAX_FEED).await?;
    let catalog: RemoteCatalog = serde_json::from_slice(&bytes)?;
    validate_remote_catalog(&catalog)?;
    let path = paths::remote_catalog_path()?;
    fs::create_dir_all(path.parent().unwrap())?;
    write_atomic(&path, &bytes)?;
    let state = SyncState {
        source_id: source_id.into(),
        etag,
    };
    write_atomic(
        &paths::catalog_sync_state_path()?,
        &serde_json::to_vec_pretty(&state)?,
    )?;
    Ok(catalog)
}

pub(crate) fn load_remote_catalog() -> Result<Option<RemoteCatalog>> {
    let path = paths::remote_catalog_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let catalog: RemoteCatalog = serde_json::from_slice(&fs::read(path)?)?;
    validate_remote_catalog(&catalog)?;
    Ok(Some(catalog))
}

fn load_sync_state() -> SyncState {
    paths::catalog_sync_state_path()
        .ok()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn validate_remote_catalog(catalog: &RemoteCatalog) -> Result<()> {
    if catalog.schema_version != 2
        || catalog.name.is_empty()
        || catalog.name.len() > 80
        || chrono::DateTime::parse_from_rfc3339(&catalog.revision).is_err()
        || catalog.themes.is_empty()
        || catalog.themes.len() > 500
    {
        return Err(AppError::Message("远程主题目录元数据无效。".into()));
    }
    let mut ids = HashSet::new();
    for theme in &catalog.themes {
        if !valid_catalog_id(&theme.id)
            || !ids.insert(theme.id.as_str())
            || semver::Version::parse(&theme.version).is_err()
            || theme.name.is_empty()
            || theme.name.len() > 80
            || theme.description.len() > 300
            || !["人物", "动漫", "游戏", "风景", "极简", "节日", "其他"]
                .contains(&theme.category.as_str())
            || !["light", "dark"].contains(&theme.variant.as_str())
        {
            return Err(AppError::Message(format!("远程主题条目无效：{}", theme.id)));
        }
        validate_remote_resource(&theme.manifest)?;
        validate_remote_resource(&theme.preview)?;
        if theme.manifest.path != format!("themes/{}.json", theme.id)
            || !theme.preview.path.starts_with("previews/")
            || theme.assets.is_empty()
            || theme.assets.len() > 4
        {
            return Err(AppError::Message(format!(
                "远程主题资源索引无效：{}",
                theme.id
            )));
        }
        let mut assets = HashSet::new();
        for asset in &theme.assets {
            validate_remote_resource(asset)?;
            if !assets.insert(asset.path.as_str()) || asset.path.starts_with("themes/") {
                return Err(AppError::Message(format!(
                    "远程主题包含重复或无效资源：{}",
                    theme.id
                )));
            }
        }
        if !theme.assets.iter().any(|asset| {
            asset.path == theme.preview.path
                && asset.sha256 == theme.preview.sha256
                && asset.size == theme.preview.size
        }) {
            return Err(AppError::Message(format!(
                "远程主题缺少匹配的预览资源：{}",
                theme.id
            )));
        }
    }
    Ok(())
}

fn validate_remote_resource(resource: &RemoteResource) -> Result<()> {
    if !allowed(&resource.path)
        || resource.size == 0
        || resource.size > MAX_FILE
        || resource.sha256.len() != 64
        || !resource
            .sha256
            .chars()
            .all(|value| value.is_ascii_hexdigit())
    {
        return Err(AppError::Message(format!(
            "远程主题资源描述无效：{}",
            resource.path
        )));
    }
    Ok(())
}

pub(crate) async fn ensure_preview(theme_id: &str) -> Result<PathBuf> {
    let catalog =
        load_remote_catalog()?.ok_or_else(|| AppError::Message("尚未同步远程主题目录。".into()))?;
    let theme = catalog
        .themes
        .iter()
        .find(|theme| theme.id == theme_id)
        .ok_or_else(|| AppError::Message(format!("远程主题不存在：{theme_id}")))?;
    download_resource(&theme.preview, &load_settings().source_id, true).await
}

pub(crate) async fn ensure_theme(theme_id: &str) -> Result<()> {
    let Some(catalog) = load_remote_catalog()? else {
        return Ok(());
    };
    let Some(theme) = catalog.themes.iter().find(|theme| theme.id == theme_id) else {
        return Ok(());
    };
    let source_id = load_settings().source_id;
    let manifest = download_resource(&theme.manifest, &source_id, false).await?;
    for asset in &theme.assets {
        download_resource(asset, &source_id, true).await?;
    }
    validate_theme(&manifest, &theme.id, &paths::cache_root()?)?;
    let mut state = load_theme_library_state();
    state
        .downloaded_versions
        .insert(theme.id.clone(), theme.version.clone());
    save_theme_library_state(&state)
}

pub(crate) fn theme_library_state() -> ThemeLibraryState {
    load_theme_library_state()
}

pub(crate) fn set_subscription(theme_id: &str, subscribed: bool) -> Result<()> {
    if !valid_catalog_id(theme_id) {
        return Err(AppError::Message(format!("主题 ID 无效：{theme_id}")));
    }
    let mut state = load_theme_library_state();
    if subscribed {
        state.subscriptions.insert(theme_id.into());
    } else {
        state.subscriptions.remove(theme_id);
    }
    save_theme_library_state(&state)
}

pub(crate) fn delete_theme(theme_id: &str) -> Result<bool> {
    if !valid_library_id(theme_id) {
        return Err(AppError::Message(format!("主题 ID 无效：{theme_id}")));
    }

    let theme = load_remote_catalog()?.and_then(|catalog| {
        catalog
            .themes
            .into_iter()
            .find(|theme| theme.id == theme_id)
    });
    let mut removed = delete_theme_files(
        theme_id,
        &paths::installed_root()?,
        &paths::cache_root()?,
        theme.as_ref(),
    )?;

    let mut state = load_theme_library_state();
    removed |= state.downloaded_versions.remove(theme_id).is_some();
    state.subscriptions.remove(theme_id);
    save_theme_library_state(&state)?;
    clear_compiled_state(theme_id, &paths::state_root()?)?;
    Ok(removed)
}

fn delete_theme_files(
    theme_id: &str,
    installed_root: &Path,
    cache_root: &Path,
    theme: Option<&RemoteTheme>,
) -> Result<bool> {
    let mut removed = false;
    let installed = installed_root.join(theme_id);
    if installed.exists() {
        fs::remove_dir_all(&installed)?;
        removed = true;
    }

    if let Some(theme) = theme {
        let resources = std::iter::once(&theme.manifest)
            .chain(std::iter::once(&theme.preview))
            .chain(&theme.assets);
        for resource in resources {
            let path = cache_root.join(&resource.path);
            if path.is_file() {
                fs::remove_file(path)?;
                removed = true;
            }
        }
    } else {
        let manifest = cache_root.join("themes").join(format!("{theme_id}.json"));
        if manifest.is_file() {
            fs::remove_file(manifest)?;
            removed = true;
        }
    }
    Ok(removed)
}

fn clear_compiled_state(theme_id: &str, root: &Path) -> Result<()> {
    let current = root.join("current-theme.json");
    let is_current = fs::read(&current)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|manifest| {
            manifest
                .get("id")
                .or_else(|| manifest.get("codeThemeId"))
                .and_then(Value::as_str)
                .map(|id| id == theme_id)
        })
        .unwrap_or(false);
    if is_current {
        for name in ["codex-theme.css", "codex-theme.js", "current-theme.json"] {
            let path = root.join(name);
            if path.is_file() {
                fs::remove_file(path)?;
            }
        }
    }
    Ok(())
}

pub(crate) async fn sync_subscriptions() -> Result<usize> {
    let subscriptions: Vec<_> = load_theme_library_state()
        .subscriptions
        .into_iter()
        .collect();
    let mut updated = 0;
    for theme_id in subscriptions {
        let before = load_theme_library_state()
            .downloaded_versions
            .get(&theme_id)
            .cloned();
        ensure_theme(&theme_id).await?;
        let after = load_theme_library_state()
            .downloaded_versions
            .get(&theme_id)
            .cloned();
        if after.is_some() && after != before {
            updated += 1;
        }
    }
    Ok(updated)
}

fn load_theme_library_state() -> ThemeLibraryState {
    paths::theme_library_state_path()
        .ok()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_theme_library_state(state: &ThemeLibraryState) -> Result<()> {
    let path = paths::theme_library_state_path()?;
    fs::create_dir_all(path.parent().unwrap())?;
    write_atomic(&path, &serde_json::to_vec_pretty(state)?)
}

async fn download_resource(
    resource: &RemoteResource,
    preferred: &str,
    image_resource: bool,
) -> Result<PathBuf> {
    let destination = paths::cache_root()?.join(&resource.path);
    if verify_cached(&destination, resource, image_resource)? {
        return Ok(destination);
    }
    let mut candidates: Vec<_> = SOURCES
        .iter()
        .filter(|source| source.id == preferred)
        .collect();
    for source in &SOURCES {
        if !candidates.iter().any(|item| item.id == source.id) {
            candidates.push(source);
        }
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()?;
    let mut last_error = None;
    for source in candidates {
        let result = async {
            let response = client
                .get(format!("{}{UPSTREAM_ROOT}{}", source.prefix, resource.path))
                .header("User-Agent", "Codex-Skin/2")
                .send()
                .await?
                .error_for_status()?;
            if response
                .content_length()
                .is_some_and(|value| value != resource.size)
            {
                return Err(AppError::Message(format!(
                    "主题资源大小不匹配：{}",
                    resource.path
                )));
            }
            let bytes = download_response(response, resource.size).await?;
            if bytes.len() as u64 != resource.size
                || hex::encode(Sha256::digest(&bytes)) != resource.sha256
            {
                return Err(AppError::Message(format!(
                    "主题资源完整性校验失败：{}",
                    resource.path
                )));
            }
            if image_resource {
                image::load_from_memory(&bytes).map_err(|error| {
                    AppError::Message(format!("主题图片无法解码：{}：{error}", resource.path))
                })?;
            }
            fs::create_dir_all(destination.parent().unwrap())?;
            write_atomic(&destination, &bytes)?;
            Ok(destination.clone())
        }
        .await;
        match result {
            Ok(path) => return Ok(path),
            Err(error) => last_error = Some(error),
        }
    }
    Err(AppError::Message(format!(
        "所有线路均无法下载主题资源 {}：{}",
        resource.path,
        last_error
            .map(|error| error.to_string())
            .unwrap_or_default()
    )))
}

fn verify_cached(path: &Path, resource: &RemoteResource, image_resource: bool) -> Result<bool> {
    let Ok(bytes) = fs::read(path) else {
        return Ok(false);
    };
    if bytes.len() as u64 != resource.size || hex::encode(Sha256::digest(&bytes)) != resource.sha256
    {
        return Ok(false);
    }
    if image_resource && image::load_from_memory(&bytes).is_err() {
        return Ok(false);
    }
    Ok(true)
}

async fn download_response(response: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len() as u64 + chunk.len() as u64 > limit {
            return Err(AppError::Message("远程资源超过声明的大小限制。".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let file = AtomicFile::new(path, AllowOverwrite);
    file.write(|handle| handle.write_all(bytes))
        .map_err(|error| AppError::Message(format!("原子写入失败：{error}")))
}

pub(crate) fn allowed(relative: &str) -> bool {
    if relative == "theme-repository.json" {
        return true;
    }
    let Some((directory, name)) = relative.split_once('/') else {
        return false;
    };
    if name.contains('/') {
        return false;
    }
    let ext = Path::new(name)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match directory {
        "themes" | "schemas" => ext == "json",
        "backgrounds" | "logos" | "previews" | "pets" => {
            ["jpg", "jpeg", "png", "webp", "avif"].contains(&ext.as_str())
        }
        _ => false,
    }
}

pub(crate) fn validate_repository(root: &Path) -> Result<()> {
    for schema in ["theme-v1.schema.json", "theme-repository-v1.schema.json"] {
        if !root.join("schemas").join(schema).is_file() {
            return Err(AppError::Message(format!("主题仓库缺少 Schema：{schema}")));
        }
    }
    let index: Value = serde_json::from_slice(&fs::read(root.join("theme-repository.json"))?)?;
    closed_object(
        &index,
        &["$schema", "schemaVersion", "name", "updatedAt", "themes"],
        &["schemaVersion", "name", "updatedAt", "themes"],
        "theme-repository.json",
    )?;
    if index.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err(AppError::Message("只支持主题仓库标准 v1。".into()));
    }
    let entries = index
        .get("themes")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Message("主题仓库 themes 无效。".into()))?;
    if entries.is_empty() || entries.len() > 500 {
        return Err(AppError::Message("主题数量必须在 1 到 500 之间。".into()));
    }
    let mut ids = HashSet::new();
    let mut manifests = HashSet::new();
    for entry in entries {
        closed_object(entry, &["id", "manifest"], &["id", "manifest"], "themes[]")?;
        let id = entry.get("id").and_then(Value::as_str).unwrap_or("");
        let manifest = entry.get("manifest").and_then(Value::as_str).unwrap_or("");
        if !valid_catalog_id(id)
            || !ids.insert(id)
            || !manifests.insert(manifest.to_owned())
            || manifest != format!("themes/{id}.json")
            || !root.join(manifest).is_file()
        {
            return Err(AppError::Message(format!("主题索引条目无效：{id}")));
        }
        validate_theme(&root.join(manifest), id, root)?;
    }
    let actual: HashSet<_> = fs::read_dir(root.join("themes"))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        })
        .map(|entry| format!("themes/{}", entry.file_name().to_string_lossy()))
        .collect();
    if actual != manifests {
        return Err(AppError::Message("主题索引与 themes 目录不一致。".into()));
    }
    Ok(())
}

fn validate_theme(path: &Path, expected_id: &str, repository: &Path) -> Result<()> {
    let root: Value = serde_json::from_slice(&fs::read(path)?)?;
    closed_object(
        &root,
        &[
            "$schema",
            "schemaVersion",
            "version",
            "displayName",
            "codeThemeId",
            "category",
            "description",
            "author",
            "variant",
            "previewImage",
            "theme",
            "home",
            "copy",
        ],
        &[
            "schemaVersion",
            "version",
            "displayName",
            "codeThemeId",
            "category",
            "description",
            "author",
            "variant",
            "previewImage",
            "theme",
            "home",
        ],
        "theme",
    )?;
    if root.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err(AppError::Message(format!(
            "远程主题必须使用 schemaVersion 1：{expected_id}"
        )));
    }
    let id = required_text(&root, "codeThemeId", 64)?;
    if id != expected_id || !valid_catalog_id(id) {
        return Err(AppError::Message(format!(
            "主题 ID 与索引不一致：{expected_id}"
        )));
    }
    let version = required_text(&root, "version", 64)?;
    if semver::Version::parse(version).is_err() {
        return Err(AppError::Message(format!("主题版本无效：{expected_id}")));
    }
    required_text(&root, "displayName", 60)?;
    required_text(&root, "description", 120)?;
    required_text(&root, "author", 60)?;
    if !["人物", "动漫", "游戏", "风景", "极简", "节日", "其他"]
        .contains(&required_text(&root, "category", 8)?)
    {
        return Err(AppError::Message(format!("主题分类无效：{expected_id}")));
    }
    if !["light", "dark"].contains(&required_text(&root, "variant", 8)?) {
        return Err(AppError::Message(format!("主题模式无效：{expected_id}")));
    }
    let theme = root
        .get("theme")
        .ok_or_else(|| AppError::Message("主题缺少 theme。".into()))?;
    closed_object(
        theme,
        &[
            "accent",
            "contrast",
            "fonts",
            "ink",
            "opaqueWindows",
            "semanticColors",
            "surface",
            "backgroundImage",
            "backgroundPosition",
            "backgroundFocus",
            "visualIntensity",
            "effects",
            "logoImage",
            "backgroundImageOpacity",
            "backgroundImageBlur",
        ],
        &["accent", "ink", "surface"],
        "theme.theme",
    )?;
    for key in ["accent", "ink", "surface"] {
        validate_color(required_text(theme, key, 7)?, key)?;
    }
    if let Some(fonts) = theme.get("fonts") {
        closed_object(fonts, &["ui", "code", "display"], &[], "theme.fonts")?;
        for key in ["ui", "code", "display"] {
            if let Some(value) = fonts.get(key) {
                validate_font_stack(value.as_str().ok_or_else(|| {
                    AppError::Message(format!("theme.fonts.{key} 必须是字符串。"))
                })?)?;
            }
        }
    }
    if let Some(semantic) = theme.get("semanticColors") {
        closed_object(
            semantic,
            &["diffAdded", "diffRemoved", "skill"],
            &[],
            "semanticColors",
        )?;
        for key in ["diffAdded", "diffRemoved", "skill"] {
            if let Some(value) = semantic.get(key).and_then(Value::as_str) {
                validate_color(value, key)?;
            }
        }
    }
    if let Some(value) = theme.get("contrast").and_then(Value::as_f64)
        && !(0.0..=100.0).contains(&value)
    {
        return Err(AppError::Message("theme.contrast 越界。".into()));
    }
    if let Some(value) = theme.get("backgroundImageOpacity").and_then(Value::as_f64)
        && !(0.0..=1.0).contains(&value)
    {
        return Err(AppError::Message("背景透明度越界。".into()));
    }
    if let Some(value) = theme.get("backgroundImageBlur").and_then(Value::as_f64)
        && !(0.0..=24.0).contains(&value)
    {
        return Err(AppError::Message("背景模糊度越界。".into()));
    }
    if let Some(value) = theme.get("backgroundPosition") {
        let value = value
            .as_str()
            .ok_or_else(|| AppError::Message("背景定位值必须是字符串。".into()))?;
        if ![
            "center",
            "center top",
            "center 20%",
            "center 30%",
            "center 40%",
            "center bottom",
            "left center",
            "right center",
        ]
        .contains(&value)
        {
            return Err(AppError::Message("背景定位值无效。".into()));
        }
    }
    if let Some(focus) = theme.get("backgroundFocus") {
        validate_background_focus(focus)?;
    }
    if let Some(value) = theme.get("visualIntensity") {
        let value = value
            .as_str()
            .ok_or_else(|| AppError::Message("视觉强度必须是字符串。".into()))?;
        if !["clear", "balanced", "immersive"].contains(&value) {
            return Err(AppError::Message("视觉强度无效。".into()));
        }
    }
    if let Some(effects) = theme.get("effects") {
        closed_object(
            effects,
            &["ambient", "intensity", "overlay", "composerAccent"],
            &[],
            "theme.effects",
        )?;
        if let Some(value) = effects.get("ambient") {
            let value = value
                .as_str()
                .ok_or_else(|| AppError::Message("氛围特效必须是字符串。".into()))?;
            if !["none", "rain", "particles", "storm"].contains(&value) {
                return Err(AppError::Message("氛围特效无效。".into()));
            }
        }
        if let Some(value) = effects.get("intensity") {
            let value = value
                .as_str()
                .ok_or_else(|| AppError::Message("特效强度必须是字符串。".into()))?;
            if !["subtle", "balanced", "vivid"].contains(&value) {
                return Err(AppError::Message("特效强度无效。".into()));
            }
        }
        if let Some(overlay) = effects.get("overlay") {
            closed_object(
                overlay,
                &["image", "triggers", "position", "widthPercent"],
                &["image", "triggers"],
                "theme.effects.overlay",
            )?;
            validate_effect_triggers(overlay.get("triggers").unwrap())?;
            if let Some(position) = overlay.get("position") {
                validate_background_focus(position)?;
            }
            if let Some(width) = overlay.get("widthPercent") {
                let width = width
                    .as_u64()
                    .ok_or_else(|| AppError::Message("瞬时叠加素材宽度必须是整数。".into()))?;
                if !(10..=80).contains(&width) {
                    return Err(AppError::Message("瞬时叠加素材宽度越界。".into()));
                }
            }
        }
        if let Some(accent) = effects.get("composerAccent") {
            closed_object(
                accent,
                &["image", "triggers", "widthPx"],
                &["image", "triggers"],
                "theme.effects.composerAccent",
            )?;
            validate_effect_triggers(accent.get("triggers").unwrap())?;
            if let Some(width) = accent.get("widthPx") {
                let width = width
                    .as_u64()
                    .ok_or_else(|| AppError::Message("输入框装饰素材宽度必须是整数。".into()))?;
                if !(48..=240).contains(&width) {
                    return Err(AppError::Message("输入框装饰素材宽度越界。".into()));
                }
            }
        }
    }
    validate_asset_path(
        repository,
        path,
        required_text(&root, "previewImage", 180)?,
        "previews",
    )?;
    for (key, directory) in [("backgroundImage", "backgrounds"), ("logoImage", "logos")] {
        if let Some(value) = theme.get(key).and_then(Value::as_str) {
            validate_asset_path(repository, path, value, directory)?;
        }
    }
    if let Some(effects) = theme.get("effects") {
        for role in ["overlay", "composerAccent"] {
            if let Some(value) = effects
                .get(role)
                .and_then(|value| value.get("image"))
                .and_then(Value::as_str)
            {
                validate_asset_path(repository, path, value, "effects")?;
            }
        }
    }
    let home = root.get("home").unwrap();
    closed_object(
        home,
        &[
            "brand",
            "eyebrow",
            "badge",
            "title",
            "subtitle",
            "footerNote",
            "composerHint",
            "tags",
            "sidebarLabels",
            "quickActions",
            "pet",
        ],
        &["brand", "title", "quickActions"],
        "home",
    )?;
    required_text(home, "brand", 200)?;
    required_text(home, "title", 200)?;
    let actions = home
        .get("quickActions")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Message("quickActions 必须是数组。".into()))?;
    if actions.len() != 4 {
        return Err(AppError::Message("quickActions 必须包含 4 项。".into()));
    }
    for action in actions {
        closed_object(
            action,
            &["icon", "title", "description", "prompt"],
            &["title", "prompt"],
            "quickActions[]",
        )?;
        required_text(action, "title", 200)?;
        required_text(action, "prompt", 1000)?;
    }
    if let Some(pet) = home.get("pet") {
        closed_object(pet, &["image", "alt", "size"], &["image"], "pet")?;
        validate_asset_path(repository, path, required_text(pet, "image", 180)?, "pets")?;
    }
    Ok(())
}

fn closed_object(value: &Value, allowed: &[&str], required: &[&str], path: &str) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::Message(format!("{path} 必须是对象。")))?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(AppError::Message(format!("{path} 包含未知字段：{key}")));
    }
    if let Some(key) = required.iter().find(|key| !object.contains_key(**key)) {
        return Err(AppError::Message(format!("{path} 缺少字段：{key}")));
    }
    Ok(())
}

fn validate_background_focus(focus: &Value) -> Result<()> {
    closed_object(focus, &["x", "y"], &["x", "y"], "theme.backgroundFocus")?;
    for axis in ["x", "y"] {
        let value = focus
            .get(axis)
            .and_then(Value::as_u64)
            .ok_or_else(|| AppError::Message(format!("背景焦点 {axis} 必须是 0–100 的整数。")))?;
        if value > 100 {
            return Err(AppError::Message(format!(
                "背景焦点 {axis} 必须是 0–100 的整数。"
            )));
        }
    }
    Ok(())
}

fn validate_effect_triggers(value: &Value) -> Result<()> {
    let triggers = value
        .as_array()
        .ok_or_else(|| AppError::Message("特效触发器必须是数组。".into()))?;
    if triggers.is_empty() || triggers.len() > 2 {
        return Err(AppError::Message("特效触发器数量无效。".into()));
    }
    let mut seen = std::collections::HashSet::new();
    for trigger in triggers {
        let trigger = trigger
            .as_str()
            .ok_or_else(|| AppError::Message("特效触发器必须是字符串。".into()))?;
        if !["task-start", "message-send"].contains(&trigger) || !seen.insert(trigger) {
            return Err(AppError::Message("特效触发器无效或重复。".into()));
        }
    }
    Ok(())
}
fn required_text<'a>(value: &'a Value, key: &str, max: usize) -> Result<&'a str> {
    let text = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Message(format!("{key} 必须是字符串。")))?;
    if text.is_empty() || text.chars().count() > max {
        return Err(AppError::Message(format!("{key} 长度无效。")));
    }
    Ok(text)
}
fn valid_catalog_id(value: &str) -> bool {
    value.len() >= 2
        && value.len() <= 64
        && value
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !value.ends_with('-')
}
fn valid_library_id(value: &str) -> bool {
    value.len() >= 2
        && value.len() <= 128
        && value
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && !value.ends_with(['-', '.'])
        && !value.contains("..")
}
fn validate_color(value: &str, key: &str) -> Result<()> {
    if value.len() != 7
        || !value.starts_with('#')
        || !value[1..].chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(AppError::Message(format!("{key} 颜色无效。")));
    }
    Ok(())
}
fn validate_font_stack(value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.chars().count() <= 300
        && value.split(',').all(|family| {
            let family = family.trim();
            if family.is_empty() {
                return false;
            }
            let quoted = family.len() >= 2
                && ((family.starts_with('"') && family.ends_with('"'))
                    || (family.starts_with('\'') && family.ends_with('\'')));
            let unquoted = if quoted {
                &family[1..family.len() - 1]
            } else {
                family
            };
            !unquoted.is_empty()
                && !unquoted.contains(['"', '\''])
                && unquoted
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == ' ' || "._-".contains(c))
        });
    if valid {
        Ok(())
    } else {
        Err(AppError::Message("主题字体配置无效。".into()))
    }
}
fn validate_asset_path(
    repository: &Path,
    manifest: &Path,
    relative: &str,
    directory: &str,
) -> Result<()> {
    let normalized = relative.replace('\\', "/");
    if !normalized.starts_with(&format!("../{directory}/"))
        || normalized[4 + directory.len()..].contains('/')
        || normalized.contains("/../")
    {
        return Err(AppError::Message(format!("主题资源路径无效：{relative}")));
    }
    let path = manifest.parent().unwrap().join(relative).canonicalize()?;
    let root = repository.canonicalize()?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err(AppError::Message(format!(
            "主题资源越界或不存在：{relative}"
        )));
    }
    let bytes = fs::read(&path)?;
    let image = image::load_from_memory(&bytes)
        .map_err(|error| AppError::Message(format!("主题图片无法解码：{error}")))?;
    let (max_bytes, max_dimension) = match directory {
        "previews" => (2 * 1024 * 1024, 2400),
        "backgrounds" => (16 * 1024 * 1024, 8192),
        "effects" => (4 * 1024 * 1024, 4096),
        _ => (20 * 1024 * 1024, 8192),
    };
    if bytes.len() > max_bytes || image.width() > max_dimension || image.height() > max_dimension {
        return Err(AppError::Message(format!(
            "主题图片超过 {directory} 的大小或尺寸限制：{relative}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_font_stacks() {
        assert!(validate_font_stack("Inter, system-ui, sans-serif").is_ok());
        assert!(validate_font_stack("\"Microsoft YaHei UI\", sans-serif").is_ok());
        assert!(validate_font_stack("sans-serif; } body { display: none").is_err());
        assert!(validate_font_stack("url(https://example.com/font.woff)").is_err());
        assert!(validate_font_stack("\"").is_err());
    }

    #[test]
    fn rejects_background_fit_in_public_theme_contract() {
        let theme = serde_json::json!({"backgroundFit": "cover"});
        let error =
            closed_object(&theme, &["accent", "ink", "surface"], &[], "theme.theme").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("theme.theme 包含未知字段：backgroundFit")
        );
    }

    #[test]
    fn validates_precise_background_focus() {
        validate_background_focus(&serde_json::json!({"x": 72, "y": 24})).unwrap();
        assert!(validate_background_focus(&serde_json::json!({"x": 101, "y": 24})).is_err());
        assert!(validate_background_focus(&serde_json::json!({"x": 50.5, "y": 24})).is_err());
        assert!(validate_background_focus(&serde_json::json!({"x": 50, "y": 24, "z": 1})).is_err());
    }

    #[test]
    fn validates_safe_effect_triggers() {
        validate_effect_triggers(&serde_json::json!(["task-start", "message-send"])).unwrap();
        assert!(
            validate_effect_triggers(&serde_json::json!(["task-start", "task-start"])).is_err()
        );
        assert!(validate_effect_triggers(&serde_json::json!(["click-anywhere"])).is_err());
        assert!(validate_effect_triggers(&serde_json::json!([])).is_err());
    }

    fn remote_catalog_fixture() -> RemoteCatalog {
        let preview = RemoteResource {
            path: "previews/example-theme.png".into(),
            sha256: "a".repeat(64),
            size: 42,
        };
        RemoteCatalog {
            schema_version: 2,
            name: "Test Catalog".into(),
            revision: "2026-07-17T12:00:00Z".into(),
            themes: vec![RemoteTheme {
                id: "example-theme".into(),
                version: "1.0.0".into(),
                name: "Example".into(),
                description: "Example theme".into(),
                category: "极简".into(),
                variant: "light".into(),
                manifest: RemoteResource {
                    path: "themes/example-theme.json".into(),
                    sha256: "b".repeat(64),
                    size: 128,
                },
                preview: preview.clone(),
                assets: vec![preview],
            }],
        }
    }

    #[test]
    fn validates_content_addressed_remote_catalog() {
        let catalog = remote_catalog_fixture();
        validate_remote_catalog(&catalog).unwrap();
        let mut invalid = catalog.clone();
        invalid.themes[0].preview.sha256 = "c".repeat(64);
        assert!(validate_remote_catalog(&invalid).is_err());
        let mut duplicate = catalog;
        duplicate.themes.push(duplicate.themes[0].clone());
        assert!(validate_remote_catalog(&duplicate).is_err());
    }

    #[test]
    fn theme_library_state_is_stable_and_strict() {
        let mut state = ThemeLibraryState::default();
        state
            .downloaded_versions
            .insert("example-theme".into(), "1.2.0".into());
        state.subscriptions.insert("example-theme".into());
        let bytes = serde_json::to_vec(&state).unwrap();
        let decoded: ThemeLibraryState = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            decoded
                .downloaded_versions
                .get("example-theme")
                .map(String::as_str),
            Some("1.2.0")
        );
        assert!(decoded.subscriptions.contains("example-theme"));
        assert!(serde_json::from_str::<ThemeLibraryState>(r#"{"unknown":true}"#).is_err());
    }

    #[test]
    fn deleting_theme_removes_installed_cached_and_compiled_files() {
        let temporary = tempfile::tempdir().unwrap();
        let installed_root = temporary.path().join("installed");
        let cache_root = temporary.path().join("cache");
        let state_root = temporary.path().join("state");
        let theme_id = "example-theme";
        fs::create_dir_all(installed_root.join(theme_id).join("1.0.0")).unwrap();
        fs::write(
            installed_root
                .join(theme_id)
                .join("1.0.0")
                .join("theme.json"),
            b"{}",
        )
        .unwrap();
        for relative in [
            "themes/example-theme.json",
            "previews/example-theme.png",
            "backgrounds/example-theme.jpg",
        ] {
            let path = cache_root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"resource").unwrap();
        }
        fs::create_dir_all(&state_root).unwrap();
        fs::write(
            state_root.join("current-theme.json"),
            br#"{"codeThemeId":"example-theme"}"#,
        )
        .unwrap();
        fs::write(state_root.join("codex-theme.css"), b"css").unwrap();
        fs::write(state_root.join("codex-theme.js"), b"js").unwrap();

        let mut theme = remote_catalog_fixture().themes.remove(0);
        theme.assets.push(RemoteResource {
            path: "backgrounds/example-theme.jpg".into(),
            sha256: "c".repeat(64),
            size: 128,
        });
        assert!(delete_theme_files(theme_id, &installed_root, &cache_root, Some(&theme)).unwrap());
        clear_compiled_state(theme_id, &state_root).unwrap();

        assert!(!installed_root.join(theme_id).exists());
        assert!(!cache_root.join("themes/example-theme.json").exists());
        assert!(!cache_root.join("previews/example-theme.png").exists());
        assert!(!cache_root.join("backgrounds/example-theme.jpg").exists());
        assert!(!state_root.join("current-theme.json").exists());
        assert!(!state_root.join("codex-theme.css").exists());
        assert!(!state_root.join("codex-theme.js").exists());
    }

    #[tokio::test]
    #[ignore = "requires the public Codex-Skin-Store repository"]
    async fn syncs_and_validates_official_catalog() {
        let temporary = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("CODEX_THEME_STORE_DATA_DIR", temporary.path());
        }
        let result = sync("github").await.unwrap();
        assert!(result.theme_count >= 5);
        assert_eq!(crate::catalog::load().unwrap().len(), result.theme_count);
        let first = load_remote_catalog().unwrap().unwrap().themes[0].id.clone();
        let preview = ensure_preview(&first).await.unwrap();
        assert!(preview.is_file());
        ensure_theme(&first).await.unwrap();
        set_subscription(&first, true).unwrap();
        assert_eq!(sync_subscriptions().await.unwrap(), 0);
        let summary = crate::catalog::find(&first).unwrap();
        assert!(summary.subscribed);
        assert_eq!(summary.installed_version, summary.remote_version);
        assert_eq!(
            crate::compiler::compile(&first).unwrap().theme_id,
            Some(first)
        );
        assert_eq!(
            sync("github").await.unwrap().theme_count,
            result.theme_count
        );
        unsafe {
            std::env::remove_var("CODEX_THEME_STORE_DATA_DIR");
        }
    }
}
