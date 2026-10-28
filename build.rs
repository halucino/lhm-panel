// 把图标与版本信息写进 exe 的 Windows 资源段，
// 这样任务栏、资源管理器、托盘都能用 LoadImageW 从资源里取图标。
#[cfg(windows)]
fn main() {
    if std::env::var_os("CARGO_CFG_TARGET_OS").map_or(false, |v| v == "windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "LHM Panel");
        res.set("FileDescription", "Desktop hardware monitor panel for LibreHardwareMonitor");
        res.set("ProductVersion", "0.1.0.0");
        res.set("FileVersion", "0.1.0.0");
        res.set("LegalCopyright", "MIT License");
        if let Err(e) = res.compile() {
            eprintln!("winres 编译资源失败: {e}");
        }
    }
}

#[cfg(not(windows))]
fn main() {}
