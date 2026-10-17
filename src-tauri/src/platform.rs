//! 平台辅助（M3/M4/M5）：跟随系统主题检测（FR-10.2）、系统编辑器打开文件（FR-9.1）、
//! 资源管理器/终端打开目录（FR-10.5）。
//!
//! 通过各平台标准机制读取系统外观偏好，失败时回退浅色。
//! 仅在启动与用户切换到"跟随系统"时检测一次，不监听运行期变化。

use std::path::Path;

/// 系统是否偏好深色外观。
pub fn os_prefers_dark() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_prefers_dark()
    }
    #[cfg(target_os = "macos")]
    {
        macos_prefers_dark()
    }
    #[cfg(target_os = "linux")]
    {
        linux_prefers_dark()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// 静默执行外部命令（Windows 下不闪控制台窗口）。
fn run_command(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(target_os = "windows")]
fn windows_prefers_dark() -> bool {
    // HKCU\...\Themes\Personalize : AppsUseLightTheme（DWORD，0 = 深色应用）
    let text = run_command(
        "reg",
        &[
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "/v",
            "AppsUseLightTheme",
        ],
    )
    .unwrap_or_default();
    for line in text.lines() {
        if line.contains("AppsUseLightTheme") {
            return line.trim_end().ends_with("0x0");
        }
    }
    false
}

#[cfg(target_os = "macos")]
fn macos_prefers_dark() -> bool {
    // 浅色模式下该键不存在（命令失败）
    run_command("defaults", &["read", "-g", "AppleInterfaceStyle"])
        .map(|s| s.contains("Dark"))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn linux_prefers_dark() -> bool {
    // GNOME / Fedora：color-scheme = prefer-dark
    if let Some(out) = run_command(
        "gsettings",
        &["get", "org.gnome.desktop.interface", "color-scheme"],
    ) {
        if out.contains("dark") {
            return true;
        }
    }
    // KDE / 通用 GTK 主题名兜底
    run_command(
        "gsettings",
        &["get", "org.gnome.desktop.interface", "gtk-theme"],
    )
    .map(|s| s.to_lowercase().contains("dark"))
    .unwrap_or(false)
}

/// 用系统默认编辑器打开文件（FR-9.1）：Windows 文件关联 / macOS open / Linux xdg-open。
pub fn open_in_editor(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let file = path.to_string_lossy().to_string();
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &file])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "linux")]
    {
        linux_open(&path.to_string_lossy())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no editor launcher for this platform",
        ))
    }
}

/// 在系统文件管理器中打开目录（FR-10.5）：Windows 资源管理器 / macOS Finder 进入该目录 /
/// Linux gio open → 按桌面环境与常见文件管理器逐个回退 → xdg-open。仅接收目录路径。
pub fn reveal_in_explorer(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path.to_string_lossy().to_string())
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        // open <dir> = Finder 进入该目录（与 Windows explorer 行为一致；open -R 是打开
        // 父目录并高亮，与「打开所在文件夹」语义不符）
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "linux")]
    {
        let target = path.to_string_lossy().to_string();
        // 1) gio open：尊重用户默认文件管理器；glib2 随附几乎必有；失败只返回非零，
        //    不会像 xdg-open（XFCE 上委托 exo-open）那样弹系统错误框
        if let Ok(st) = std::process::Command::new("gio").arg("open").arg(&target).status() {
            if st.success() {
                return Ok(());
            }
        }
        // 2) 桌面环境专属 + 常见文件管理器逐个尝试：xdg-open 默认关联损坏
        //    （如「Failed to execute default File Manager」）时仍能打开
        let de = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase();
        let mut candidates: Vec<&str> = Vec::new();
        if de.contains("gnome") {
            candidates.push("nautilus");
        } else if de.contains("kde") {
            candidates.push("dolphin");
        } else if de.contains("xfce") {
            candidates.push("thunar");
        } else if de.contains("mate") {
            candidates.push("caja");
        } else if de.contains("x-cinnamon") {
            candidates.push("nemo");
        } else if de.contains("lxqt") {
            candidates.push("pcmanfm-qt");
        }
        for prog in ["nautilus", "dolphin", "thunar", "caja", "nemo", "pcmanfm-qt", "pcmanfm"] {
            if !candidates.contains(&prog) {
                candidates.push(prog);
            }
        }
        for prog in &candidates {
            if std::process::Command::new(prog).arg(&target).spawn().is_ok() {
                return Ok(());
            }
        }
        // 3) 最后兜底 xdg-open（无 gio 且无已知文件管理器时的最后尝试）
        std::process::Command::new("xdg-open")
            .arg(&target)
            .spawn()
            .map(|_| ())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no file manager launcher for this platform",
        ))
    }
}

/// Linux 通用打开（gio open 优先，失败回退 xdg-open）：默认编辑器 / 浏览器共用。
#[cfg(target_os = "linux")]
fn linux_open(target: &str) -> std::io::Result<()> {
    if let Ok(st) = std::process::Command::new("gio").arg("open").arg(target).status() {
        if st.success() {
            return Ok(());
        }
    }
    std::process::Command::new("xdg-open").arg(target).spawn().map(|_| ())
}

/// 用系统默认浏览器打开 URL（关于对话框官网链接）。
pub fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "linux")]
    {
        linux_open(url)
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no browser launcher for this platform",
        ))
    }
}

/// 在指定目录打开系统终端（FR-10.5）：Windows 新控制台 / macOS Terminal / Linux 常见终端模拟器。
pub fn open_in_terminal(dir: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        // cmd 以新控制台启动，current_dir 即终端工作目录
        std::process::Command::new("cmd")
            .current_dir(dir)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-a", "Terminal", &dir.to_string_lossy()])
            .spawn()
            .map(|_| ())
    }
    #[cfg(target_os = "linux")]
    {
        // 依桌面环境常见顺序尝试；全部失败报错
        for prog in [
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
        ] {
            let mut cmd = std::process::Command::new(prog);
            if prog == "gnome-terminal" {
                cmd.arg(format!("--working-directory={}", dir.display()));
            } else {
                cmd.current_dir(dir);
            }
            if cmd.spawn().is_ok() {
                return Ok(());
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no terminal emulator found",
        ))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no terminal launcher for this platform",
        ))
    }
}
