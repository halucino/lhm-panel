#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod tray;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use serde::{Deserialize, Serialize};
use wmi::WMIConnection;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ---------------- 配色 ----------------
const C_TEXT: egui::Color32 = egui::Color32::from_rgb(232, 232, 238);
const C_DIM: egui::Color32 = egui::Color32::from_rgb(150, 152, 164);
const C_GREEN: egui::Color32 = egui::Color32::from_rgb(74, 222, 128);
const C_YELLOW: egui::Color32 = egui::Color32::from_rgb(250, 204, 21);
const C_ORANGE: egui::Color32 = egui::Color32::from_rgb(251, 146, 60);
const C_RED: egui::Color32 = egui::Color32::from_rgb(239, 68, 68);
const C_DEVICE: egui::Color32 = egui::Color32::from_rgb(108, 156, 214);

// ---------------- 日志 ----------------

fn data_dir() -> PathBuf {
    // 优先放在 exe 同级的 data 目录：一定可写，也不受环境变量影响。
    // 若 exe 目录不可写（例如装在 Program Files），再退回 APPDATA。
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let local = dir.join("data");
            if std::fs::create_dir_all(&local).is_ok() {
                let probe = local.join(".writable");
                if std::fs::write(&probe, b"1").is_ok() {
                    let _ = std::fs::remove_file(&probe);
                    return local;
                }
            }
        }
    }
    let base = std::env::var("APPDATA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("LOCALAPPDATA").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| ".".to_string());
    PathBuf::from(base).join("LhmPanel")
}

fn log_line(msg: &str) {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("panel.log"))
    {
        let _ = writeln!(f, "{msg}");
    }
}

// ---------------- 传感器 ----------------

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
struct RawSensor {
    name: String,
    identifier: String,
    sensor_type: String,
    value: f32,
    #[serde(default)]
    parent: String,
}

#[derive(Debug, Clone)]
struct Sensor {
    name: String,
    identifier: String,
    kind: String,
    value: f32,
    device: String,
}

fn device_from_identifier(id: &str) -> String {
    let mut parts = id.trim_start_matches('/').split('/');
    let a = parts.next().unwrap_or("device");
    let b = parts.next().unwrap_or("");
    if b.is_empty() {
        a.to_string()
    } else {
        format!("{a}/{b}")
    }
}

impl From<RawSensor> for Sensor {
    fn from(r: RawSensor) -> Self {
        let device = if r.parent.trim().is_empty() {
            device_from_identifier(&r.identifier)
        } else {
            r.parent.trim().to_string()
        };
        Self {
            name: r.name,
            identifier: r.identifier,
            kind: r.sensor_type,
            value: r.value,
            device,
        }
    }
}

fn fmt_value(kind: &str, v: f32) -> String {
    match kind {
        "Temperature" => format!("{v:.1} °C"),
        "Load" | "Level" => format!("{v:.0} %"),
        "Clock" => format!("{v:.0} MHz"),
        "Voltage" => format!("{v:.3} V"),
        "Power" => format!("{v:.1} W"),
        "Fan" => format!("{v:.0} RPM"),
        "Data" => format!("{v:.1} GB"),
        "SmallData" => format!("{v:.1} MB"),
        "Throughput" => format!("{v:.1} MB/s"),
        _ => format!("{v:.1}"),
    }
}

fn frac_for(s: &Sensor) -> Option<f32> {
    match s.kind.as_str() {
        "Load" | "Level" => Some((s.value / 100.0).clamp(0.0, 1.0)),
        "Temperature" => Some(((s.value - 30.0) / 65.0).clamp(0.0, 1.0)),
        _ => None,
    }
}

fn value_color(s: &Sensor) -> egui::Color32 {
    let v = s.value;
    match s.kind.as_str() {
        "Temperature" => {
            if v < 55.0 {
                C_GREEN
            } else if v < 70.0 {
                C_YELLOW
            } else if v < 82.0 {
                C_ORANGE
            } else {
                C_RED
            }
        }
        "Load" | "Level" => {
            if v < 50.0 {
                C_GREEN
            } else if v < 80.0 {
                C_YELLOW
            } else {
                C_ORANGE
            }
        }
        _ => C_TEXT,
    }
}

/// 硬盘类设备：这类传感器的名字（Temperature #1、Used Space、Life…）在不同盘之间完全重名，
/// 需要在标签里带上设备名才能分辨。
fn is_disk_device(device: &str) -> bool {
    let d = device.to_ascii_lowercase();
    d.contains("ssd")
        || d.contains("nvme")
        || d.contains("hdd")
        || d.contains("drive")
        || d.contains("t700")
        || d.contains("wd_")
        || d.contains("crucial")
        || d.contains("samsung")
        || d.contains("kingston")
        || d.contains("aigo")
}

/// 阈值/告警类传感器：显示的是盘的报警点（如 Warning 119°C），不是实测温度，
/// 混进面板会被误读成过热，默认不勾选、也不提供勾选。
fn is_threshold_sensor(s: &Sensor) -> bool {
    let n = s.name.to_ascii_lowercase();
    n.contains("warning temperature")
        || n.contains("critical temperature")
        || n.contains("warning temp")
        || n.contains("critical temp")
        || n.contains("threshold")
        || n.contains("alarm")
}

/// 面板/设置页里显示的名字：硬盘类带上设备前缀，其他保持原名。
fn display_name(s: &Sensor) -> String {
    if is_disk_device(&s.device) {
        format!("{} · {}", s.device, s.name)
    } else {
        s.name.clone()
    }
}

