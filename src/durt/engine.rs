// DURT 引擎链: 版本探针 + 会话工厂 (建连入口收口在 DurtSession)。
use super::session::{DurtSession, Result, SessionPlan};
use durt_sidecar::quic::tls::TlsParams;
use std::net::SocketAddr;

pub fn engine_probe() -> u32 {
    durt_wire::PROTO_VERSION as u32
}

/// 客户端建会话 (QUIC connect + CONTROL 常开流 + 事件泵)。
pub async fn connect(plan: SessionPlan) -> Result<DurtSession> {
    DurtSession::open(plan).await
}

/// 服务端建会话 (监听 bind_addr, 接受一条入站连接)。
pub async fn listen(
    conn_id: u64,
    bind_addr: SocketAddr,
    tls: TlsParams,
    session_id: u64,
    enable_datagram: bool,
) -> Result<DurtSession> {
    DurtSession::listen(conn_id, bind_addr, tls, session_id, enable_datagram).await
}
