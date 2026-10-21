//! DURT 会话引擎: QUIC 连接 + 通道多路 + 事件循环。
//!
//! 职责 (在 durt-sidecar 传输原语之上做会话级编排, 不碰 Flutter/bridge):
//!   - `open` (客户端) / `listen` (服务端) 建 QUIC 连接 (mTLS ECDSA-P384),
//!     spawn 连接事件循环 (QuicConn::run) + 会话事件泵
//!   - 通道多路: 每条 QUIC 双向 stream 承载一个 `DurtChannel`; 非 CONTROL 流首记录是
//!     `ChannelHello(0x01)` (durt.idl §2.1), CONTROL 常开流不发 ChannelHello (§3)
//!   - stream 平面 record 帧 = RecordHeader(varint type + varint length) + body
//!     (durt.idl §2); 本引擎负责通道解帧与字节搬运, record body 编解码归 bridge 线
//!   - datagram 平面: 视频片 (VideoDatagramHdr 13B + payload) 原样收发
//!
//! 通道→RFC9218 urgency (durt.idl §1 URGENCY_*): INPUT=0 / CONTROL=1 / TERM=2 /
//! CLIP=4 / FILES=6 / AUX=3; incremental: FILES=true, 其余 false。

use durt_sidecar::ipc::proto::Evt;
use durt_sidecar::quic::conn::{ConnHandle, QuicConn};
use durt_sidecar::quic::tls::TlsParams;
use durt_sidecar::SidecarError;
use durt_wire as wire;
use hbb_common::tokio;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DEFAULT_IDLE_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_MAX_DATAGRAM_SIZE: usize = 1350;
const DEFAULT_CC_STATS_INTERVAL_MS: u64 = 1_000;
const EVT_CHANNEL_CAP: usize = 1024;
const OP_CHANNEL_CAP: usize = 256;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const SEND_DEADLINE: Duration = Duration::from_secs(30);
const SEND_BACKPRESSURE_SLEEP: Duration = Duration::from_millis(2);

/// 会话通道 (durt.idl ChannelKind 的会话侧子集)。
///
/// ScreenCtrl(0x04) / Network(0x07) 无独立枚举位, 解复用时归入 `Aux` 桶。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DurtChannel {
    Control,
    Input,
    Terminal,
    Files,
    Clipboard,
    Aux,
}

impl DurtChannel {
    /// 线协议 channel_kind (durt.idl §1 ChannelKind)。
    pub fn channel_kind(self) -> u8 {
        match self {
            Self::Control => wire::ChannelKind::Control.repr(),
            Self::Input => wire::ChannelKind::Input.repr(),
            Self::Terminal => wire::ChannelKind::Terminal.repr(),
            Self::Files => wire::ChannelKind::Files.repr(),
            Self::Clipboard => wire::ChannelKind::Clipboard.repr(),
            Self::Aux => wire::ChannelKind::AuxRpc.repr(),
        }
    }

    /// 反查: 未知/未枚举 kind (ScreenCtrl/Network) 一律归 `Aux` 桶。
    pub fn from_channel_kind(v: u8) -> Self {
        match wire::ChannelKind::from_repr(v) {
            Some(wire::ChannelKind::Control) => Self::Control,
            Some(wire::ChannelKind::Input) => Self::Input,
            Some(wire::ChannelKind::Terminal) => Self::Terminal,
            Some(wire::ChannelKind::Files) => Self::Files,
            Some(wire::ChannelKind::Clipboard) => Self::Clipboard,
            _ => Self::Aux,
        }
    }

    /// RFC9218 urgency (durt.idl §1, 越小越优先)。
    pub fn default_urgency(self) -> u8 {
        match self {
            Self::Input => 0,
            Self::Control => 1,
            Self::Terminal => 2,
            Self::Clipboard => 4,
            Self::Aux => 3,
            Self::Files => 6,
        }
    }

    /// incremental: FILES 与其它刚性小流交织才不饿死别人 (durt.idl §1)。
    pub fn incremental(self) -> bool {
        matches!(self, Self::Files)
    }
}

