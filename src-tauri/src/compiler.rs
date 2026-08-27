use crate::{
    catalog,
    cdp::Payload,
    error::{AppError, Result},
    paths,
};
use atomicwrites::{AllowOverwrite, AtomicFile};
use base64::Engine;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub fn compile(theme_id: &str) -> Result<Payload> {
    let summary = catalog::find(theme_id)?;
    let raw = fs::read_to_string(&summary.manifest_path)?;
    let root: Value = serde_json::from_str(&raw)?;
    let package = root.get("packageVersion").and_then(Value::as_u64) == Some(1);
    let theme_value = if package {
        let colors = root.get("colors").cloned().unwrap_or_else(|| json!({}));
        json!({"accent":colors.get("accent"),"ink":colors.get("text"),"surface":colors.get("panel"),"semanticColors":{"diffAdded":colors.get("highlight"),"diffRemoved":colors.get("accentAlt"),"skill":colors.get("secondary")},"backgroundImageOpacity":0.22,"effects":root.get("effects")})
    } else {
        root.get("theme").cloned().unwrap_or(Value::Null)
    };
    let theme = theme_value
        .as_object()
        .ok_or_else(|| AppError::Message("主题缺少 theme 对象。".into()))?;
    let home = if package {
        json!({
        "brand":root.get("projectLabel").or_else(||root.get("name")),
        "eyebrow":root.get("brandSubtitle"),"title":root.get("tagline").or_else(||root.get("name")),
        "subtitle":root.get("quote").or_else(||root.get("description")),
        "quickActions":[
            {"icon":"01","title":"解释代码","description":"梳理模块与关键流程","prompt":"请解释当前项目的核心架构与关键代码。"},
            {"icon":"02","title":"修复问题","description":"定位并修复一个明确问题","prompt":"请检查当前项目并修复最重要的问题。"},
            {"icon":"03","title":"添加测试","description":"补充可靠的自动化验证","prompt":"请为当前改动补充自动化测试。"},
            {"icon":"04","title":"审查改动","description":"评估风险和回归点","prompt":"请审查当前改动并指出风险。"}
        ]})
    } else {
        root.get("home").cloned().unwrap_or_else(|| json!({}))
    };
    let copy = root.get("copy").cloned().unwrap_or_else(|| json!({}));
    let manifest_dir = summary
        .manifest_path
        .parent()
        .ok_or_else(|| AppError::Message("主题清单目录无效。".into()))?;
    let accent = color(theme.get("accent"), "#d96f4d")?;
    let ink = color(theme.get("ink"), "#20231f")?;
    let surface = color(theme.get("surface"), "#f5f6f3")?;
    let variant = root
        .get("variant")
        .and_then(Value::as_str)
        .unwrap_or("light");
    let dark = variant == "dark";
    let semantic = theme.get("semanticColors").and_then(Value::as_object);
    let added = color(semantic.and_then(|v| v.get("diffAdded")), "#00c853")?;
    let removed = color(semantic.and_then(|v| v.get("diffRemoved")), "#ff5f38")?;
    let skill = color(semantic.and_then(|v| v.get("skill")), "#cc7d5e")?;
    let under = mix(
        &surface,
        if dark { "#000000" } else { "#ffffff" },
        if dark { 0.18 } else { 0.34 },
    )?;
    let elevated = mix(
        &surface,
        if dark { "#ffffff" } else { "#000000" },
        if dark { 0.08 } else { 0.04 },
    )?;
    let secondary = alpha(&ink, if dark { 0.72 } else { 0.68 })?;
    let tertiary = alpha(&ink, if dark { 0.52 } else { 0.48 })?;
    let border = alpha(&accent, if dark { 0.34 } else { 0.22 })?;
    let shadow = if dark { "#000000" } else { ink.as_str() };
    let (accent_r, accent_g, accent_b) = rgb(&accent)?;
    let accent_luma =
        0.2126 * f64::from(accent_r) + 0.7152 * f64::from(accent_g) + 0.0722 * f64::from(accent_b);
    let button_text = if accent_luma > 150.0 {
        "#111111"
    } else {
        "#ffffff"
    };
    let background_path = if package {
        package_asset(root.get("image"), manifest_dir)?
    } else {
        asset(theme.get("backgroundImage"), manifest_dir, "backgrounds")?
    };
    let background = data_url(background_path)?;
    let effects = theme.get("effects").and_then(Value::as_object);
    let overlay = effects
        .and_then(|value| value.get("overlay"))
        .and_then(Value::as_object);
    let composer_accent = effects
        .and_then(|value| value.get("composerAccent"))
        .and_then(Value::as_object);
    let effect_asset = |value: Option<&Value>| {
        if package {
            package_asset(value, manifest_dir)
        } else {
            asset(value, manifest_dir, "effects")
        }
    };
    let overlay_image = data_url(effect_asset(overlay.and_then(|value| value.get("image")))?)?;
    let composer_accent_image = data_url(effect_asset(
        composer_accent.and_then(|value| value.get("image")),
    )?)?;
    let effects_config = json!({
        "ambient": effects.and_then(|value| value.get("ambient")).and_then(Value::as_str).unwrap_or("none"),
        "intensity": effects.and_then(|value| value.get("intensity")).and_then(Value::as_str).unwrap_or("balanced"),
        "overlay": overlay.map(|value| json!({
            "image": overlay_image,
            "triggers": value.get("triggers"),
            "position": value.get("position"),
            "widthPercent": value.get("widthPercent")
        })),
        "composerAccent": composer_accent.map(|value| json!({
            "image": composer_accent_image,
            "triggers": value.get("triggers"),
            "widthPx": value.get("widthPx")
        }))
    });
    let logo = if package {
        None
    } else {
        data_url(asset(theme.get("logoImage"), manifest_dir, "logos")?)?
    };
    let pet = if package {
        None
    } else {
        data_url(asset(
            home.get("pet").and_then(|v| v.get("image")),
            manifest_dir,
            "pets",
        )?)?
    };
    let ui_font = font_stack(
        theme
            .get("fonts")
            .and_then(|v| v.get("ui"))
            .and_then(Value::as_str)
            .unwrap_or("Inter, system-ui, sans-serif"),
    )?;
    let code_font = font_stack(
        theme
            .get("fonts")
            .and_then(|v| v.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("ui-monospace, monospace"),
    )?;
    let display_font = font_stack(
        theme
            .get("fonts")
            .and_then(|v| v.get("display"))
            .and_then(Value::as_str)
            .unwrap_or(ui_font),
    )?;
    let opacity = theme
        .get("backgroundImageOpacity")
        .and_then(Value::as_f64)
        .unwrap_or(0.18)
        .clamp(0.0, 1.0);
    let blur = theme
        .get("backgroundImageBlur")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 24.0);
    let default_background_position = match summary.id.as_str() {
        "dilraba-star" => "center top",
        "enfp-pop" => "center 20%",
        "zhu-xudan-racing" => "center 30%",
        _ => "center",
    };
    let background_position = if let Some(focus) = theme.get("backgroundFocus") {
        let x = focus
            .get("x")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .min(100);
        let y = focus
            .get("y")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .min(100);
        format!("{x}% {y}%")
    } else {
        match theme.get("backgroundPosition").and_then(Value::as_str) {
            Some(value)
                if [
                    "center",
                    "center top",
                    "center 20%",
                    "center 30%",
                    "center 40%",
                    "center bottom",
                    "left center",
                    "right center",
                ]
                .contains(&value) =>
            {
                value.to_owned()
            }
            _ => default_background_position.to_owned(),
        }
    };
    let visual_intensity = theme
        .get("visualIntensity")
        .and_then(Value::as_str)
        .unwrap_or("balanced");
    let intensity = match visual_intensity {
        "clear" => [
            ".78", ".58", ".42", ".90", ".42", ".88", ".34", ".96", ".94", ".94", ".78",
        ],
        "immersive" => [
            ".34", ".16", ".08", ".56", ".12", ".54", ".08", ".76", ".78", ".74", ".46",
        ],
        _ => [
            ".58", ".28", ".16", ".78", ".25", ".76", ".18", ".86", ".82", ".88", ".62",
        ],
    };
    // `backgroundFit` is intentionally not part of the public theme manifest: Codex
    // rejects unknown fields in `theme.theme`. Curated hero art still uses a safe
    // compiler-side default, which is emitted only into the injected runtime config.
    let background_fit = if matches!(
        summary.id.as_str(),
        "dilraba-star" | "enfp-pop" | "jackson-sage" | "kun-stage" | "zhu-xudan-racing"
    ) {
        "cover"
    } else {
        "smart"
    };

    let mut css = CSS_TEMPLATE.to_owned();
    for (key, value) in [
        ("ID", summary.id.as_str()),
        ("VARIANT", variant),
        ("ACCENT", &accent),
        ("INK", &ink),
        ("SURFACE", &surface),
        ("UNDER", &under),
        ("ELEVATED", &elevated),
        ("SECONDARY", &secondary),
        ("TERTIARY", &tertiary),
        ("BORDER", &border),
        ("SHADOW", shadow),
        ("BUTTON_TEXT", button_text),
        ("ADDED", &added),
        ("REMOVED", &removed),
        ("SKILL", &skill),
        ("UI_FONT", ui_font),
        ("CODE_FONT", code_font),
        ("DISPLAY_FONT", display_font),
        ("BACKGROUND_POSITION", background_position.as_str()),
        ("HOME_SHELL_STRONG", intensity[0]),
        ("HOME_SHELL_MID", intensity[1]),
        ("HOME_SHELL_WEAK", intensity[2]),
        ("HERO_STRONG", intensity[3]),
        ("HERO_MID", intensity[4]),
        ("HERO_OVERLAY_STRONG", intensity[5]),
        ("HERO_OVERLAY_MID", intensity[6]),
        ("PANEL_ALPHA", intensity[7]),
        ("TASK_ALPHA", intensity[8]),
        ("SIDEBAR_STRONG", intensity[9]),
        ("SIDEBAR_MID", intensity[10]),
    ] {
        css = css.replace(&format!("{{{{{key}}}}}"), value);
    }
    css = css
        .replace("{{COLOR_SCHEME}}", variant)
        .replace("{{BACKGROUND}}", background.as_deref().unwrap_or("none"))
        .replace("{{BACKGROUND_OPACITY}}", &opacity.to_string())
        .replace("{{BACKGROUND_BLUR}}", &format!("{blur}px"));

    let config = json!({
        "themeId": summary.id,
        "home": home,
        "copy": copy,
        "background": background,
        "backgroundFit": background_fit,
        "effects": effects_config,
        "logo": logo,
        "pet": pet
    });
    let javascript = format!(
        "const __codexSkinConfig={};\n{}",
        serde_json::to_string(&config)?,
        JS_TEMPLATE
    );
    let payload = Payload {
        css,
        javascript,
        theme_id: Some(summary.id),
    };
    write_state(&raw, &payload)?;
    Ok(payload)
}