fn short_disk_name(device: &str) -> String {
    let d = device.trim();
    let low = d.to_ascii_lowercase();
    if low.starts_with("ct") && low.contains("t700") {
        let cap = if low.contains("2000") {
            "2TB"
        } else if low.contains("1000") {
            "1TB"
        } else if low.contains("4000") {
            "4TB"
        } else {
            ""
        };
        return if cap.is_empty() {
            "Crucial T700".to_string()
        } else {
            format!("Crucial T700 {cap}")
        };
    }
    let mut parts: Vec<&str> = Vec::new();
    for tok in d.split_whitespace() {
        let t = tok.to_ascii_lowercase();
        if t == "nvme" || t == "ssd" || t == "pcie" || t == "gen4" || t == "gen5" {
            continue;
        }
        parts.push(tok);
    }
    let joined = parts.join(" ");
    if joined.is_empty() {
        d.to_string()
    } else {
        joined
    }
}

fn device_rank(device: &str) -> u8 {
    let d = device.to_ascii_lowercase();
    if d.contains("ryzen") || d.contains("intel") || d.contains("cpu") {
        0
    } else if d.contains("nvidia") || d.contains("radeon") || d.contains("arc") || d.contains("gpu") {
        1
    } else if is_disk_device(device) {
        3
    } else if d.contains("memory") || d.contains("ram") {
        2
    } else {
        4
    }
}

fn kind_rank(kind: &str) -> u8 {
    match kind {
        "Temperature" => 0,
        "Load" => 1,
        "Clock" => 2,
        "Power" => 3,
        "Voltage" => 4,
        "Fan" => 5,
        "Data" => 6,
        "SmallData" => 7,
        "Throughput" => 8,
        _ => 9,
    }
}

fn kind_label(kind: &str) -> String {
    let cn = match kind {
        "Temperature" => "温度",
        "Load" => "占用",
        "Clock" => "频率",
        "Power" => "功耗",
        "Voltage" => "电压",
        "Fan" => "风扇",
        "Data" => "容量",
        "SmallData" => "小容量",
        "Throughput" => "吞吐",
        "Level" => "液位/电量",
        "Factor" => "系数",
        "Timing" => "时序",
        _ => "其他",
    };
    format!("{cn} · {kind}")
}

fn is_default_enabled(s: &Sensor) -> bool {
    // 阈值/告警类（Warning 119°C、Critical 129°C）永远不默认勾选，
    // 否则面板上会出现两个吓人的红字，其实只是盘的报警门限
    if is_threshold_sensor(s) {
        return false;
    }
    match s.kind.as_str() {
        "Temperature" => {
            let n = s.name.to_lowercase();
            // 硬盘只默认勾「复合温度 / Composite」，其余 Temperature #1/#2 不勾，避免一堆重复数字
            if is_disk_device(&s.device) {
                return n.contains("composite");
            }
            !s.name.contains("Core #") && !n.contains("distance") && !n.contains("drive")
        }
        "Load" => {
            s.name == "CPU Total" || s.name == "GPU Core" || s.identifier.contains("/ram/")
        }
        "Fan" => true,
        _ => false,
    }
}

// ---------------- 配置 ----------------

#[derive(Serialize, Deserialize, Clone)]
struct Config {
    width: f32,
    height: f32,
    /// 窗口左上角的屏幕坐标（**逻辑像素**）。None 表示首次运行，交给系统决定位置。
    /// 早期版本误存为物理像素，见 `pos_migrated`。
    #[serde(default)]
    pos_x: Option<i32>,
    #[serde(default)]
    pos_y: Option<i32>,
    /// 坐标单位迁移标记：false 表示还是旧版物理像素值，需要丢弃一次。
    #[serde(default)]
    pos_migrated: bool,
    opacity: f32,
    font_scale: f32,
    show_bars: bool,
    #[serde(default)]
    bars_migrated: bool,
    auto_launch: bool,
    lhm_path: String,
    autostart: bool,
    enabled: BTreeSet<String>,
    initialized: bool,
}

const DEFAULT_LHM: &str = "E:\\soft\\LibreHardwareMonitor\\LibreHardwareMonitor.exe";

impl Default for Config {
    fn default() -> Self {
        Self {
            width: 380.0,
            height: 440.0,
            pos_x: None,
            pos_y: None,
            pos_migrated: true,
            opacity: 0.92,
            font_scale: 1.0,
            show_bars: false,
            bars_migrated: true,
            auto_launch: true,
            lhm_path: DEFAULT_LHM.to_string(),
            autostart: false,
            enabled: BTreeSet::new(),
            initialized: false,
        }
    }
}

fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

fn load_config() -> Config {
    let mut cfg = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Config>(&s).ok())
        .unwrap_or_default();
    // 以注册表实际状态为准，避免配置与系统不一致
    cfg.autostart = autostart_enabled();
    // 一次性迁移：旧配置里柱状图是开着的，按要求统一关掉。
    // 之后用户可以在设置里手动打开，这里不再覆盖。
    if !cfg.bars_migrated {
        cfg.show_bars = false;
        cfg.bars_migrated = true;
    }
    // 坐标单位迁移：旧版把物理像素直接存进 pos_x/pos_y，
    // 现在统一按逻辑像素处理，旧值若继续用会再漂一次（DPI≠100% 时）。
    // 直接丢弃旧坐标，让系统重新居中放置一次，之后按新单位记录。
    if !cfg.pos_migrated {
        cfg.pos_x = None;
        cfg.pos_y = None;
        cfg.pos_migrated = true;
        log_line("坐标单位迁移：已丢弃旧版物理像素坐标");
    }
    cfg
}

fn save_config(cfg: &Config) {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(s) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(config_path(), s);
    }
}

// ---------------- 以管理员启动 LHM ----------------