/// 建连计划 (客户端)。admission 线的 `ConnectPlan` 落地后由主线程合并字段。
pub struct SessionPlan {
    pub peer_addr: SocketAddr,
    pub tls: TlsParams,
    pub conn_id: u64,
    pub session_id: u64,
    pub enable_datagram: bool,
}

/// 会话事件 (事件泵 → 上层)。
pub enum SessionEvent {
    /// stream 平面一条完整 record 的 body (record 头已在引擎内去帧; record_type 一并上抛供桥分派)。
    StreamRecord {
        channel: DurtChannel,
        record_type: u16,
        payload: Vec<u8>,
    },
    /// 含 13B VideoDatagramHdr 的原始视频 datagram。
    VideoDatagram(Vec<u8>),
    /// 非视频 datagram 原始字节。OM 当前不提供音频 (IPC_CONTRACT §audio),
    /// 此桶保留给音频/其它非视频 datagram, 上层按 RecordHeader 自行分派。
    AudioDatagram(Vec<u8>),
    /// 连接关闭/失败。
    Closed(String),
}

/// 会话错误。
#[derive(Debug)]
pub enum SessionError {
    Transport(String),
    Handshake(String),
    ChannelNotOpen(DurtChannel),
    RecordTooLarge(usize),
    SendTimeout { stream_id: u64, sent: usize, total: usize },
    DatagramDropped,
    Internal(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(s) => write!(f, "传输层错误: {s}"),
            Self::Handshake(s) => write!(f, "握手失败: {s}"),
            Self::ChannelNotOpen(k) => write!(f, "通道未打开: {k:?}"),
            Self::RecordTooLarge(n) => write!(f, "record 超长: {n}"),
            Self::SendTimeout { stream_id, sent, total } => {
                write!(f, "stream {stream_id} 发送超时: {sent}/{total}")
            }
            Self::DatagramDropped => write!(f, "datagram 被发送队列丢弃"),
            Self::Internal(s) => write!(f, "内部错误: {s}"),
        }
    }
}

impl std::error::Error for SessionError {}

pub type Result<T> = std::result::Result<T, SessionError>;

fn map_sidecar(e: SidecarError) -> SessionError {
    SessionError::Transport(e.to_string())
}
// ---- record 头安全解码 (durt_wire::decode_varint 在截断时越界 panic, 这里先探长) ----

fn try_decode_varint(buf: &[u8], off: usize) -> Option<(u64, usize)> {
    if off >= buf.len() {
        return None;
    }
    let len = 1usize << (buf[off] >> 6);
    if off + len > buf.len() {
        return None;
    }
    Some(wire::decode_varint(buf, off))
}

fn try_decode_record_header(buf: &[u8], off: usize) -> Option<(u64, u64, usize)> {
    let (ty, n1) = try_decode_varint(buf, off)?;
    let (len, n2) = try_decode_varint(buf, off + n1)?;
    Some((ty, len, n1 + n2))
}

/// ChannelHello 体 (durt.idl §2.1): kind + instance_id + session_id + flags + auth_blob + resume_ctx。
/// flags=0 / auth_blob 空 / resume_ctx 空 —— 授权与恢复上下文由 bridge 线补。
fn encode_channel_hello(kind: DurtChannel, instance_id: u64, session_id: u64) -> Vec<u8> {
    let mut body = Vec::with_capacity(16);
    body.push(kind.channel_kind());
    wire::encode_varint(instance_id, &mut body);
    wire::encode_varint(session_id, &mut body);
    body.push(0u8);
    wire::encode_varint(0, &mut body);
    wire::encode_varint(0, &mut body);
    body
}

fn encode_record(record_type: u16, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + body.len());
    wire::encode_record_header(record_type as u64, body.len() as u64, &mut out);
    out.extend_from_slice(body);
    out
}

