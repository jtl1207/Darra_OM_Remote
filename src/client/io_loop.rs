// 连接引擎替换: 拉起产品自带会话组件 Darra远程会话.exe(SessionHost),stdin 注入 START <json>。
// 契约对齐 src/控制端/windows/SessionHost/Program.cs 的 ParseStart:
//   首行 "START {json}",uid/token 必填,kind ∈ remote|files|terminal。
// token 只经 stdin 驻内存,不进 argv/标题/日志。
use base::log;
use base::rendezvous_proto::ConnType;
use std::io::Write;
use std::path::PathBuf;

/// 连接产物常量单点(与 flutter/lib/consts.dart 同步维护)。
pub const SESSION_HOST_EXE_NAME: &str = "Darra远程会话.exe";
/// 查找顺序: ① 本进程 exe 同目录 ② C:\DARRA\OM_CONSOLE\。
pub const SESSION_HOST_SEARCH_DIRS: [&str; 2] = ["", r"C:\DARRA\OM_CONSOLE\"];

/// 会话通道类型 → SessionHost kind 字符串。
fn kind_of(conn_type: ConnType) -> &'static str {
    match conn_type {
        ConnType::FILE_TRANSFER => "files",
        ConnType::TERMINAL => "terminal",
        _ => "remote",
    }
}

/// 解析 SessionHost 可执行文件: 同目录优先,其次 C:\DARRA\OM_CONSOLE\。
pub fn resolve_session_host() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    for dir in SESSION_HOST_SEARCH_DIRS {
        let base: PathBuf = if dir.is_empty() {
            match &exe_dir {
                Some(d) => d.clone(),
                None => continue,
            }
        } else {
            PathBuf::from(dir)
        };
        let candidate = base.join(SESSION_HOST_EXE_NAME);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// 会话控制行(KIND/QUALITY/SNAPSHOT/CLOSE): 写给最近拉起的 SessionHost。
/// token/密码不得经此通道;契约见 SessionHost/Program.cs。
pub fn send_control_line(line: &str) {
    if let Ok(mut guard) = SESSION_HOST_STDIN.lock() {
        if let Some(stdin) = guard.as_mut() {
            let _ = writeln!(stdin, "{}", line);
        }
    }
}

static SESSION_HOST_STDIN: std::sync::Mutex<Option<std::process::ChildStdin>> =
    std::sync::Mutex::new(None);

/// 会话连接入口: spawn SessionHost + stdin 注入 START,成功后置 set_connected。
pub fn start_session_host<T: crate::ui_session_interface::InvokeUiSession>(
    session: &crate::ui_session_interface::Session<T>,
    round: u32,
) {
    let (uid, token, kind, name) = {
        let lc = session.lc.read().unwrap();
        (
            lc.id.clone(),
            lc.conn_token.clone().unwrap_or_default(),
            kind_of(lc.conn_type).to_owned(),
            lc.id.clone(),
        )
    };
    if uid.is_empty() {
        log::error!("session {}: 目标设备 uid 为空,拒绝拉起 SessionHost", round);
        return;
    }
    if token.is_empty() {
        log::error!("session {}: 尚未登录(无 token),拒绝拉起 SessionHost", round);
        return;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (kind, name);
        log::error!("session {}: SessionHost 仅支持 Windows", round);
    }
    #[cfg(target_os = "windows")]
    {
        let exe = match resolve_session_host() {
            Some(p) => p,
            None => {
                log::error!(
                    "session {}: 未找到 {} (同目录 / C:\\DARRA\\OM_CONSOLE\\)",
                    round,
                    SESSION_HOST_EXE_NAME
                );
                return;
            }
        };
        let start_json = serde_json::json!({
            "uid": uid,
            "token": token,
            "kind": kind,
            "name": name,
        })
        .to_string();
        let mut child = std::process::Command::new(&exe)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn();
        let child = match &mut child {
            Ok(c) => c,
            Err(err) => {
                log::error!("session {}: 拉起 SessionHost 失败: {}", round, err);
                return;
            }
        };
        // token 只经 stdin 驻内存,不进 argv/日志;stdin 转交控制行通道。
        let mut stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                log::error!("session {}: SessionHost stdin 不可用", round);
                return;
            }
        };
        if let Err(err) = writeln!(stdin, "START {}", start_json) {
            log::error!("session {}: 写入 START 失败: {}", round, err);
            return;
        }
        if let Ok(mut guard) = SESSION_HOST_STDIN.lock() {
            *guard = Some(stdin);
        }
        log::info!("session {}: SessionHost 已拉起 (kind={})", round, kind);
        session
            .connection_round_state
            .lock()
            .unwrap()
            .set_connected();
    }
}