fn launch_lhm_admin(path: &str) -> bool {
    if !std::path::Path::new(path).exists() {
        log_line(&format!("LHM 路径不存在: {path}"));
        return false;
    }
    let esc = path.replace('\'', "''");
    let cmd = format!("Start-Process -FilePath '{esc}' -Verb RunAs");
    let mut c = std::process::Command::new("powershell");
    c.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &cmd]);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    match c.spawn() {
        Ok(_) => {
            log_line(&format!("已请求以管理员启动 LHM: {path}"));
            true
        }
        Err(e) => {
            log_line(&format!("启动 LHM 失败: {e}"));
            false
        }
    }
}

// ---------------- 数据来源 ----------------

static SOURCE: AtomicU8 = AtomicU8::new(0);

fn parse_leading_f32(s: &str) -> Option<f32> {
    let t = s.trim();
    let end = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(t.len());
    t[..end].parse::<f32>().ok()
}

fn kind_from_path(path: &str) -> &'static str {
    let p = path.to_ascii_lowercase();
    if p.contains("/temperature") {
        "Temperature"
    } else if p.contains("/load") {
        "Load"
    } else if p.contains("/clock") {
        "Clock"
    } else if p.contains("/voltage") {
        "Voltage"
    } else if p.contains("/power") {
        "Power"
    } else if p.contains("/fan") {
        "Fan"
    } else if p.contains("/throughput") {
        "Throughput"
    } else if p.contains("/smalldata") {
        "SmallData"
    } else if p.contains("/data") {
        "Data"
    } else if p.contains("/level") {
        "Level"
    } else {
        "Other"
    }
}

fn walk_json(node: &serde_json::Value, device: &str, out: &mut Vec<Sensor>) {
    let text = node.get("Text").and_then(|v| v.as_str()).unwrap_or("");
    let value = node.get("Value").and_then(|v| v.as_str()).unwrap_or("");
    // LHM 的传感器真实 ID 在 SensorId（形如 /amdcpu/0/temperature/2），
    // id 字段只是个整数序号，不能用。
    let sensor_id = node.get("SensorId").and_then(|v| v.as_str()).unwrap_or("");
    let sensor_type = node.get("Type").and_then(|v| v.as_str()).unwrap_or("");
    let hardware_id = node.get("HardwareId").and_then(|v| v.as_str()).unwrap_or("");
    let children = node.get("Children").and_then(|v| v.as_array());
    let has_children = children.map(|c| !c.is_empty()).unwrap_or(false);

    // 叶子节点 = 一个传感器
    if !has_children {
        if sensor_id.is_empty() || text.is_empty() || value.trim().is_empty() {
            return;
        }
        let Some(v) = parse_leading_f32(value) else {
            return;
        };
        if !v.is_finite() {
            return;
        }
        out.push(Sensor {
            name: text.to_string(),
            // LHM 里同一个 SensorId 可能对应多个传感器（GPU Memory 与 GPU Bus
            // 都报 /gpu-nvidia/0/load/3），只存 SensorId 会让它们共用勾选状态。
            identifier: format!("{sensor_id}|{sensor_type}|{text}"),
            kind: if sensor_type.is_empty() {
                kind_from_path(sensor_id).to_string()
            } else {
                sensor_type.to_string()
            },
            value: v,
            device: if device.is_empty() {
                "unknown".to_string()
            } else {
                device.to_string()
            },
        });
        return;
    }

    // 带 HardwareId 的节点是硬件本体，它的名字就是分组名
    let next_device = if !hardware_id.is_empty() && !text.is_empty() {
        text.to_string()
    } else {
        device.to_string()
    };

    if let Some(cs) = children {
        for c in cs {
            walk_json(c, &next_device, out);
        }
    }
}

fn http_sensors() -> Option<Vec<Sensor>> {
    use std::io::Read;
    use std::net::TcpStream;

    let addr: std::net::SocketAddr = "127.0.0.1:8085".parse().ok()?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(250)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(1500))).ok()?;
    s.write_all(b"GET /data.json HTTP/1.1\r\nHost: 127.0.0.1:8085\r\nConnection: close\r\n\r\n")
        .ok()?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let body = text.split("\r\n\r\n").nth(1)?;
    let root: serde_json::Value = serde_json::from_str(body).ok()?;
    let mut out = Vec::new();
    walk_json(&root, "", &mut out);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

// ---------------- 采集线程 ----------------

fn spawn_collector(interval: Arc<AtomicU64>, tx: Sender<Vec<Sensor>>) {
    std::thread::spawn(move || {
        let mut conn: Option<WMIConnection> = None;
        loop {
            let mut out: Vec<Sensor> = Vec::new();

            if conn.is_none() {
                conn = WMIConnection::with_namespace_path("ROOT\\LibreHardwareMonitor").ok();
            }

            if let Some(c) = &conn {
                match c.raw_query::<RawSensor>(
                    "SELECT Name, Identifier, SensorType, Value, Parent FROM Sensor",
                ) {
                    Ok(list) => {
                        for r in list {
                            if r.value.is_finite() {
                                out.push(Sensor::from(r));
                            }
                        }
                    }
                    Err(_) => conn = None,
                }
            }

            let src = if !out.is_empty() {
                1u8
            } else {
                match http_sensors() {
                    Some(v) => {
                        out = v;
                        2u8
                    }
                    None => 0u8,
                }
            };
            SOURCE.store(src, Ordering::Relaxed);

            let _ = tx.send(out);
            let ms = interval.load(Ordering::Relaxed).max(200);
            std::thread::sleep(Duration::from_millis(ms));
        }
    });
}

// ---------------- 开机自启（HKCU Run 键） ----------------