/// datagram 平面解复用 (durt.idl §11.1): 高 4 位 magic=0xD 且 version=1 且
/// frame_type 合法 → 视频; 视频 magic 但版本/帧型非法 → 静默丢弃 (idl §1.4);
/// 其余 → 非视频桶 (AudioDatagram, 见 `SessionEvent::AudioDatagram` 注释)。
fn classify_datagram(blob: &[u8]) -> Option<SessionEvent> {
    let first = *blob.first()?;
    let (magic, version) = wire::unpack_magic_ver(first);
    if magic == (wire::DATAGRAM_MAGIC & 0x0f) {
        let ok = version == 1
            && blob.len() >= wire::VIDEO_DATAGRAM_HDR_LEN as usize
            && wire::VideoFrameType::from_repr(blob[1]).is_some();
        if ok {
            Some(SessionEvent::VideoDatagram(blob.to_vec()))
        } else {
            None
        }
    } else {
        Some(SessionEvent::AudioDatagram(blob.to_vec()))
    }
}

/// 一条 stream 的 record 重组缓冲。
#[derive(Default)]
struct StreamReasm {
    buf: Vec<u8>,
}

/// 出站路由项 (通道 → 当前承载 stream)。
#[derive(Clone, Copy)]
struct StreamEntry {
    stream_id: u64,
    instance_id: u64,
}

struct SessionInner {
    handle: ConnHandle,
    session_id: u64,
    /// 通道 → 出站 stream (双向流, 同一 stream 也承载对端回程)。
    out_streams: Mutex<HashMap<DurtChannel, StreamEntry>>,
    /// stream → 通道 (入站解复用; 对端新开流由首记录 ChannelHello 归类)。
    in_channel: Mutex<HashMap<u64, DurtChannel>>,
    /// 同类通道实例号 (ChannelHello.instance_id, 同端单调递增)。
    next_instance: Mutex<HashMap<DurtChannel, u64>>,
    events_tx: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    events_rx: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<SessionEvent>>>,
}

/// DURT 会话: 一条 QUIC 连接 + 通道多路 + 事件泵。
#[derive(Clone)]
pub struct DurtSession {
    inner: Arc<SessionInner>,
}
impl DurtSession {
    /// 客户端建会话: connect + 等握手 established + 建 CONTROL 常开流 + spawn 事件泵。
    pub async fn open(plan: SessionPlan) -> Result<Self> {
        let (evt_tx, evt_rx) = tokio::sync::mpsc::channel::<Evt>(EVT_CHANNEL_CAP);
        let (op_tx, op_rx) = tokio::sync::mpsc::channel(OP_CHANNEL_CAP);
        let quic = QuicConn::connect(
            plan.conn_id,
            plan.peer_addr,
            plan.tls,
            DEFAULT_IDLE_TIMEOUT_MS,
            DEFAULT_MAX_DATAGRAM_SIZE,
            plan.enable_datagram,
            DEFAULT_CC_STATS_INTERVAL_MS,
            evt_tx,
        )
        .await
        .map_err(map_sidecar)?;
        let sess = Self::start(
            quic,
            plan.conn_id,
            plan.session_id,
            plan.enable_datagram,
            DEFAULT_MAX_DATAGRAM_SIZE,
            op_tx,
            op_rx,
            evt_rx,
        )
        .await?;
        // CONTROL 常开流 (durt.idl §3: 连接建立即有, 不发 ChannelHello)。
        let sid = sess
            .inner
            .handle
            .open_stream(
                DurtChannel::Control.default_urgency(),
                DurtChannel::Control.incremental(),
            )
            .await
            .map_err(map_sidecar)?;
        sess.inner
            .in_channel
            .lock()
            .unwrap()
            .insert(sid, DurtChannel::Control);
        sess.inner.out_streams.lock().unwrap().insert(
            DurtChannel::Control,
            StreamEntry { stream_id: sid, instance_id: 0 },
        );
        Ok(sess)
    }

