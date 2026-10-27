// 把图标与版本信息写进 exe 的 Windows 资源段：
// 资源管理器里能看到图标、产品名、版本号、版权。
#[cfg(windows)]
fn main() {
    if std::env::var_os("CARGO_CFG_TARGET_OS").map_or(false, |v| v == "windows") {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "LHM Panel");
        res.set("FileDescription", "Desktop hardware monitor panel for LibreHardwareMonitor");
        res.set("ProductVersion", "1.0.0.0");
        res.set("FileVersion", "1.0.0.0");
        res.set("LegalCopyright", "MIT License");
        res.set("OriginalFilename", "lhm-panel.exe");
        if let Err(e) = res.compile() {
            eprintln!("winres 编译资源失败: {e}");
        }
    }
}

#[cfg(not(windows))]
fn main() {}