const AUTOSTART_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const AUTOSTART_NAME: &str = "LhmPanel";

fn reg_run(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("reg")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn autostart_enabled() -> bool {
    reg_run(&["query", AUTOSTART_KEY, "/v", AUTOSTART_NAME])
}

fn set_autostart(on: bool) -> bool {
    if on {
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if exe.is_empty() {
            return false;
        }
        reg_run(&[
            "add",
            AUTOSTART_KEY,
            "/v",
            AUTOSTART_NAME,
            "/t",
            "REG_SZ",
            "/d",
            &format!("\"{exe}\""),
            "/f",
        ])
    } else {
        reg_run(&["delete", AUTOSTART_KEY, "/v", AUTOSTART_NAME, "/f"])
    }
}

// ---------------- 应用 ----------------

struct PanelApp {
    rx: Receiver<Vec<Sensor>>,
    sensors: Vec<Sensor>,
    cfg: Config,
    show_settings: bool,
    dirty: bool,
    last_save: Instant,
    interval: Arc<AtomicU64>,
    started: Instant,
    launch_tried: bool,
    launch_note: String,
    saved_once: bool,
    /// 系统托盘。创建失败时为 None，不影响主功能。
    tray: Option<crate::tray::Tray>,
    /// 是否已用 Win32 API 恢复过窗口位置（只做一次）
    pos_restored: bool,
    /// 在此时间点之前不记录窗口坐标，避免把恢复过程中的中间态写回配置
    skip_pos_record_until: Instant,
}

fn install_fonts(ctx: &egui::Context) {
    const CANDIDATES: [&str; 4] = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\Deng.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
    ];
    for p in CANDIDATES {
        let Ok(bytes) = std::fs::read(p) else { continue };
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("cjk".to_owned(), egui::FontData::from_owned(bytes));
        if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            fam.insert(0, "cjk".to_owned());
        }
        if let Some(fam) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
            fam.push("cjk".to_owned());
        }
        ctx.set_fonts(fonts);
        log_line(&format!("字体已加载: {p}"));
        return;
    }
    log_line("未找到中文字体，使用内置字体");
}

