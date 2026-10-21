// 便携服务共享内存(仅 Windows)。会话引擎外置后无常驻共享内存,释放入口保留供壳层收口调用。
#[cfg(target_os = "windows")]
pub fn drop_portable_service_shared_memory() {}