    /// 服务端建会话: 监听 bind_addr, 接受一条入站 QUIC 连接 (一连接=一台被控端)。
    /// CONTROL 流由对端开出, 本端经首记录归类后收养为回程流。
    pub async fn listen(
        conn_id: u64,
        bind_addr: SocketAddr,
        tls: TlsParams,
        session_id: u64,
        enable_datagram: bool,
    ) -> Result<Self> {
        let (evt_tx, evt_rx) = tokio::sync::mpsc::channel::<Evt>(EVT_CHANNEL_CAP);
        let (op_tx, op_rx) = tokio::sync::mpsc::channel(OP_CHANNEL_CAP);
        let quic = durt_sidecar::quic::server::accept_one(
            conn_id,
            bind_addr,
            tls,
            DEFAULT_IDLE_TIMEOUT_MS,
            DEFAULT_MAX_DATAGRAM_SIZE,
            enable_datagram,
            DEFAULT_CC_STATS_INTERVAL_MS,
            evt_tx,
        )
        .await
        .map_err(map_sidecar)?;
        Self::start(
            quic,
            conn_id,
            session_id,
            enable_datagram,
            DEFAULT_MAX_DATAGRAM_SIZE,
            op_tx,
            op_rx,
            evt_rx,
        )
        .await
    }

    /// 连接事件循环 + 会话事件泵 + 等 established。
    #[allow(clippy::too_many_arguments)]
    async fn start(
        quic: QuicConn,
        conn_id: u64,
        session_id: u64,
        enable_datagram: bool,
        max_datagram_size: usize,
        op_tx: tokio::sync::mpsc::Sender<durt_sidecar::quic::conn::ConnOp>,
        op_rx: tokio::sync::mpsc::Receiver<durt_sidecar::quic::conn::ConnOp>,
        evt_rx: tokio::sync::mpsc::Receiver<Evt>,
    ) -> Result<Self> {
        let handle = ConnHandle {
            conn_id,
            op_tx,
            enable_datagram,
            max_datagram_size,
        };
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let inner = Arc::new(SessionInner {
            handle,
            session_id,
            out_streams: Mutex::new(HashMap::new()),
            in_channel: Mutex::new(HashMap::new()),
            next_instance: Mutex::new(HashMap::new()),
            events_tx,
            events_rx: Mutex::new(Some(events_rx)),
        });
        let (est_tx, est_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(quic.run(op_rx));
        tokio::spawn(run_pump(evt_rx, inner.clone(), est_tx));
        let est = tokio::time::timeout(HANDSHAKE_TIMEOUT, est_rx)
            .await
            .map_err(|_| SessionError::Handshake("握手超时".into()))?
            .map_err(|_| SessionError::Handshake("事件泵提前退出".into()))?;
        let _peer_fp = est?;
        Ok(Self { inner })
    }
}
impl DurtSession {
    /// 会话号 (ChannelHello.session_id, 连接级一致)。
    pub fn session_id(&self) -> u64 {
        self.inner.session_id
    }

    /// 开一条通道流并发出 ChannelHello(0x01) 首记录。回 stream_id。
    ///
    /// urgency 由调用方按 durt.idl §1 URGENCY_* 传入 (见 `DurtChannel::default_urgency`);
    /// incremental 由通道语义决定 (FILES=true)。CONTROL 常开, 禁止走本入口。
    pub async fn open_channel(&self, kind: DurtChannel, urgency: u8) -> Result<u64> {
        if kind == DurtChannel::Control {
            return Err(SessionError::Internal("CONTROL 常开, 禁止 open_channel".into()));
        }
        let stream_id = self
            .inner
            .handle
            .open_stream(urgency, kind.incremental())
            .await
            .map_err(map_sidecar)?;
        let instance_id = {
            let mut g = self.inner.next_instance.lock().unwrap();
            let c = g.entry(kind).or_insert(0);
            let id = *c;
            *c += 1;
            id
        };
        self.inner.in_channel.lock().unwrap().insert(stream_id, kind);
        self.inner.out_streams.lock().unwrap().insert(
            kind,
            StreamEntry { stream_id, instance_id },
        );
        let hello = encode_channel_hello(kind, instance_id, self.inner.session_id);
        self.send_all(stream_id, encode_record(wire::RecordType::ChannelHello.repr(), &hello))
            .await?;
        Ok(stream_id)
    }