impl PanelApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        log_line("PanelApp::new");
        install_fonts(&cc.egui_ctx);
        let cfg = load_config();
        cc.egui_ctx
            .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                cfg.width, cfg.height,
            )));
        let interval = Arc::new(AtomicU64::new(700));
        let (tx, rx) = channel();
        spawn_collector(interval.clone(), tx);

        // 托盘图标：用 32×32 的小图。
        // Windows 托盘期望 16/32/48 尺寸，1024×1024 的原图会被 Shell 拒绝，
        // 表现为「注册成功但看不到图标」。
        const TRAY_PNG: &[u8] = include_bytes!("../assets/tray32.png");
        let tray = crate::tray::Tray::new(TRAY_PNG, cfg.autostart);
        match &tray {
            Some(_) => log_line("托盘图标已创建"),
            None => log_line("托盘创建失败，仅使用主窗口"),
        }

        Self {
            rx,
            sensors: Vec::new(),
            cfg,
            show_settings: false,
            dirty: false,
            last_save: Instant::now(),
            interval,
            started: Instant::now(),
            launch_tried: false,
            launch_note: String::new(),
            saved_once: false,
            tray,
            pos_restored: false,
            skip_pos_record_until: Instant::now(),
        }
    }

    /// 把已勾选的传感器按设备分组，组内按「类型 → 名称」排序。
    /// 返回的设备顺序沿用首次出现的顺序，保证 CPU 通常在最上面。
    fn grouped_enabled(&self) -> Vec<(String, Vec<Sensor>)> {
        let mut order: Vec<String> = Vec::new();
        let mut buckets: std::collections::HashMap<String, Vec<Sensor>> =
            std::collections::HashMap::new();

        for s in self.sensors.iter() {
            if !self.cfg.enabled.contains(&s.identifier) {
                continue;
            }
            if !buckets.contains_key(&s.device) {
                order.push(s.device.clone());
            }
            buckets
                .entry(s.device.clone())
                .or_default()
                .push(s.clone());
        }

        // 让 CPU / GPU 排在前面，硬盘与网卡靠后
        order.sort_by_key(|d| device_rank(d));

        order
            .into_iter()
            .map(|dev| {
                let mut list = buckets.remove(&dev).unwrap_or_default();
                list.sort_by(|a, b| {
                    kind_rank(&a.kind)
                        .cmp(&kind_rank(&b.kind))
                        .then(a.name.cmp(&b.name))
                });
                (dev, list)
            })
            .collect()
    }

    /// 一行传感器：左边名称，右边彩色数值。
    /// 设备前缀已由调用方按组合并成一条标题，这里不再重复。
    fn draw_row(ui: &mut egui::Ui, s: &Sensor, fs: f32, bars: bool) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&s.name).size(11.5 * fs).color(C_DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(fmt_value(&s.kind, s.value))
                        .size(13.0 * fs)
                        .strong()
                        .color(value_color(s)),
                );
            });
        });
        if bars {
            if let Some(f) = frac_for(s) {
                let w = ui.available_width().max(20.0);
                ui.add(
                    egui::ProgressBar::new(f)
                        .desired_width(w)
                        .fill(value_color(s)),
                );
            }
        }
        ui.add_space(3.0);
    }

    fn ui_dashboard(&mut self, ui: &mut egui::Ui) {
        if self.sensors.is_empty() {
            let fs = self.cfg.font_scale;
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new("未检测到 LibreHardwareMonitor")
                    .size(13.0 * fs)
                    .color(C_ORANGE),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(
                    "LHM 0.9.6 没有 WMI 接口，请打开它的 Web Server：\nOptions → Remote Web Server → Run（端口 8085）。\n\nLHM 本身也必须以管理员身份运行，否则读不到 CPU 温度。",
                )
                .size(11.0 * fs)
                .color(C_DIM),
            );
            if !self.launch_note.is_empty() {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(&self.launch_note)
                        .size(10.5 * fs)
                        .color(C_DIM),
                );
            }
            return;
        }

        let groups = self.grouped_enabled();
        if groups.is_empty() {
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new("还没有选择任何传感器 → 点右上角 ⚙")
                    .size(12.0 * self.cfg.font_scale)
                    .color(C_DIM),
            );
            return;
        }

        let fs = self.cfg.font_scale;
        let bars = self.cfg.show_bars;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (idx, (device, list)) in groups.iter().enumerate() {
                    if idx > 0 {
                        ui.add_space(2.0);
                    }
                    // 设备标题只画一次：相同前缀天然合并成一组
                    let title = if is_disk_device(device) {
                        short_disk_name(device)
                    } else {
                        device.clone()
                    };
                    ui.label(
                        egui::RichText::new(title)
                            .size(10.5 * fs)
                            .strong()
                            .color(C_DEVICE),
                    );
                    ui.add_space(1.0);
                    for s in list {
                        Self::draw_row(ui, s, fs, bars);
                    }
                }
            });
    }

    fn ui_settings(&mut self, ui: &mut egui::Ui) {
        let fs = self.cfg.font_scale;
        ui.horizontal(|ui| {
            if ui.button("← 返回").clicked() {
                self.show_settings = false;
                self.dirty = true;
            }
            ui.label(egui::RichText::new("设置").size(13.0 * fs).strong());
        });
        ui.separator();

        let mut changed = false;
        // 默认不画柱状图，只保留按温度/占用变色的数值文字。
        // 老配置文件里 show_bars 可能仍是 true，这里用启动时的一次性迁移把它关掉。
        changed |= ui
            .add(egui::Slider::new(&mut self.cfg.opacity, 0.25..=1.0).text("不透明度"))
            .changed();
        changed |= ui
            .add(egui::Slider::new(&mut self.cfg.font_scale, 0.75..=2.2).text("字号"))
            .changed();
        changed |= ui.checkbox(&mut self.cfg.show_bars, "显示柱状进度条").changed();

        let mut secs = self.interval.load(Ordering::Relaxed) as f32 / 1000.0;
        if ui
            .add(egui::Slider::new(&mut secs, 0.3..=5.0).text("刷新间隔 (秒)"))
            .changed()
        {
            self.interval
                .store((secs * 1000.0) as u64, Ordering::Relaxed);
        }

        ui.separator();
        ui.label(egui::RichText::new("启动").size(11.5 * fs).strong());
        let mut auto_start = self.cfg.autostart;
        if ui
            .checkbox(&mut auto_start, "开机自动启动（登录 Windows 后自动运行面板）")
            .changed()
        {
            self.cfg.autostart = auto_start;
            changed = true;
            if set_autostart(auto_start) {
                self.launch_note = if auto_start {
                    "已加入开机自启".to_string()
                } else {
                    "已取消开机自启".to_string()
                };
            } else {
                self.launch_note = "写入开机自启注册表失败".to_string();
            }
        }

        ui.separator();
        ui.label(egui::RichText::new("LibreHardwareMonitor").size(11.5 * fs).strong());
        changed |= ui
            .checkbox(&mut self.cfg.auto_launch, "启动面板时自动以管理员拉起 LHM")
            .changed();
        ui.horizontal(|ui| {
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.cfg.lhm_path)
                        .desired_width(240.0)
                        .font(egui::TextStyle::Small),
                )
                .changed();
        });
        ui.horizontal(|ui| {
            if ui.button("立即以管理员启动 LHM").clicked() {
                let p = self.cfg.lhm_path.clone();
                let ok = launch_lhm_admin(&p);
                self.launch_note = if ok {
                    "已请求提权启动，请在 UAC 弹窗点『是』".to_string()
                } else {
                    "启动失败，请检查路径".to_string()
                };
                self.launch_tried = true;
            }
        });
        if !self.launch_note.is_empty() {
            ui.label(
                egui::RichText::new(&self.launch_note)
                    .size(10.5 * fs)
                    .color(C_DIM),
            );
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("全选").clicked() {
                for s in &self.sensors {
                    self.cfg.enabled.insert(s.identifier.clone());
                }
                changed = true;
            }
            if ui.button("清空").clicked() {
                self.cfg.enabled.clear();
                changed = true;
            }
            if ui.button("恢复默认").clicked() {
                self.cfg.enabled.clear();
                for s in &self.sensors {
                    if is_default_enabled(s) {
                        self.cfg.enabled.insert(s.identifier.clone());
                    }
                }
                changed = true;
            }
        });

        ui.separator();
        let snapshot: Vec<Sensor> = self.sensors.clone();
        let mut groups: BTreeMap<String, BTreeMap<String, Vec<&Sensor>>> = BTreeMap::new();
        for s in &snapshot {
            groups
                .entry(s.device.clone())
                .or_default()
                .entry(s.kind.clone())
                .or_default()
                .push(s);
        }

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if groups.is_empty() {
                    ui.label(egui::RichText::new("暂无传感器数据").color(C_TEXT));
                }
                for (device, kinds) in groups {
                    let mut ordered: Vec<(String, Vec<&Sensor>)> = kinds.into_iter().collect();
                    ordered.sort_by_key(|(k, _)| kind_rank(k));

                    egui::CollapsingHeader::new(
                        egui::RichText::new(device.clone())
                            .size(12.0 * fs)
                            .strong()
                            .color(C_TEXT),
                    )
                    .default_open(false)
                    .id_salt(format!("dev::{device}"))
                    .show(ui, |ui| {
                        for (kind, list) in ordered {
                            let total = list.len();
                            let on_count = list
                                .iter()
                                .filter(|s| self.cfg.enabled.contains(&s.identifier))
                                .count();

                            egui::CollapsingHeader::new(
                                egui::RichText::new(format!(
                                    "{} ({on_count}/{total})",
                                    kind_label(&kind)
                                ))
                                .size(11.0 * fs)
                                .color(C_TEXT),
                            )
                            .default_open(false)
                            .id_salt(format!("kind::{device}::{kind}"))
                            .show(ui, |ui| {
                                for s in &list {
                                    let mut on = self.cfg.enabled.contains(&s.identifier);
                                    let label = format!(
                                        "{}    {}",
                                        display_name(s),
                                        fmt_value(&s.kind, s.value)
                                    );
                                    if ui
                                        .checkbox(
                                            &mut on,
                                            egui::RichText::new(label)
                                                .size(11.5 * fs)
                                                .color(C_TEXT),
                                        )
                                        .changed()
                                    {
                                        if on {
                                            self.cfg.enabled.insert(s.identifier.clone());
                                        } else {
                                            self.cfg.enabled.remove(&s.identifier);
                                        }
                                        changed = true;
                                    }
                                }
                            });
                        }
                    });
                }
            });

        if changed {
            self.dirty = true;
        }
    }

    /// 顶部 10px 透明条：只画一根极淡的直线（直角），不显示任何文字。
    fn ui_drag_bar(&mut self, ui: &mut egui::Ui) {
        let w = ui.available_width().max(10.0);
        let (rect, _resp) = ui.allocate_exact_size(egui::vec2(w, 10.0), egui::Sense::hover());
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                rect.left_top(),
                egui::pos2(rect.right(), rect.top() + 1.0),
            ),
            0.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 14),
        );
    }

    /// 整窗任意位置按住都能拖动窗口。
    /// 用 CentralPanel 的最外层响应兜底：它是整块背景，天然位于所有子控件之下，
    /// 所以点按钮、滑杆、复选框仍然正常，只有空白处的拖拽才会移动窗口。
    fn ui_drag_everywhere(&mut self, ctx: &egui::Context, ui: &egui::Ui) {
        let rect = ui.max_rect();
        let id = egui::Id::new("window_drag_everywhere");
        let resp = ui.interact(rect, id, egui::Sense::drag());
        if !resp.dragged() {
            return;
        }
        // 底部 12px 是缩放手柄的领地，别抢它
        let on_bottom = ctx
            .input(|i| i.pointer.interact_pos())
            .map(|p| p.y > rect.bottom() - 12.0)
            .unwrap_or(false);
        if !on_bottom {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
    }

    /// 右下角浮动的 ⚙ / ✕，不占布局空间。
    fn ui_corner_buttons(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let fs = self.cfg.font_scale.clamp(0.8, 1.8);
        let btn = 15.0 * fs;
        let base = ui.max_rect();
        let area = egui::Rect::from_min_max(
            egui::pos2(base.right() - btn * 2.7, base.bottom() - btn * 1.3),
            base.right_bottom(),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::right_to_left(egui::Align::BOTTOM)),
        );
        let hit = |c: &mut egui::Ui, label: &str, tip: &str| -> bool {
            c.add(
                egui::Button::new(egui::RichText::new(label).size(btn * 0.8).color(C_DIM))
                    .fill(egui::Color32::TRANSPARENT)
                    .stroke(egui::Stroke::NONE)
                    .min_size(egui::vec2(btn, btn * 0.85)),
            )
            .on_hover_text(tip)
            .clicked()
        };
        // 退出按钮用 ❎ 更醒目，与 ⚙ 形成对比
        if hit(&mut child, "❎", "退出") {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if hit(&mut child, "⚙", "设置") {
            self.show_settings = !self.show_settings;
            self.dirty = true;
        }
    }

    /// 最下面一条 10px 的边缘：按住拖动即可改变窗口大小（只改宽高，不改位置）。
    fn ui_resize_grip(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        let w = ui.available_width().max(10.0);
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 10.0), egui::Sense::drag());
        if resp.hovered() || resp.dragged() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if resp.dragged() {
            let d = resp.drag_delta();
            let cur = ctx.input(|i| i.screen_rect().size());
            let next = egui::vec2(
                (cur.x + d.x).clamp(160.0, 2400.0),
                (cur.y + d.y).clamp(120.0, 2000.0),
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(next));
        }
        // 底边细线，提示这里可拖拽
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.center().y - 0.5),
                egui::pos2(rect.right(), rect.center().y + 0.5),
            ),
            0.0,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 16),
        );
    }
}

