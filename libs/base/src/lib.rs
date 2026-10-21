pub mod config;
pub mod keyboard;
pub mod platform;
pub mod protos;

pub mod common;
pub mod fs;
pub mod log_throttle;
#[cfg(target_os = "linux")]
pub mod sh;

pub use protos::message as message_proto;

// 共同件收编自原 hbb_common: 调用面保持 base::X 同路径。
pub use anyhow;
pub use anyhow::bail;
pub use base64;
pub use common::{
    get_created_time, get_exe_time, get_modified_time, get_version_number, init_log, sleep,
    ConnType, ResultType,
};
pub use dlopen;
pub use sha2;
pub use env_logger;
pub use futures_util;
pub use lazy_static;
pub use libc;
pub use log;
pub use rand;
pub use regex;
pub use sodiumoxide;
pub use sysinfo;
pub use thiserror;
pub use tokio;
pub use whoami;

// 历史 rendezvous 命名下仅会话通道类型仍在使用。
pub mod rendezvous_proto {
    pub use crate::common::ConnType;
}

#[cfg(target_os = "linux")]
pub use x11;