    /// 某通道当前承载的 stream。
    pub fn channel_stream(&self, kind: DurtChannel) -> Result<u64> {
        self.inner
            .out_streams
            .lock()
            .unwrap()
            .get(&kind)
            .map(|e| e.stream_id)
            .ok_or(SessionError::ChannelNotOpen(kind))
    }

    /// 在通道上发一条 record: RecordHeader(type, len) + body (durt.idl §2)。
    pub async fn send_record(&self, kind: DurtChannel, record_type: u16, body: &[u8]) -> Result<()> {
        if body.len() > wire::STREAM_RECORD_MAX as usize {
            return Err(SessionError::RecordTooLarge(body.len()));
        }
        let stream_id = self.channel_stream(kind)?;
        self.send_all(stream_id, encode_record(record_type, body)).await
    }

    /// 发一条原始视频 datagram (含 13B VideoDatagramHdr, 引擎不解析)。
    pub async fn send_video_datagram(&self, raw: Vec<u8>) -> Result<()> {
        let ok = self
            .inner
            .handle
            .send_datagram(raw)
            .await
            .map_err(map_sidecar)?;
        if ok {
            Ok(())
        } else {
            Err(SessionError::DatagramDropped)
        }
    }

    /// 事件接收器 (只可取一次; 重复调用返回已关闭的空接收器)。
    pub fn events(&self) -> tokio::sync::mpsc::UnboundedReceiver<SessionEvent> {
        if let Some(rx) = self.inner.events_rx.lock().unwrap().take() {
            return rx;
        }
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(tx);
        rx
    }

    /// 关闭连接 (应用错误码 0)。
    pub async fn close(&self) {
        let _ = self
            .inner
            .handle
            .close_conn(0, b"session close".to_vec())
            .await;
    }