impl eframe::App for PanelApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut got = false;
        while let Ok(list) = self.rx.try_recv() {
            self.sensors = list;
            got = true;
        }
        if got && !self.cfg.initialized && !self.sensors.is_empty() {
            for s in &self.sensors {
                if is_default_enabled(s) {
                    self.cfg.enabled.insert(s.identifier.clone());
                }
            }
            self.cfg.initialized = true;
            self.dirty = true;
        }

        // ---- 托盘动作 ----
        // tray-icon 的事件通过全局 handler 投递到 channel，这里每帧取一次
        if let Some(tray) = &self.tray {
            while let Some(action) = tray.poll() {
                match action {
                    tray::TrayAction::Show => {
                        // 把窗口拉回前台；若已最小化则还原
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    tray::TrayAction::ToggleAutostart => {
                        let next = !self.cfg.autostart;
                        if set_autostart(next) {
                            self.cfg.autostart = next;
                            self.dirty = true;
                            tray.set_autostart_enabled(next);
                            log_line(&format!("托盘切换开机自启: {next}"));
                        } else {
                            log_line("托盘切换开机自启失败");
                        }
                    }
                    tray::TrayAction::Exit => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }

        // 没有数据且已等待 6 秒 → 自动提权拉起 LHM（只试一次）
        if self.sensors.is_empty()
            && self.cfg.auto_launch
            && !self.launch_tried
            && self.started.elapsed() > Duration::from_secs(6)
        {
            self.launch_tried = true;
            let p = self.cfg.lhm_path.clone();
            let ok = launch_lhm_admin(&p);
            self.launch_note = if ok {
                "已自动请求以管理员启动 LHM，请在 UAC 弹窗点『是』".to_string()
            } else {
                format!("自动启动失败：路径 {p}")
            };
        }

        // 记录窗口位置与尺寸。
        //
        // 单位说明（实测 200% 缩放验证）：
        //   - `outer_rect` 返回**物理像素**
        //   - eframe 0.29 的 `with_position` / `with_inner_size` 在 Windows 上
        //     同样按**物理像素**处理（不会乘 DPI）
        // 所以两端单位一致，直接用 `outer_rect` 的原值存盘即可，无需换算。
        // 早前误按逻辑像素处理，导致恢复后窗口大小和位置都缩半。
        if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
            let x = outer.min.x.round() as i32;
            let y = outer.min.y.round() as i32;
            let w = outer.width().round();
            let h = outer.height().round();

            // 位置恢复完成前，绝不记录坐标。
            //
            // 否则会踩到经典的「先写后读」：首帧记录逻辑把默认位置（如 123,123）
            // 写进 self.cfg，随后 restore_window_position 读到的就是被污染的值。
            // 注意这里只跳过「坐标」，不能让整个 update 提前返回，
            // 否则后面 saved_once 的首次落盘会被跳过，配置文件永远不生成。
            let can_record_pos = self.pos_restored
                && Instant::now() >= self.skip_pos_record_until;

            if can_record_pos
                && (self.cfg.pos_x != Some(x) || self.cfg.pos_y != Some(y))
            {
                self.cfg.pos_x = Some(x);
                self.cfg.pos_y = Some(y);
                self.dirty = true;
            }
            // 尺寸不受恢复流程影响，任何时候都可以记录；
            // 排除异常值，避免窗口最小化时把配置写坏
            if w >= 160.0 && h >= 120.0 && (self.cfg.width != w || self.cfg.height != h) {
                self.cfg.width = w;
                self.cfg.height = h;
                self.dirty = true;
            }
        }

        // 首次进入立刻落盘一次，避免用户没动任何控件时配置文件始终不存在
        if !self.saved_once {
            self.saved_once = true;
            save_config(&self.cfg);
            log_line("首次写入配置");

            // 修补扩展样式：任务栏有图标、Alt+Tab 不出现
            #[cfg(windows)]
            apply_taskbar_only_style();
        }

        // 位置恢复必须等窗口稳定后再做。
        // 首帧调 SetWindowPos 会被 winit 的后续初始化覆盖（实测设 (600,350) 后跑到 (62,62)），
        // 这里等到启动满 300ms 再执行一次，之后不再干预。
        #[cfg(windows)]
        if !self.pos_restored && self.started.elapsed() > Duration::from_millis(300) {
            self.pos_restored = true;
            restore_window_position(&self.cfg);
            // 刚恢复完，别让下面的记录逻辑把中间态写回配置
            self.skip_pos_record_until = Instant::now() + Duration::from_millis(500);
        }

        if self.dirty && self.last_save.elapsed() > Duration::from_millis(500) {
            save_config(&self.cfg);
            self.dirty = false;
            self.last_save = Instant::now();
        }

        let alpha = (255.0 * self.cfg.opacity.clamp(0.0, 1.0)) as u8;
        let bg = egui::Color32::from_rgba_unmultiplied(18, 18, 22, alpha);
        // 上边两个角画成直角，只保留下方圆角，贴住屏幕顶端时更干净
        let frame = egui::Frame::none()
            .fill(bg)
            .rounding(egui::Rounding {
                nw: 0.0,
                ne: 0.0,
                sw: 10.0,
                se: 10.0,
            })
            .inner_margin(egui::Margin::same(9.0));

        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            // 先铺一层全窗口拖拽响应，再画内容；控件天然压在上面
            self.ui_drag_everywhere(ctx, ui);
            self.ui_drag_bar(ui);
            if self.show_settings {
                self.ui_settings(ui);
            } else {
                self.ui_dashboard(ui);
            }
            self.ui_resize_grip(ctx, ui);
            // 按钮最后画，保证压在拖拽/缩放手柄之上
            self.ui_corner_buttons(ctx, ui);
        });

        ctx.request_repaint_after(Duration::from_millis(400));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        save_config(&self.cfg);
        log_line("退出，配置已保存");
    }
}

