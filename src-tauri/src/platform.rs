use crate::{
    cdp::{self, Payload},
    error::{AppError, Result},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use sysinfo::{ProcessesToUpdate, Signal, System};

#[derive(Clone, Debug)]
#[cfg_attr(windows, allow(dead_code))]
pub struct Installation {
    pub app_path: PathBuf,
    pub executable: PathBuf,
}

pub fn discover() -> Result<Installation> {
    #[cfg(windows)]
    {
        return windows_platform::discover();
    }
    #[cfg(target_os = "macos")]
    {
        return macos_platform::discover();
    }
    #[allow(unreachable_code)]
    Err(AppError::Message(
        "当前系统不支持 Codex 平台适配器。".into(),
    ))
}

pub async fn restart_and_inject(payload: &Payload, timeout: Duration) -> Result<()> {
    let installation = discover()?;
    stop(&installation).await?;
    start(&installation, true)?;

    let deadline = Instant::now() + timeout;
    let mut last_injection_error = None;
    while Instant::now() < deadline {
        if cdp::is_ready().await {
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(10));
            if remaining > Duration::ZERO {
                match cdp::inject(payload, remaining).await {
                    Ok(outcome) if outcome.applied > 0 => return Ok(()),
                    Ok(outcome) => {
                        last_injection_error = Some(if outcome.candidates == 0 {
                            "Codex 主窗口尚未完成初始化。".to_owned()
                        } else {
                            "当前 Codex 页面结构暂不受支持，请更新 Codex-Skin。".to_owned()
                        });
                    }
                    Err(error) => last_injection_error = Some(error.to_string()),
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let detail = last_injection_error.unwrap_or_else(|| {
        "未检测到本机 127.0.0.1:9229 调试端口；请确认 Codex 未被安全软件阻止启动。".into()
    });
    Err(AppError::Message(format!(
        "Codex 已启动，但主题未能在 90 秒内完成 CDP 注入：{detail}"
    )))
}

async fn stop(installation: &Installation) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    let force_after = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let mut system = System::new_all();
        system.refresh_processes(ProcessesToUpdate::All, true);
        let mut found = false;
        let signal = if Instant::now() >= force_after {
            Signal::Kill
        } else {
            Signal::Term
        };
        for process in system.processes().values() {
            let Some(path) = process.exe() else {
                continue;
            };
            if belongs_to(path, installation) {
                found = true;
                let _ = process.kill_with(signal).or_else(|| Some(process.kill()));
            }
        }
        if !found {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(AppError::Message("Codex 未能完全退出，请重试。".into()))
}

fn belongs_to(path: &Path, _installation: &Installation) -> bool {
    #[cfg(windows)]
    {
        return path == _installation.executable || path.starts_with(&_installation.app_path);
    }
    #[cfg(target_os = "macos")]
    {
        return path == _installation.executable || path.starts_with(&_installation.app_path);
    }
    #[allow(unreachable_code)]
    false
}

fn start(_installation: &Installation, enable_cdp: bool) -> Result<()> {
    #[cfg(windows)]
    {
        return windows_platform::start(_installation, enable_cdp);
    }
    #[cfg(target_os = "macos")]
    {
        return macos_platform::start(_installation, enable_cdp);
    }
    #[allow(unreachable_code)]
    Err(AppError::Message("当前系统不支持 Codex 启动。".into()))
}

#[cfg(windows)]
mod windows_platform {
    use super::*;
    use windows::{
        Win32::{
            System::Com::{
                CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            UI::Shell::{
                ACTIVATEOPTIONS, ApplicationActivationManager, IApplicationActivationManager,
            },
        },
        core::HSTRING,
    };
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};

    const EXECUTABLE_LOCATIONS: [&str; 6] = [
        "app\\Codex.exe",
        "app\\ChatGPT.exe",
        "Codex.exe",
        "ChatGPT.exe",
        "OpenAI Codex.exe",
        "app\\OpenAI Codex.exe",
    ];

    fn installation_from_path(path: &Path) -> Option<Installation> {
        if path.is_file() {
            return Some(Installation {
                app_path: path.parent()?.to_owned(),
                executable: path.to_owned(),
            });
        }
        if !path.is_dir() {
            return None;
        }
        for relative in EXECUTABLE_LOCATIONS {
            let executable = path.join(relative);
            if executable.is_file() {
                return Some(Installation {
                    app_path: executable.parent()?.to_owned(),
                    executable,
                });
            }
        }
        let manifest = path.join("AppxManifest.xml");
        if let Ok(contents) = std::fs::read_to_string(manifest)
            && let Ok(pattern) = regex::Regex::new(
                r#"(?i)<Application\b[^>]*\bExecutable\s*=\s*[\"']([^\"']+)[\"']"#,
            )
            && let Some(executable) = pattern
                .captures(&contents)
                .and_then(|captures| captures.get(1))
                .map(|value| path.join(value.as_str().replace('/', "\\")))
                .filter(|candidate| candidate.is_file())
        {
            return Some(Installation {
                app_path: executable.parent()?.to_owned(),
                executable,
            });
        }
        None
    }

    fn common_install_locations() -> Vec<PathBuf> {
        let mut locations = Vec::new();
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            locations.extend([
                local.join("Programs\\Codex"),
                local.join("Programs\\OpenAI Codex"),
                local.join("Codex"),
                local.join("OpenAI\\Codex"),
            ]);
        }
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(program_files) = std::env::var_os(variable) {
                let program_files = PathBuf::from(program_files);
                locations.extend([
                    program_files.join("Codex"),
                    program_files.join("OpenAI\\Codex"),
                ]);
            }
        }
        locations
    }

    fn packaged_installations() -> Vec<Installation> {
        let current_user = RegKey::predef(HKEY_CURRENT_USER);
        let Ok(repository) = current_user.open_subkey(
            r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages",
        ) else {
            return Vec::new();
        };
        repository
            .enum_keys()
            .filter_map(|item| item.ok())
            .filter(|name| name.to_ascii_lowercase().contains("codex"))
            .filter_map(|name| {
                let package = repository.open_subkey(name).ok()?;
                let root: String = package.get_value("PackageRootFolder").ok()?;
                installation_from_path(Path::new(&root))
            })
            .collect()
    }

    pub fn discover() -> Result<Installation> {
        if let Some(configured) = std::env::var_os("CODEX_APP_PATH")
            && let Some(installation) = installation_from_path(Path::new(&configured))
        {
            return Ok(installation);
        }
        let mut candidates = packaged_installations();
        candidates.extend(
            common_install_locations()
                .iter()
                .filter_map(|path| installation_from_path(path)),
        );
        candidates.sort_by_key(|installation| {
            std::fs::metadata(&installation.executable)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        candidates
            .pop()
            .ok_or_else(|| AppError::Message(
                "未找到 Windows Codex 安装。请安装 Codex，或将 CODEX_APP_PATH 设置为 Codex.exe、ChatGPT.exe 或其安装目录。".into(),
            ))
    }

    fn launch_arguments(enable_cdp: bool) -> [&'static str; 2] {
        if enable_cdp {
            [
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=9229",
            ]
        } else {
            ["", ""]
        }
    }

    fn start_executable(installation: &Installation, enable_cdp: bool) -> Result<()> {
        let arguments = launch_arguments(enable_cdp);
        std::process::Command::new(&installation.executable)
            .args(
                arguments
                    .into_iter()
                    .filter(|argument| !argument.is_empty()),
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| AppError::Message(format!("启动 Codex 失败：{error}")))
    }

    pub fn start(installation: &Installation, enable_cdp: bool) -> Result<()> {
        let packaged = installation
            .executable
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("\\windowsapps\\openai.codex_");
        if !packaged {
            return start_executable(installation, enable_cdp);
        }
        let arguments = if enable_cdp {
            "--remote-debugging-address=127.0.0.1 --remote-debugging-port=9229"
        } else {
            ""
        };
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                .ok()
                .map_err(|error| AppError::Message(format!("初始化 Windows COM 失败：{error}")))?;
            let manager: IApplicationActivationManager =
                CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
                    .map_err(|error| {
                        AppError::Message(format!("创建 Codex 激活器失败：{error}"))
                    })?;
            let result = manager.ActivateApplication(
                &HSTRING::from("OpenAI.Codex_2p2nqsd0c76g0!App"),
                &HSTRING::from(arguments),
                ACTIVATEOPTIONS(0),
            );
            CoUninitialize();
            if result.is_ok() {
                return Ok(());
            }
        }
        // Some preview or enterprise packages use a different application user model ID.
        // Their packaged executable is still launchable, so keep it as a safe fallback.
        start_executable(installation, enable_cdp)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn resolves_executable_and_install_directory_overrides() {
            let temporary = tempfile::tempdir().unwrap();
            let app = temporary.path().join("app");
            std::fs::create_dir(&app).unwrap();
            let executable = app.join("Codex.exe");
            std::fs::write(&executable, []).unwrap();

            assert_eq!(
                installation_from_path(&executable).unwrap().executable,
                executable
            );
            assert_eq!(
                installation_from_path(temporary.path()).unwrap().executable,
                executable
            );
        }

        #[test]
        fn ignores_missing_override() {
            assert!(installation_from_path(Path::new("Z:\\missing-codex")).is_none());
        }

        #[test]
        fn resolves_packaged_executable_from_manifest() {
            let temporary = tempfile::tempdir().unwrap();
            let binaries = temporary.path().join("binaries");
            std::fs::create_dir(&binaries).unwrap();
            let executable = binaries.join("DesktopClient.exe");
            std::fs::write(&executable, []).unwrap();
            std::fs::write(
                temporary.path().join("AppxManifest.xml"),
                r#"<Package><Applications><Application Id="App" Executable="binaries\DesktopClient.exe" /></Applications></Package>"#,
            )
            .unwrap();

            assert_eq!(
                installation_from_path(temporary.path()).unwrap().executable,
                executable
            );
        }

        #[test]
        fn starts_cdp_on_loopback_only() {
            assert_eq!(
                launch_arguments(true),
                [
                    "--remote-debugging-address=127.0.0.1",
                    "--remote-debugging-port=9229"
                ]
            );
            assert_eq!(launch_arguments(false), ["", ""]);
        }
    }
}

#[cfg(target_os = "macos")]
mod macos_platform {
    use super::*;
    pub fn discover() -> Result<Installation> {
        let home = dirs::home_dir().unwrap_or_default();
        let candidates = [
            std::env::var_os("CODEX_APP_PATH").map(PathBuf::from),
            Some(PathBuf::from("/Applications/Codex.app")),
            Some(home.join("Applications/Codex.app")),
            Some(PathBuf::from("/Applications/ChatGPT.app")),
            Some(home.join("Applications/ChatGPT.app")),
        ];
        for app_path in candidates.into_iter().flatten() {
            let directory = app_path.join("Contents/MacOS");
            for name in ["Codex", "ChatGPT", "OpenAI Codex"] {
                let executable = directory.join(name);
                if executable.is_file() {
                    return Ok(Installation {
                        app_path,
                        executable,
                    });
                }
            }
            if let Ok(mut files) = std::fs::read_dir(&directory) {
                if let Some(Ok(entry)) = files.next() {
                    return Ok(Installation {
                        app_path,
                        executable: entry.path(),
                    });
                }
            }
        }
        Err(AppError::Message(
            "未找到 macOS Codex 安装，可通过 CODEX_APP_PATH 指定位置。".into(),
        ))
    }
    fn launch_arguments(enable_cdp: bool) -> Vec<&'static str> {
        if enable_cdp {
            vec![
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=9229",
            ]
        } else {
            Vec::new()
        }
    }

    pub fn start(installation: &Installation, enable_cdp: bool) -> Result<()> {
        // Starting the bundle through `open -n` can leave Electron's profile lock or
        // debug arguments behind. Spawn the app executable only after `stop` has
        // observed the previous process tree exit, so the debugging port is owned by
        // the new Codex instance.
        std::process::Command::new(&installation.executable)
            .args(launch_arguments(enable_cdp))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| AppError::Message(format!("启动 Codex 失败：{error}")))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn starts_cdp_on_loopback_only() {
            assert_eq!(
                launch_arguments(true),
                [
                    "--remote-debugging-address=127.0.0.1",
                    "--remote-debugging-port=9229"
                ]
            );
            assert!(launch_arguments(false).is_empty());
        }
    }
}