    /// 整段字节写入 stream; 流控背压时退避重试到 SEND_DEADLINE。
    async fn send_all(&self, stream_id: u64, bytes: Vec<u8>) -> Result<()> {
        let deadline = tokio::time::Instant::now() + SEND_DEADLINE;
        let mut sent = 0usize;
        while sent < bytes.len() {
            if tokio::time::Instant::now() >= deadline {
                return Err(SessionError::SendTimeout {
                    stream_id,
                    sent,
                    total: bytes.len(),
                });
            }
            let n = self
                .inner
                .handle
                .send_stream(stream_id, bytes[sent..].to_vec(), false)
                .await
                .map_err(map_sidecar)?;
            if n == 0 {
                tokio::time::sleep(SEND_BACKPRESSURE_SLEEP).await;
            }
            sent += n;
        }
        Ok(())
    }
}
/// 会话事件泵: 把边车 Evt 翻译成 SessionEvent (record 解帧 + 通道解复用 + datagram 分类)。
async fn run_pump(
    mut evt_rx: tokio::sync::mpsc::Receiver<Evt>,
    inner: Arc<SessionInner>,
    established: tokio::sync::oneshot::Sender<Result<String>>,
) {
    let mut established = Some(established);
    let mut reasm: HashMap<u64, StreamReasm> = HashMap::new();
    while let Some(evt) = evt_rx.recv().await {
        match evt {
            Evt::ConnState { state, peer_fp, reason, .. } => match state {
                "established" => {
                    if let Some(tx) = established.take() {
                        let _ = tx.send(Ok(peer_fp.unwrap_or_default()));
                    }
                }
                "failed" => {
                    let why = reason.unwrap_or_else(|| "failed".to_string());
                    if let Some(tx) = established.take() {
                        let _ = tx.send(Err(SessionError::Handshake(why.clone())));
                    }
                    let _ = inner.events_tx.send(SessionEvent::Closed(why));
                }
                "closed" => {
                    let _ = inner.events_tx.send(SessionEvent::Closed(
                        reason.unwrap_or_else(|| "closed".to_string()),
                    ));
                }
                _ => {}
            },
            Evt::StreamOpen { stream_id, .. } => {
                reasm.entry(stream_id).or_default();
            }
            Evt::StreamData { stream_id, blob, fin, .. } => {
                feed_stream(stream_id, &blob, &mut reasm, &inner);
                if fin {
                    reasm.remove(&stream_id);
                }
            }
            Evt::Datagram { blob, .. } => {
                if let Some(ev) = classify_datagram(&blob) {
                    let _ = inner.events_tx.send(ev);
                }
            }
            Evt::SidecarFatal { detail, .. } => {
                if let Some(tx) = established.take() {
                    let _ = tx.send(Err(SessionError::Transport(detail.clone())));
                }
                let _ = inner.events_tx.send(SessionEvent::Closed(detail));
            }
            _ => {}
        }
    }
    // 边车事件通道关闭 = 连接事件循环已退出。
    if let Some(tx) = established.take() {
        let _ = tx.send(Err(SessionError::Transport("连接事件循环提前退出".into())));
    }
    let _ = inner
        .events_tx
        .send(SessionEvent::Closed("连接事件循环已退出".to_string()));
}

/// 一段 stream 字节喂进 record 重组, 完整 record 上抛。
fn feed_stream(
    stream_id: u64,
    chunk: &[u8],
    reasm: &mut HashMap<u64, StreamReasm>,
    inner: &SessionInner,
) {
    let st = reasm.entry(stream_id).or_default();
    st.buf.extend_from_slice(chunk);
    loop {
        let Some((ty, len, hdr_len)) = try_decode_record_header(&st.buf, 0) else {
            break;
        };
        if len > wire::STREAM_RECORD_MAX as u64 {
            // 坏帧: 清缓冲止损, 后续字节等下一轮重新对齐。
            st.buf.clear();
            break;
        }
        let total = hdr_len + len as usize;
        if st.buf.len() < total {
            break;
        }
        let body = st.buf[hdr_len..total].to_vec();
        st.buf.drain(..total);
        let channel = resolve_channel(stream_id, ty, &body, inner);
        let _ = inner.events_tx.send(SessionEvent::StreamRecord {
            channel,
            record_type: ty as u16,
            payload: body,
        });
    }
}

/// stream → 通道归类。
/// 首记录 ChannelHello(0x01) 带 channel_kind (durt.idl §2.1); CONTROL 常开流
/// 不发 ChannelHello (首记录是 ConnHello 等), 其余一律按 CONTROL 解 (§3)。
/// 对端开出的通道在本端尚无出站流时一并收养为回程流 (双向流)。
fn resolve_channel(
    stream_id: u64,
    record_type: u64,
    body: &[u8],
    inner: &SessionInner,
) -> DurtChannel {
    if let Some(k) = inner.in_channel.lock().unwrap().get(&stream_id) {
        return *k;
    }
    let kind = if record_type == wire::RecordType::ChannelHello.repr() as u64 {
        DurtChannel::from_channel_kind(*body.first().unwrap_or(&0))
    } else {
        DurtChannel::Control
    };
    inner.in_channel.lock().unwrap().insert(stream_id, kind);
    inner
        .out_streams
        .lock()
        .unwrap()
        .entry(kind)
        .or_insert(StreamEntry { stream_id, instance_id: 0 });
    kind
}
#[cfg(test)]
mod tests {
    use super::*;

