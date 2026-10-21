// 文件系统共同件(收编自原 hbb_common/base::fs,仅保留调用面)。

/// 用户 home 目录的字符串形式(配置/缓存路径拼接用)。
pub fn get_home_as_string() -> String {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    {
        crate::config::APP_DIR.read().unwrap().clone()
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    {
        if let Some(path) = home_dir() {
            path.to_string_lossy().to_string()
        } else {
            String::new()
        }
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn home_dir() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(std::path::PathBuf::from)
    }
}
