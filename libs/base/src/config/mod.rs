// 配置共同件收编(原 hbb_common::config,只留真调用面): 主配置 / 本机配置 / 同级设备配置 / 用户默认值。
// 持久化: confy(TOML),路径: directories_next(ProjectDirs),与原实现同语义。
pub mod keys;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::RwLock,
    time::SystemTime,
};

use serde::{Deserialize, Serialize};

lazy_static::lazy_static! {
    pub static ref APP_NAME: RwLock<String> = RwLock::new("DarraOM".to_owned());
    pub static ref ORG: RwLock<String> = RwLock::new("Darra".to_owned());
    /// 移动端/自定义数据根;桌面为空串时走 ProjectDirs。
    pub static ref APP_DIR: RwLock<String> = RwLock::new("".to_owned());
    /// 运行期覆盖本机设置(只读语义,优先于落盘值)。
    pub static ref OVERWRITE_LOCAL_SETTINGS: RwLock<HashMap<String, String>> = Default::default();
    pub static ref OVERWRITE_SETTINGS: RwLock<HashMap<String, String>> = Default::default();
    pub static ref OVERWRITE_DISPLAY_SETTINGS: RwLock<HashMap<String, String>> = Default::default();
    pub static ref DEFAULT_LOCAL_SETTINGS: RwLock<HashMap<String, String>> = Default::default();
    pub static ref DEFAULT_SETTINGS: RwLock<HashMap<String, String>> = Default::default();
    pub static ref DEFAULT_DISPLAY_SETTINGS: RwLock<HashMap<String, String>> = Default::default();
    /// 内置默认(可被覆盖层替换,供读取兜底)。
    pub static ref BUILTIN_SETTINGS: RwLock<HashMap<String, String>> = Default::default();
    /// 硬锁定设置(自定义客户端配置注入,只读)。
    pub static ref HARD_SETTINGS: RwLock<HashMap<String, String>> = Default::default();
    /// 新落盘的同级设备配置 id 列表(加载器登记)。
    pub static ref NEW_STORED_PEER_CONFIG: std::sync::Mutex<std::collections::HashSet<String>> =
        Default::default();
    pub static ref ANDROID_RUSTLS_PLATFORM_VERIFIER_INITIALIZED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
}

/// 只读视角开关(扁平化进配置的布尔项,`.v` 取值)。
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct ViewOnly {
    #[serde(default)]
    pub v: bool,
}

/// 策略开关: 只允许被控 / 只允许主控 / 禁用设置 / 禁用地址簿 / 禁用账号。
pub fn is_incoming_only() -> bool {
    HARD_SETTINGS
        .read()
        .unwrap()
        .get("incoming-only")
        .map(|v| v == "Y")
        .unwrap_or(false)
}
pub fn is_outgoing_only() -> bool {
    HARD_SETTINGS
        .read()
        .unwrap()
        .get("outgoing-only")
        .map(|v| v == "Y")
        .unwrap_or(false)
}
pub fn is_disable_settings() -> bool {
    HARD_SETTINGS
        .read()
        .unwrap()
        .get("disable-settings")
        .map(|v| v == "Y")
        .unwrap_or(false)
}
pub fn is_disable_ab() -> bool {
    HARD_SETTINGS
        .read()
        .unwrap()
        .get("disable-ab")
        .map(|v| v == "Y")
        .unwrap_or(false)
}
pub fn is_disable_account() -> bool {
    HARD_SETTINGS
        .read()
        .unwrap()
        .get("disable-account")
        .map(|v| v == "Y")
        .unwrap_or(false)
}

pub fn is_disable_installation() -> bool {
    cfg!(any(target_os = "android", target_os = "ios"))
}

/// "Y"/"N"/"1"/"true" 等字符串开关 → bool(空=默认 true 语义由调用方定,此处仅显式否)。
pub fn option2bool(_name: &str, value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    v != "n" && v != "no" && v != "false" && v != "0"
}

/// 通用命名配置槽存(调用面: common_store(&cfg, "_suffix"))。
pub fn common_store<T: Serialize>(config: &T, suffix: &str) {
    let path = Config::path(format!("common_{suffix}.toml"));
    let _ = store_path(path, config);
}

pub fn common_load<T: Serialize + for<'de> Deserialize<'de> + Default + std::fmt::Debug>(
    suffix: &str,
) -> T {
    let path = Config::path(format!("common_{suffix}.toml"));
    load_path(path)
}

pub fn load_path<T: Serialize + for<'de> Deserialize<'de> + Default + std::fmt::Debug>(
    file: PathBuf,
) -> T {
    match confy::load_path(&file) {
        Ok(config) => config,
        Err(err) => {
            log::debug!("load config '{}' failed: {}", file.display(), err);
            T::default()
        }
    }
}

