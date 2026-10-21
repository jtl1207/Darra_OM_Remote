// 会话骨架: UI 会话回调接口 + 会话容器 + 连接轮次状态。
// 连接引擎 = SessionHost 外部进程,本文件只承载会话簿记,不承载协议栈。
use crate::client::LoginConfigHandler;
use base::message_proto::*;
use base::rendezvous_proto::ConnType;
use scrap::CodecFormat;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, RwLock};

/// 画质状态(推送到 UI 的可选项集合)。
#[derive(Debug, Default)]
pub struct QualityStatus {
    pub speed: Option<String>,
    pub fps: HashMap<usize, i32>,
    pub delay: Option<i32>,
    pub target_bitrate: Option<i32>,
    pub codec_format: Option<CodecFormat>,
    pub chroma: Option<String>,
}

/// 连接轮次状态: 轮次号单调递增,标记已连接/已断开。
#[derive(Default)]
pub struct ConnectionRoundState {
    round: u32,
    connected: bool,
}

impl ConnectionRoundState {
    pub fn new_round(&mut self) -> u32 {
        self.round = self.round.wrapping_add(1);
        self.connected = false;
        self.round
    }
    /// set_connected 回调点(SessionHost 拉起成功后置位)。
    pub fn set_connected(&mut self) {
        self.connected = true;
    }
    pub fn is_connected(&self) -> bool {
        self.connected
    }
    pub fn is_round_gt(&self, round: u32) -> bool {
        if round == u32::MAX && self.round == 0 {
            true
        } else {
            round < self.round
        }
    }
    pub fn set_disconnected(&mut self, round: u32) -> bool {
        if self.is_round_gt(round) {
            false
        } else {
            self.connected = false;
            true
        }
    }
}

/// UI 会话回调: FlutterHandler 的实现面。
pub trait InvokeUiSession: Clone + Default {
    fn set_cursor_data(&self, cd: CursorData);
    fn set_cursor_id(&self, id: String);
    fn set_cursor_position(&self, cp: CursorPosition);
    fn set_display(&self, x: i32, y: i32, w: i32, h: i32, cursor_embedded: bool, scale: f64);
    fn update_privacy_mode(&self);
    fn set_permission(&self, name: &str, value: bool);
    fn close_success(&self);
    fn update_quality_status(&self, status: QualityStatus);
    fn set_connection_type(&self, is_secured: bool, direct: bool, stream_type: &str);
    fn adapt_size(&self) {}
    fn on_rgba(&self, display: usize, rgba: &mut scrap::ImageRgb);
    fn on_texture(&self, _display: usize, _texture: *mut std::os::raw::c_void) {}
    fn set_peer_info(&self, pi: &PeerInfo);
    fn set_displays(&self, displays: &Vec<DisplayInfo>);
    fn set_platform_additions(&self, data: &str);
    fn set_multiple_windows_session(&self, sessions: Vec<WindowsSession>);
    fn is_multi_ui_session(&self) -> bool;
    fn set_current_display(&self, disp_idx: i32);
    fn on_connected(&self, conn_type: ConnType);
    fn msgbox(&self, msgtype: &str, title: &str, text: &str, link: &str, retry: bool);
    fn cancel_msgbox(&self, tag: &str);
    fn switch_display(&self, display: &SwitchDisplay);
    fn update_block_input_state(&self, on: bool);
    fn get_rgba(&self, display: usize) -> *const u8;
    fn next_rgba(&self, display: usize);
    fn printer_request(&self, id: i32, path: String);
}

/// 会话容器: 权限开关 + 登录配置 + UI 回调。
#[derive(Clone, Default)]
pub struct Session<T: InvokeUiSession> {
    pub password: String,
    pub lc: Arc<RwLock<LoginConfigHandler>>,
    pub ui_handler: T,
    pub server_keyboard_enabled: Arc<RwLock<bool>>,
    pub server_file_transfer_enabled: Arc<RwLock<bool>>,
    pub server_clipboard_enabled: Arc<RwLock<bool>>,
    pub reconnect_count: Arc<AtomicUsize>,
    pub connection_round_state: Arc<Mutex<ConnectionRoundState>>,
    /// 光标形状缓存(对端发来的光标数据,按 id 索引)。
    pub cursor_shapes: Arc<RwLock<HashMap<u64, CursorData>>>,
}

impl<T: InvokeUiSession> Deref for Session<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.ui_handler
    }
}

impl<T: InvokeUiSession> DerefMut for Session<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.ui_handler
    }
}

