# lhm-panel

轻量级桌面硬件监控面板。从 [LibreHardwareMonitor](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) 的 Web Server 读取传感器数据，把 CPU / GPU / 硬盘的温度、占用、频率、功耗、风扇转速显示成一个常驻桌面、可自由缩放的小浮窗。

用 Rust + [egui](https://github.com/emilk/egui) 写成，**单个 exe，3.7 MB，无运行时依赖**，不需要装 .NET / VC++ 运行库。

![screenshot](screenshots/main.png)

---

## 为什么需要它

LibreHardwareMonitor 能读到全部硬件传感器，但它的界面是一个 .NET 树形窗口，又大又占地方，不适合常驻桌面。

`lhm-panel` 只做一件事：把 LHM 已经采到的数据，用一个 380×440 的半透明浮窗显示出来，按设备分组、按温度变色、位置和勾选项全部记住。

```
LibreHardwareMonitor（管理员运行，负责读传感器）
        │ HTTP GET http://127.0.0.1:8085/data.json
        ▼
   lhm-panel（普通权限运行，只负责显示）
```

---

## 功能

### 显示

- **按设备分组**：同一设备的传感器收进一组，设备名（`AMD Ryzen 9 9950X3D`、`NVIDIA GeForce RTX 5070 Ti`）只显示一次
- **温度变色**：`<55°C` 绿、`55–70°C` 黄、`70–82°C` 橙、`>82°C` 红
- **占用变色**：`<50%` 绿、`50–80%` 黄、`>80%` 橙
- **硬盘简称**：`CT2000T700SSD3` → `Crucial T700 2TB`，`aigo NVMe SSD P7000D 4TB` → `aigo P7000D 4TB`
- **可选柱状进度条**：默认关闭，设置里可开

### 交互

- **整窗任意位置拖动**：按住空白处即可移动，不用找标题栏
- **按住底边缩放**：最下方 10px 是缩放手柄，鼠标移上去会变成上下箭头
- **右下角浮动按钮**：`⚙` 设置、`✕` 退出
- **无边框半透明**：上边缘直角、下边缘圆角，不抢视线
- **任务栏有图标，Alt+Tab 不显示**：切窗口时不会碍事

### 配置

- **勾选式选择传感器**：设置页按「设备 → 类型」两级折叠，逐项勾选，标题显示 `(已选/总数)`
- **三个快捷按钮**：`全选` / `清空` / `恢复默认`
- **持久化**：窗口位置、大小、不透明度、字号、刷新间隔、勾选项全部存到 `data/config.json`
- **开机自启**：设置里勾选即写入 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`

### 防误读

- **阈值类传感器默认排除**：NVMe 的 `Warning Temperature`（如 119°C）、`Critical Temperature`（如 129°C）是报警门限，不是实测温度。混进面板会被误读成过热，所以默认不勾选
- **硬盘只默认勾复合温度**：`Temperature #1` / `Temperature #2` 是控制器内部节点，意义不大

---

## 环境要求

| 项目 | 要求 |
|---|---|
| 系统 | Windows 10 / 11，64 位 |
| LibreHardwareMonitor | **0.9.4 或更新**（旧版不认 Zen 5 的 SMN 地址） |
| LHM 权限 | 必须以**管理员身份**运行 |
| LHM Web Server | `Options → Remote Web Server → Run`，端口 `8085` |

### 为什么 LHM 必须管理员运行

读 CPU 温度需要访问 AMD 的 SMN 寄存器或 Intel 的 MSR，这需要加载内核驱动。普通权限下驱动加载失败，LHM 就读不到 `Core (Tctl/Tdie)`，面板上 CPU 会一直显示 `--`。

### 为什么走 HTTP 而不是 WMI

LibreHardwareMonitor 0.9.6 **没有 WMI 接口**（`root\LibreHardwareMonitor` 命名空间不存在，这是实测结论，不是配置问题）。它对外开放的数据通道就是 Web Server 的 `data.json`。

---

## 快速开始

### 方式一：直接下载

1. 从 [Releases](https://github.com/halucino/lhm-panel/releases) 下载 `lhm-panel.exe`（或直接下 [`release/lhm-panel.exe`](release/lhm-panel.exe)）
2. 放到任意目录，双击运行
3. 首次运行会在同目录生成 `data\config.json` 和 `data\panel.log`

### 方式二：从源码构建

```powershell
git clone https://github.com/halucino/lhm-panel.git
cd lhm-panel
cargo build --release
# 产物：target\release\lhm-panel.exe
```

只需要 Rust stable 工具链（MSVC target），依赖版本已在 `Cargo.lock` 固定。

一键构建并同步到 `release/`：

```powershell
pwsh -File scripts/build.ps1
```

---

## 使用说明

| 操作 | 说明 |
|---|---|
| 移动窗口 | 窗口内任意空白处按住拖 |
| 缩放窗口 | 按住最下面 10px 的底边拖动 |
| 打开设置 | 右下角 `⚙` |
| 退出 | 右下角 `✕` |
| 选择传感器 | 设置页 → 展开设备 → 展开类型 → 逐项勾选 |

首次启动会自动勾选一组默认传感器：CPU 封装温度、CPU 总占用、GPU 核心温度、GPU 核心占用、内存占用、风扇转速、硬盘复合温度。

---

## 配置文件

位置：`<exe 同目录>\data\config.json`。如果 exe 所在目录不可写（比如装在 `Program Files`），会退回 `%APPDATA%\LhmPanel\config.json`。

```json
{
  "width": 380.0,
  "height": 440.0,
  "pos_x": 245,
  "pos_y": 245,
  "opacity": 0.92,
  "font_scale": 1.0,
  "show_bars": false,
  "auto_launch": true,
  "lhm_path": "E:\\soft\\LibreHardwareMonitor\\LibreHardwareMonitor.exe",
  "autostart": false,
  "enabled": [
    "/amdcpu/0/temperature/2|Temperature|Core (Tctl/Tdie)",
    "/amdcpu/0/load/0|Load|CPU Total",
    "/gpu-nvidia/0/temperature/0|Temperature|GPU Core",
    "/gpu-nvidia/0/load/0|Load|GPU Core",
    "/ram/load/0|Load|Memory"
  ],
  "initialized": true
}
```

| 字段 | 说明 |
|---|---|
| `width` / `height` | 窗口尺寸（逻辑像素） |
| `pos_x` / `pos_y` | 窗口左上角屏幕坐标；`null` 表示首次运行，交给系统放置 |
| `opacity` | 背景不透明度，`0.25`–`1.0` |
| `font_scale` | 字号缩放，`0.75`–`2.2` |
| `show_bars` | 是否显示柱状进度条 |
| `auto_launch` | 启动时若 6 秒无数据，自动请求提权拉起 LHM |
| `lhm_path` | LibreHardwareMonitor.exe 的路径 |
| `autostart` | 是否开机自启（与注册表实际状态同步） |
| `enabled` | 勾选的传感器，格式 `SensorId\|Type\|Name` |
| `initialized` | 是否已套用过默认勾选 |

> `enabled` 里用 `SensorId|Type|Name` 三元组而不是单 `SensorId`，是因为 LHM 的 `SensorId` **不保证唯一**——实测中 `GPU Memory` 和 `GPU Bus` 共用同一个 `/gpu-nvidia/0/load/3`。

---

## 数据来源

| 指标 | LHM 中的位置 |
|---|---|
| CPU 温度 | `/amdcpu/0/temperature/2`（`Core (Tctl/Tdie)`） |
| CPU 占用 | `/amdcpu/0/load/0`（`CPU Total`） |
| GPU 温度 | `/gpu-nvidia/0/temperature/0`（`GPU Core`） |
| GPU 占用 | `/gpu-nvidia/0/load/0`（`GPU Core`） |
| 内存占用 | `/ram/load/0` |
| 硬盘温度 | `/nvme/N/temperature/0`（`Composite Temperature`） |

`data.json` 里每个传感器节点形如：

```json
{
  "Text": "Core (Tctl/Tdie)",
  "SensorId": "/amdcpu/0/temperature/2",
  "Type": "Temperature",
  "Value": "60.1 °C",
  "Children": []
}
```

设备名来自带 `HardwareId` 的祖先节点（如 `AMD Ryzen 9 9950X3D`）。

---

## 项目结构

```
lhm-panel/
├─ src/main.rs              全部逻辑（约 1350 行）
├─ build.rs                 用 winres 嵌入图标与版本信息
├─ assets/icon.ico          应用图标（7 种尺寸）
├─ release/lhm-panel.exe    预编译产物
├─ scripts/build.ps1        一键构建 + 同步 release/
├─ scripts/make_icon.py     生成图标
└─ .github/workflows/       打 tag 自动构建并发布 Release
```

`src/main.rs` 分四块：传感器采集（HTTP 拉取 + JSON 遍历）、egui 界面（分组渲染 + 设置页）、配置持久化、Windows 窗口样式补丁（任务栏/Alt+Tab）。

---

## 已知限制

- **仅 Windows**。`wmi` 和 `windows` 依赖是 Windows 专属；Linux / macOS 需要重写采集层。
- **依赖 LibreHardwareMonitor**。不装它什么都读不到——普通进程无法直接读 AMD SMN 寄存器或 Intel MSR。
- **LHM 必须常驻**。它一退出，面板立刻变成「无数据」。建议在 LHM 里勾 `Run on Windows startup` + `Start minimized` + `Minimize to tray`。
- **阈值类传感器仍可手动勾选**。虽然默认排除，但设置页里没有禁止，勾了会显示报警门限而不是实测值。
- **开机自启只拉起面板**，不会拉起 LHM。LHM 需要它自己的自启项，且要解决提权问题。

---

## 排错

| 现象 | 原因 / 处理 |
|---|---|
| 面板显示「未检测到 LibreHardwareMonitor」 | LHM 没运行，或 Web Server 没开 |
| 显示「无数据」但 LHM 在跑 | 浏览器打开 `http://127.0.0.1:8085/data.json` 验证 |
| CPU 温度一直 `--` | LHM 没以管理员运行，读不到 Tctl/Tdie |
| CPU 温度常年 30°C 不变 | 主板没暴露核心传感器，或 LHM 驱动被拦截 |
| 面板一闪就消失 | 看同目录 `data\panel.log`，通常有 `PANIC` 记录 |
| 任务栏没图标 | 需要 Windows 10 1809+；`ITaskbarList::AddTab` 在老系统上可能失效 |
| 双击无反应 | 检查是否被杀软拦截；本程序无签名 |
| 窗口跑到屏幕外 | 删掉 `data/config.json` 里的 `pos_x`/`pos_y`，重启 |

### 关于 Windows 11 内存完整性

如果 LHM 里所有传感器都是 `N/A`，检查 设置 → 隐私和安全性 → Windows 安全中心 → 设备安全性 → 内核隔离 → **内存完整性**。开启状态下 HVCI 会拒绝加载 WinRing0 类驱动，LHM 读不到任何硬件数据。

---

## 贡献

欢迎提 Issue 和 PR。改动 `src/main.rs` 后：

```powershell
cargo build --release
pwsh -File scripts/build.ps1   # 同步到 release/
git add -A && git commit -m "fix: ..."
```

---

## 许可证

[MIT](LICENSE)
