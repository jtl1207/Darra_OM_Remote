//! DURT 连接引擎链接: 准入取计划 → QUIC 会话 → 通道 → 双向泵。
//!
//! 连接入口的唯一通道 (rendezvous/hbbs/hbbr/WebRTC/KCP/直连打洞不在本路径)。
//! 出站: rustdesk `Message` 字节 → `message_to_durt` → record 走 `send_record`,
//! 视频 datagram 走 `send_video_datagram`。
//! 入站: `SessionEvent` → `durt_to_message` → `Message` 字节推进 `DurtStream.rx`。
//! 令牌纪律: token 只进 `ConnectTarget.token` 驻内存, 绝不进日志/错误串。

use base::message_proto::Message;
use bytes::{Bytes, BytesMut};
use hbb_common::{
    anyhow::anyhow, log, protobuf::Message as _, stream::DurtStream, tokio,
    tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender}, ResultType, Stream,
};
use std::io;

use crate::durt::admission::{request_plan, ConnectTarget};
use crate::durt::bridge::{durt_to_message, message_to_durt, DurtIn, DurtOut};
use crate::durt::session::{DurtChannel, DurtSession, SessionEvent, SessionPlan};

pub use crate::durt::admission::SessionKind;

/// 会话用途 → DURT 通道开关 (与 bridge 的 channel_plan_for 同口径: Input+ScreenCtrl 恒开)。
struct ChannelSet {
    files: bool,
    terminal: bool,
}

fn channel_set(kind: SessionKind) -> ChannelSet {
    ChannelSet {
        files: kind == SessionKind::Files,
        terminal: kind == SessionKind::Terminal,
    }
}

/// 建 DURT 连接并返回 rustdesk `Stream::Durt` 承载。
///
/// `uid` = 已登记设备 uid (只允许从设备列表发起, 无 IP 直连/配对码路径)。
/// `token` 只驻 `ConnectTarget` 内存, 任何日志/错误串都不得包含它。
pub async fn open(uid: &str, token: &str, kind: SessionKind) -> ResultType<Stream> {
    let target = ConnectTarget {
        uid: uid.to_owned(),
        token: token.to_owned(),
        kind,
    };
    let mut plan = request_plan(&target)
        .await
        .map_err(|e| anyhow!("DURT 准入失败: {e}"))?;
    // 票据清零 (drop_plan 语义的清零半边; handle 已在 request_plan 内一次性消费)。
    for ticket in plan.tickets.iter_mut() {
        for byte in ticket.iter_mut() {
            *byte = 0;
        }
    }
    plan.tickets.clear();
    let peer_addr = plan.endpoint;
    let conn_id = plan.conn_id;
    let session_id = plan.session_id;
    let tls = plan.tls;
    let session = DurtSession::open(SessionPlan {
        peer_addr,
        tls,
        conn_id,
        session_id,
        enable_datagram: true,
    })
    .await
    .map_err(|e| anyhow!("DURT 会话建立失败: {e}"))?;
    let set = channel_set(kind);
    open_channels(&session, &set).await?;
    Ok(spawn_link(session))
}

/// 开 INPUT + (ScreenCtrl 的解复用桶 AUX) + 按会话用途的 FILES/TERMINAL。
/// CONTROL 由 `DurtSession::open` 常开, 不走 `open_channel` (durt.idl §3)。
async fn open_channels(session: &DurtSession, set: &ChannelSet) -> ResultType<()> {
    let mut kinds = vec![DurtChannel::Input, DurtChannel::Aux];
    if set.files {
        kinds.push(DurtChannel::Files);
    }
    if set.terminal {
        kinds.push(DurtChannel::Terminal);
    }
    for kind in kinds {
        let urgency = kind.default_urgency();
        session
            .open_channel(kind, urgency)
            .await
            .map_err(|e| anyhow!("DURT 通道打开失败 {kind:?}: {e}"))?;
    }
    Ok(())
}