fn color(value: Option<&Value>, fallback: &str) -> Result<String> {
    let value = value.and_then(Value::as_str).unwrap_or(fallback);
    if value.len() == 7
        && value.starts_with('#')
        && value[1..].chars().all(|c| c.is_ascii_hexdigit())
    {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(AppError::Message(format!("主题颜色无效：{value}")))
    }
}
fn font_stack(value: &str) -> Result<&str> {
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
        Ok(value)
    } else {
        Err(AppError::Message("主题字体配置无效。".into()))
    }
}
fn rgb(value: &str) -> Result<(u8, u8, u8)> {
    Ok((
        u8::from_str_radix(&value[1..3], 16).map_err(|_| AppError::Message("颜色无效。".into()))?,
        u8::from_str_radix(&value[3..5], 16).map_err(|_| AppError::Message("颜色无效。".into()))?,
        u8::from_str_radix(&value[5..7], 16).map_err(|_| AppError::Message("颜色无效。".into()))?,
    ))
}
fn mix(left: &str, right: &str, amount: f64) -> Result<String> {
    let (lr, lg, lb) = rgb(left)?;
    let (rr, rg, rb) = rgb(right)?;
    let blend = |a: u8, b: u8| ((a as f64) * (1.0 - amount) + (b as f64) * amount).round() as u8;
    Ok(format!(
        "#{:02x}{:02x}{:02x}",
        blend(lr, rr),
        blend(lg, rg),
        blend(lb, rb)
    ))
}
fn alpha(value: &str, amount: f64) -> Result<String> {
    let (r, g, b) = rgb(value)?;
    Ok(format!("rgb({r} {g} {b} / {amount:.3})"))
}