pub fn store_path<T: Serialize>(path: PathBuf, cfg: T) -> crate::ResultType<()> {
    confy::store_path(path, cfg)?;
    Ok(())
}

/// 主配置(键值选项集)。
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct Config {
    #[serde(default)]
    pub options: HashMap<String, String>,
}

/// 二号配置槽(独立文件,结构同主配置)。
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct Config2 {
    #[serde(default)]
    pub options: HashMap<String, String>,
}

impl Config2 {
    pub fn file() -> PathBuf {
        Config::with_extension(Config::path(format!("{}2", *APP_NAME.read().unwrap())))
    }
    pub fn is_empty(&self) -> bool {
        self.options.is_empty()
    }
}

impl Config {
    pub fn get_option(key: &str) -> String {
        if let Some(v) = OVERWRITE_SETTINGS.read().unwrap().get(key) {
            return v.clone();
        }
        Self::get().options.get(key).cloned().unwrap_or_default()
    }

    pub fn set_option(key: String, value: String) {
        let mut config = Self::get();
        config.options.insert(key, value);
        config.store();
    }

    pub fn get_options() -> HashMap<String, String> {
        Self::get().options
    }

    /// 整体替换选项集。
    pub fn set_options(options: HashMap<String, String>) {
        let config = Config { options };
        config.store();
    }

    pub fn is_empty(&self) -> bool {
        self.options.is_empty()
    }

    pub fn get() -> Config {
        load_path(Self::file())
    }

    pub fn store(&self) {
        let _ = store_path(Self::file(), self.clone());
    }

    pub fn file() -> PathBuf {
        Self::with_extension(Self::path(APP_NAME.read().unwrap().clone()))
    }

    fn with_extension(mut path: PathBuf) -> PathBuf {
        path.set_extension("toml");
        path
    }

    /// 配置根下的子路径。
    pub fn path<P: AsRef<Path>>(p: P) -> PathBuf {
        if !APP_DIR.read().unwrap().is_empty() {
            let mut path: PathBuf = APP_DIR.read().unwrap().clone().into();
            path.push(p);
            return path;
        }
        if let Some(project) = directories_next::ProjectDirs::from(
            "",
            &ORG.read().unwrap(),
            &APP_NAME.read().unwrap(),
        ) {
            let mut path = project.config_dir().to_path_buf();
            path.push(p);
            return path;
        }
        "".into()
    }

    pub fn get_home() -> PathBuf {
        if let Some(dirs) = directories_next::UserDirs::new() {
            dirs.home_dir().to_path_buf()
        } else if let Ok(path) = std::env::current_dir() {
            path
        } else {
            std::env::temp_dir()
        }
    }

    pub fn log_path() -> PathBuf {
        Self::path("logs")
    }
}

fn cfg_file(name: &str) -> PathBuf {
    Config::with_extension(Config::path(name.to_owned()))
}

/// 本机设置(语言/快捷键/最近目标等)。
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct LocalConfig {
    #[serde(default)]
    remote_id: String,
    #[serde(default)]
    kb_layout_type: String,
    #[serde(default)]
    pub fav: Vec<String>,
    #[serde(default)]
    options: HashMap<String, String>,
    #[serde(default)]
    ui_flutter: HashMap<String, String>,
}

lazy_static::lazy_static! {
    static ref LOCAL_CONFIG: RwLock<LocalConfig> = RwLock::new(LocalConfig::load());
}

impl LocalConfig {
    fn load() -> LocalConfig {
        load_path(cfg_file("_local"))
    }
    fn store(&self) {
        let _ = store_path(cfg_file("_local"), self.clone());
    }
    pub fn get_option(key: &str) -> String {
        if let Some(v) = OVERWRITE_LOCAL_SETTINGS.read().unwrap().get(key) {
            return v.clone();
        }
        LOCAL_CONFIG
            .read()
            .unwrap()
            .options
            .get(key)
            .cloned()
            .unwrap_or_default()
    }
    pub fn set_option(key: String, value: String) {
        let mut config = LOCAL_CONFIG.write().unwrap();
        config.options.insert(key, value);
        config.store();
    }
    pub fn get_kb_layout_type() -> String {
        LOCAL_CONFIG.read().unwrap().kb_layout_type.clone()
    }
    pub fn set_kb_layout_type(kb_layout_type: String) {
        let mut config = LOCAL_CONFIG.write().unwrap();
        config.kb_layout_type = kb_layout_type;
        config.store();
    }
    pub fn get_remote_id() -> String {
        LOCAL_CONFIG.read().unwrap().remote_id.clone()
    }
    pub fn get_fav() -> Vec<String> {
        LOCAL_CONFIG.read().unwrap().fav.clone()
    }
    pub fn set_fav(fav: Vec<String>) {
        let mut config = LOCAL_CONFIG.write().unwrap();
        if config.fav == fav {
            return;
        }
        config.fav = fav;
        config.store();
    }
    pub fn set_remote_id(remote_id: &str) {
        let mut config = LOCAL_CONFIG.write().unwrap();
        if remote_id == config.remote_id {
            return;
        }
        config.remote_id = remote_id.into();
        config.store();
    }
    pub fn get_ui_flutter(key: &str) -> String {
        LOCAL_CONFIG
            .read()
            .unwrap()
            .ui_flutter
            .get(key)
            .cloned()
            .unwrap_or_default()
    }
    pub fn set_ui_flutter(key: String, value: String) {
        let mut config = LOCAL_CONFIG.write().unwrap();
        config.ui_flutter.insert(key, value);
        config.store();
    }
    /// 同 get_ui_flutter(调用面命名)。
    pub fn get_flutter_option(key: &str) -> String {
        Self::get_ui_flutter(key)
    }
    /// 同 set_ui_flutter(调用面命名)。
    pub fn set_flutter_option(key: String, value: String) {
        Self::set_ui_flutter(key, value)
    }
}

