//! 系统托盘图标。
//!
//! ## 为什么必须独立线程
//!
//! `tray-icon` 在 Windows 上通过 `Shell_NotifyIconW` 注册图标，
//! 图标回调依赖**创建它的那个线程的消息循环**。
//! eframe 的 UI 线程被 winit 独占，托盘窗口的 `WM_TRAY` 消息收不到，
//! 图标虽然注册成功但点击没反应、有时甚至不显示。
//!
//! 所以这里开一个专用线程：
//!   1. 在该线程创建 `TrayIcon`（对象必须一直留在这个线程）
//!   2. 跑一个 `GetMessage` 循环处理托盘消息
//!   3. 通过 channel 与 UI 线程双向通信

use std::sync::mpsc::{channel, Receiver, Sender};

use tray_icon::menu::{MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconEvent};

/// 托盘产生的动作，交给 UI 线程处理。
pub enum TrayAction {
    Show,
    ToggleAutostart,
    Exit,
}

/// UI 线程发给托盘线程的指令。
enum TrayCommand {
    SetAutostart(bool),
    Quit,
}

pub struct Tray {
    rx: Receiver<TrayAction>,
    tx: Sender<TrayCommand>,
}

impl Tray {
    /// 在独立线程创建托盘图标。
    ///
    /// `icon_png` 是 PNG 字节，`autostart_enabled` 决定菜单项初始文案。
    pub fn new(icon_png: &'static [u8], autostart_enabled: bool) -> Option<Self> {
        let (action_tx, action_rx) = channel();
        let (cmd_tx, cmd_rx) = channel();

        let (ready_tx, ready_rx) = channel::<bool>();

        std::thread::Builder::new()
            .name("lhm-tray".into())
            .spawn(move || {
                match build_tray(icon_png, autostart_enabled, action_tx, cmd_rx) {
                    Ok(ok) => {
                        let _ = ready_tx.send(ok);
                        if ok {
                            run_message_loop();
                        }
                    }
                    Err(e) => {
                        crate::log_line(&format!("托盘线程失败: {e:?}"));
                        let _ = ready_tx.send(false);
                    }
                }
            })
            .ok()?;

        // 等托盘创建完成再返回，避免主线程先发指令
        let ok = ready_rx.recv_timeout(std::time::Duration::from_secs(3)).ok()?;
        if !ok {
            return None;
        }

        Some(Self {
            rx: action_rx,
            tx: cmd_tx,
        })
    }

    pub fn poll(&self) -> Option<TrayAction> {
        self.rx.try_recv().ok()
    }

    /// 通知托盘线程更新菜单文案。
    pub fn set_autostart_enabled(&self, on: bool) {
        let _ = self.tx.send(TrayCommand::SetAutostart(on));
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        let _ = self.tx.send(TrayCommand::Quit);
    }
}

/// 在托盘线程内创建图标与菜单，并注册事件回调。
fn build_tray(
    icon_png: &'static [u8],
    autostart_enabled: bool,
    action_tx: Sender<TrayAction>,
    cmd_rx: Receiver<TrayCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    // 优先从 exe 资源加载图标（winres 把 assets/icon.ico 编为资源 ID 1）。
    // 这条路走 LoadImageW，比 Icon::from_rgba 的 CreateIcon 路径可靠——
    // 后者在部分环境下会返回无效 HICON，Shell_NotifyIconW 收到空句柄就静默失败。
    let icon = match Icon::from_resource(1, Some((32, 32))) {
        Ok(i) => {
            crate::log_line("托盘图标: 从资源 ID 1 加载成功");
            i
        }
        Err(e) => {
            crate::log_line(&format!("托盘图标: 资源加载失败 {e:?}，回退到 PNG"));
            let img = image::load_from_memory(icon_png)?.to_rgba8();
            let (w, h) = img.dimensions();
            crate::log_line(&format!("托盘图标: PNG 回退 {w}x{h}"));
            Icon::from_rgba(img.into_raw(), w, h)?
        }
    };

    let menu = tray_icon::menu::Menu::new();
    let show_item = MenuItem::new("显示面板", true, None);
    let autostart_item = MenuItem::new(autostart_label(autostart_enabled), true, None);
    let exit_item = MenuItem::new("退出", true, None);
    let _ = menu.append(&show_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&autostart_item);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&exit_item);

    let _tray = match TrayIcon::builder()
        .with_tooltip("LHM Panel - 硬件监控")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .build()
    {
        Ok(t) => {
            crate::log_line("TrayIcon::build 成功");
            t
        }
        Err(e) => {
            crate::log_line(&format!("TrayIcon::build 失败: {e:?}"));
            return Err(Box::new(e));
        }
    };

    // 菜单事件：把菜单项 id 映射成动作
    let show_id = show_item.id().clone();
    let auto_id = autostart_item.id().clone();
    let exit_id = exit_item.id().clone();
    let tx_menu = action_tx.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == show_id {
            let _ = tx_menu.send(TrayAction::Show);
        } else if e.id == auto_id {
            let _ = tx_menu.send(TrayAction::ToggleAutostart);
        } else if e.id == exit_id {
            let _ = tx_menu.send(TrayAction::Exit);
        }
    }));

    // 图标双击 -> 显示面板
    let tx_tray = action_tx;
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::DoubleClick { button, .. } = e {
            if button == tray_icon::MouseButton::Left {
                let _ = tx_tray.send(TrayAction::Show);
            }
        }
    }));

    // 托盘对象必须活到消息循环结束，用 Box::leak 延长生命周期到进程退出
    // （托盘线程只创建一次，泄漏一对象可接受，且进程结束时 OS 会回收）
    Box::leak(Box::new(_tray));

    // MenuItem 内部是 Rc<RefCell<..>>，不是 Send，不能丢到别的线程改。
    // 所以把菜单项和 cmd_rx 一起交给消息循环，由同一个线程轮询处理。
    MENU_ITEMS.with(|c| {
        *c.borrow_mut() = Some((autostart_item, cmd_rx));
    });

    Ok(true)
}

thread_local! {
    static MENU_ITEMS: std::cell::RefCell<
        Option<(tray_icon::menu::MenuItem, Receiver<TrayCommand>)>,
    > = const { std::cell::RefCell::new(None) };
}

/// 托盘线程的消息循环。
///
/// `tray-icon` 在 Windows 上会创建一个隐藏窗口接收 `Shell_NotifyIcon` 回调，
/// 必须有人泵消息，否则图标点击与菜单都不会响应。
fn run_message_loop() {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetMessageW, TranslateMessage, MSG,
    };

    use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, PM_REMOVE};

    let mut msg = MSG::default();
    loop {
        // 先处理主线程发来的指令（非阻塞）
        let quit = MENU_ITEMS.with(|c| {
            let mut slot = c.borrow_mut();
            let Some((item, rx)) = slot.as_mut() else {
                return false;
            };
            let mut stop = false;
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    TrayCommand::SetAutostart(on) => item.set_text(autostart_label(on)),
                    TrayCommand::Quit => {
                        stop = true;
                        break;
                    }
                }
            }
            stop
        });
        if quit {
            break;
        }

        // 泵托盘消息；没有消息时让出 CPU
        let has = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) };
        if has.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}

fn autostart_label(on: bool) -> String {
    if on {
        "取消开机自启".to_string()
    } else {
        "设置开机自启".to_string()
    }
}