fn asset(value: Option<&Value>, manifest_dir: &Path, expected: &str) -> Result<Option<PathBuf>> {
    let Some(relative) = value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let normalized = relative.replace('\\', "/");
    if !normalized.starts_with(&format!("../{expected}/")) || normalized.contains("/../") {
        return Err(AppError::Message(format!("主题资源路径无效：{relative}")));
    }
    let path = manifest_dir.join(relative).canonicalize()?;
    if !path.is_file() {
        return Err(AppError::Message(format!(
            "主题资源不存在：{}",
            path.display()
        )));
    }
    Ok(Some(path))
}

fn package_asset(value: Option<&Value>, manifest_dir: &Path) -> Result<Option<PathBuf>> {
    let Some(name) = value.and_then(Value::as_str) else {
        return Ok(None);
    };
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || Path::new(name).file_name().and_then(|v| v.to_str()) != Some(name)
    {
        return Err(AppError::Message(format!("签名主题资源路径无效：{name}")));
    }
    let path = manifest_dir.join(name).canonicalize()?;
    if !path.is_file() {
        return Err(AppError::Message(format!(
            "签名主题资源不存在：{}",
            path.display()
        )));
    }
    Ok(Some(path))
}

fn data_url(path: Option<PathBuf>) -> Result<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let extension = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "avif" => "image/avif",
        _ => return Err(AppError::Message("不支持的主题图片格式。".into())),
    };
    let bytes = fs::read(&path)?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err(AppError::Message("主题图片超过 20 MB 限制。".into()));
    }
    image::load_from_memory(&bytes)
        .map_err(|error| AppError::Message(format!("主题图片无法解码：{error}")))?;
    Ok(Some(format!(
        "url(\"data:{mime};base64,{}\")",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )))
}

fn write_state(raw: &str, payload: &Payload) -> Result<()> {
    let root = paths::state_root()?;
    fs::create_dir_all(&root)?;
    for (name, content) in [
        ("codex-theme.css", payload.css.as_str()),
        ("codex-theme.js", payload.javascript.as_str()),
        ("current-theme.json", raw),
    ] {
        let file = AtomicFile::new(root.join(name), AllowOverwrite);
        file.write(|handle| handle.write_all(content.as_bytes()))
            .map_err(|error| AppError::Message(format!("写入主题状态失败：{error}")))?;
    }
    Ok(())
}