    // 复用 durt-sidecar 测试夹具 (P-384 / ecdsa-SHA384), 与 handshake_test 同一套台账指纹
    // (握手时 conn 层按 expect_peer_fp 复核, 常量漂移会直接让 make_pair 失败)。
    const CA_CRT: &str = include_str!("../../../../static/Durt/sidecar/tests/fixtures/durt-test-certs/ca.crt");
    const DEVICE_CRT: &str = include_str!("../../../../static/Durt/sidecar/tests/fixtures/durt-test-certs/device.crt");
    const DEVICE_KEY: &str = include_str!("../../../../static/Durt/sidecar/tests/fixtures/durt-test-certs/device.key");
    const OPERATOR_CRT: &str = include_str!("../../../../static/Durt/sidecar/tests/fixtures/durt-test-certs/operator.crt");
    const OPERATOR_KEY: &str = include_str!("../../../../static/Durt/sidecar/tests/fixtures/durt-test-certs/operator.key");
    const DEVICE_FP: &str = "007d0398e164163d9a2a0104e0f8522dc4cb261e70828b7ef98fe0c0e6f88515";
    const OPERATOR_FP: &str = "c402274cbb5959a2347ef54ed1f5e7110eed452e763c091e7b63fefa09892a10";

    fn device_tls() -> TlsParams {
        TlsParams {
            cert_chain_pem: DEVICE_CRT.to_string(),
            private_key_pem: DEVICE_KEY.to_string(),
            ca_pem: CA_CRT.to_string(),
            require_client_cert: true,
            expect_peer_fp: OPERATOR_FP.to_string(),
            key_curve: "P384".to_string(),
            alpn: "durt/1".to_string(),
        }
    }

    fn operator_tls() -> TlsParams {
        TlsParams {
            cert_chain_pem: OPERATOR_CRT.to_string(),
            private_key_pem: OPERATOR_KEY.to_string(),
            ca_pem: CA_CRT.to_string(),
            require_client_cert: true,
            expect_peer_fp: DEVICE_FP.to_string(),
            key_curve: "P384".to_string(),
            alpn: "durt/1".to_string(),
        }
    }

    /// 预探一个空闲 UDP 端口交回 listen 重绑 (accept_one 自己 bind, 不回 addr)。
    fn probe_addr() -> SocketAddr {
        let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let a = s.local_addr().unwrap();
        drop(s);
        a
    }