/// 建双向泵并包成 `Stream::Durt`。
fn spawn_link(session: DurtSession) -> Stream {
    let (ui_tx, engine_rx) = unbounded_channel::<Bytes>();
    let (to_ui_tx, to_ui_rx) = unbounded_channel::<Result<BytesMut, io::Error>>();
    spawn_outbound(session.clone(), engine_rx);
    spawn_inbound(session, to_ui_tx);
    let local = std::net::SocketAddr::from(([0, 0, 0, 0], 0));
    Stream::Durt(DurtStream::new(ui_tx, to_ui_rx, local))
}

/// 出站泵: UI 写进 `DurtStream.tx` 的 Message 字节 → 桥 → 引擎。
fn spawn_outbound(session: DurtSession, mut from_ui: UnboundedReceiver<Bytes>) {
    tokio::spawn(async move {
        while let Some(bytes) = from_ui.recv().await {
            let msg = match Message::parse_from_bytes(&bytes) {
                Ok(m) => m,
                Err(err) => {
                    log::warn!(
                        "durt 出站 Message 解码失败: {} ({} 字节)",
                        err,
                        bytes.len()
                    );
                    continue;
                }
            };
            for out in message_to_durt(&msg) {
                let res = match out {
                    DurtOut::Record {
                        channel,
                        record_type,
                        body,
                    } => {
                        let kind = DurtChannel::from_channel_kind(channel);
                        session.send_record(kind, record_type, &body).await
                    }
                    DurtOut::VideoDatagram(raw) => session.send_video_datagram(raw).await,
                    DurtOut::AudioDatagram(raw) => {
                        // OM 当前不提供音频 (durt.idl §audio); datagram 平面引擎不解析载荷,
                        // 保留通路时字节与视频 datagram 同走。
                        session.send_video_datagram(raw).await
                    }
                };
                if let Err(err) = res {
                    log::warn!("durt 出站发送失败: {err}");
                }
            }
        }
    });
}

/// 入站泵: 引擎事件 → 桥 → Message 字节推进 `DurtStream.rx`。
fn spawn_inbound(session: DurtSession, to_ui: UnboundedSender<Result<BytesMut, io::Error>>) {
    let mut events = session.events();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Some(SessionEvent::StreamRecord {
                    record_type, payload, ..
                }) => {
                    let din = DurtIn::Record {
                        record_type,
                        body: payload,
                    };
                    push_message(&to_ui, durt_to_message(&din));
                }
                Some(SessionEvent::VideoDatagram(raw)) => {
                    push_message(&to_ui, durt_to_message(&DurtIn::VideoDatagram(raw)));
                }
                Some(SessionEvent::AudioDatagram(raw)) => {
                    push_message(&to_ui, durt_to_message(&DurtIn::AudioDatagram(raw)));
                }
                Some(SessionEvent::Closed(reason)) => {
                    let err = io::Error::new(io::ErrorKind::ConnectionAborted, reason);
                    let _ = to_ui.send(Err(err));
                    break;
                }
                None => {
                    let err = io::Error::new(io::ErrorKind::ConnectionAborted, "durt 事件流结束");
                    let _ = to_ui.send(Err(err));
                    break;
                }
            }
        }
    });
}

/// 桥不处理的记录 (Ping/Pong/FlowUpdate 等引擎级) 返回 `None`, 自然丢弃;
/// 有 Message 就编码推进 rx, 编码失败记日志 (不空吞)。
fn push_message(to_ui: &UnboundedSender<Result<BytesMut, io::Error>>, msg: Option<Message>) {
    let Some(msg) = msg else {
        return;
    };
    match msg.write_to_bytes() {
        Ok(bytes) => {
            let _ = to_ui.send(Ok(BytesMut::from(bytes.as_slice())));
        }
        Err(err) => log::warn!("durt 入站 Message 编码失败: {}", err),
    }
}