const CSS_TEMPLATE: &str = r#"
:root,html[data-codex-window-type="electron"],html[data-codex-window-type="electron"] body {
  color-scheme:{{COLOR_SCHEME}}!important; --codex-theme-id:"{{ID}}"; --codex-theme-accent:{{ACCENT}};
  --color-background-surface:{{SURFACE}}!important; --color-background-surface-under:{{UNDER}}!important; --color-background-panel:{{SURFACE}}!important; --color-background-editor-opaque:{{SURFACE}}!important;
  --color-background-elevated-primary:{{ELEVATED}}!important; --color-background-elevated-secondary:{{ELEVATED}}!important;
  --color-background-elevated-primary-opaque:{{ELEVATED}}!important; --color-background-elevated-secondary-opaque:{{ELEVATED}}!important;
  --color-background-control:{{ELEVATED}}!important; --color-background-control-opaque:{{ELEVATED}}!important;
  --color-background-button-secondary:{{ELEVATED}}!important; --color-background-button-tertiary-hover:rgb(from {{ACCENT}} r g b / .14)!important;
  --color-background-button-primary:{{ACCENT}}!important; --color-background-accent:rgb(from {{ACCENT}} r g b / .14)!important;
  --color-background-accent-hover:rgb(from {{ACCENT}} r g b / .20)!important; --color-background-accent-active:rgb(from {{ACCENT}} r g b / .26)!important;
  --color-text-foreground:{{INK}}!important; --color-text-foreground-secondary:{{SECONDARY}}!important; --color-text-foreground-tertiary:{{TERTIARY}}!important; --color-text-accent:{{ACCENT}}!important;
  --color-text-button-primary:{{BUTTON_TEXT}}!important; --color-text-button-secondary:{{INK}}!important; --color-text-button-tertiary:{{INK}}!important; --color-text-on-accent:{{BUTTON_TEXT}}!important;
  --color-icon-primary:{{INK}}!important; --color-icon-secondary:{{SECONDARY}}!important; --color-icon-tertiary:{{TERTIARY}}!important; --color-icon-accent:{{ACCENT}}!important;
  --color-border:{{BORDER}}!important; --color-border-light:rgb(from {{INK}} r g b / .12)!important; --color-border-heavy:rgb(from {{INK}} r g b / .20)!important; --color-border-focus:{{ACCENT}}!important; --color-accent-blue:{{ACCENT}}!important;
  --color-accent-green:{{ADDED}}!important; --color-accent-orange:{{REMOVED}}!important; --color-accent-purple:{{SKILL}}!important;
  --color-token-main-surface-primary:{{SURFACE}}!important; --color-token-side-bar-background:{{UNDER}}!important;
  --color-token-input-background:{{ELEVATED}}!important; --color-token-input-foreground:{{INK}}!important; --color-token-input-placeholder-foreground:{{TERTIARY}}!important; --color-token-input-border:{{BORDER}}!important;
  --color-token-dropdown-background:{{ELEVATED}}!important; --color-token-dropdown-foreground:{{INK}}!important; --color-token-menu-background:{{ELEVATED}}!important; --color-token-menu-foreground:{{INK}}!important; --color-token-menu-border:{{BORDER}}!important;
  --color-token-editor-widget-background:{{ELEVATED}}!important; --color-token-diff-surface:color-mix(in srgb,{{SURFACE}} 92%,{{INK}})!important;
  --color-token-text-primary:{{INK}}!important; --color-token-text-secondary:{{SECONDARY}}!important; --color-token-text-tertiary:{{TERTIARY}}!important; --color-token-conversation-body:{{SECONDARY}}!important;
  --color-token-text-preformat-foreground:{{INK}}!important; --color-token-text-preformat-background:rgb(from {{INK}} r g b / .08)!important; --color-token-text-code-block-background:rgb(from {{INK}} r g b / .08)!important;
  --color-token-border:{{BORDER}}!important; --color-token-border-default:{{BORDER}}!important; --color-token-border-light:rgb(from {{INK}} r g b / .10)!important; --color-token-border-heavy:rgb(from {{INK}} r g b / .18)!important;
  --vscode-font-family:{{UI_FONT}}!important; --font-sans-default:{{UI_FONT}}!important; --font-mono-default:{{CODE_FONT}}!important;
  --vscode-foreground:{{INK}}!important; --vscode-descriptionForeground:{{SECONDARY}}!important; --vscode-focusBorder:{{ACCENT}}!important;
  --vscode-editor-background:{{SURFACE}}!important; --vscode-editor-foreground:{{INK}}!important; --vscode-sideBar-background:{{UNDER}}!important;
  --vscode-input-background:{{ELEVATED}}!important; --vscode-input-foreground:{{INK}}!important; --vscode-input-placeholderForeground:{{TERTIARY}}!important;
  --vscode-dropdown-background:{{ELEVATED}}!important; --vscode-dropdown-foreground:{{INK}}!important; --vscode-menu-background:{{ELEVATED}}!important; --vscode-menu-foreground:{{INK}}!important; --vscode-menu-border:{{BORDER}}!important;
  --vscode-widget-background:{{ELEVATED}}!important; --vscode-widget-foreground:{{INK}}!important; --vscode-widget-border:{{BORDER}}!important; --vscode-widget-shadow:rgb(from {{SHADOW}} r g b / .38)!important;
  --vscode-chat-editedFileForeground:{{INK}}!important; --vscode-chat-requestBackground:{{ELEVATED}}!important; --vscode-chat-requestBorder:{{BORDER}}!important;
  --vscode-button-background:{{ACCENT}}!important; --vscode-button-foreground:{{BUTTON_TEXT}}!important; --vscode-button-secondaryBackground:{{ELEVATED}}!important; --vscode-button-secondaryForeground:{{INK}}!important;
}
html[data-codex-window-type="electron"].electron-opaque,html[data-codex-window-type="electron"].electron-opaque body { background:{{UNDER}} !important; }
html[data-codex-window-type="electron"] .main-surface,html[data-codex-window-type="electron"] .browser-main-surface { background:rgb(from {{SURFACE}} r g b / .72)!important; box-shadow:0 0 0 1px {{BORDER}},0 18px 48px rgb(from {{SHADOW}} r g b / .18)!important; }
html[data-codex-window-type="electron"] .app-shell-left-panel { background:rgb(from {{UNDER}} r g b / .90)!important; border-right:1px solid {{BORDER}}!important; }
html[data-codex-window-type="electron"] .app-shell-left-panel :is(button,a):hover { color:{{ACCENT}}!important; background:rgb(from {{ACCENT}} r g b / .12)!important; }
html[data-codex-window-type="electron"] .composer-surface-chrome { color:{{INK}}!important; background:rgb(from {{ELEVATED}} r g b / .94)!important; border:1px solid {{BORDER}}!important; border-radius:20px!important; box-shadow:0 12px 34px rgb(from {{SHADOW}} r g b / .28)!important; }
html[data-codex-window-type="electron"] .composer-surface-chrome .ProseMirror { color:{{INK}}!important; caret-color:{{ACCENT}}!important; }
html[data-codex-window-type="electron"] :is([role="dialog"],[role="menu"],[role="listbox"],[data-radix-popper-content-wrapper] > *) { color:{{INK}}!important; background:rgb(from {{ELEVATED}} r g b / .98)!important; border-color:{{BORDER}}!important; box-shadow:0 20px 54px rgb(from {{SHADOW}} r g b / .38)!important; }
html[data-codex-window-type="electron"] :is([role="dialog"],[role="menu"],[role="listbox"],[data-radix-popper-content-wrapper] > *) :is(button,a,[role="menuitem"],[role="option"]) { color:inherit; }
html[data-codex-window-type="electron"] [role="tooltip"] { color:{{INK}}!important; background:{{ELEVATED}}!important; border:1px solid {{BORDER}}!important; }
html[data-codex-window-type="electron"] [data-content-search-unit-key$=":assistant"],html[data-codex-window-type="electron"] [data-message-author-role="assistant"] { color:{{INK}}!important; }
html[data-codex-window-type="electron"] [data-user-message-bubble="true"] { background:rgb(from {{ACCENT}} r g b / .15)!important; border:1px solid {{BORDER}}!important; }
html[data-codex-window-type="electron"].codex-theme-native body { background-color:{{UNDER}}!important; background-image:linear-gradient(rgb(from {{UNDER}} r g b / calc(1 - {{BACKGROUND_OPACITY}})),rgb(from {{UNDER}} r g b / calc(1 - {{BACKGROUND_OPACITY}}))),{{BACKGROUND}}!important; background-repeat:no-repeat,no-repeat!important; background-position:center,{{BACKGROUND_POSITION}}!important; background-size:cover,var(--codex-theme-background-size,cover)!important; background-attachment:fixed,fixed!important; }
.codex-theme-sidebar-logo { position:absolute; top:10px; left:16px; z-index:20; width:min(112px,55%); height:30px; padding-right:6px; object-fit:contain; object-position:left center; pointer-events:none; background:transparent; filter:drop-shadow(0 1px 8px rgb(from {{SURFACE}} r g b / .78)); }
html[data-codex-window-type="electron"] main.codex-theme-native-home-shell { background:linear-gradient(90deg,rgb(from {{SURFACE}} r g b / {{HOME_SHELL_STRONG}}),rgb(from {{SURFACE}} r g b / {{HOME_SHELL_MID}}) 60%,rgb(from {{SURFACE}} r g b / {{HOME_SHELL_WEAK}}))!important; border:0!important; box-shadow:none!important; }
html[data-codex-window-type="electron"] main.codex-theme-native-task-shell { background:linear-gradient(rgb(from {{SURFACE}} r g b / {{TASK_ALPHA}}),rgb(from {{SURFACE}} r g b / calc({{TASK_ALPHA}} + .04)))!important; }
html[data-codex-window-type="electron"] main.codex-theme-native-home-shell > header.app-header-tint { background:transparent!important; border-color:transparent!important; box-shadow:none!important; backdrop-filter:none!important; text-shadow:0 1px 10px rgb(from {{SURFACE}} r g b / .88); }
html[data-codex-window-type="electron"] aside.codex-theme-native-home-sidebar { background:linear-gradient(90deg,rgb(from {{UNDER}} r g b / {{SIDEBAR_STRONG}}),rgb(from {{UNDER}} r g b / {{SIDEBAR_MID}}))!important; border-color:transparent!important; box-shadow:none!important; backdrop-filter:none!important; }
html[data-codex-window-type="electron"] .codex-theme-native-home { --thread-content-max-width:min(1180px,calc(100cqw - 48px))!important; color:{{INK}}!important; background:transparent!important; container-type:inline-size; }
.codex-theme-native-home > div:first-child { min-height:100%!important; padding-top:clamp(14px,2.2vh,28px)!important; }
.codex-theme-native-home-hero { position:relative!important; isolation:isolate; width:calc(100% - clamp(28px,4cqw,56px))!important; max-width:1180px!important; min-height:clamp(280px,38vh,470px)!important; margin-inline:auto!important; padding:clamp(24px,4cqw,54px)!important; overflow:hidden!important; border:1px solid {{BORDER}}!important; border-radius:clamp(18px,2.2cqw,28px)!important; background:linear-gradient(90deg,rgb(from {{SURFACE}} r g b / {{HERO_STRONG}}),rgb(from {{SURFACE}} r g b / {{HERO_MID}}) 58%,transparent)!important; box-shadow:0 20px 54px rgb(from {{SHADOW}} r g b / .24)!important; }
.codex-theme-native-home-hero::before { content:""; position:absolute; z-index:0; inset:0; pointer-events:none; background:linear-gradient(135deg,rgb(from {{SURFACE}} r g b / .12),transparent 58%); backdrop-filter:blur({{BACKGROUND_BLUR}}); }
.codex-theme-native-home-hero::after { content:""; position:absolute; z-index:0; inset:0; pointer-events:none; background:linear-gradient(90deg,rgb(from {{SURFACE}} r g b / {{HERO_OVERLAY_STRONG}}),rgb(from {{SURFACE}} r g b / {{HERO_OVERLAY_MID}}) 62%,transparent); }
.codex-theme-native-home-copy { position:relative!important; z-index:1!important; width:min(54%,620px)!important; min-height:100%!important; align-items:flex-start!important; justify-content:center!important; text-align:left!important; color:{{INK}}!important; text-shadow:0 1px 12px rgb(from {{SURFACE}} r g b / .78); }
.codex-theme-native-home-copy [data-testid="home-icon"] { display:none!important; }
.codex-theme-native-home-copy :is(h1,h2,[class*="text-"]) { color:{{INK}}!important; font-family:{{DISPLAY_FONT}}!important; }
.codex-theme-native-home-suggestions { overflow:visible!important; }
.codex-theme-native-home-suggestions button { min-height:112px!important; padding:15px 14px!important; color:{{INK}}!important; border:1px solid {{BORDER}}!important; border-radius:18px!important; background:rgb(from {{ELEVATED}} r g b / {{PANEL_ALPHA}})!important; box-shadow:0 12px 30px rgb(from {{SHADOW}} r g b / .20)!important; backdrop-filter:blur(12px) saturate(1.08)!important; transition:transform .16s ease,border-color .16s ease,background-color .16s ease!important; }
.codex-theme-native-home-suggestions button:nth-child(1){background:color-mix(in srgb,rgb(from {{ELEVATED}} r g b / {{PANEL_ALPHA}}) 90%,{{REMOVED}})!important}.codex-theme-native-home-suggestions button:nth-child(2){background:color-mix(in srgb,rgb(from {{ELEVATED}} r g b / {{PANEL_ALPHA}}) 90%,{{ADDED}})!important}.codex-theme-native-home-suggestions button:nth-child(3){background:color-mix(in srgb,rgb(from {{ELEVATED}} r g b / {{PANEL_ALPHA}}) 90%,{{ACCENT}})!important}.codex-theme-native-home-suggestions button:nth-child(4){background:color-mix(in srgb,rgb(from {{ELEVATED}} r g b / {{PANEL_ALPHA}}) 90%,{{SKILL}})!important}
@media(hover:hover){.codex-theme-native-home-suggestions button:hover{transform:translateY(-3px);border-color:{{ACCENT}}!important;background:rgb(from {{ELEVATED}} r g b / .96)!important}}
.codex-theme-native-home-utility { border:1px solid {{BORDER}}!important; border-bottom:0!important; border-radius:18px 18px 0 0!important; background:rgb(from {{ELEVATED}} r g b / {{PANEL_ALPHA}})!important; box-shadow:none!important; backdrop-filter:blur(14px) saturate(1.06)!important; }
.codex-theme-native-home:has(.codex-theme-native-home-utility) .composer-surface-chrome { border-radius:0 0 18px 18px!important; border-top:0!important; }
.codex-theme-native-task { position:relative; isolation:isolate; background:rgb(from {{SURFACE}} r g b / {{TASK_ALPHA}})!important; }
#codex-theme-effect-ambient { position:fixed; z-index:1; inset:0; overflow:hidden; pointer-events:none; opacity:var(--codex-effect-opacity,.5); }
#codex-theme-effect-ambient i { position:absolute; display:block; left:calc(2% + var(--effect-index) * 5%); top:calc(4% + var(--effect-index) * 3%); animation-delay:calc(var(--effect-index) * -.37s); }
#codex-theme-effect-ambient b { position:absolute; display:block; left:calc(8% + var(--effect-index) * 8%); top:calc(18% + var(--effect-index) * 5%); width:3px; height:3px; border-radius:50%; background:{{ACCENT}}; box-shadow:0 0 8px 3px rgb(from {{ACCENT}} r g b / .34); animation:codex-theme-particle 8s ease-in-out infinite; animation-delay:calc(var(--effect-index) * -1.3s); }
#codex-theme-effect-ambient[data-kind="rain"] i,#codex-theme-effect-ambient[data-kind="storm"] i { width:1.5px; height:18px; border-radius:99px; background:linear-gradient(transparent,rgb(from {{INK}} r g b / .66),transparent); transform:rotate(20deg); animation:codex-theme-rain 3.2s linear infinite; }
#codex-theme-effect-ambient[data-kind="particles"] i { width:3px; height:3px; border-radius:50%; background:{{ACCENT}}; box-shadow:0 0 8px 3px rgb(from {{ACCENT}} r g b / .34); animation:codex-theme-particle 8s ease-in-out infinite; }
#codex-theme-effect-ambient[data-kind="storm"]::after { content:""; position:absolute; width:min(42vw,520px); height:3px; right:-3%; top:20%; background:white; box-shadow:0 0 8px 3px {{ACCENT}},0 0 24px 8px rgb(from {{ACCENT}} r g b / .45); clip-path:polygon(0 40%,42% 0,38% 44%,72% 25%,58% 64%,100% 48%,55% 100%,62% 62%,22% 82%,34% 48%); animation:codex-theme-lightning 11s steps(1,end) infinite; }
#codex-theme-effect-overlay { position:fixed; z-index:18; left:var(--codex-effect-x,72%); top:var(--codex-effect-y,28%); width:var(--codex-effect-width,42%); max-height:72vh; object-fit:contain; opacity:0; transform:translate(-50%,-50%) scale(.92); filter:drop-shadow(0 0 14px rgb(from {{ACCENT}} r g b / .46)); pointer-events:none; }
#codex-theme-effect-overlay.is-active { animation:codex-theme-overlay 1.35s cubic-bezier(.18,.74,.18,1) both; }
#codex-theme-composer-accent { position:fixed; z-index:24; width:var(--codex-effect-composer-width,120px); max-height:180px; object-fit:contain; opacity:0; transform:translate3d(55vw,-44vh,0) rotate(720deg) scale(.7); transform-origin:center; filter:drop-shadow(0 5px 8px rgb(from {{SHADOW}} r g b / .58)) drop-shadow(0 0 7px rgb(from {{ACCENT}} r g b / .52)); pointer-events:none; }
#codex-theme-composer-accent.is-active { animation:codex-theme-composer-arrive 1.05s cubic-bezier(.16,.72,.18,1) both; }
@keyframes codex-theme-rain { from { translate:0 -80px; } to { translate:-24px 110vh; } }
@keyframes codex-theme-particle { 0%,100% { translate:0 20px; opacity:.16; } 50% { translate:26px -70px; opacity:.9; } }
@keyframes codex-theme-lightning { 0%,92.5%,93.4%,94.1%,100% { opacity:0; } 92.8%,93.1% { opacity:.9; } 93.7% { opacity:.4; } }
@keyframes codex-theme-overlay { 0% { opacity:0; transform:translate(-50%,-50%) scale(.90); } 18% { opacity:var(--codex-effect-opacity,.6); transform:translate(-50%,-50%) scale(1); } 62% { opacity:calc(var(--codex-effect-opacity,.6) * .82); } 100% { opacity:0; transform:translate(-50%,-50%) scale(1.04); } }
@keyframes codex-theme-composer-arrive { 0% { opacity:0; transform:translate3d(55vw,-44vh,0) rotate(720deg) scale(.7); } 8% { opacity:var(--codex-effect-opacity,.6); } 100% { opacity:var(--codex-effect-opacity,.6); transform:translate3d(0,0,0) rotate(-10deg) scale(1); } }
@container(max-width:760px){.codex-theme-native-home-hero{min-height:260px;padding:24px}.codex-theme-native-home-copy{width:68%!important}.codex-theme-native-home-suggestions button{min-height:102px;padding:12px 10px!important}}
@container(max-width:520px){.codex-theme-native-home-hero{min-height:240px}.codex-theme-native-home-copy{width:100%!important}.codex-theme-native-home-hero::after{background:rgb(from {{SURFACE}} r g b / .58)}}
@media(max-height:760px){.codex-theme-native-home-hero{min-height:240px!important}.codex-theme-native-home-suggestions button{min-height:94px!important}}
@media(prefers-reduced-motion:reduce){.codex-theme-native-home-suggestions button{transition:none!important}#codex-theme-effect-ambient{display:none!important}#codex-theme-effect-overlay,#codex-theme-composer-accent{animation:none!important;display:none!important}}
"#;

const JS_TEMPLATE: &str = r#"
(() => {
  globalThis.__codexThemeStore?.dispose?.();
  document.querySelectorAll('.codex-theme-sidebar-logo,#codex-theme-effect-ambient,#codex-theme-effect-overlay,#codex-theme-composer-accent').forEach(node=>node.remove());
  const cfg=__codexSkinConfig,root=document.documentElement,assetUrl=value=>value?.startsWith('url("')?value.slice(5,-2):'';
  root.classList.add('codex-theme-native');root.dataset.codexThemeId=cfg.themeId;
  let sidebarLogo=null,scheduled=false,disposed=false,frame=null,lastRunning=false,ambientLayer=null,effectOverlay=null,composerAccent=null;
  const decorated=['codex-theme-native-home','codex-theme-native-task','codex-theme-native-home-hero','codex-theme-native-home-copy','codex-theme-native-home-suggestions','codex-theme-native-home-utility','codex-theme-native-home-shell','codex-theme-native-task-shell','codex-theme-native-home-sidebar','codex-theme-native-task-sidebar'];
  const clearDecorations=()=>{for(const cls of decorated)for(const node of document.querySelectorAll(`.${cls}`))node.classList.remove(cls);};
  const prune=(selector,keep)=>{for(const node of document.querySelectorAll(selector))if(node!==keep)node.remove();};
  const updateBackgroundFit=()=>{const image=globalThis.__codexThemeStore?.backgroundImage;if(!image?.naturalWidth||!image.naturalHeight)return;let fit=cfg.backgroundFit||'smart';if(fit==='smart'){const imageRatio=image.naturalWidth/image.naturalHeight,windowRatio=innerWidth/Math.max(1,innerHeight),visible=Math.min(imageRatio/windowRatio,windowRatio/imageRatio);fit=1-visible>.34?'contain':'cover';}root.style.setProperty('--codex-theme-background-size',fit==='contain'?'contain':'cover');};
  const applyCopy=()=>{if(cfg.copy?.title)document.title=cfg.copy.title;const replacements=cfg.copy?.replacePlaceholders||{};for(const node of document.querySelectorAll('[placeholder],[data-placeholder]'))for(const attr of ['placeholder','data-placeholder']){const value=node.getAttribute(attr);if(value&&replacements[value])node.setAttribute(attr,replacements[value]);}const editor=document.querySelector('.ProseMirror,[contenteditable="true"][role="textbox"]');if(editor&&cfg.home?.composerHint){editor.setAttribute('aria-label',cfg.home.composerHint);editor.dataset.placeholder=cfg.home.composerHint;}};
  const ensureSidebarLogo=()=>{prune('.codex-theme-sidebar-logo',sidebarLogo);if(sidebarLogo?.isConnected||!cfg.logo)return;const sidebar=document.querySelector('.app-shell-left-panel');if(!sidebar)return;sidebarLogo=document.createElement('img');sidebarLogo.className='codex-theme-sidebar-logo';sidebarLogo.src=assetUrl(cfg.logo);sidebarLogo.alt=cfg.home?.brand||'';sidebarLogo.decoding='async';sidebar.append(sidebarLogo);};
  const reducedMotion=()=>matchMedia('(prefers-reduced-motion: reduce)').matches;
  const triggers=(entry,type)=>Array.isArray(entry?.triggers)&&entry.triggers.includes(type);
  const restart=node=>{if(!node||reducedMotion())return;node.classList.remove('is-active');void node.offsetWidth;node.classList.add('is-active');};
  const positionComposerAccent=()=>{if(!composerAccent)return;const composer=document.querySelector('.composer-surface-chrome'),rect=composer?.getBoundingClientRect();if(!rect)return;const width=Number(cfg.effects?.composerAccent?.widthPx)||120;composerAccent.style.left=`${Math.round(rect.left-width*.28)}px`;composerAccent.style.top=`${Math.round(rect.top-width*.62)}px`;};
  const triggerEffects=type=>{if(triggers(cfg.effects?.overlay,type))restart(effectOverlay);if(triggers(cfg.effects?.composerAccent,type)){positionComposerAccent();restart(composerAccent);}else if(type==='message-send')composerAccent?.classList.remove('is-active');};
  const ensureEffects=()=>{prune('#codex-theme-effect-ambient',ambientLayer);prune('#codex-theme-effect-overlay',effectOverlay);prune('#codex-theme-composer-accent',composerAccent);const effects=cfg.effects||{},opacity=effects.intensity==='subtle'?'.28':effects.intensity==='vivid'?'.82':'.52';root.style.setProperty('--codex-effect-opacity',opacity);if(!ambientLayer&&effects.ambient&&effects.ambient!=='none'){ambientLayer=document.createElement('div');ambientLayer.id='codex-theme-effect-ambient';ambientLayer.dataset.kind=effects.ambient;ambientLayer.setAttribute('aria-hidden','true');for(let index=0;index<20;index++){const drop=document.createElement('i');drop.style.setProperty('--effect-index',String(index));ambientLayer.append(drop);}if(effects.ambient==='storm')for(let index=0;index<10;index++){const particle=document.createElement('b');particle.style.setProperty('--effect-index',String(index));ambientLayer.append(particle);}document.body.append(ambientLayer);}if(!effectOverlay&&effects.overlay?.image){effectOverlay=document.createElement('img');effectOverlay.id='codex-theme-effect-overlay';effectOverlay.alt='';effectOverlay.src=assetUrl(effects.overlay.image);effectOverlay.style.setProperty('--codex-effect-x',`${effects.overlay.position?.x??72}%`);effectOverlay.style.setProperty('--codex-effect-y',`${effects.overlay.position?.y??28}%`);effectOverlay.style.setProperty('--codex-effect-width',`${effects.overlay.widthPercent??42}%`);document.body.append(effectOverlay);}if(!composerAccent&&effects.composerAccent?.image){composerAccent=document.createElement('img');composerAccent.id='codex-theme-composer-accent';composerAccent.alt='';composerAccent.src=assetUrl(effects.composerAccent.image);composerAccent.style.setProperty('--codex-effect-composer-width',`${effects.composerAccent.widthPx??120}px`);document.body.append(composerAccent);positionComposerAccent();}};
  const runningTask=()=>Boolean(document.querySelector('[data-app-action-sidebar-thread-running="true"],[data-app-action-sidebar-thread-is-running="true"],[data-thread-running="true"],[data-is-running="true"],[aria-label*="正在运行"]'));
  const update=()=>{frame=null;scheduled=false;if(disposed)return;clearDecorations();ensureEffects();const shell=document.querySelector('main.main-surface,.browser-main-surface');if(!shell){applyCopy();return;}const home=document.querySelector('[role="main"]:has([data-testid="home-icon"])');for(const route of document.querySelectorAll('[role="main"]'))route.classList.add(route===home?'codex-theme-native-home':'codex-theme-native-task');shell.classList.add(home?'codex-theme-native-home-shell':'codex-theme-native-task-shell');document.querySelector('.app-shell-left-panel')?.classList.add(home?'codex-theme-native-home-sidebar':'codex-theme-native-task-sidebar');if(home){const icon=home.querySelector('[data-testid="home-icon"]'),copy=icon?.parentElement,hero=copy?.parentElement?.parentElement;if(hero)hero.classList.add('codex-theme-native-home-hero');if(copy)copy.classList.add('codex-theme-native-home-copy');for(const candidate of home.querySelectorAll('[class*="home-suggestions"],[class*="homeSuggestions"]'))candidate.classList.add('codex-theme-native-home-suggestions');for(const area of home.querySelectorAll('[data-composer-utility-bar-scroll-area]'))area.parentElement?.classList.add('codex-theme-native-home-utility');}const running=runningTask();if(running&&!lastRunning)triggerEffects('task-start');lastRunning=running;positionComposerAccent();applyCopy();ensureSidebarLogo();updateBackgroundFit();};
  const schedule=()=>{if(disposed||scheduled)return;scheduled=true;frame=requestAnimationFrame(update);};
  const observer=new MutationObserver(schedule);observer.observe(root,{childList:true,subtree:true,attributes:true,attributeFilter:['aria-label','data-app-action-sidebar-thread-running','data-app-action-sidebar-thread-is-running','data-thread-running','data-is-running']});
  const resizeHandler=()=>{updateBackgroundFit();schedule();};window.addEventListener('resize',resizeHandler);
  const sendHandler=event=>{if(!(event.target instanceof Element))return;const byClick=event.type==='click'&&event.target.closest('button[type="submit"],[data-testid*="send"]'),byKey=event.type==='keydown'&&event.key==='Enter'&&!event.shiftKey&&event.target.closest('textarea,input,[contenteditable="true"]');if(byClick||byKey)triggerEffects('message-send');};document.addEventListener('click',sendHandler,true);document.addEventListener('keydown',sendHandler,true);
  const backgroundImage=cfg.background?new Image():null;if(backgroundImage){backgroundImage.addEventListener('load',updateBackgroundFit);backgroundImage.src=assetUrl(cfg.background);}
  globalThis.__codexThemeStore={observer,resizeHandler,backgroundImage,injectionId:null,dispose(){disposed=true;if(frame!==null)cancelAnimationFrame(frame);frame=null;scheduled=false;observer.disconnect();window.removeEventListener('resize',resizeHandler);document.removeEventListener('click',sendHandler,true);document.removeEventListener('keydown',sendHandler,true);clearDecorations();document.querySelectorAll('.codex-theme-sidebar-logo,#codex-theme-effect-ambient,#codex-theme-effect-overlay,#codex-theme-composer-accent').forEach(node=>node.remove());sidebarLogo=ambientLayer=effectOverlay=composerAccent=null;root.classList.remove('codex-theme-native');root.removeAttribute('data-codex-theme-id');root.style.removeProperty('--codex-theme-background-size');root.style.removeProperty('--codex-effect-opacity');}};
  update();
  return true;
})();
"#;

#[cfg(test)]
mod tests {
    use super::{JS_TEMPLATE, font_stack};

    #[test]
    fn rejects_css_in_font_stacks() {
        assert!(font_stack("Inter, system-ui, sans-serif").is_ok());
        assert!(font_stack("\"Microsoft YaHei UI\", \"PingFang SC\", sans-serif").is_ok());
        assert!(font_stack("sans-serif; } body { display: none").is_err());
        assert!(font_stack("url(https://example.com/font.woff)").is_err());
        assert!(font_stack("\"unterminated").is_err());
        assert!(font_stack("\"").is_err());
    }

    #[test]
    fn native_renderer_decorates_codex_routes_without_a_covering_home_node() {
        assert!(JS_TEMPLATE.contains("[role=\"main\"]:has([data-testid=\"home-icon\"])"));
        assert!(JS_TEMPLATE.contains("codex-theme-native-home"));
        assert!(JS_TEMPLATE.contains("codex-theme-native-task"));
        assert!(JS_TEMPLATE.contains("codex-theme-native-home-sidebar"));
        assert!(JS_TEMPLATE.contains("codex-theme-effect-ambient"));
        assert!(JS_TEMPLATE.contains("prefers-reduced-motion: reduce"));
        assert!(JS_TEMPLATE.contains("triggerEffects('message-send')"));
        assert!(JS_TEMPLATE.contains("cancelAnimationFrame(frame)"));
        assert!(JS_TEMPLATE.contains("prune('.codex-theme-sidebar-logo',sidebarLogo)"));
        assert!(JS_TEMPLATE.contains("if(disposed||scheduled)return"));
        assert!(!JS_TEMPLATE.contains("createElement('section')"));
        assert!(!JS_TEMPLATE.contains("innerHTML"));
        assert!(!JS_TEMPLATE.contains("id='codex-theme-home'"));
    }
}