/// 会话操作面: 视图/输入选项按同级设备配置落盘;输入与协议事件由 SessionHost 窗口
/// 自捕获,控制端只经 KIND/QUALITY/SNAPSHOT/CLOSE 控制行沟通(契约见 SessionHost/Program.cs)。
impl<T: InvokeUiSession> Session<T> {
    // ---- 身份/状态 ----
    pub fn get_id(&self) -> String {
        self.lc.read().unwrap().id.clone()
    }
    pub fn get_conn_token(&self) -> Option<String> {
        self.lc.read().unwrap().conn_token.clone()
    }
    pub fn get_peer_version(&self) -> i64 {
        self.lc.read().unwrap().version
    }
    pub fn is_default(&self) -> bool {
        self.lc.read().unwrap().conn_type == ConnType::DEFAULT_CONN
    }
    pub fn is_view_camera(&self) -> bool {
        self.lc.read().unwrap().conn_type == ConnType::VIEW_CAMERA
    }
    pub fn is_terminal(&self) -> bool {
        self.lc.read().unwrap().conn_type == ConnType::TERMINAL
    }
    pub fn is_file_transfer(&self) -> bool {
        self.lc.read().unwrap().conn_type == ConnType::FILE_TRANSFER
    }
    pub fn is_port_forward(&self) -> bool {
        self.lc.read().unwrap().conn_type == ConnType::PORT_FORWARD
    }
    pub fn is_rdp(&self) -> bool {
        self.lc.read().unwrap().conn_type == ConnType::RDP
    }
    pub fn peer_platform(&self) -> String {
        self.load_config().info.platform
    }
    pub fn get_remember(&self) -> bool {
        self.lc.read().unwrap().remember
    }
    pub fn load_config(&self) -> base::config::PeerConfig {
        base::config::PeerConfig::load(&self.lc.read().unwrap().id)
    }

    // ---- 同级设备选项(落盘 PeerConfig) ----
    pub fn get_option(&self, k: String) -> String {
        self.load_config().options.get(&k).cloned().unwrap_or_default()
    }
    pub fn set_option(&self, k: String, v: String) {
        let id = self.lc.read().unwrap().id.clone();
        let mut c = base::config::PeerConfig::load(&id);
        if v.is_empty() {
            c.options.remove(&k);
        } else {
            c.options.insert(k, v);
        }
        c.store(&id);
    }
    pub fn get_flutter_option(&self, k: String) -> String {
        self.load_config().ui_flutter.get(&k).cloned().unwrap_or_default()
    }
    pub fn save_flutter_option(&self, k: String, v: String) {
        let id = self.lc.read().unwrap().id.clone();
        let mut c = base::config::PeerConfig::load(&id);
        if v.is_empty() {
            c.ui_flutter.remove(&k);
        } else {
            c.ui_flutter.insert(k, v);
        }
        c.store(&id);
    }
    pub fn get_toggle_option(&self, name: String) -> bool {
        let v = self.get_option(name);
        v == "Y" || v == "true" || v == "1"
    }
    pub fn toggle_option(&self, name: String) {
        let next = if self.get_toggle_option(name.clone()) { "N" } else { "Y" };
        self.set_option(name, next.to_owned());
    }
    pub fn get_view_style(&self) -> String {
        self.get_option("view-style".to_owned())
    }
    pub fn save_view_style(&self, value: String) {
        self.set_option("view-style".to_owned(), value);
    }
    pub fn get_scroll_style(&self) -> String {
        self.get_option("scroll-style".to_owned())
    }
    pub fn save_scroll_style(&self, value: String) {
        self.set_option("scroll-style".to_owned(), value);
    }
    pub fn get_edge_scroll_edge_thickness(&self) -> i32 {
        self.get_option("edge-scroll-edge-thickness".to_owned())
            .parse()
            .unwrap_or(20)
    }
    pub fn save_edge_scroll_edge_thickness(&self, value: i32) {
        self.set_option("edge-scroll-edge-thickness".to_owned(), value.to_string());
    }
    pub fn get_image_quality(&self) -> String {
        self.get_option("image-quality".to_owned())
    }
    pub fn save_image_quality(&self, value: String) {
        self.set_option("image-quality".to_owned(), value);
    }
    pub fn get_custom_image_quality(&self) -> Vec<i32> {
        self.get_option("custom-image-quality".to_owned())
            .split(',')
            .filter_map(|s| s.parse::<i32>().ok())
            .collect()
    }
    pub fn save_custom_image_quality(&self, custom_image_quality: i32) {
        self.set_option(
            "custom-image-quality".to_owned(),
            custom_image_quality.to_string(),
        );
    }
    pub fn get_keyboard_mode(&self) -> String {
        let v = self.get_option("keyboard-mode".to_owned());
        if v.is_empty() {
            "legacy".to_owned()
        } else {
            v
        }
    }
    pub fn save_keyboard_mode(&self, value: String) {
        self.set_option("keyboard-mode".to_owned(), value);
    }
    pub fn is_keyboard_mode_supported(&self, mode: String) -> bool {
        matches!(mode.as_str(), "legacy" | "map" | "translate")
    }
    pub fn get_reverse_mouse_wheel(&self) -> String {
        self.get_option("reverse-mouse-wheel".to_owned())
    }
    pub fn save_reverse_mouse_wheel(&self, value: String) {
        self.set_option("reverse-mouse-wheel".to_owned(), value);
    }
    pub fn get_displays_as_individual_windows(&self) -> String {
        self.get_option("displays-as-individual-windows".to_owned())
    }
    pub fn save_displays_as_individual_windows(&self, value: String) {
        self.set_option("displays-as-individual-windows".to_owned(), value);
    }
    pub fn get_use_all_my_displays_for_the_remote_session(&self) -> String {
        self.get_option("use-all-my-displays".to_owned())
    }
    pub fn save_use_all_my_displays_for_the_remote_session(&self, value: String) {
        self.set_option("use-all-my-displays".to_owned(), value);
    }
    pub fn get_trackpad_speed(&self) -> i32 {
        self.get_option("trackpad-speed".to_owned())
            .parse()
            .unwrap_or(0)
    }
    pub fn save_trackpad_speed(&self, trackpad_speed: i32) {
        self.set_option("trackpad-speed".to_owned(), trackpad_speed.to_string());
    }
    pub fn set_custom_fps(&self, custom_fps: i32) {
        self.set_option("custom-fps".to_owned(), custom_fps.to_string());
    }