fn run() -> eframe::Result<()> {
    let cfg = load_config();

    let make = |transparent: bool, decorated: bool| {
        let mut vb = egui::ViewportBuilder::default()
            .with_inner_size([cfg.width, cfg.height])
            .with_min_inner_size([160.0, 120.0])
            .with_decorations(decorated)
            .with_transparent(transparent)
            .with_always_on_top()
            .with_resizable(true)
            // 任务栏保留图标（由 apply_taskbar_only_style 手工加回），Alt+Tab 不显示
            .with_taskbar(true)
            .with_title("LHM Panel");

        // 位置不在这里设：eframe 的 with_position 在双屏 + DPI 缩放场景下会算错，
        // 改由首帧的 restore_window_position() 用 SetWindowPos 精确恢复。
        let _ = (cfg.pos_x, cfg.pos_y);

        eframe::NativeOptions {
            viewport: vb,
            ..Default::default()
        }
    };

    log_line("尝试透明无边框窗口");
    let r = eframe::run_native(
        "LHM Panel",
        make(true, false),
        Box::new(|cc| Ok(Box::new(PanelApp::new(cc)))),
    );

    if r.is_err() {
        log_line(&format!("透明窗口失败: {:?}，回退普通窗口", r.as_ref().err()));
        let r2 = eframe::run_native(
            "LHM Panel",
            make(false, true),
            Box::new(|cc| Ok(Box::new(PanelApp::new(cc)))),
        );
        if let Err(e) = &r2 {
            log_line(&format!("普通窗口也失败: {e:?}"));
        }
        return r2;
    }
    r
}