/// 同级设备(地址簿条目)的序列化信息。
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct PeerInfoSerde {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub platform: String,
}

/// 单个同级设备配置(选项/Flutter 侧状态)。
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct PeerConfig {
    #[serde(default)]
    pub info: PeerInfoSerde,
    #[serde(default)]
    pub options: HashMap<String, String>,
    #[serde(default)]
    pub ui_flutter: HashMap<String, String>,
    #[serde(default)]
    pub view_only: ViewOnly,
}

impl PeerConfig {
    pub const BATCH_LOADING_COUNT: usize = 100;

    pub fn load(id: &str) -> PeerConfig {
        load_path(cfg_file(&format!("_peer_{id}")))
    }
    pub fn store(&self, id: &str) {
        let _ = store_path(cfg_file(&format!("_peer_{id}")), self.clone());
    }
    pub fn exists(id: &str) -> bool {
        cfg_file(&format!("_peer_{id}")).exists()
    }
    pub fn remove(id: &str) {
        let _ = std::fs::remove_file(cfg_file(&format!("_peer_{id}")));
    }
    /// 已存条目(id, 修改时间, 路径),按修改时间倒序。
    pub fn get_vec_id_modified_time_path(
        _filter: &Option<Vec<String>>,
    ) -> Vec<(String, SystemTime, PathBuf)> {
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(Config::path("")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if let Some(id) = name.strip_prefix("_peer_") {
                    let id = id.strip_suffix(".toml").unwrap_or(id).to_owned();
                    let t = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    v.push((id, t, e.path()));
                }
            }
        }
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v
    }
    /// 批量取一段条目 → ((id, 时间, 配置)…, 下一段起点)。
    pub fn batch_peers(
        v: &Vec<(String, SystemTime, PathBuf)>,
        from: usize,
        to: Option<usize>,
    ) -> (Vec<(String, SystemTime, PeerConfig)>, usize) {
        let to = to.unwrap_or(v.len()).min(v.len());
        let from = from.min(to);
        let mut out = Vec::new();
        for (id, t, p) in &v[from..to] {
            out.push((id.clone(), *t, load_path(p.clone())));
        }
        (out, to)
    }
    /// 全量条目(id, 时间, 配置);fav 非空时只留收藏。
    pub fn peers(fav: Option<Vec<String>>) -> Vec<(String, SystemTime, PeerConfig)> {
        let v = Self::get_vec_id_modified_time_path(&None);
        let _ = &fav;
        v.into_iter()
            .map(|(id, t, p)| (id, t, load_path(p)))
            .collect()
    }
    /// 启动预热: 无落盘缓存可加载时静默返回。
    pub fn preload_peers() {}
}

/// 用户默认值(新建项目等默认选项)。
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct UserDefaultConfig {
    #[serde(default)]
    options: HashMap<String, String>,
}

impl UserDefaultConfig {
    pub fn load() -> UserDefaultConfig {
        load_path(cfg_file("_user_default"))
    }
    pub fn set(&mut self, key: String, value: String) {
        self.options.insert(key, value);
        let _ = store_path(cfg_file("_user_default"), self.clone());
    }
    pub fn get(&self, key: &str) -> String {
        self.options.get(key).cloned().unwrap_or_default()
    }
}

/// 局域网发现的同级设备清单。
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct LanPeers {
    #[serde(default)]
    pub peers: Vec<LanPeer>,
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct LanPeer {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub platform: String,
}

impl LanPeers {
    pub fn load() -> LanPeers {
        load_path(cfg_file("_lan_peers"))
    }
    pub fn store(&self) {
        let _ = store_path(cfg_file("_lan_peers"), self.clone());
    }
}