    // ---- 会话控制 / 输入转发 ----
    /// 输入(键鼠/触控)由 SessionHost 会话窗自捕获,控制端不转发。
    pub fn send_key_event(&self, _evt: &KeyEvent) {}
    pub fn send_mouse(
        &self,
        _mask: i32,
        _x: i32,
        _y: i32,
        _alt: bool,
        _ctrl: bool,
        _shift: bool,
        _command: bool,
    ) {
    }
    pub fn input_key(
        &self,
        _name: &str,
        _down: bool,
        _press: bool,
        _alt: bool,
        _ctrl: bool,
        _shift: bool,
        _command: bool,
    ) {
    }
    pub fn input_string(&self, _value: &str) {}
    pub fn send_touch_scale(&self, _scale: i32, _alt: bool, _ctrl: bool, _shift: bool, _command: bool) {}
    pub fn send_touch_pan_event(
        &self,
        _event: &str,
        _x: i32,
        _y: i32,
        _alt: bool,
        _ctrl: bool,
        _shift: bool,
        _command: bool,
    ) {
    }
    pub fn handle_flutter_key_event(
        &self,
        _session_id: crate::flutter_ffi::SessionID,
        _keyboard_mode: &str,
        _character: &str,
        _usb_hid: i32,
        _lock_modes: i32,
        _down_or_up: bool,
    ) {
    }
    pub fn handle_flutter_raw_key_event(
        &self,
        _session_id: crate::flutter_ffi::SessionID,
        _keyboard_mode: &str,
        _name: &str,
        _platform_code: i32,
        _position_code: i32,
        _lock_modes: i32,
        _down_or_up: bool,
    ) {
    }
    pub fn lock_screen(&self) {}
    pub fn ctrl_alt_del(&self) {}
    pub fn printer_response(&self, _id: i32, _path: String, _printer_name: String) {}

    // ---- 视频/显示 ----
    pub fn refresh_video(&self, _display: i32) {}
    pub fn capture_displays(&self, _add: Vec<i32>, _sub: Vec<i32>, _set: Vec<i32>) {}
    pub fn change_resolution(&self, _display: i32, _width: i32, _height: i32) {}
    /// 切换远端显示(整数下标,经 UI 回调转换为 SwitchDisplay 语义)。
    pub fn switch_display(&self, _display: i32) {}
    pub fn is_multi_ui_session(&self) -> bool {
        self.ui_handler.is_multi_ui_session()
    }
    pub fn request_init_msgs(&self, _display: usize) {}
    pub fn use_texture_render_changed(&self) {}
    pub fn update_supported_decodings(&self) {}

    /// 关闭会话: 经控制行 CLOSE 通知 SessionHost。
    pub fn close(&self) {
        crate::client::io_loop::send_control_line("CLOSE");
    }
}

/// 会话连接入口: 拉起 SessionHost 并注入 START(见 client::io_loop)。
pub fn io_loop<T: InvokeUiSession>(session: Session<T>, round: u32) {
    crate::client::io_loop::start_session_host(&session, round);
}
