import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("application version manifests stay aligned", async () => {
  const [npmManifest, tauriManifest, cargoManifest, cargoLock] = await Promise.all([
    readFile(new URL("../package.json", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/Cargo.lock", import.meta.url), "utf8"),
  ]);
  const version = JSON.parse(npmManifest).version;
  assert.equal(JSON.parse(tauriManifest).version, version);
  assert.match(cargoManifest, new RegExp(`^version = "${version.replaceAll(".", "\\.")}"$`, "m"));
  assert.match(cargoLock, new RegExp(`name = "codex-skin"\\r?\\nversion = "${version.replaceAll(".", "\\.")}"`));
});

test("Windows release publishes the Tauri graphical Setup installer", async () => {
  const [cargo, tauri, ci, build, readme, rustApp, installer, chineseMessages, icon] = await Promise.all([
    readFile(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
    readFile(new URL("../.github/workflows/ci.yml", import.meta.url), "utf8"),
    readFile(new URL("../.github/workflows/build.yml", import.meta.url), "utf8"),
    readFile(new URL("../README.md", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8"),
    readFile(new URL("../installer/windows/CodexThemeStore.iss", import.meta.url), "utf8"),
    readFile(new URL("../installer/windows/languages/ChineseSimplified.isl", import.meta.url), "utf8"),
    readFile(new URL("../assets/Codex-Skin.ico", import.meta.url)),
  ]);

  assert.match(cargo, /name = "codex-skin"/);
  assert.match(tauri, /"productName": "Codex-Skin"/);
  assert.match(ci, /Codex-Skin-win-x64/);
  assert.match(ci, /cargo test --manifest-path src-tauri\/Cargo\.toml/);
  assert.match(ci, /Online theme assets must not be bundled/);
  assert.match(build, /Codex-Skin-Setup-win-x64\.exe/);
  assert.match(build, /Codex-Skin-\$\{\{ matrix\.rid \}\}\.pkg/);
  assert.match(build, /Codex-Skin-installers-SHA256SUMS\.txt/);
  assert.match(tauri, /icons\/icon\.ico/);
  assert.match(installer, /SetupIconFile=.*Codex-Skin\.ico/);
  assert.equal(icon.readUInt16LE(0), 0);
  assert.equal(icon.readUInt16LE(2), 1);
  const iconSizes = Array.from({ length: icon.readUInt16LE(4) }, (_, index) => {
    const width = icon[6 + index * 16];
    return width === 0 ? 256 : width;
  });
  assert.deepEqual(iconSizes, [16, 24, 32, 48, 64, 128, 256]);
  assert.match(installer, /MessagesFile: "\{#SourcePath\}\\languages\\ChineseSimplified\.isl"/);
  assert.match(installer, /Software\\Classes\\dreamskin\\shell\\open\\command/);
  assert.match(installer, /Software\\Classes\\\.dreamskin/);
  assert.match(rustApp, /tauri_plugin_single_instance::init/);
  assert.match(rustApp, /tauri_plugin_deep_link::init/);
  assert.match(chineseMessages, /Inno Setup version 6\.5\.0\+/);
  assert.doesNotMatch(readme, /Windows (?:x64 )?便携版|Windows portable|Codex-Skin-win-x64\.(?:exe|zip)/);
  assert.doesNotMatch(`${readme}\n${rustApp}\n${ci}\n${build}\n${installer}`, /Codex主题商店\.exe|CodexThemeStore-(?:Setup|osx)|CodexSkin-theme/);
});

test("official theme publishing reads the Store source of truth", async () => {
  const [workflow, builder, discovery, cargo, verifier, catalogTool] = await Promise.all([
    readFile(new URL("../.github/workflows/release-official-themes.yml", import.meta.url), "utf8"),
    readFile(new URL("../tools/dreamskin/build-official-themes.mjs", import.meta.url), "utf8"),
    readFile(new URL("../tools/dreamskin/find-pending-official-themes.mjs", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/bin/dreamskin-verify.rs", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/bin/catalog-tool.rs", import.meta.url), "utf8"),
  ]);
  assert.match(workflow, /repository: lixiaobaivv\/Codex-Skin-Store/);
  assert.match(workflow, /environment: theme-publishing/);
  assert.match(workflow, /store_commit/);
  assert.match(workflow, /theme-\$\{THEME_ID\}-v\$\{version\}/);
  assert.match(workflow, /Install Linux verifier dependencies/);
  assert.match(workflow, /libwebkit2gtk-4\.1-dev/);
  assert.match(workflow, /libayatana-appindicator3-dev/);
  assert.match(builder, /CODEX_SKIN_THEME_SOURCE/);
  assert.match(builder, /theme-repository\.json/);
  assert.match(builder, /effect-overlay\.png/);
  assert.match(builder, /composer-accent\.png/);
  assert.doesNotMatch(builder, /const ids = \["dilraba-star"/);
  assert.match(discovery, /catalog\.package === null/);
  assert.match(cargo, /name = "dreamskin-verify"[\s\S]*path = "src\/bin\/dreamskin-verify\.rs"/);
  assert.match(cargo, /name = "catalog-tool"[\s\S]*path = "src\/bin\/catalog-tool\.rs"/);
  assert.match(verifier, /codex_skin_lib::verify_dreamskin/);
  assert.match(catalogTool, /codex_skin_lib::catalog_validate/);
});

test("macOS package registers Codex-Skin URL and document activation", async () => {
  const [plist, cargo, packageScript, icon] = await Promise.all([
    readFile(new URL("../installer/macos/Info.plist", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/Cargo.toml", import.meta.url), "utf8"),
    readFile(new URL("../installer/macos/build-pkg.sh", import.meta.url), "utf8"),
    readFile(new URL("../installer/macos/AppIcon.png", import.meta.url)),
  ]);
  assert.match(cargo, /name = "codex-skin"/);
  assert.match(plist, /<key>CFBundleURLSchemes<\/key>[\s\S]*<string>dreamskin<\/string>/);
  assert.match(plist, /<key>CFBundleTypeExtensions<\/key>[\s\S]*<string>dreamskin<\/string>/);
  assert.match(plist, /<key>CFBundleIconFile<\/key>[\s\S]*<string>Codex-Skin\.icns<\/string>/);
  assert.match(packageScript, /Applications\/Codex-Skin\.app/);
  assert.match(packageScript, /Contents\/MacOS\/Codex-Skin/);
  assert.match(packageScript, /npm run tauri -- build/);
  assert.match(packageScript, /iconutil -c icns/);
  assert.equal(icon.subarray(1, 4).toString("ascii"), "PNG");
  assert.equal(icon.readUInt32BE(16), 1024);
  assert.equal(icon.readUInt32BE(20), 1024);
  assert.equal(icon[25], 6, "AppIcon.png must be RGBA so macOS keeps transparent corners");
});

test("Tauri desktop uses horizontal categories and single-instance activation", async () => {
  const [frontend, styles, rustApp] = await Promise.all([
    readFile(new URL("../src/main.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8"),
  ]);
  assert.match(frontend, /\["全部", "人物", "动漫", "游戏", "风景", "极简", "节日", "其他"\]/);
  assert.match(styles, /\.categories \{[^}]*display: flex/);
  assert.match(rustApp, /tauri_plugin_single_instance::init/);
  assert.match(rustApp, /external-activation/);
});

test("desktop client updates use the published platform installers", async () => {
  const [frontend, updater, workflow] = await Promise.all([
    readFile(new URL("../src/main.ts", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/updater.rs", import.meta.url), "utf8"),
    readFile(new URL("../.github/workflows/build.yml", import.meta.url), "utf8"),
  ]);
  for (const asset of [
    "Codex-Skin-Setup-win-x64.exe",
    "Codex-Skin-osx-arm64.pkg",
    "Codex-Skin-osx-x64.pkg",
    "Codex-Skin-installers-SHA256SUMS.txt",
  ]) {
    assert.match(updater, new RegExp(asset.replaceAll(".", "\\.")));
    assert.match(workflow, new RegExp(asset.replaceAll(".", "\\.")));
  }
  assert.match(updater, /api\.github\.com\/repos\/lixiaobaivv\/Codex-Skin\/releases\?per_page=100/);
  assert.match(updater, /select_latest_release/);
  assert.match(updater, /SHA-256 校验失败/);
  assert.match(frontend, /check_app_update/);
  assert.match(frontend, /app-update-progress/);
  assert.match(frontend, /install_app_update/);
});

test("theme application uses one adaptive action and production rejects fixture trust", async () => {
  const [frontend, styles, rustApp, dreamskin, compiler, repository] = await Promise.all([
    readFile(new URL("../src/main.ts", import.meta.url), "utf8"),
    readFile(new URL("../src/styles.css", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/dreamskin.rs", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/compiler.rs", import.meta.url), "utf8"),
    readFile(new URL("../src-tauri/src/repository.rs", import.meta.url), "utf8"),
  ]);
  assert.match(frontend, /id="apply" class="button primary">应用主题<\/button>/);
  assert.doesNotMatch(frontend, /id="restart"|应用并重启 Codex<\/button>/);
  assert.match(frontend, /call<boolean>\("theme_runtime_ready"\)/);
  assert.match(frontend, /ready \? "apply_theme" : "restart_and_apply"/);
  assert.doesNotMatch(frontend, /已取消应用主题|没有启用本机主题端口，需要重启后应用主题/);
  assert.doesNotMatch(frontend, /window\.confirm\(`下载并安装 Codex-Skin/);
  assert.match(styles, /grid-template-columns: 1fr repeat\(5, auto\)/);
  assert.match(rustApp, /async fn theme_runtime_ready\(\) -> bool/);
  assert.match(rustApp, /async fn apply_theme[\s\S]*for attempt in 0\.\.3/);
  assert.match(rustApp, /outcome\.candidates == 0/);
  assert.match(rustApp, /当前 Codex 页面结构暂不受支持/);
  assert.match(rustApp, /async fn rollback_theme\(\)[\s\S]*if !cdp::is_ready\(\)\.await[\s\S]*当前已经是默认主题/);
  assert.match(rustApp, /for attempt in 0\.\.3[\s\S]*cdp::remove/);
  assert.match(await readFile(new URL("../src-tauri/src/cdp.rs", import.meta.url), "utf8"), /Client::builder\(\)[\s\S]*\.no_proxy\(\)/);
  assert.match(await readFile(new URL("../src-tauri/src/cdp.rs", import.meta.url), "utf8"), /MainContentSurface[\s\S]*contenteditable/);
  assert.match(dreamskin, /#\[cfg\(test\)\]\s+"codex-skin\.sample\.2026-01"/);
  assert.match(compiler, /fn font_stack\(value: &str\)/);
  assert.match(compiler, /"glass" => \[/);
  assert.match(compiler, /"WINDOW_ALPHA", if opaque_windows \{ "1" \} else \{ "\.06" \}/);
  assert.match(repository, /fn validate_font_stack\(value: &str\)/);
  assert.match(repository, /"clear", "balanced", "immersive", "glass"/);
});

test("desktop catalog uses conditional sync and on-demand persistent resources", async () => {
  const [repository, frontend] = await Promise.all([
    readFile(new URL("../src-tauri/src/repository.rs", import.meta.url), "utf8"),
    readFile(new URL("../src/main.ts", import.meta.url), "utf8"),
  ]);
  assert.match(repository, /desktop-catalog-v2\.json/);
  assert.match(repository, /IF_NONE_MATCH/);
  assert.match(repository, /ensure_preview/);
  assert.match(repository, /ensure_theme/);
  assert.match(repository, /sync_subscriptions/);
  assert.match(repository, /theme-library-state\.json|theme_library_state_path/);
  assert.match(repository, /Sha256::digest/);
  assert.doesNotMatch(repository, /archive\/refs\/heads\/main\.zip/);
  assert.match(frontend, /IntersectionObserver/);
  assert.match(frontend, /在线/);
  assert.match(frontend, /已下载/);
  assert.match(frontend, /有更新/);
  assert.match(frontend, /set_theme_subscription/);
  assert.match(frontend, /delete_theme/);
  assert.match(frontend, /删除主题/);
});

test("macOS release status documents the unsigned graphical client", async () => {
  const readme = await readFile(new URL("../README.md", import.meta.url), "utf8");
  assert.match(readme, /macOS Apple Silicon/);
  assert.match(readme, /PKG 会声明 `dreamskin:\/\/` 和 `\.dreamskin`/);
  assert.match(readme, /CI 已在 Apple Silicon 与 Intel 目标上完成 PKG 构建验证/);
  assert.match(readme, /仍建议在对应真实设备上验收系统关联与启动行为/);
  assert.match(readme, /macOS PKG 当前未签名、未公证/);
});
