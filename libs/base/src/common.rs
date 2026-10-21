// 共同件收编: 错误类型 / 日志初始化 / 版本与时间工具 / 会话通道类型。
use std::time::{SystemTime, UNIX_EPOCH};

pub type ResultType<F, E = anyhow::Error> = anyhow::Result<F, E>;

/// 吞掉错误只记调试日志(调用面: base::allow_err!)。
#[macro_export]
macro_rules! allow_err {
    ($e:expr) => {
        if let Err(err) = $e {
            $crate::log::debug!(
                "{:?}, {}:{}:{}:{}",
                err,
                module_path!(),
                file!(),
                line!(),
                column!()
            );
        }
    };
    ($e:expr, $($arg:tt)*) => {
        if let Err(err) = $e {
            $crate::log::debug!(
                "{:?}, {}, {}:{}:{}:{}",
                err,
                format_args!($($arg)*),
                module_path!(),
                file!(),
                line!(),
                column!()
            );
        }
    };
}

/// 会话通道类型(语义对齐历史 rendezvous.proto::ConnType,数值契约不变)。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum ConnType {
    #[default]
    DEFAULT_CONN = 0,
    FILE_TRANSFER = 1,
    PORT_FORWARD = 2,
    RDP = 3,
    VIEW_CAMERA = 4,
    TERMINAL = 5,
}

/// "1.2.3-4" → 数值版本号(末位×10 为补丁留位)。
pub fn get_version_number(v: &str) -> i64 {
    let mut versions = v.split('-');
    let mut n = 0;
    if let Some(v) = versions.next() {
        let mut last = 0;
        for x in v.split('.') {
            last = x.parse::<i64>().unwrap_or(0);
            n = n * 1000 + last;
        }
        n -= last;
        n += last * 10;
    }
    if let Some(v) = versions.next() {
        n += v.parse::<i64>().unwrap_or(0);
    }
    n
}

pub fn get_modified_time(path: &std::path::Path) -> SystemTime {
    std::fs::metadata(path)
        .map(|m| m.modified().unwrap_or(UNIX_EPOCH))
        .unwrap_or(UNIX_EPOCH)
}

pub fn get_created_time(path: &std::path::Path) -> SystemTime {
    std::fs::metadata(path)
        .map(|m| m.created().unwrap_or(UNIX_EPOCH))
        .unwrap_or(UNIX_EPOCH)
}

pub fn get_exe_time() -> SystemTime {
    std::env::current_exe().map_or(UNIX_EPOCH, |path| {
        let m = get_modified_time(&path);
        let c = get_created_time(&path);
        if m > c {
            m
        } else {
            c
        }
    })
}

/// 毫秒级异步睡眠。
pub async fn sleep(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

/// 日志初始化: Debug 走 env_logger,Release 走文件日志(config::Config::log_path())。
pub fn init_log(_is_async: bool, _name: &str) {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        #[cfg(debug_assertions)]
        {
            let _ = env_logger::builder()
                .filter_level(log::LevelFilter::Info)
                .try_init();
        }
        #[cfg(not(debug_assertions))]
        {
            let mut path = crate::config::Config::log_path();
            if !_name.is_empty() {
                path.push(_name);
            }
            if let Ok(logger) = flexi_logger::Logger::try_with_str("info") {
                let _ = logger
                    .log_to_file(flexi_logger::FileSpec::default().directory(path))
                    .start();
            }
        }
    });
}
