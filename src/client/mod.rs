// 会话连接侧: 登录配置持有者 + SessionHost 连接入口。
pub mod io_loop;

use base::rendezvous_proto::ConnType;

/// 登录配置持有(会话建立参数;token 只驻内存)。
#[derive(Default)]
pub struct LoginConfigHandler {
    pub id: String,
    pub conn_type: ConnType,
    pub conn_token: Option<String>,
    pub force_relay: bool,
    pub switch_uuid: Option<String>,
    pub adapter_luid: Option<i64>,
    pub shared_password: Option<String>,
    pub view_only: base::config::ViewOnly,
    pub version: i64,
    pub remember: bool,
}

impl LoginConfigHandler {
    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        &mut self,
        id: String,
        conn_type: ConnType,
        switch_uuid: Option<String>,
        force_relay: bool,
        adapter_luid: Option<i64>,
        shared_password: Option<String>,
        conn_token: Option<String>,
    ) {
        self.id = id;
        self.conn_type = conn_type;
        self.switch_uuid = switch_uuid;
        self.force_relay = force_relay;
        self.adapter_luid = adapter_luid;
        self.shared_password = shared_password;
        self.conn_token = conn_token;
    }
}
