// linux 杂项命令(收编自原 hbb_common::sh)。
use std::process::Command;

/// 执行 shell 命令并去掉尾部换行。
pub fn run_cmds_trim_newline(cmd: String) -> String {
    let out = Command::new("sh").arg("-c").arg(&cmd).output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .trim_end_matches('\n')
            .to_owned(),
        Err(_) => String::new(),
    }
}