/// 用 Win32 `SetWindowPos` 把窗口放到配置里记录的坐标。
///
/// 为什么不用 eframe 的 `with_position`：在「双屏 + 200% 缩放」环境下实测，
/// 存 (600,350) 恢复后变成 (-60,364)，X 轴偏移一个屏幕宽度。
/// 直接调系统 API 可以完全控制，且与 `outer_rect` 的物理像素坐标一致。
#[cfg(windows)]
fn restore_window_position(cfg: &Config) {
    let (Some(x), Some(y)) = (cfg.pos_x, cfg.pos_y) else {
        return;
    };

    use windows::core::HSTRING;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

    unsafe {
        let title = HSTRING::from("LHM Panel");
        let Ok(hwnd) = FindWindowW(None, &title) else {
            log_line("恢复位置：找不到窗口句柄");
            return;
        };
        if hwnd.is_invalid() {
            log_line("恢复位置：句柄无效");
            return;
        }

        // 坐标补偿：乘以 DPI 缩放比。
        //
        // 实测（200% 缩放）两端单位不一致：
        //   - `outer_rect` / `GetWindowRect` 返回**物理像素**
        //   - `SetWindowPos` 在本进程里按**逻辑像素**解释传入值
        // 直接传 600 会落在物理 300；乘 2 后传 1200 才落在物理 600。
        let dpi = GetDpiForWindow(hwnd);
        let scale = if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 };
        let px = (x as f32 * scale).round() as i32;
        let py = (y as f32 * scale).round() as i32;

        let r = SetWindowPos(
            hwnd,
            None,
            px,
            py,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        match r {
            Ok(()) => log_line(&format!(
                "恢复位置: 目标({x},{y}) dpi={dpi} scale={scale} 传入({px},{py})"
            )),
            Err(e) => log_line(&format!("恢复位置失败: {e:?}")),
        }
    }
}

/// 让窗口留在任务栏（有图标、可点击还原），但不出现在 Alt+Tab 列表里。
///
/// 原理：
/// 1. 给窗口加 `WS_EX_TOOLWINDOW` —— Windows 会同时把它从任务栏和 Alt+Tab 中移除[citation:7][citation:13]；
/// 2. 再用 `ITaskbarList::AddTab(hwnd)` 手工把任务栏按钮加回来[citation:7][citation:11]。
///
/// `ITaskbarList` 是 STA 公寓对象，而 eframe 的 UI 线程已是 MTA，
/// 直接在主线程 `CoCreateInstance` 会返回 `RPC_E_CHANGED_MODE`[citation:10]，
/// 所以这里开一个独立线程、自己初始化 STA 再创建。
#[cfg(windows)]
fn apply_taskbar_only_style() {
    std::thread::spawn(|| {
        use windows::core::{Interface, HSTRING};
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::{ITaskbarList, TaskbarList};
        use windows::Win32::UI::WindowsAndMessaging::{
            FindWindowW, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE,
            SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_APPWINDOW,
            WS_EX_TOOLWINDOW,
        };

        unsafe {
            // 独立线程必须是 STA
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

            // 按标题找窗口：eframe 不暴露 HWND，但标题是我们自己设的，唯一且稳定
            let title = HSTRING::from("LHM Panel");
            let hwnd: HWND = match FindWindowW(None, &title) {
                Ok(h) if !h.is_invalid() => h,
                _ => {
                    log_line("找不到窗口句柄，跳过任务栏修补");
                    CoUninitialize();
                    return;
                }
            };

            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            log_line(&format!("原始 exstyle=0x{ex:X}"));

            // 去掉 APPWINDOW、加上 TOOLWINDOW：窗口不再进 Alt+Tab，同时被任务栏摘掉
            let new_ex = (ex & !(WS_EX_APPWINDOW.0 as u32)) | WS_EX_TOOLWINDOW.0 as u32;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex as isize);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            );

            // 把任务栏按钮加回来
            match CoCreateInstance::<_, ITaskbarList>(
                &TaskbarList,
                None,
                CLSCTX_INPROC_SERVER,
            ) {
                Ok(list) => {
                    let _ = list.HrInit();
                    match list.AddTab(hwnd) {
                        Ok(()) => log_line("ITaskbarList::AddTab 成功（任务栏有图标、Alt+Tab 不显示）"),
                        Err(e) => log_line(&format!("AddTab 失败: {e:?}")),
                    }
                }
                Err(e) => log_line(&format!("创建 ITaskbarList 失败: {e:?}")),
            }

            CoUninitialize();
        }
    });
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        log_line(&format!("PANIC: {info}"));
    }));

    log_line("=== 启动 ===");
    if let Err(e) = run() {
        log_line(&format!("run 返回错误: {e:?}"));
    }
    log_line("=== 结束 ===");
}