    /// 服务端 listen(等首包) + 客户端 open → 一对已握手会话。
    async fn make_pair() -> (DurtSession, DurtSession) {
        let server_addr = probe_addr();
        let server_task = tokio::spawn(async move {
            DurtSession::listen(1, server_addr, device_tls(), 100, true)
                .await
                .expect("服务端 listen 失败")
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        let client = DurtSession::open(SessionPlan {
            peer_addr: server_addr,
            tls: operator_tls(),
            conn_id: 2,
            session_id: 100,
            enable_datagram: true,
        })
        .await
        .expect("客户端 open 失败");
        let server = tokio::time::timeout(Duration::from_secs(10), server_task)
            .await
            .expect("服务端建会话超时")
            .expect("服务端任务 panic");
        (client, server)
    }

    async fn recv_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<SessionEvent>) -> SessionEvent {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("等会话事件超时")
            .expect("事件通道已关闭")
    }

    /// 1) 连接建立: 握手成功 + CONTROL 常开流开出 + 对端收到首记录。
    #[tokio::test]
    async fn connect_opens_control_stream_and_delivers_first_record() {
        let (client, server) = make_pair().await;
        // 握手成功: open/listen 只在 ConnState=established (对端指纹复核通过) 后返回。
        assert_eq!(client.session_id(), 100);
        assert_eq!(server.session_id(), 100);
        // CONTROL 常开流已开。
        let control_sid = client.channel_stream(DurtChannel::Control).expect("CONTROL 流未开");
        assert_eq!(control_sid % 4, 0, "client 发起的双向流 id 低 2 位应为 0");
        // 首记录上行: CONTROL 不发 ChannelHello (durt.idl §3), 直接一条业务 record。
        let body = b"ping-body".to_vec();
        client
            .send_record(DurtChannel::Control, wire::RecordType::Ping.repr(), &body)
            .await
            .expect("CONTROL 发送失败");
        let mut ev = server.events();
        match recv_event(&mut ev).await {
            SessionEvent::StreamRecord { channel, record_type, payload } => {
                assert_eq!(channel, DurtChannel::Control, "首记录应归 CONTROL 通道");
                assert_eq!(record_type, wire::RecordType::Ping.repr(), "record 头应解出 type");
                assert_eq!(payload, body, "首记录 body 应字节相等");
            }
            other => panic!("首记录事件不符: {}", describe(&other)),
        }
        client.close().await;
        server.close().await;
    }
    fn describe(ev: &SessionEvent) -> String {
        match ev {
            SessionEvent::StreamRecord { channel, payload, .. } => {
                format!("StreamRecord({channel:?}, {} 字节)", payload.len())
            }
            SessionEvent::VideoDatagram(b) => format!("VideoDatagram({} 字节)", b.len()),
            SessionEvent::AudioDatagram(b) => format!("AudioDatagram({} 字节)", b.len()),
            SessionEvent::Closed(s) => format!("Closed({s})"),
        }
    }

    fn video_datagram(stream_id: u8, tag: &[u8]) -> Vec<u8> {
        let hdr = wire::VideoDatagramHdr {
            magic_ver: wire::pack_magic_ver(wire::DATAGRAM_MAGIC, 1),
            frame_type: wire::VideoFrameType::P as u8,
            media_stream_id: stream_id,
            frame_seq: 7,
            slice_idx: 0,
            slice_cnt: 1,
            fec_group: 0,
            fec_k: 0,
        };
        let mut raw = hdr.encode().to_vec();
        raw.extend_from_slice(tag);
        raw
    }

    /// 2) 帧流: 视频 datagram 原样到对端 (13B VideoDatagramHdr + payload 字节相等)。
    #[tokio::test]
    async fn video_datagram_round_trip_byte_identical() {
        let (client, server) = make_pair().await;
        let raw = video_datagram(1, b"payload-bytes");
        client
            .send_video_datagram(raw.clone())
            .await
            .expect("视频 datagram 发送失败");
        let mut ev = server.events();
        match recv_event(&mut ev).await {
            SessionEvent::VideoDatagram(got) => assert_eq!(got, raw, "视频 datagram 应字节相等"),
            other => panic!("应收到 VideoDatagram: {}", describe(&other)),
        }
        client.close().await;
        server.close().await;
    }

    /// 3) 文件往返 + 通道多路不串: Files 与 Terminal 各开一流, 各自 body 归各自通道。
    #[tokio::test]
    async fn files_record_round_trip_and_channels_do_not_cross() {
        let (client, server) = make_pair().await;
        client
            .open_channel(DurtChannel::Files, DurtChannel::Files.default_urgency())
            .await
            .expect("Files 开通道失败");
        client
            .open_channel(DurtChannel::Terminal, DurtChannel::Terminal.default_urgency())
            .await
            .expect("Terminal 开通道失败");
        let file_body = b"file-open-body".to_vec();
        let term_body = b"term-data-body".to_vec();
        client
            .send_record(DurtChannel::Files, 0x30, &file_body)
            .await
            .expect("Files 发送失败");
        client
            .send_record(DurtChannel::Terminal, 0x10, &term_body)
            .await
            .expect("Terminal 发送失败");
        let mut ev = server.events();
        let mut got_file = false;
        let mut got_term = false;
        for _ in 0..4 {
            match recv_event(&mut ev).await {
                SessionEvent::StreamRecord { channel, record_type, payload } => {
                    if payload == file_body {
                        assert_eq!(channel, DurtChannel::Files, "Files body 不得串到别的通道");
                        assert_eq!(record_type, 0x30, "Files 首业务 record type");
                        got_file = true;
                    } else if payload == term_body {
                        assert_eq!(channel, DurtChannel::Terminal, "Terminal body 不得串到别的通道");
                        assert_eq!(record_type, 0x10, "Terminal 业务 record type");
                        got_term = true;
                    }
                }
                other => panic!("应收到 StreamRecord: {}", describe(&other)),
            }
            if got_file && got_term {
                break;
            }
        }
        assert!(got_file, "未收到 Files body (往返失败)");
        assert!(got_term, "未收到 Terminal body (往返失败)");
        client.close().await;
        server.close().await;
    }
}
