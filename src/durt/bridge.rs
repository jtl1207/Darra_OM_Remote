//! DURT 记录 ↔ rustdesk 内部 Message 协议桥。
//!
//! 连接引擎替换的核心翻译层: UI/会话状态机继续吃吐 `base::message_proto::Message`
//! (protobuf, 见 `libs/base/protos/message.proto`), DURT 线上跑的是 durt.idl 的 record。
//! 本桥只做两侧对齐, UI 一律不动。
//!
//! 两侧权威:
//! - rustdesk 侧: `message.proto` 的 `Message`/`VideoFrame`/`MouseEvent`/`KeyEvent`/
//!   `FileAction`/`FileResponse`/`TerminalAction`/`TerminalResponse`/`LoginRequest`/
//!   `LoginResponse`/`PeerInfo`。
//! - DURT 侧: `durt_wire` 的 V2 体 + `RecordType`/`ChannelKind` 常量; legacy 体
//!   (durt.idl §2/§5/§6/§7) 在本文件最小内联编解码。
//! - 媒体: DURT 屏幕 datagram = 13B `VideoDatagramHdr` + payload (H.264/H.265/VP9/AV1)。
//!
//! 已知有损点 (逐条注释, 不藏):
//! - `VideoDatagramHdr` 不带 codec, codec 由 StartMedia.codec_pref 带外协商;
//!   `durt_to_message` 对视频一律回 `h264s` (字节/关键帧/display 保真)。
//! - rgb/yuv 是"改用未压缩像素"的指令 (无像素载荷), DURT datagram 只承载编码字节,
//!   故走 SCREEN_CTRL `CodecRenegotiate` 桥内哨兵 0xFF/0xFE。
//! - `KeyEvent.press` 是 UI 层 down+up 合成, 线上只有 down; 反向回 `down=true`。
//! - `MouseEvent` 的 BACK/FORWARD 键 (mask bit3/4) 在 DURT PtrButtonV2 只有 0/1/2 三键,
//!   只保 LEFT/RIGHT/MIDDLE。
//! - `CursorData` (光标形状) DURT 无对应记录, 本桥忽略。
//!
//! 能力边界 (DURT 不承载, 显式忽略并记 log, 非 TODO):
//! - rustdesk 旧认证轨 `Hash`/`SignedId`/`PublicKey`/`Auth2FA`/`OSLogin`: 认证走
//!   P-384 mTLS + omt1 票据 (durt.idl §3.2), 不承载旧口令/2FA 轨。
//! - `switch_sides_*` / `plugin_*` / `voice_call_*`: 超出 DURT 能力面。
//! - 光标形状 (`CursorData` / `Misc.request_cursor_data`) 与 Cliprdr `Files` 文件清单
//!   (ClipV2 fmt 恒 UTF8, 无 file 形态); 剪贴板 > 1 MiB 超出 ClipV2 上限。
//! - 媒体编排 (`option`/`video_received`/`*_fps`/`client_record_status`) 与显示控制
//!   (`change_resolution` 等) 的部分字段无 DURT 载体, 映射为最贴近记录且有损, 逐条注明。

use base::message_proto::{
    file_action, file_response, key_event, login_request, login_response, message, terminal_action,
    terminal_response, video_frame, CloseTerminal, ControlKey, CursorPosition, DisplayInfo,
    EncodedVideoFrame, EncodedVideoFrames, FileAction, FileDirCreate, FileDirectory, FileEntry,
    FileRemoveDir, FileRemoveFile, FileRename, FileResponse, FileTransferBlock, FileTransferDone,
    FileTransferDigest, FileTransferError, FileTransferReceiveRequest, FileTransferSendRequest,
    FileType, KeyEvent, LoginRequest, LoginResponse, Message, MouseEvent,
    OpenTerminal, PeerInfo, ReadDir, ResizeTerminal, TerminalAction, TerminalClosed, TerminalData,
    TerminalError, TerminalOpened, TerminalResponse, VideoFrame,
};
use base::message_proto::{
    back_notification, cliprdr, elevation_request, misc, permission_info, pointer_device_event,
    port_forward_channel, touch_event, AudioFormat, BackNotification, ChatMessage, Clipboard,
    Cliprdr, ElevationRequest, ElevationRequestWithLogon, MessageBox, MessageQuery, Misc,
    MultiClipboards, PermissionInfo, PointerDeviceEvent, PortForwardChannel, PortForwardClose,
    PortForwardData, PortForwardOpen, PortForwardOpened, PortForwardWindowUpdate,
    ScreenshotRequest, ScreenshotResponse, SupportedEncoding, SwitchDisplay, TestDelay,
    TogglePrivacyMode,
};
use hbb_common::log;
use hbb_common::protobuf::Enum as _;
use durt_wire::{
    pack_magic_ver, ChannelKind, DisplayDescriptorV2, DisplayTopologyChangedV2,
    DisplayTopologyEndV2, DisplaySelectV2, FileBatchAckV2, FileBatchEntryChunkV2,
    FileBatchEntryOpenV2, FileBatchFinishV2, FileBatchOpenV2, KeyEventV2, PtrButtonV2, PtrMoveV2,
    PtrScrollV2, RecordType, TermCloseV2, TermExitV2, TermOutputV2, TextInputV2, VideoDatagramHdr,
    VideoFrameType, DATAGRAM_MAGIC, PROTO_VERSION, VIDEO_DATAGRAM_HDR_LEN,
};
use durt_wire::{
    ClipV2AckBody, ClipV2CancelBody, ClipV2ChunkBody, ClipV2OfferBody, ClipV2RequestBody,
    ClipboardV2CancelReason, ClipboardV2Direction, ClipboardV2Format, DisplayTopologyRequestV2,
    VideoFeedbackKind, CLIPBOARD_V2_CHUNK_MAX, CLIPBOARD_V2_MAX_BYTES, CLIPBOARD_V2_VERSION,
    DISPLAY_TOPOLOGY_REQUEST_FLAGS_KNOWN, DISPLAY_TOPOLOGY_V2_VERSION,
};
use crate::durt::wire::control::{Ping, Pong};
use crate::durt::wire::media::{StartMedia, StopMedia, VideoFeedback};
#[cfg(test)]
use durt_wire::{decode_record_header, encode_record_header};

/// DURT 通道号 (`durt_wire::ChannelKind` 的线值)。
#[allow(non_camel_case_types)]
pub type durt_channel = u8;

/// UI 侧 → DURT 线 的产物。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurtOut {
    /// 一条 stream 记录。`body` 为 RecordHeader 之后的 payload, 帧头由引擎按
    /// `durt_wire::encode_record_header` 组装。
    Record {
        channel: durt_channel,
        record_type: u16,
        body: Vec<u8>,
    },
    /// 完整屏幕 datagram = 13B `VideoDatagramHdr` + payload。
    VideoDatagram(Vec<u8>),
    /// 完整音频 datagram (DURT 线上无音频 record, 由媒体边车旁路承载)。
    AudioDatagram(Vec<u8>),
}

/// DURT 线 → UI 侧 的输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurtIn {
    Record {
        record_type: u16,
        body: Vec<u8>,
    },
    VideoDatagram(Vec<u8>),
    AudioDatagram(Vec<u8>),
}

// ============ 桥内常量 (durt.idl §6/§7/§8 + rustdesk common::input) ============

/// `KeyEventV2.keycode` 中 ControlKey 编码基址 (与 chr 扫描码不冲突)。
const KEYCODE_CONTROL_BASE: u32 = 0x0100_0000;

// rustdesk 鼠标 mask 布局: `mask = buttons << 3 | type`。
const MOUSE_TYPE_MOVE: i32 = 0;
const MOUSE_TYPE_DOWN: i32 = 1;
const MOUSE_TYPE_UP: i32 = 2;
const MOUSE_TYPE_WHEEL: i32 = 3;
const MOUSE_TYPE_TRACKPAD: i32 = 4;
const MOUSE_TYPE_MASK: i32 = 0x7;
const MOUSE_BUTTON_LEFT: i32 = 0x01;
const MOUSE_BUTTON_RIGHT: i32 = 0x02;
const MOUSE_BUTTON_WHEEL: i32 = 0x04;

// durt.idl §6.2 modifiers 位 (与 DOM KeyboardEvent 对齐)。
const MOD_SHIFT: u8 = 0x01;
const MOD_CTRL: u8 = 0x02;
const MOD_ALT: u8 = 0x04;
const MOD_META: u8 = 0x08;

// durt.idl §7 FileOpen.mode / FileOpKind / FileOpStatus / FileEntryKind。
const FILE_OPEN_MODE_PUT: u8 = 0;
const FILE_OPEN_MODE_GET: u8 = 1;
const FILE_OPEN_MODE_LIST: u8 = 2;
#[allow(dead_code)] // 本桥不搬运 stat 明细, 保留号位
const FOP_KIND_STAT: u8 = 1;
const FOP_KIND_MKDIR: u8 = 2;
const FOP_KIND_RENAME: u8 = 3;
const FOP_KIND_DELETE: u8 = 4;
const FOP_STATUS_OK: u8 = 0;
const FOP_STATUS_IO_ERROR: u8 = 4;
const FOP_FLAG_RECURSIVE: u8 = 0x01;
const FENTRY_KIND_FILE: u8 = 0;
const FENTRY_KIND_DIR: u8 = 1;
const FENTRY_KIND_SYMLINK: u8 = 2;

// durt.idl §5 TermSig / §8 CodecRenegotiate 哨兵 (raw 退路)。
const TERM_SIG_SIGTERM: u8 = 2;
const CODEC_RENEGOTIATE_RAW_RGB: u8 = 0xFF;
const CODEC_RENEGOTIATE_RAW_YUV: u8 = 0xFE;

/// V2 输入/显示体都要求 topology_epoch 非零; 真值由 DisplayTopology 下发,
/// 本桥在无上下文时用此占位 (非零, 合法)。
const TOPOLOGY_EPOCH_PLACEHOLDER: [u8; 16] = [0x5A; 16];
/// V2 体要求 generation > 0。
const GENERATION_PLACEHOLDER: u64 = 1;

// ============ legacy 体最小内联编解码 (durt.idl §2.5 定宽小端 + varint/blob/str) ============

/// 追加式小端写入 (varint = QUIC RFC9000 §16)。
struct W(Vec<u8>);
impl W {
    fn new() -> Self {
        Self(Vec::new())
    }
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn varint(&mut self, v: u64) {
        durt_wire::encode_varint(v, &mut self.0);
    }
    /// varint len + bytes (blob/str 同布局)。
    fn blob(&mut self, v: &[u8]) {
        self.varint(v.len() as u64);
        self.0.extend_from_slice(v);
    }
    fn str(&mut self, v: &str) {
        self.blob(v.as_bytes());
    }
    fn finish(self) -> Vec<u8> {
        self.0
    }
}

/// 顺序小端读取。decode 末尾必须 `done()`。
struct R<'a> {
    b: &'a [u8],
    p: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RErr;
impl<'a> R<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], RErr> {
        if self.p + n > self.b.len() {
            return Err(RErr);
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, RErr> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, RErr> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().map_err(|_| RErr)?))
    }
    fn u32(&mut self) -> Result<u32, RErr> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| RErr)?))
    }
    fn i32(&mut self) -> Result<i32, RErr> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().map_err(|_| RErr)?))
    }
    fn u64(&mut self) -> Result<u64, RErr> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().map_err(|_| RErr)?))
    }
    fn varint(&mut self) -> Result<u64, RErr> {
        if self.p >= self.b.len() {
            return Err(RErr);
        }
        let (v, n) = durt_wire::decode_varint(self.b, self.p);
        if self.p + n > self.b.len() {
            return Err(RErr);
        }
        self.p += n;
        Ok(v)
    }
    fn blob(&mut self) -> Result<Vec<u8>, RErr> {
        let n = self.varint()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> Result<String, RErr> {
        String::from_utf8(self.blob()?).map_err(|_| RErr)
    }
    fn done(&self) -> bool {
        self.p == self.b.len()
    }
}

// ---- legacy 体: 文件 (durt.idl §7) ----

/// `FileOpen` 0x30: `u8 mode | str path | u64 total_size | u32 chunk_size`。
fn enc_file_open(mode: u8, path: &str, total_size: u64, chunk_size: u32) -> Vec<u8> {
    let mut w = W::new();
    w.u8(mode);
    w.str(path);
    w.u64(total_size);
    w.u32(chunk_size);
    w.finish()
}
fn dec_file_open(b: &[u8]) -> Option<(u8, String, u64, u32)> {
    let mut r = R::new(b);
    let mode = r.u8().ok()?;
    let path = r.str().ok()?;
    let total = r.u64().ok()?;
    let chunk = r.u32().ok()?;
    if !r.done() {
        return None;
    }
    Some((mode, path, total, chunk))
}

/// `FileListEntry` 0x34: `str name | u64 size | u64 mtime_unix_ms | u8 kind | u32 mode_bits`。
fn enc_file_list_entry(name: &str, size: u64, mtime: u64, kind: u8, mode_bits: u32) -> Vec<u8> {
    let mut w = W::new();
    w.str(name);
    w.u64(size);
    w.u64(mtime);
    w.u8(kind);
    w.u32(mode_bits);
    w.finish()
}
fn dec_file_list_entry(b: &[u8]) -> Option<(String, u64, u64, u8, u32)> {
    let mut r = R::new(b);
    let name = r.str().ok()?;
    let size = r.u64().ok()?;
    let mtime = r.u64().ok()?;
    let kind = r.u8().ok()?;
    let bits = r.u32().ok()?;
    if !r.done() {
        return None;
    }
    Some((name, size, mtime, kind, bits))
}

/// `FileOpRequest` 0x38: `varint opId | u8 opKind | str path | str path2 | u8 flags`。
/// 布局权威 = `static/Durt/client/src/wire/codecs.ts` §7.1 (与 host C# 一致)。
fn enc_file_op_request(
    op_id: u64,
    op_kind: u8,
    path: &str,
    path2: &str,
    flags: u8,
) -> Vec<u8> {
    let mut w = W::new();
    w.varint(op_id);
    w.u8(op_kind);
    w.str(path);
    w.str(path2);
    w.u8(flags);
    w.finish()
}
fn dec_file_op_request(b: &[u8]) -> Option<(u64, u8, String, String, u8)> {
    let mut r = R::new(b);
    let op_id = r.varint().ok()?;
    let kind = r.u8().ok()?;
    let path = r.str().ok()?;
    let path2 = r.str().ok()?;
    let flags = r.u8().ok()?;
    if !r.done() {
        return None;
    }
    Some((op_id, kind, path, path2, flags))
}

/// `FileOpResult` 0x39: `varint opId | u8 status | str message | u8 hasStat
///   [ | u8 isDir | varint size | varint mtimeUnixSec ]`。
fn enc_file_op_result(op_id: u64, status: u8, message: &str) -> Vec<u8> {
    let mut w = W::new();
    w.varint(op_id);
    w.u8(status);
    w.str(message);
    w.u8(0); // hasStat = 0: 本桥不搬运 stat 明细
    w.finish()
}
fn dec_file_op_result(b: &[u8]) -> Option<(u64, u8, String)> {
    let mut r = R::new(b);
    let op_id = r.varint().ok()?;
    let status = r.u8().ok()?;
    let message = r.str().ok()?;
    let has_stat = r.u8().ok()?;
    if has_stat == 1 {
        let _is_dir = r.u8().ok()?;
        let _size = r.varint().ok()?;
        let _mtime = r.varint().ok()?;
    }
    if !r.done() {
        return None;
    }
    Some((op_id, status, message))
}

// ---- legacy 体: 终端 (durt.idl §5) ----

/// `TermOpen` 0x14: `u8 mode | str program | u16 cols | u16 rows | str cwd`。
fn enc_term_open(mode: u8, program: &str, cols: u16, rows: u16, cwd: &str) -> Vec<u8> {
    let mut w = W::new();
    w.u8(mode);
    w.str(program);
    w.u16(cols);
    w.u16(rows);
    w.str(cwd);
    w.finish()
}
fn dec_term_open(b: &[u8]) -> Option<(u8, String, u16, u16, String)> {
    let mut r = R::new(b);
    let mode = r.u8().ok()?;
    let program = r.str().ok()?;
    let cols = r.u16().ok()?;
    let rows = r.u16().ok()?;
    let cwd = r.str().ok()?;
    if !r.done() {
        return None;
    }
    Some((mode, program, cols, rows, cwd))
}

/// `TermData` 0x10: `blob raw`。
fn enc_term_data(raw: &[u8]) -> Vec<u8> {
    let mut w = W::new();
    w.blob(raw);
    w.finish()
}
fn dec_term_data(b: &[u8]) -> Option<Vec<u8>> {
    let mut r = R::new(b);
    let raw = r.blob().ok()?;
    if !r.done() {
        return None;
    }
    Some(raw)
}

/// `TermResize` 0x11: `u16 cols | u16 rows`。
fn enc_term_resize(cols: u16, rows: u16) -> Vec<u8> {
    let mut w = W::new();
    w.u16(cols);
    w.u16(rows);
    w.finish()
}
fn dec_term_resize(b: &[u8]) -> Option<(u16, u16)> {
    let mut r = R::new(b);
    let cols = r.u16().ok()?;
    let rows = r.u16().ok()?;
    if !r.done() {
        return None;
    }
    Some((cols, rows))
}

/// `TermSignal` 0x12: `u8 sig`。
fn enc_term_signal(sig: u8) -> Vec<u8> {
    let mut w = W::new();
    w.u8(sig);
    w.finish()
}
fn dec_term_signal(b: &[u8]) -> Option<u8> {
    let mut r = R::new(b);
    let sig = r.u8().ok()?;
    if !r.done() {
        return None;
    }
    Some(sig)
}

// ---- legacy 体: 连接/通道控制 (durt.idl §2.1/§3) ----

/// `ConnHello` 0x05: `u8 min | u8 max | u32 cap_bits | varint session_id | u8 role
///   | blob device_fp | blob ext`。
fn enc_conn_hello(
    cap_bits: u32,
    session_id: u64,
    role: u8,
    device_fp: &[u8],
) -> Vec<u8> {
    let mut w = W::new();
    w.u8(PROTO_VERSION);
    w.u8(PROTO_VERSION);
    w.u32(cap_bits);
    w.varint(session_id);
    w.u8(role);
    w.blob(device_fp);
    w.blob(&[]); // ext 预留
    w.finish()
}
fn dec_conn_hello(b: &[u8]) -> Option<(u32, u64, u8, Vec<u8>)> {
    let mut r = R::new(b);
    let _min = r.u8().ok()?;
    let _max = r.u8().ok()?;
    let caps = r.u32().ok()?;
    let sid = r.varint().ok()?;
    let role = r.u8().ok()?;
    let fp = r.blob().ok()?;
    let _ext = r.blob().ok()?;
    if !r.done() {
        return None;
    }
    Some((caps, sid, role, fp))
}

/// `ChannelHello` 0x01: `u8 channel_kind | varint instance_id | varint session_id
///   | u8 flags | blob auth_blob | blob resume_ctx`。
fn enc_channel_hello(
    kind: u8,
    instance_id: u64,
    session_id: u64,
    flags: u8,
) -> Vec<u8> {
    let mut w = W::new();
    w.u8(kind);
    w.varint(instance_id);
    w.varint(session_id);
    w.u8(flags);
    w.blob(&[]);
    w.blob(&[]);
    w.finish()
}
fn dec_channel_hello(b: &[u8]) -> Option<(u8, u64, u64, u8)> {
    let mut r = R::new(b);
    let kind = r.u8().ok()?;
    let instance = r.varint().ok()?;
    let sid = r.varint().ok()?;
    let flags = r.u8().ok()?;
    let _auth = r.blob().ok()?;
    let _resume = r.blob().ok()?;
    if !r.done() {
        return None;
    }
    Some((kind, instance, sid, flags))
}

/// `ChannelAck` 0x02: `varint instance_id | u8 accepted_caps`。
fn enc_channel_ack(instance_id: u64, accepted_caps: u8) -> Vec<u8> {
    let mut w = W::new();
    w.varint(instance_id);
    w.u8(accepted_caps);
    w.finish()
}
fn dec_channel_ack(b: &[u8]) -> Option<(u64, u8)> {
    let mut r = R::new(b);
    let instance = r.varint().ok()?;
    let caps = r.u8().ok()?;
    if !r.done() {
        return None;
    }
    Some((instance, caps))
}

/// `ConnClose` 0x09: `u16 reason | str detail`。
fn enc_conn_close(reason: u16, detail: &str) -> Vec<u8> {
    let mut w = W::new();
    w.u16(reason);
    w.str(detail);
    w.finish()
}
fn dec_conn_close(b: &[u8]) -> Option<(u16, String)> {
    let mut r = R::new(b);
    let reason = r.u16().ok()?;
    let detail = r.str().ok()?;
    if !r.done() {
        return None;
    }
    Some((reason, detail))
}

/// `CodecRenegotiate` 0x43: `u8 new_codec | u16 w | u16 h`。
fn enc_codec_renegotiate(new_codec: u8, w: u16, h: u16) -> Vec<u8> {
    let mut o = W::new();
    o.u8(new_codec);
    o.u16(w);
    o.u16(h);
    o.finish()
}
fn dec_codec_renegotiate(b: &[u8]) -> Option<(u8, u16, u16)> {
    let mut r = R::new(b);
    let codec = r.u8().ok()?;
    let w = r.u16().ok()?;
    let h = r.u16().ok()?;
    if !r.done() {
        return None;
    }
    Some((codec, w, h))
}

// ============ AUX RPC 最小内联编解码 (durt.idl §12: call_id varint | str method | blob args) ============

/// `RpcRequest` 0x70 体。
fn enc_rpc_request(call_id: u64, method: &str, args: &[u8]) -> Vec<u8> {
    let mut w = W::new();
    w.varint(call_id);
    w.str(method);
    w.blob(args);
    w.finish()
}

fn dec_rpc_request(b: &[u8]) -> Option<(u64, String, Vec<u8>)> {
    let mut r = R::new(b);
    let call_id = r.varint().ok()?;
    let method = r.str().ok()?;
    let args = r.blob().ok()?;
    if !r.done() {
        return None;
    }
    Some((call_id, method, args))
}

/// `RpcResponse` 0x71 体: `call_id varint | u16 status | blob result`。
fn enc_rpc_response(call_id: u64, status: u16, result: &[u8]) -> Vec<u8> {
    let mut w = W::new();
    w.varint(call_id);
    w.u16(status);
    w.blob(result);
    w.finish()
}

fn dec_rpc_response(b: &[u8]) -> Option<(u64, u16, Vec<u8>)> {
    let mut r = R::new(b);
    let call_id = r.varint().ok()?;
    let status = r.u16().ok()?;
    let result = r.blob().ok()?;
    if !r.done() {
        return None;
    }
    Some((call_id, status, result))
}

// ============ 两侧公共映射辅助 ============

/// 构造一条 stream 记录产物。
fn rec(channel: durt_channel, record_type: RecordType, body: Vec<u8>) -> DurtOut {
    DurtOut::Record {
        channel,
        record_type: record_type as u16,
        body,
    }
}

/// 把 rustdesk `display` 序号编成 V2 体要的 16B `display_id` (恒非零)。
/// 高位放桥标记 0x01, 末字节放 display 序号, 保证 display=0 时整体非零。
fn display_id_for(display: i32) -> [u8; 16] {
    let mut id = [0u8; 16];
    id[0] = 0x01;
    id[15] = (display as u32 & 0xFF) as u8;
    id
}

/// `display_id` → rustdesk `display` 序号 (与 `display_id_for` 互逆)。
fn display_from_id(id: &[u8; 16]) -> i32 {
    id[15] as i32
}

/// `MouseEvent`/`Cursor` 的屏幕坐标 → V2 归一坐标 (0..=INPUT_V2_NORM_MAX)。
/// 本桥无分辨率上下文, norm 侧留 0, 反向只回读 physical (rustdesk x/y)。
fn norm_zero() -> (u32, u32) {
    (0, 0)
}

/// rustdesk `modifiers` (protobuf 枚举) → DURT 4bit 位图。
fn modifiers_to_bits(mods: &[hbb_common::protobuf::EnumOrUnknown<ControlKey>]) -> u8 {
    let mut bits = 0u8;
    for m in mods {
        match m.enum_value() {
            Ok(ControlKey::Shift) => bits |= MOD_SHIFT,
            Ok(ControlKey::Control) => bits |= MOD_CTRL,
            Ok(ControlKey::Alt) => bits |= MOD_ALT,
            Ok(ControlKey::Meta) => bits |= MOD_META,
            _ => {}
        }
    }
    bits
}

/// DURT 4bit 位图 → rustdesk modifiers 列表 (固定 Shift/Control/Alt/Meta 顺序)。
fn bits_to_modifiers(bits: u8) -> Vec<hbb_common::protobuf::EnumOrUnknown<ControlKey>> {
    let mut out = Vec::new();
    if bits & MOD_SHIFT != 0 {
        out.push(ControlKey::Shift.into());
    }
    if bits & MOD_CTRL != 0 {
        out.push(ControlKey::Control.into());
    }
    if bits & MOD_ALT != 0 {
        out.push(ControlKey::Alt.into());
    }
    if bits & MOD_META != 0 {
        out.push(ControlKey::Meta.into());
    }
    out
}

/// `ControlKey` → `KeyEventV2.keycode` 高位基址编码。
fn control_key_code(ck: ControlKey) -> u32 {
    KEYCODE_CONTROL_BASE + ck as i32 as u32
}

/// `KeyEventV2.keycode` → `ControlKey` (非 ControlKey 编码返回 None)。
fn control_key_from_code(code: u32) -> Option<ControlKey> {
    if code < KEYCODE_CONTROL_BASE {
        return None;
    }
    ControlKey::from_i32((code - KEYCODE_CONTROL_BASE) as i32)
}

/// sha256 占位 (V2 体要求 digest 非零); 本桥不搬运真实内容摘要, 只保证非零合法。
fn digest_placeholder(seed: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(seed);
    let out = h.finalize();
    let mut d = [0u8; 32];
    d.copy_from_slice(&out);
    if d == [0u8; 32] {
        d[0] = 1;
    }
    d
}

fn bytes16_placeholder(seed: u64) -> [u8; 16] {
    let mut id = [0u8; 16];
    id[..8].copy_from_slice(&seed.to_le_bytes());
    id[8..].copy_from_slice(&(!seed).to_le_bytes());
    if id == [0u8; 16] {
        id[0] = 1;
    }
    id
}

// ============ UI 侧 → DURT 线 ============

/// UI 侧 Message → DURT 记录/datagram 列表。
/// 一个 Message 可拆成多条 (鼠标按 mask 拆、文件列表按条目拆、peer_info displays 拆)。
/// `Message` oneof 每个变体都有明确分支; 返回空 Vec = 能力边界不承载 (durt_skip 已记 log)。
pub fn message_to_durt(msg: &Message) -> Vec<DurtOut> {
    match msg.union.as_ref() {
        Some(message::Union::VideoFrame(vf)) => video_frame_to_durt(vf),
        Some(message::Union::AudioFrame(af)) => vec![DurtOut::AudioDatagram(af.data.to_vec())],
        Some(message::Union::MouseEvent(me)) => mouse_to_durt(me),
        Some(message::Union::KeyEvent(ke)) => key_to_durt(ke),
        Some(message::Union::FileAction(fa)) => file_action_to_durt(fa),
        Some(message::Union::FileResponse(fr)) => file_response_to_durt(fr),
        Some(message::Union::TerminalAction(ta)) => terminal_action_to_durt(ta),
        Some(message::Union::TerminalResponse(tr)) => terminal_response_to_durt(tr),
        Some(message::Union::LoginRequest(lr)) => login_request_to_durt(lr),
        Some(message::Union::LoginResponse(lr)) => login_response_to_durt(lr),
        Some(message::Union::PeerInfo(pi)) => peer_info_to_durt(pi),
        Some(message::Union::CursorId(id)) => vec![cursor_id_to_durt(*id)],
        // CursorData 是光标形状 (像素), DURT 无对应记录, 忽略 (见文件头"已知有损点")。
        Some(message::Union::CursorData(_)) => Vec::new(),
        // CursorPosition 是光标位置, DURT 侧即指针坐标, 复用 PtrMoveV2。
        Some(message::Union::CursorPosition(cp)) => vec![cursor_position_to_durt(cp)],
        Some(message::Union::TestDelay(td)) => test_delay_to_durt(td),
        Some(message::Union::Misc(m)) => misc_to_durt(m),
        Some(message::Union::Clipboard(c)) => clipboard_to_durt(c),
        Some(message::Union::MultiClipboards(mc)) => multi_clipboards_to_durt(mc),
        Some(message::Union::Cliprdr(c)) => cliprdr_to_durt(c),
        Some(message::Union::MessageBox(mb)) => message_box_to_durt(mb),
        Some(message::Union::ScreenshotRequest(sr)) => screenshot_request_to_durt(sr),
        Some(message::Union::ScreenshotResponse(sr)) => screenshot_response_to_durt(sr),
        Some(message::Union::PointerDeviceEvent(p)) => pointer_device_event_to_durt(p),
        Some(message::Union::PortForwardChannel(pf)) => port_forward_to_durt(pf),
        // 认证走 P-384 mTLS + omt1 票据 (durt.idl §3.2), 不承载 rustdesk 旧口令/2FA 轨。
        Some(message::Union::Hash(_)) => durt_skip("hash 旧口令挑战"),
        Some(message::Union::SignedId(_)) => durt_skip("signed_id 旧认证"),
        Some(message::Union::PublicKey(_)) => durt_skip("public_key 旧密钥交换"),
        Some(message::Union::Auth2fa(_)) => durt_skip("auth_2fa 旧 2FA"),
        Some(message::Union::OsLogin(_)) => durt_skip("os_login 旧系统登录"),
        // 能力边界: 两侧互换 / 语音通话 DURT 不承载 (见文件头)。
        Some(message::Union::SwitchSidesResponse(_)) => durt_skip("switch_sides_response"),
        Some(message::Union::VoiceCallRequest(_)) | Some(message::Union::VoiceCallResponse(_)) => {
            durt_skip("voice_call_*")
        }
        // `message::Union` non_exhaustive: 新增变体显式忽略并记 log (能力边界)。
        _ => durt_skip("message 未映射变体"),
    }
}

/// `video_frame` → 屏幕 VideoDatagram (每 EncodedVideoFrame 一条)。
/// rgb/yuv 是无像素载荷的"回退未压缩"指令, 走 SCREEN_CTRL CodecRenegotiate。
fn video_frame_to_durt(vf: &VideoFrame) -> Vec<DurtOut> {
    let display = vf.display;
    let (frames, frame_type_of) = match vf.union.as_ref() {
        Some(video_frame::Union::H264s(f)) => (f, true),
        Some(video_frame::Union::H265s(f)) => (f, true),
        Some(video_frame::Union::Vp8s(f)) => (f, true),
        Some(video_frame::Union::Vp9s(f)) => (f, true),
        Some(video_frame::Union::Av1s(f)) => (f, true),
        Some(video_frame::Union::Rgb(_)) => {
            return vec![rec(
                ChannelKind::ScreenCtrl.repr(),
                RecordType::CodecRenegotiate,
                enc_codec_renegotiate(CODEC_RENEGOTIATE_RAW_RGB, 0, 0),
            )];
        }
        Some(video_frame::Union::Yuv(_)) => {
            return vec![rec(
                ChannelKind::ScreenCtrl.repr(),
                RecordType::CodecRenegotiate,
                enc_codec_renegotiate(CODEC_RENEGOTIATE_RAW_YUV, 0, 0),
            )];
        }
        _ => return Vec::new(),
    };
    if !frame_type_of {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(frames.frames.len());
    for (i, f) in frames.frames.iter().enumerate() {
        out.push(DurtOut::VideoDatagram(encode_video_datagram(
            f,
            display,
            i as u32,
        )));
    }
    out
}

/// 一个 EncodedVideoFrame → 13B `VideoDatagramHdr` + payload。
fn encode_video_datagram(f: &EncodedVideoFrame, display: i32, seq: u32) -> Vec<u8> {
    let hdr = VideoDatagramHdr {
        magic_ver: pack_magic_ver(DATAGRAM_MAGIC, PROTO_VERSION),
        frame_type: if f.key {
            VideoFrameType::IIdr.repr()
        } else {
            VideoFrameType::P.repr()
        },
        media_stream_id: (display as u32 & 0xFF) as u8,
        frame_seq: seq,
        slice_idx: 0,
        slice_cnt: 1,
        fec_group: 0,
        fec_k: 0,
    };
    let mut buf = Vec::with_capacity(VIDEO_DATAGRAM_HDR_LEN as usize + f.data.len());
    buf.extend_from_slice(&hdr.encode());
    buf.extend_from_slice(&f.data);
    buf
}

/// `mouse_event` → PtrMoveV2 / PtrButtonV2 / PtrScrollV2 (按 mask 拆)。
fn mouse_to_durt(me: &MouseEvent) -> Vec<DurtOut> {
    let evt_type = me.mask & MOUSE_TYPE_MASK;
    let buttons = (me.mask >> 3) & 0xFF;
    let (nx, ny) = norm_zero();
    let display_id = display_id_for(0);
    let ch = ChannelKind::Input.repr();
    match evt_type {
        MOUSE_TYPE_MOVE => {
            let v = PtrMoveV2 {
                event_seq: 1,
                topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
                generation: GENERATION_PLACEHOLDER,
                display_id,
                norm_x: nx,
                norm_y: ny,
                physical_x: me.x,
                physical_y: me.y,
                buttons: (buttons & 0x07) as u8,
                reserved: [0u8; 3],
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::PtrMoveV2, body)],
                Err(_) => Vec::new(),
            }
        }
        MOUSE_TYPE_DOWN | MOUSE_TYPE_UP => {
            let Some(button) = lowest_button(buttons) else {
                return Vec::new();
            };
            let v = PtrButtonV2 {
                event_seq: 1,
                topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
                generation: GENERATION_PLACEHOLDER,
                display_id,
                norm_x: nx,
                norm_y: ny,
                physical_x: me.x,
                physical_y: me.y,
                button,
                down: if evt_type == MOUSE_TYPE_DOWN { 1 } else { 0 },
                reserved: 0,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::PtrButtonV2, body)],
                Err(_) => Vec::new(),
            }
        }
        MOUSE_TYPE_WHEEL | MOUSE_TYPE_TRACKPAD => {
            // rustdesk 滚轮: x/y 即滚动增量 (服务端再乘 WHEEL_DELTA)。
            let v = PtrScrollV2 {
                event_seq: 1,
                topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
                generation: GENERATION_PLACEHOLDER,
                display_id,
                norm_x: nx,
                norm_y: ny,
                physical_x: 0,
                physical_y: 0,
                dx: me.x as i16,
                dy: me.y as i16,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::PtrScrollV2, body)],
                Err(_) => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

/// rustdesk button 位 → DURT PtrButtonV2.button (0=left 1=right 2=middle)。
/// BACK/FORWARD (bit3/4) 无 DURT 对应, 丢弃 (见文件头)。
fn lowest_button(buttons: i32) -> Option<u8> {
    if buttons & MOUSE_BUTTON_LEFT != 0 {
        Some(0)
    } else if buttons & MOUSE_BUTTON_RIGHT != 0 {
        Some(1)
    } else if buttons & MOUSE_BUTTON_WHEEL != 0 {
        Some(2)
    } else {
        None
    }
}

/// DURT PtrButtonV2.button → rustdesk button 位。
fn button_bit(button: u8) -> i32 {
    match button {
        1 => MOUSE_BUTTON_RIGHT,
        2 => MOUSE_BUTTON_WHEEL,
        _ => MOUSE_BUTTON_LEFT,
    }
}

/// `key_event` → KeyEventV2 (control_key/chr/win2win_hotkey) 或 TextInputV2 (unicode/seq)。
fn key_to_durt(ke: &KeyEvent) -> Vec<DurtOut> {
    let ch = ChannelKind::Input.repr();
    let mods = modifiers_to_bits(&ke.modifiers);
    let down: u8 = if ke.down || ke.press { 1 } else { 0 };
    match ke.union.as_ref() {
        Some(key_event::Union::ControlKey(ck)) => {
            let code = control_key_code(ck.enum_value().unwrap_or(ControlKey::Unknown));
            let v = KeyEventV2 {
                event_seq: 1,
                topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
                generation: GENERATION_PLACEHOLDER,
                display_id: display_id_for(0),
                keycode: code,
                down,
                modifiers: mods,
                reserved: 0,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::KeyEventV2, body)],
                Err(_) => Vec::new(),
            }
        }
        Some(key_event::Union::Chr(code)) => {
            if *code == 0 {
                return Vec::new();
            }
            let v = KeyEventV2 {
                event_seq: 1,
                topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
                generation: GENERATION_PLACEHOLDER,
                display_id: display_id_for(0),
                keycode: *code,
                down,
                modifiers: mods,
                reserved: 0,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::KeyEventV2, body)],
                Err(_) => Vec::new(),
            }
        }
        Some(key_event::Union::Win2winHotkey(code)) => {
            if *code == 0 {
                return Vec::new();
            }
            let v = KeyEventV2 {
                event_seq: 1,
                topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
                generation: GENERATION_PLACEHOLDER,
                display_id: display_id_for(0),
                keycode: *code,
                down,
                modifiers: mods,
                reserved: 0,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::KeyEventV2, body)],
                Err(_) => Vec::new(),
            }
        }
        // unicode/char 文本输入 → TextInputV2 (IME 合成文本)。
        Some(key_event::Union::Unicode(c)) => {
            let mut buf = [0u8; 4];
            match char::from_u32(*c) {
                Some(ch) => {
                    let s = ch.encode_utf8(&mut buf).to_string();
                    text_to_durt(s.into_bytes())
                }
                None => Vec::new(),
            }
        }
        Some(key_event::Union::Seq(seq)) => text_to_durt(seq.as_bytes().to_vec()),
        _ => Vec::new(),
    }
}

/// 文本字节 → TextInputV2 记录。
fn text_to_durt(utf8: Vec<u8>) -> Vec<DurtOut> {
    if utf8.is_empty() {
        return Vec::new();
    }
    let v = TextInputV2 {
        event_seq: 1,
        topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
        generation: GENERATION_PLACEHOLDER,
        display_id: display_id_for(0),
        utf8,
    };
    match v.encode() {
        Ok(body) => vec![rec(ChannelKind::Input.repr(), RecordType::TextInputV2, body)],
        Err(_) => Vec::new(),
    }
}

/// `cursor_id` → DisplaySelectV2 (光标焦点跳到哪块屏 ≈ 选屏)。
fn cursor_id_to_durt(id: u64) -> DurtOut {
    let v = DisplaySelectV2 {
        request_id: if id == 0 { 1 } else { id },
        media_stream_id: (id & 0xFF) as u8,
        flags: 0,
        reserved: 0,
        topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
        generation: GENERATION_PLACEHOLDER,
        display_id: display_id_for((id & 0xFF) as i32),
    };
    let body = v.encode().unwrap_or_default();
    rec(ChannelKind::ScreenCtrl.repr(), RecordType::DisplaySelectV2, body)
}

/// `cursor_position` → PtrMoveV2 (DURT 无独立光标位置记录, 复用指针移动)。
fn cursor_position_to_durt(cp: &CursorPosition) -> DurtOut {
    let (nx, ny) = norm_zero();
    let v = PtrMoveV2 {
        event_seq: 1,
        topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
        generation: GENERATION_PLACEHOLDER,
        display_id: display_id_for(0),
        norm_x: nx,
        norm_y: ny,
        physical_x: cp.x,
        physical_y: cp.y,
        buttons: 0,
        reserved: [0u8; 3],
    };
    let body = v.encode().unwrap_or_default();
    rec(ChannelKind::Input.repr(), RecordType::PtrMoveV2, body)
}

// ============ 控制面 / 剪贴板 / 杂项: UI → DURT ============

/// 显式不承载 (能力边界见文件头): 记 log 后返回空。
fn durt_skip(what: &str) -> Vec<DurtOut> {
    log::debug!("durt.bridge: DURT 不承载 {what}, 显式忽略");
    Vec::new()
}

/// `test_delay` → Ping (from_client 探测) / Pong (回声)。延迟探测走控制面。
fn test_delay_to_durt(td: &TestDelay) -> Vec<DurtOut> {
    let ch = ChannelKind::Control.repr();
    let t = td.time.max(0) as u64;
    if td.from_client {
        let v = Ping {
            nonce: if t == 0 { 1 } else { t },
            send_unix_ms: t,
        };
        vec![rec(ch, RecordType::Ping, v.encode())]
    } else {
        let v = Pong {
            nonce: if t == 0 { 1 } else { t },
            send_unix_ms: t,
            recv_unix_ms: t + td.last_delay as u64,
        };
        vec![rec(ch, RecordType::Pong, v.encode())]
    }
}

/// `video_feedback` 一条 (ScreenCtrl)。
fn vf_rec(kind: u8, recv_delivery_rate: u32) -> DurtOut {
    let v = VideoFeedback {
        kind,
        frame_seq: 0,
        slice_idx: 0,
        recv_delivery_rate,
        recv_loss_ppm: 0,
        recv_jitter_us: 0,
    };
    rec(ChannelKind::ScreenCtrl.repr(), RecordType::VideoFeedback, v.encode())
}

/// `start_media` 一条 (ScreenCtrl, 恒 VIDEO / stream 1)。
fn start_media_rec(init_target_bps: u32, max_fps: u16) -> DurtOut {
    let v = StartMedia {
        media_kind: 0,
        media_stream_id: 1,
        init_target_bps,
        max_fps,
        w: 0,
        h: 0,
        codec_pref: 0,
    };
    rec(ChannelKind::ScreenCtrl.repr(), RecordType::StartMedia, v.encode())
}

fn stop_media_rec() -> DurtOut {
    let v = StopMedia { media_stream_id: 1 };
    rec(ChannelKind::ScreenCtrl.repr(), RecordType::StopMedia, v.encode())
}

/// `display` 序号 → DisplaySelectV2 (选屏)。
fn display_select_rec(display: i32) -> DurtOut {
    let v = DisplaySelectV2 {
        request_id: 1,
        media_stream_id: (display as u32 & 0xFF) as u8,
        flags: 0,
        reserved: 0,
        topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
        generation: GENERATION_PLACEHOLDER,
        display_id: display_id_for(display),
    };
    let body = v.encode().unwrap_or_default();
    rec(ChannelKind::ScreenCtrl.repr(), RecordType::DisplaySelectV2, body)
}

/// 拓扑重协商 (FORCE) — 承载无 display 载荷的分辨率/采集集变更 (w/h 等有损)。
fn display_topology_force_rec() -> DurtOut {
    let v = DisplayTopologyRequestV2 {
        version: DISPLAY_TOPOLOGY_V2_VERSION,
        flags: DISPLAY_TOPOLOGY_REQUEST_FLAGS_KNOWN,
        reserved: 0,
        known_topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
        known_generation: GENERATION_PLACEHOLDER,
    };
    let body = v.encode().unwrap_or_default();
    rec(
        ChannelKind::ScreenCtrl.repr(),
        RecordType::DisplayTopologyRequestV2,
        body,
    )
}

/// 无专用 DURT 记录的 Misc/控制子项 → AuxRpc `RpcRequest` (durt.idl §12)。
fn misc_rpc_rec(method: &str, args: Vec<u8>) -> DurtOut {
    rec(
        ChannelKind::AuxRpc.repr(),
        RecordType::RpcRequest,
        enc_rpc_request(1, method, &args),
    )
}

fn misc_bool_rec(method: &str, on: bool) -> DurtOut {
    misc_rpc_rec(method, vec![on as u8])
}

/// `misc` → DURT。逐个 oneof 显式分支; 无对应能力面的记 log 返回空 (见文件头能力边界)。
fn misc_to_durt(m: &Misc) -> Vec<DurtOut> {
    match m.union.as_ref() {
        // 关闭原因 → ConnClose (0x09)。
        Some(misc::Union::CloseReason(s)) => vec![rec(
            ChannelKind::Control.repr(),
            RecordType::ConnClose,
            enc_conn_close(0 /* NORMAL */, s),
        )],
        // 媒体编排 → VideoFeedback / StartMedia / StopMedia (无对应字段有损)。
        Some(misc::Union::RefreshVideo(_)) | Some(misc::Union::RefreshVideoDisplay(_)) => {
            vec![vf_rec(VideoFeedbackKind::ReqIdr.repr(), 0)]
        }
        Some(misc::Union::VideoReceived(ok)) => {
            if *ok {
                vec![vf_rec(VideoFeedbackKind::CcStat.repr(), 1)]
            } else {
                vec![vf_rec(VideoFeedbackKind::Nack.repr(), 0)]
            }
        }
        Some(misc::Union::AutoAdjustFps(n)) | Some(misc::Union::FullSpeedFps(n)) => {
            vec![start_media_rec(0, (*n).min(u16::MAX as u32) as u16)]
        }
        Some(misc::Union::ClientRecordStatus(on)) => {
            if *on {
                vec![start_media_rec(0, 0)]
            } else {
                vec![stop_media_rec()]
            }
        }
        // OptionMessage 仅 custom_image_quality / custom_fps 两标量过桥 (载体 StartMedia)。
        Some(misc::Union::Option(om)) => vec![start_media_rec(
            om.custom_image_quality.max(0) as u32,
            om.custom_fps.max(0).min(u16::MAX as i32) as u16,
        )],
        // 显示控制 → DisplaySelectV2 / DisplayTopologyRequestV2 (w/h 无载体, 有损)。
        Some(misc::Union::SwitchDisplay(sd)) => vec![display_select_rec(sd.display)],
        Some(misc::Union::ChangeDisplayResolution(dr)) => vec![display_select_rec(dr.display)],
        Some(misc::Union::ToggleVirtualDisplay(tv)) => vec![display_select_rec(tv.display)],
        Some(misc::Union::FollowCurrentDisplay(d)) => vec![display_select_rec(*d)],
        Some(misc::Union::ChangeResolution(_)) | Some(misc::Union::CaptureDisplays(_)) => {
            vec![display_topology_force_rec()]
        }

        // 无 DURT 专用记录的控制面 → RpcRequest (0x70) 承载。
        Some(misc::Union::ChatMessage(cm)) => {
            let mut w = W::new();
            w.str(&cm.text);
            vec![misc_rpc_rec("misc.chat_message", w.finish())]
        }
        Some(misc::Union::AudioFormat(af)) => {
            let mut w = W::new();
            w.u32(af.sample_rate);
            w.u32(af.channels);
            vec![misc_rpc_rec("misc.audio_format", w.finish())]
        }
        Some(misc::Union::PermissionInfo(pi)) => {
            let mut w = W::new();
            w.i32(pi.permission.value());
            w.u8(pi.enabled as u8);
            vec![misc_rpc_rec("misc.permission_info", w.finish())]
        }
        Some(misc::Union::BackNotification(bn)) => {
            let mut w = W::new();
            match bn.union.as_ref() {
                Some(back_notification::Union::PrivacyModeState(s)) => {
                    w.u8(0);
                    w.i32(s.value());
                }
                Some(back_notification::Union::BlockInputState(s)) => {
                    w.u8(1);
                    w.i32(s.value());
                }
                None => {
                    w.u8(2);
                    w.i32(0);
                }
                // 该 oneof non_exhaustive: 未知状态按无状态编码。
                _ => {
                    w.u8(2);
                    w.i32(0);
                }
            }
            w.str(&bn.details);
            w.str(&bn.impl_key);
            vec![misc_rpc_rec("misc.back_notification", w.finish())]
        }

        Some(misc::Union::ElevationRequest(er)) => {
            let mut w = W::new();
            match er.union.as_ref() {
                Some(elevation_request::Union::Direct(b)) => {
                    w.u8(0);
                    w.u8(*b as u8);
                }
                Some(elevation_request::Union::Logon(l)) => {
                    w.u8(1);
                    w.str(&l.username);
                    w.str(&l.password);
                }
                // 该 oneof non_exhaustive: 未知形态按占位标记。
                None => w.u8(2),
                _ => w.u8(2),
            }
            vec![misc_rpc_rec("misc.elevation_request", w.finish())]
        }
        Some(misc::Union::ElevationResponse(s)) => {
            let mut w = W::new();
            w.str(s);
            vec![misc_rpc_rec("misc.elevation_response", w.finish())]
        }
        Some(misc::Union::Uac(b)) => vec![misc_bool_rec("misc.uac", *b)],
        Some(misc::Union::RestartRemoteDevice(b)) => {
            vec![misc_bool_rec("misc.restart_remote_device", *b)]
        }
        Some(misc::Union::StopService(b)) => vec![misc_bool_rec("misc.stop_service", *b)],
        Some(misc::Union::PortableServiceRunning(b)) => {
            vec![misc_bool_rec("misc.portable_service_running", *b)]
        }
        Some(misc::Union::ForegroundWindowElevated(b)) => {
            vec![misc_bool_rec("misc.foreground_window_elevated", *b)]
        }
        Some(misc::Union::TogglePrivacyMode(tp)) => {
            let mut w = W::new();
            w.str(&tp.impl_key);
            w.u8(tp.on as u8);
            vec![misc_rpc_rec("misc.toggle_privacy_mode", w.finish())]
        }
        Some(misc::Union::SupportedEncoding(se)) => {
            let mut w = W::new();
            w.u8(se.h264 as u8);
            w.u8(se.h265 as u8);
            w.u8(se.vp8 as u8);
            w.u8(se.av1 as u8);
            vec![misc_rpc_rec("misc.supported_encoding", w.finish())]
        }
        Some(misc::Union::SelectedSid(sid)) => {
            let mut w = W::new();
            w.u32(*sid);
            vec![misc_rpc_rec("misc.selected_sid", w.finish())]
        }
        Some(misc::Union::MessageQuery(mq)) => {
            let mut w = W::new();
            w.i32(mq.switch_display);
            vec![misc_rpc_rec("misc.message_query", w.finish())]
        }
        // 能力边界: 光标形状 / 两侧互换 / 插件面 DURT 不承载。
        Some(misc::Union::RequestCursorData(_)) => durt_skip("misc.request_cursor_data 光标形状"),
        Some(misc::Union::SwitchSidesRequest(_)) | Some(misc::Union::SwitchBack(_)) => {
            durt_skip("misc.switch_sides_*")
        }
        Some(misc::Union::PluginRequest(_)) | Some(misc::Union::PluginFailure(_)) => {
            durt_skip("misc.plugin_*")
        }
        None => Vec::new(),
        _ => durt_skip("misc 未映射子项"),
    }
}

/// `clipboard` → ClipV2 Offer + Chunk(s) (durt.idl §9)。format 无载体 (fmt 恒 UTF8, 有损);
/// > 1 MiB 超出 ClipV2 上限, 不承载。
fn clipboard_to_durt(c: &Clipboard) -> Vec<DurtOut> {
    clip_offer_and_chunks(&c.content, 1)
}

/// `multi_clipboards` → 每个 Clipboard 一次 Offer+Chunk 流 (transfer_id 递增)。
fn multi_clipboards_to_durt(mc: &MultiClipboards) -> Vec<DurtOut> {
    let mut out = Vec::new();
    for (i, c) in mc.clipboards.iter().enumerate() {
        out.extend(clip_offer_and_chunks(&c.content, i as u64 + 1));
    }
    out
}

/// 一段字节 → ClipV2OfferBody + 每 ≤61440B 一条 ClipV2ChunkBody (空内容仅 Offer)。
fn clip_offer_and_chunks(content: &[u8], transfer_id: u64) -> Vec<DurtOut> {
    if content.len() > CLIPBOARD_V2_MAX_BYTES as usize {
        return durt_skip("clipboard > 1MiB (ClipV2 上限)");
    }
    let ch = ChannelKind::Clipboard.repr();
    let offer = ClipV2OfferBody {
        version: CLIPBOARD_V2_VERSION,
        transfer_id,
        direction: ClipboardV2Direction::OperatorToDevice.repr(),
        sequence: 1,
        fmt: ClipboardV2Format::Utf8.repr(),
        size: content.len() as u32,
        sha256: digest_placeholder(content),
    };
    let mut out = Vec::new();
    match offer.encode() {
        Ok(body) => out.push(rec(ch, RecordType::ClipV2Offer, body)),
        Err(_) => return Vec::new(),
    }
    for (i, slice) in content.chunks(CLIPBOARD_V2_CHUNK_MAX as usize).enumerate() {
        let chunk = ClipV2ChunkBody {
            transfer_id,
            sequence: 1,
            offset: (i as u32) * CLIPBOARD_V2_CHUNK_MAX,
            data: slice.to_vec(),
        };
        match chunk.encode() {
            Ok(body) => out.push(rec(ch, RecordType::ClipV2Chunk, body)),
            Err(_) => return Vec::new(),
        }
    }
    out
}

/// `cliprdr` (Windows 文件剪贴板) → ClipV2 最贴近形态: 数据应答 → Offer+Chunk;
/// 内容请求 → ClipV2Request; 就绪/格式表 → ClipV2Ack; 清空 → ClipV2Cancel;
/// `Files` 清单 ClipV2 无 file 形态 (fmt 恒 UTF8), 不承载。
fn cliprdr_to_durt(c: &Cliprdr) -> Vec<DurtOut> {
    let ch = ChannelKind::Clipboard.repr();
    match c.union.as_ref() {
        Some(cliprdr::Union::FormatDataResponse(fr)) => clip_offer_and_chunks(&fr.format_data, 1),
        Some(cliprdr::Union::FileContentsResponse(fr)) => {
            clip_offer_and_chunks(&fr.requested_data, 1)
        }
        Some(cliprdr::Union::FormatDataRequest(_)) | Some(cliprdr::Union::FileContentsRequest(_)) => {
            let v = ClipV2RequestBody {
                request_id: 1,
                direction: ClipboardV2Direction::DeviceToOperator.repr(),
                max_bytes: CLIPBOARD_V2_MAX_BYTES,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::ClipV2Request, body.to_vec())],
                Err(_) => Vec::new(),
            }
        }
        Some(cliprdr::Union::Ready(_))
        | Some(cliprdr::Union::FormatList(_))
        | Some(cliprdr::Union::FormatListResponse(_)) => {
            let v = ClipV2AckBody {
                transfer_id: 1,
                sequence: 1,
                next_offset: 0,
                credit: CLIPBOARD_V2_CHUNK_MAX,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::ClipV2Ack, body.to_vec())],
                Err(_) => Vec::new(),
            }
        }
        Some(cliprdr::Union::TryEmpty(_)) => {
            let v = ClipV2CancelBody {
                transfer_id: 1,
                sequence: 1,
                reason: ClipboardV2CancelReason::UserCancel.repr(),
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::ClipV2Cancel, body.to_vec())],
                Err(_) => Vec::new(),
            }
        }
        Some(cliprdr::Union::Files(_)) => durt_skip("cliprdr.Files 文件清单"),
        None => Vec::new(),
        _ => durt_skip("cliprdr 未映射子项"),
    }
}

/// `pointer_device_event` (触控/笔) → PtrButtonV2 / PtrMoveV2 / PtrScrollV2。
/// 触控增量坐标按绝对坐标过桥 (有损); `modifiers` 无 DURT 载体, 丢弃。
fn pointer_device_event_to_durt(p: &PointerDeviceEvent) -> Vec<DurtOut> {
    let te = match p.union.as_ref() {
        Some(pointer_device_event::Union::TouchEvent(te)) => te,
        _ => return durt_skip("pointer_device_event 未映射子项"),
    };
    let ch = ChannelKind::Input.repr();
    let btn = |down: u8, x: i32, y: i32| PtrButtonV2 {
        event_seq: 1,
        topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
        generation: GENERATION_PLACEHOLDER,
        display_id: display_id_for(0),
        norm_x: 0,
        norm_y: 0,
        physical_x: x,
        physical_y: y,
        button: 0,
        down,
        reserved: 0,
    };
    let out = match te.union.as_ref() {
        Some(touch_event::Union::PanStart(ps)) => {
            match btn(1, ps.x, ps.y).encode() {
                Ok(body) => vec![rec(ch, RecordType::PtrButtonV2, body)],
                Err(_) => Vec::new(),
            }
        }
        Some(touch_event::Union::PanUpdate(pu)) => {
            let v = PtrMoveV2 {
                event_seq: 1,
                topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
                generation: GENERATION_PLACEHOLDER,
                display_id: display_id_for(0),
                norm_x: 0,
                norm_y: 0,
                physical_x: pu.x,
                physical_y: pu.y,
                buttons: 1,
                reserved: [0u8; 3],
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::PtrMoveV2, body)],
                Err(_) => Vec::new(),
            }
        }
        Some(touch_event::Union::PanEnd(pe)) => {
            match btn(0, pe.x, pe.y).encode() {
                Ok(body) => vec![rec(ch, RecordType::PtrButtonV2, body)],
                Err(_) => Vec::new(),
            }
        }
        Some(touch_event::Union::ScaleUpdate(su)) => {
            let v = PtrScrollV2 {
                event_seq: 1,
                topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
                generation: GENERATION_PLACEHOLDER,
                display_id: display_id_for(0),
                norm_x: 0,
                norm_y: 0,
                physical_x: 0,
                physical_y: 0,
                dx: 0,
                dy: su.scale.clamp(i16::MIN as i32, i16::MAX as i32) as i16,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::PtrScrollV2, body)],
                Err(_) => Vec::new(),
            }
        }
        _ => durt_skip("touch_event 未映射子项"),
    };
    out
}

/// `message_box` → RpcRequest `message_box` (msgtype/title/text/link)。
fn message_box_to_durt(mb: &MessageBox) -> Vec<DurtOut> {
    let mut w = W::new();
    w.str(&mb.msgtype);
    w.str(&mb.title);
    w.str(&mb.text);
    w.str(&mb.link);
    vec![misc_rpc_rec("message_box", w.finish())]
}

/// `screenshot_request` → RpcRequest `screenshot_request` (display + sid)。
fn screenshot_request_to_durt(sr: &ScreenshotRequest) -> Vec<DurtOut> {
    let mut w = W::new();
    w.i32(sr.display);
    w.str(&sr.sid);
    vec![misc_rpc_rec("screenshot_request", w.finish())]
}

/// `screenshot_response` → RpcResponse (status=0/空 msg=成功; result = sid|msg|data)。
fn screenshot_response_to_durt(sr: &ScreenshotResponse) -> Vec<DurtOut> {
    let mut w = W::new();
    w.str(&sr.sid);
    w.str(&sr.msg);
    w.blob(&sr.data);
    let status = if sr.msg.is_empty() { 0 } else { 1 };
    vec![rec(
        ChannelKind::AuxRpc.repr(),
        RecordType::RpcResponse,
        enc_rpc_response(1, status, &w.finish()),
    )]
}

/// `port_forward_channel` → RpcRequest `port_forward.*`。DURT NETWORK 面是 route/ACL
/// grant 体系 (durt.idl §13), 本桥无 grant 上下文, 故经 AuxRpc 承载, 不伪造 NetworkTcpOpen。
fn port_forward_to_durt(pf: &PortForwardChannel) -> Vec<DurtOut> {
    match pf.union.as_ref() {
        Some(port_forward_channel::Union::Open(o)) => {
            let mut w = W::new();
            w.i32(o.channel_id);
            w.str(&o.host);
            w.i32(o.port);
            w.u32(o.window);
            vec![misc_rpc_rec("port_forward.open", w.finish())]
        }
        Some(port_forward_channel::Union::Opened(o)) => {
            let mut w = W::new();
            w.i32(o.channel_id);
            w.u8(o.success as u8);
            w.str(&o.message);
            w.u32(o.window);
            vec![misc_rpc_rec("port_forward.opened", w.finish())]
        }
        Some(port_forward_channel::Union::Data(d)) => {
            let mut w = W::new();
            w.i32(d.channel_id);
            w.blob(&d.data);
            vec![misc_rpc_rec("port_forward.data", w.finish())]
        }
        Some(port_forward_channel::Union::Close(c)) => {
            let mut w = W::new();
            w.i32(c.channel_id);
            vec![misc_rpc_rec("port_forward.close", w.finish())]
        }
        Some(port_forward_channel::Union::WindowUpdate(u)) => {
            let mut w = W::new();
            w.i32(u.channel_id);
            w.u32(u.add);
            vec![misc_rpc_rec("port_forward.window_update", w.finish())]
        }
        None => Vec::new(),
        _ => durt_skip("port_forward 未映射子项"),
    }
}

// ============ 文件: UI → DURT ============

/// `file_action` → FileOpen / FileOpRequest / FileBatchOpenV2 / FileBatchEntryOpenV2。
fn file_action_to_durt(fa: &FileAction) -> Vec<DurtOut> {
    let ch = ChannelKind::Files.repr();
    match fa.union.as_ref() {
        // ReadDir → FileOpen.mode=2 (list)。
        Some(file_action::Union::ReadDir(rd)) => vec![rec(
            ch,
            RecordType::FileOpen,
            enc_file_open(FILE_OPEN_MODE_LIST, &rd.path, 0, 0),
        )],
        // ReadEmptyDirs 同 list 语义 (无独立 DURT 记录)。
        Some(file_action::Union::ReadEmptyDirs(rd)) => vec![rec(
            ch,
            RecordType::FileOpen,
            enc_file_open(FILE_OPEN_MODE_LIST, &rd.path, 0, 0),
        )],
        // mkdir → FileOpRequest(Mkdir)。
        Some(file_action::Union::Create(c)) => vec![rec(
            ch,
            RecordType::FileOpRequest,
            enc_file_op_request(
                c.id as u64,
                FOP_KIND_MKDIR,
                &c.path,
                "",
                FOP_FLAG_RECURSIVE,
            ),
        )],
        // remove_dir → FileOpRequest(Delete, recursive)。
        Some(file_action::Union::RemoveDir(rd)) => vec![rec(
            ch,
            RecordType::FileOpRequest,
            enc_file_op_request(
                rd.id as u64,
                FOP_KIND_DELETE,
                &rd.path,
                "",
                if rd.recursive {
                    FOP_FLAG_RECURSIVE
                } else {
                    0
                },
            ),
        )],
        // remove_file → FileOpRequest(Delete)。
        Some(file_action::Union::RemoveFile(rf)) => vec![rec(
            ch,
            RecordType::FileOpRequest,
            enc_file_op_request(rf.id as u64, FOP_KIND_DELETE, &rf.path, "", 0),
        )],
        // rename → FileOpRequest(Rename, path2=new_name)。
        Some(file_action::Union::Rename(r)) => vec![rec(
            ch,
            RecordType::FileOpRequest,
            enc_file_op_request(
                r.id as u64,
                FOP_KIND_RENAME,
                &r.path,
                &r.new_name,
                0,
            ),
        )],
        // send (下载) → FileBatchOpenV2(GET) + 按文件条目 FileBatchEntryOpenV2(Request)。
        Some(file_action::Union::Send(s)) => file_send_to_durt(s),
        // receive (上传) → FileBatchOpenV2(PUT) + 按文件条目 FileBatchEntryOpenV2。
        Some(file_action::Union::Receive(r)) => file_receive_to_durt(r),
        // cancel → FileBatchFinishV2(ABORT)。
        Some(file_action::Union::Cancel(c)) => vec![rec(
            ch,
            RecordType::FileBatchFinishV2,
            enc_batch_abort(c.id as u64),
        )],
        // AllFiles / SendConfirm 无对应 DURT 记录, 交上层。
        _ => Vec::new(),
    }
}

/// FileTransferSendRequest → FileBatchOpenV2(GET)。
fn file_send_to_durt(s: &FileTransferSendRequest) -> Vec<DurtOut> {
    let ch = ChannelKind::Files.repr();
    let batch_id = if s.id > 0 { s.id as u64 } else { 1 };
    let manifest = digest_placeholder(s.path.as_bytes());
    let open = FileBatchOpenV2 {
        version: durt_wire::FILE_BATCH_V2_VERSION,
        mode: FILE_OPEN_MODE_GET,
        flags: durt_wire::FILE_BATCH_V2_FLAG_SNAPSHOT_REQUIRED,
        batch_id,
        entry_count: if s.file_num > 0 {
            s.file_num as u32
        } else {
            1
        },
        total_bytes: 0,
        manifest_sha256: manifest,
        chunk_bytes: durt_wire::FILE_BATCH_V2_CHUNK_MAX,
        root_path: s.path.clone(),
    };
    match open.encode() {
        Ok(body) => vec![rec(ch, RecordType::FileBatchOpenV2, body)],
        Err(_) => Vec::new(),
    }
}

/// FileTransferReceiveRequest → FileBatchOpenV2(PUT) + 每个 FileEntry 一条 EntryOpenV2。
fn file_receive_to_durt(r: &FileTransferReceiveRequest) -> Vec<DurtOut> {
    let ch = ChannelKind::Files.repr();
    let batch_id = if r.id > 0 { r.id as u64 } else { 1 };
    let manifest = digest_placeholder(r.path.as_bytes());
    let mut out = Vec::new();
    let open = FileBatchOpenV2 {
        version: durt_wire::FILE_BATCH_V2_VERSION,
        mode: FILE_OPEN_MODE_PUT,
        flags: durt_wire::FILE_BATCH_V2_FLAG_ATOMIC_NEW_TREE,
        batch_id,
        entry_count: if r.file_num > 0 {
            r.file_num as u32
        } else {
            r.files.len() as u32
        },
        total_bytes: r.total_size,
        manifest_sha256: manifest,
        chunk_bytes: durt_wire::FILE_BATCH_V2_CHUNK_MAX,
        root_path: r.path.clone(),
    };
    match open.encode() {
        Ok(body) => out.push(rec(ch, RecordType::FileBatchOpenV2, body)),
        Err(_) => return Vec::new(),
    }
    for (i, f) in r.files.iter().enumerate() {
        let entry = FileBatchEntryOpenV2 {
            batch_id,
            entry_id: (i as u64) + 1,
            phase: 0, // FileBatchEntryPhaseV2::Request
            kind: 0,  // FileBatchEntryKindV2::Unspecified (Request 阶段必须 0)
            flags: 0,
            mode_bits: 0,
            size: 0,
            content_sha256: [0u8; 32],
            rel_path: f.name.clone(),
        };
        match entry.encode() {
            Ok(body) => out.push(rec(ch, RecordType::FileBatchEntryOpenV2, body)),
            Err(_) => {}
        }
    }
    out
}

/// FileBatchFinishV2(ABORT) 体 (legacy 布局同 V2, 走 durt_wire codec)。
fn enc_batch_abort(batch_id: u64) -> Vec<u8> {
    let v = FileBatchFinishV2 {
        version: durt_wire::FILE_BATCH_V2_VERSION,
        action: 3, // FileBatchFinishActionV2::Abort
        status: 10, // Cancelled
        batch_id,
        entry_count: 0,
        total_bytes: 0,
        manifest_sha256: digest_placeholder(&batch_id.to_le_bytes()),
        receipt_id: [0u8; 16], // Abort 时必须为零
    };
    v.encode().unwrap_or_default()
}

// ============ 文件: 反向 (durt_to_message) 用的正向对 ============

/// FileResponse → FileListEntry / FileBatchEntryChunkV2 / FileBatchAckV2 /
/// FileBatchFinishV2 / FileOpResult。
fn file_response_to_durt(fr: &FileResponse) -> Vec<DurtOut> {
    let ch = ChannelKind::Files.repr();
    match fr.union.as_ref() {
        // FileDirectory → 每个 entry 一条 FileListEntry。
        Some(file_response::Union::Dir(fd)) => {
            let mut out = Vec::new();
            for e in &fd.entries {
                out.push(rec(
                    ch,
                    RecordType::FileListEntry,
                    enc_file_list_entry(
                        &e.name,
                        e.size,
                        e.modified_time,
                        file_entry_kind(e),
                        0,
                    ),
                ));
            }
            out
        }
        // FileTransferBlock → FileBatchEntryChunkV2。
        Some(file_response::Union::Block(b)) => {
            let v = FileBatchEntryChunkV2 {
                batch_id: if b.id > 0 { b.id as u64 } else { 1 },
                entry_id: if b.file_num > 0 {
                    b.file_num as u64
                } else {
                    1
                },
                offset: b.blk_id as u64 * b.data.len() as u64,
                chunk_len: b.data.len() as u32,
                chunk_sha256: digest_placeholder(&b.data),
                data: b.data.to_vec(),
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::FileBatchEntryChunkV2, body)],
                Err(_) => Vec::new(),
            }
        }
        // FileTransferError → FileOpResult(IoError)。
        Some(file_response::Union::Error(e)) => vec![rec(
            ch,
            RecordType::FileOpResult,
            enc_file_op_result(e.id as u64, FOP_STATUS_IO_ERROR, &e.error),
        )],
        // FileTransferDone → FileBatchFinishV2(Result)。
        Some(file_response::Union::Done(d)) => vec![rec(
            ch,
            RecordType::FileBatchFinishV2,
            enc_batch_result(d.id as u64),
        )],
        // FileTransferDigest → FileBatchAckV2。
        Some(file_response::Union::Digest(d)) => {
            let v = FileBatchAckV2 {
                batch_id: if d.id > 0 { d.id as u64 } else { 1 },
                entry_id: if d.file_num > 0 {
                    d.file_num as u64
                } else {
                    0
                },
                next_offset: d.transferred_size,
                credit_limit: d.file_size,
                status: 0,
                flags: 0,
                observed_sha256: digest_placeholder(&d.file_size.to_le_bytes()),
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::FileBatchAckV2, body)],
                Err(_) => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

/// FileEntry 的 protobuf FileType → DURT FileEntryKind。
fn file_entry_kind(e: &FileEntry) -> u8 {
    match e.entry_type.enum_value().unwrap_or(FileType::File) {
        FileType::Dir | FileType::DirDrive | FileType::DirLink => FENTRY_KIND_DIR,
        FileType::FileLink => FENTRY_KIND_SYMLINK,
        _ => FENTRY_KIND_FILE,
    }
}

/// FileBatchFinishV2(Result) 体。
fn enc_batch_result(batch_id: u64) -> Vec<u8> {
    let v = FileBatchFinishV2 {
        version: durt_wire::FILE_BATCH_V2_VERSION,
        action: 2, // Result
        status: 0,
        batch_id,
        entry_count: 0,
        total_bytes: 0,
        manifest_sha256: digest_placeholder(&batch_id.to_le_bytes()),
        receipt_id: bytes16_placeholder(batch_id),
    };
    v.encode().unwrap_or_default()
}

// ============ 终端: UI → DURT ============

/// `terminal_action` → TermOpen / TermData / TermResize / TermSignal。
fn terminal_action_to_durt(ta: &TerminalAction) -> Vec<DurtOut> {
    let ch = ChannelKind::Terminal.repr();
    match ta.union.as_ref() {
        Some(terminal_action::Union::Open(o)) => vec![rec(
            ch,
            RecordType::TermOpen,
            enc_term_open(0, "", o.cols as u16, o.rows as u16, ""),
        )],
        Some(terminal_action::Union::Data(d)) => vec![rec(
            ch,
            RecordType::TermData,
            enc_term_data(&d.data),
        )],
        Some(terminal_action::Union::Resize(r)) => vec![rec(
            ch,
            RecordType::TermResize,
            enc_term_resize(r.cols as u16, r.rows as u16),
        )],
        // close → TermSignal(SIGTERM); TermCloseV2 留给被控端回错误。
        Some(terminal_action::Union::Close(_)) => vec![rec(
            ch,
            RecordType::TermSignal,
            enc_term_signal(TERM_SIG_SIGTERM),
        )],
        _ => Vec::new(),
    }
}

/// `terminal_response` → TermLeaseGrantAckV2(opened) / TermOutputV2(data) /
/// TermExitV2(closed) / TermCloseV2(error)。
fn terminal_response_to_durt(tr: &TerminalResponse) -> Vec<DurtOut> {
    let ch = ChannelKind::Terminal.repr();
    match tr.union.as_ref() {
        Some(terminal_response::Union::Opened(o)) => {
            let pty_id = if o.terminal_id > 0 {
                o.terminal_id as u64
            } else {
                1
            };
            let v = durt_wire::TermLeaseGrantAckV2 {
                lease_id: bytes16_placeholder(pty_id),
                pty_id,
                attach_epoch: GENERATION_PLACEHOLDER,
                receipt_id: bytes16_placeholder(pty_id ^ 0xA5A5),
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::TermLeaseGrantAckV2, body)],
                Err(_) => Vec::new(),
            }
        }
        Some(terminal_response::Union::Data(d)) => {
            let pty_id = if d.terminal_id > 0 {
                d.terminal_id as u64
            } else {
                1
            };
            let chunk = d.data.to_vec();
            if chunk.is_empty() {
                return Vec::new();
            }
            let v = TermOutputV2 {
                lease_id: bytes16_placeholder(pty_id),
                pty_id,
                attach_epoch: GENERATION_PLACEHOLDER,
                output_seq: 1,
                event_length: chunk.len() as u32,
                chunk_offset: 0,
                flags: durt_wire::TERM_OUTPUT_FLAG_LAST,
                reserved: 0,
                chunk,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::TermOutputV2, body)],
                Err(_) => Vec::new(),
            }
        }
        Some(terminal_response::Union::Closed(c)) => {
            let pty_id = if c.terminal_id > 0 {
                c.terminal_id as u64
            } else {
                1
            };
            let v = TermExitV2 {
                lease_id: bytes16_placeholder(pty_id),
                pty_id,
                attach_epoch: GENERATION_PLACEHOLDER,
                last_output_seq: 1,
                exit_code: c.exit_code,
                flags: 0,
                reserved: 0,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::TermExitV2, body)],
                Err(_) => Vec::new(),
            }
        }
        Some(terminal_response::Union::Error(e)) => {
            let pty_id = if e.terminal_id > 0 {
                e.terminal_id as u64
            } else {
                1
            };
            let v = TermCloseV2 {
                lease_id: bytes16_placeholder(pty_id),
                pty_id,
                resume_token: [1u8; 32], // V2 校验要求非零
                attach_epoch: GENERATION_PLACEHOLDER,
                last_acked_seq: 1,
                reason: 1,
                reserved: 0,
            };
            match v.encode() {
                Ok(body) => vec![rec(ch, RecordType::TermCloseV2, body)],
                Err(_) => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

// ============ 登录 / peer_info: UI → DURT ============

/// `login_request` → ConnHello + 各通道 ChannelHello。
fn login_request_to_durt(lr: &LoginRequest) -> Vec<DurtOut> {
    let ch = ChannelKind::Control.repr();
    let mut out = Vec::new();
    // role: 1 = operator (主控侧发起登录)。
    out.push(rec(
        ch,
        RecordType::ConnHello,
        enc_conn_hello(
            cap_bits_for(lr),
            lr.session_id,
            1,
            &lr.hwid,
        ),
    ));
    for (kind, flag) in channel_plan_for(lr) {
        out.push(rec(
            ch,
            RecordType::ChannelHello,
            enc_channel_hello(kind, 1, lr.session_id, flag),
        ));
    }
    out
}

/// LoginRequest 携带的会话类型 → 需要开的 DURT 通道。
fn channel_plan_for(lr: &LoginRequest) -> Vec<(u8, u8)> {
    let mut plan = vec![
        (ChannelKind::Input.repr(), 0),
        (ChannelKind::ScreenCtrl.repr(), 0),
    ];
    match lr.union.as_ref() {
        Some(login_request::Union::FileTransfer(_)) => {
            plan.push((ChannelKind::Files.repr(), 0));
        }
        Some(login_request::Union::Terminal(_)) => {
            plan.push((ChannelKind::Terminal.repr(), 0));
        }
        _ => {}
    }
    plan
}

/// LoginRequest → ConnHello.cap_bits (本桥按会话类型给最小交集)。
fn cap_bits_for(lr: &LoginRequest) -> u32 {
    let mut caps = durt_wire::CAP_FILE_BATCH_V2 | durt_wire::CAP_INPUT_V2;
    if matches!(lr.union, Some(login_request::Union::Terminal(_))) {
        caps |= durt_wire::CAP_TERMINAL_RESUME_V2;
    }
    caps |= durt_wire::CAP_DISPLAY_TOPOLOGY_V2;
    caps
}

/// `login_response` → ConnClose(error) 或 ConnHello(成功回执)。
fn login_response_to_durt(lr: &LoginResponse) -> Vec<DurtOut> {
    let ch = ChannelKind::Control.repr();
    match lr.union.as_ref() {
        Some(login_response::Union::Error(err)) => vec![rec(
            ch,
            RecordType::ConnClose,
            enc_conn_close(5 /* PROTOCOL_ERROR */, err),
        )],
        Some(login_response::Union::PeerInfo(pi)) => {
            let mut out = peer_info_to_durt(pi);
            out.insert(
                0,
                rec(
                    ch,
                    RecordType::ConnHello,
                    enc_conn_hello(0, 0, 0, &[]),
                ),
            );
            // 登录行的 ChannelAck(0x02) 回执 (accepting)。
            out.insert(1, rec(ch, RecordType::ChannelAck, enc_channel_ack(1, 0x01)));
            out
        }
        _ => Vec::new(),
    }
}

/// `peer_info` 的 displays → DisplayDescriptorV2 + DisplayTopologyEndV2。
fn peer_info_to_durt(pi: &PeerInfo) -> Vec<DurtOut> {
    let ch = ChannelKind::ScreenCtrl.repr();
    let mut out = Vec::new();
    for (i, d) in pi.displays.iter().enumerate() {
        out.push(rec(
            ch,
            RecordType::DisplayDescriptorV2,
            enc_display_descriptor(d, i as u32, i as i32 == pi.current_display),
        ));
    }
    out.push(rec(
        ch,
        RecordType::DisplayTopologyEndV2,
        enc_display_topology_end(pi.displays.len() as u16),
    ));
    out
}

/// DisplayInfo → DisplayDescriptorV2 体。
fn enc_display_descriptor(d: &DisplayInfo, index: u32, primary: bool) -> Vec<u8> {
    let scale_milli = if d.scale > 0.0 {
        (d.scale * 1000.0).round() as u32
    } else {
        1000
    };
    let v = DisplayDescriptorV2 {
        topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
        generation: GENERATION_PLACEHOLDER,
        display_id: display_id_for(index as i32),
        left: d.x,
        top: d.y,
        right: d.x + d.width,
        bottom: d.y + d.height,
        scale_x_milli: scale_milli.clamp(durt_wire::DISPLAY_SCALE_MILLI_MIN, durt_wire::DISPLAY_SCALE_MILLI_MAX),
        scale_y_milli: scale_milli.clamp(durt_wire::DISPLAY_SCALE_MILLI_MIN, durt_wire::DISPLAY_SCALE_MILLI_MAX),
        flags: if primary {
            durt_wire::DISPLAY_DESCRIPTOR_FLAG_PRIMARY
        } else {
            0
        },
        rotation: 0,
        reserved: 0,
    };
    v.encode().unwrap_or_default()
}

/// DisplayTopologyEndV2 体。
fn enc_display_topology_end(count: u16) -> Vec<u8> {
    let v = DisplayTopologyEndV2 {
        topology_epoch: TOPOLOGY_EPOCH_PLACEHOLDER,
        generation: GENERATION_PLACEHOLDER,
        display_count: count,
        reserved: 0,
        topology_digest: digest_placeholder(&count.to_le_bytes()),
    };
    v.encode().unwrap_or_default()
}

/// RpcRequest → Message: 本桥自产的 `misc.*` / `message_box` / `screenshot_request` /
/// `port_forward.*` 方法按 method 还原; 其他 method 不还原 (None)。
fn rpc_request_to_message(method: &str, args: &[u8]) -> Option<Message> {
    let mut r = R::new(args);
    match method {
        "misc.chat_message" => {
            let text = r.str().ok()?;
            let mut ms = Misc::new();
            ms.set_chat_message(ChatMessage {
                text,
                ..Default::default()
            });
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        "misc.audio_format" => {
            let sample_rate = r.u32().ok()?;
            let channels = r.u32().ok()?;
            let mut ms = Misc::new();
            ms.set_audio_format(AudioFormat {
                sample_rate,
                channels,
                ..Default::default()
            });
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        "misc.permission_info" => {
            let perm = permission_info::Permission::from_i32(r.i32().ok()?)?;
            let enabled = r.u8().ok()? != 0;
            let mut ms = Misc::new();
            ms.set_permission_info(PermissionInfo {
                permission: perm.into(),
                enabled,
                ..Default::default()
            });
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        "misc.back_notification" => {
            let tag = r.u8().ok()?;
            let state = r.i32().ok()?;
            let mut bn = BackNotification {
                details: r.str().ok()?,
                impl_key: r.str().ok()?,
                ..Default::default()
            };
            match tag {
                0 => bn.set_privacy_mode_state(back_notification::PrivacyModeState::from_i32(state)?),
                1 => bn.set_block_input_state(back_notification::BlockInputState::from_i32(state)?),
                _ => return None,
            }
            let mut ms = Misc::new();
            ms.set_back_notification(bn);
            Some(msg_with(move |m| m.set_misc(ms)))
        }

        "misc.elevation_request" => {
            let tag = r.u8().ok()?;
            let mut er = ElevationRequest::new();
            match tag {
                0 => er.set_direct(r.u8().ok()? != 0),
                1 => {
                    let username = r.str().ok()?;
                    let password = r.str().ok()?;
                    er.set_logon(ElevationRequestWithLogon {
                        username,
                        password,
                        ..Default::default()
                    });
                }
                _ => return None,
            }
            let mut ms = Misc::new();
            ms.set_elevation_request(er);
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        "misc.elevation_response" => {
            let mut ms = Misc::new();
            ms.set_elevation_response(r.str().ok()?);
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        "misc.uac" | "misc.restart_remote_device" | "misc.stop_service"
        | "misc.portable_service_running" | "misc.foreground_window_elevated" => {
            let on = r.u8().ok()? != 0;
            let mut ms = Misc::new();
            match method {
                "misc.uac" => ms.set_uac(on),
                "misc.restart_remote_device" => ms.set_restart_remote_device(on),
                "misc.stop_service" => ms.set_stop_service(on),
                "misc.portable_service_running" => ms.set_portable_service_running(on),
                _ => ms.set_foreground_window_elevated(on),
            }
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        "misc.toggle_privacy_mode" => {
            let impl_key = r.str().ok()?;
            let on = r.u8().ok()? != 0;
            let mut ms = Misc::new();
            ms.set_toggle_privacy_mode(TogglePrivacyMode {
                impl_key,
                on,
                ..Default::default()
            });
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        "misc.supported_encoding" => {
            let mut se = SupportedEncoding::default();
            se.h264 = r.u8().ok()? != 0;
            se.h265 = r.u8().ok()? != 0;
            se.vp8 = r.u8().ok()? != 0;
            se.av1 = r.u8().ok()? != 0;
            let mut ms = Misc::new();
            ms.set_supported_encoding(se);
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        "misc.selected_sid" => {
            let mut ms = Misc::new();
            ms.set_selected_sid(r.u32().ok()?);
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        "misc.message_query" => {
            let mut ms = Misc::new();
            ms.set_message_query(MessageQuery {
                switch_display: r.i32().ok()?,
                ..Default::default()
            });
            Some(msg_with(move |m| m.set_misc(ms)))
        }

        "message_box" => {
            let mb = MessageBox {
                msgtype: r.str().ok()?,
                title: r.str().ok()?,
                text: r.str().ok()?,
                link: r.str().ok()?,
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_message_box(mb)))
        }
        "screenshot_request" => {
            let sr = ScreenshotRequest {
                display: r.i32().ok()?,
                sid: r.str().ok()?,
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_screenshot_request(sr)))
        }
        "port_forward.open" => {
            let channel_id = r.i32().ok()?;
            let host = r.str().ok()?;
            let port = r.i32().ok()?;
            let window = r.u32().ok()?;
            let mut pf = PortForwardChannel::new();
            pf.set_open(PortForwardOpen {
                channel_id,
                host,
                port,
                window,
                ..Default::default()
            });
            Some(msg_with(move |m| m.set_port_forward_channel(pf)))
        }
        "port_forward.opened" => {
            let channel_id = r.i32().ok()?;
            let success = r.u8().ok()? != 0;
            let message = r.str().ok()?;
            let window = r.u32().ok()?;
            let mut pf = PortForwardChannel::new();
            pf.set_opened(PortForwardOpened {
                channel_id,
                success,
                message,
                window,
                ..Default::default()
            });
            Some(msg_with(move |m| m.set_port_forward_channel(pf)))
        }
        "port_forward.data" => {
            let channel_id = r.i32().ok()?;
            let data = r.blob().ok()?;
            let mut pf = PortForwardChannel::new();
            pf.set_data(PortForwardData {
                channel_id,
                data: data.into(),
                ..Default::default()
            });
            Some(msg_with(move |m| m.set_port_forward_channel(pf)))
        }
        "port_forward.close" => {
            let mut pf = PortForwardChannel::new();
            pf.set_close(PortForwardClose {
                channel_id: r.i32().ok()?,
                ..Default::default()
            });
            Some(msg_with(move |m| m.set_port_forward_channel(pf)))
        }
        "port_forward.window_update" => {
            let channel_id = r.i32().ok()?;
            let add = r.u32().ok()?;
            let mut pf = PortForwardChannel::new();
            pf.set_window_update(PortForwardWindowUpdate {
                channel_id,
                add,
                ..Default::default()
            });
            Some(msg_with(move |m| m.set_port_forward_channel(pf)))
        }
        _ => {
            log::debug!("durt.bridge: RpcRequest 未映射 method={method}");
            None
        }
    }
}

/// RpcResponse → `screenshot_response` (本桥 RpcResponse 只由 ScreenshotResponse 产生)。
fn rpc_response_to_message(status: u16, result: &[u8]) -> Option<Message> {
    let mut r = R::new(result);
    let sid = r.str().ok()?;
    let mut msg_text = r.str().ok()?;
    let data = r.blob().ok()?;
    if msg_text.is_empty() && status != 0 {
        msg_text = format!("rpc status={status}");
    }
    let sr = ScreenshotResponse {
        sid,
        msg: msg_text,
        data: data.into(),
        ..Default::default()
    };
    Some(msg_with(move |m| m.set_screenshot_response(sr)))
}

// ============ DURT 线 → UI 侧 ============

/// DURT 输入 → UI 侧 Message。`None` = 本桥不处理, 交上层。
pub fn durt_to_message(input: &DurtIn) -> Option<Message> {
    match input {
        DurtIn::Record { record_type, body } => record_to_message(*record_type, body),
        DurtIn::VideoDatagram(bytes) => video_datagram_to_message(bytes),
        DurtIn::AudioDatagram(bytes) => {
            let mut m = Message::new();
            let mut af = base::message_proto::AudioFrame::new();
            af.data = bytes.clone().into();
            m.set_audio_frame(af);
            Some(m)
        }
    }
}

/// 一条 stream 记录 → Message。
fn record_to_message(record_type: u16, body: &[u8]) -> Option<Message> {
    let ty = RecordType::from_repr(record_type)?;
    match ty {
        // ---- 输入 ----
        RecordType::PtrMoveV2 => {
            let v = PtrMoveV2::decode(body).ok()?;
            let me = MouseEvent {
                mask: MOUSE_TYPE_MOVE | ((v.buttons as i32 & 0x07) << 3),
                x: v.physical_x,
                y: v.physical_y,
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_mouse_event(me)))
        }
        RecordType::PtrButtonV2 => {
            let v = PtrButtonV2::decode(body).ok()?;
            let evt_type = if v.down == 1 {
                MOUSE_TYPE_DOWN
            } else {
                MOUSE_TYPE_UP
            };
            let me = MouseEvent {
                mask: evt_type | (button_bit(v.button) << 3),
                x: v.physical_x,
                y: v.physical_y,
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_mouse_event(me)))
        }
        RecordType::PtrScrollV2 => {
            let v = PtrScrollV2::decode(body).ok()?;
            let me = MouseEvent {
                mask: MOUSE_TYPE_WHEEL,
                x: v.dx as i32,
                y: v.dy as i32,
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_mouse_event(me)))
        }
        RecordType::KeyEventV2 => {
            let v = KeyEventV2::decode(body).ok()?;
            let mut ke = KeyEvent {
                down: v.down == 1,
                press: false,
                modifiers: bits_to_modifiers(v.modifiers),
                ..Default::default()
            };
            if let Some(ck) = control_key_from_code(v.keycode) {
                ke.union = Some(key_event::Union::ControlKey(ck.into()));
            } else {
                ke.union = Some(key_event::Union::Chr(v.keycode));
            }
            Some(msg_with(move |m| m.set_key_event(ke)))
        }
        RecordType::TextInputV2 => {
            let v = TextInputV2::decode(body).ok()?;
            let mut ke = KeyEvent {
                down: true,
                press: false,
                ..Default::default()
            };
            let text = String::from_utf8(v.utf8.clone()).ok()?;
            // 单字符回 unicode, 多字符回 seq (与 message_to_durt 互逆)。
            let mut it = text.chars();
            if let (Some(c), None) = (it.next(), it.next()) {
                ke.union = Some(key_event::Union::Unicode(c as u32));
            } else {
                ke.union = Some(key_event::Union::Seq(text));
            }
            Some(msg_with(move |m| m.set_key_event(ke)))
        }

        // ---- 文件 ----
        RecordType::FileOpen => {
            let (mode, path, _total, _chunk) = dec_file_open(body)?;
            let fa = match mode {
                FILE_OPEN_MODE_LIST => {
                    let rd = ReadDir {
                        path,
                        include_hidden: true,
                        ..Default::default()
                    };
                    let mut fa = FileAction::new();
                    fa.set_read_dir(rd);
                    fa
                }
                FILE_OPEN_MODE_GET => {
                    let s = FileTransferSendRequest {
                        path,
                        ..Default::default()
                    };
                    let mut fa = FileAction::new();
                    fa.set_send(s);
                    fa
                }
                _ => {
                    let r = FileTransferReceiveRequest {
                        path,
                        ..Default::default()
                    };
                    let mut fa = FileAction::new();
                    fa.set_receive(r);
                    fa
                }
            };
            Some(msg_with(move |m| m.set_file_action(fa)))
        }
        RecordType::FileListEntry => {
            let (name, size, mtime, kind, _bits) = dec_file_list_entry(body)?;
            let entry = FileEntry {
                entry_type: match kind {
                    FENTRY_KIND_DIR => FileType::Dir.into(),
                    FENTRY_KIND_SYMLINK => FileType::FileLink.into(),
                    _ => FileType::File.into(),
                },
                name,
                size,
                modified_time: mtime,
                ..Default::default()
            };
            let fd = FileDirectory {
                entries: vec![entry],
                ..Default::default()
            };
            let mut fr = FileResponse::new();
            fr.set_dir(fd);
            Some(msg_with(move |m| m.set_file_response(fr)))
        }
        RecordType::FileOpRequest => {
            let (op_id, kind, path, path2, flags) = dec_file_op_request(body)?;
            let fa = match kind {
                FOP_KIND_MKDIR => {
                    let c = FileDirCreate {
                        id: op_id as i32,
                        path,
                        ..Default::default()
                    };
                    let mut fa = FileAction::new();
                    fa.set_create(c);
                    fa
                }
                FOP_KIND_DELETE => {
                    if flags & FOP_FLAG_RECURSIVE != 0 {
                        let rd = FileRemoveDir {
                            id: op_id as i32,
                            path,
                            recursive: true,
                            ..Default::default()
                        };
                        let mut fa = FileAction::new();
                        fa.set_remove_dir(rd);
                        fa
                    } else {
                        let rf = FileRemoveFile {
                            id: op_id as i32,
                            path,
                            ..Default::default()
                        };
                        let mut fa = FileAction::new();
                        fa.set_remove_file(rf);
                        fa
                    }
                }
                FOP_KIND_RENAME => {
                    let r = FileRename {
                        id: op_id as i32,
                        path,
                        new_name: path2,
                        ..Default::default()
                    };
                    let mut fa = FileAction::new();
                    fa.set_rename(r);
                    fa
                }
                _ => return None,
            };
            Some(msg_with(move |m| m.set_file_action(fa)))
        }
        RecordType::FileOpResult => {
            let (op_id, status, msg_text) = dec_file_op_result(body)?;
            let fr = if status == FOP_STATUS_OK {
                let d = FileTransferDone {
                    id: op_id as i32,
                    ..Default::default()
                };
                let mut fr = FileResponse::new();
                fr.set_done(d);
                fr
            } else {
                let e = FileTransferError {
                    id: op_id as i32,
                    error: msg_text,
                    ..Default::default()
                };
                let mut fr = FileResponse::new();
                fr.set_error(e);
                fr
            };
            Some(msg_with(move |m| m.set_file_response(fr)))
        }
        RecordType::FileBatchEntryChunkV2 => {
            let v = FileBatchEntryChunkV2::decode(body).ok()?;
            let blk_id = if v.chunk_len > 0 {
                (v.offset / v.chunk_len as u64) as u32
            } else {
                0
            };
            let b = FileTransferBlock {
                id: v.batch_id as i32,
                file_num: v.entry_id as i32,
                data: v.data.clone().into(),
                blk_id,
                ..Default::default()
            };
            let mut fr = FileResponse::new();
            fr.set_block(b);
            Some(msg_with(move |m| m.set_file_response(fr)))
        }
        RecordType::FileBatchAckV2 => {
            let v = FileBatchAckV2::decode(body).ok()?;
            let d = FileTransferDigest {
                id: v.batch_id as i32,
                file_num: v.entry_id as i32,
                file_size: v.credit_limit,
                transferred_size: v.next_offset,
                ..Default::default()
            };
            let mut fr = FileResponse::new();
            fr.set_digest(d);
            Some(msg_with(move |m| m.set_file_response(fr)))
        }
        RecordType::FileBatchFinishV2 => {
            let v = FileBatchFinishV2::decode(body).ok()?;
            let d = FileTransferDone {
                id: v.batch_id as i32,
                ..Default::default()
            };
            let mut fr = FileResponse::new();
            fr.set_done(d);
            Some(msg_with(move |m| m.set_file_response(fr)))
        }
        RecordType::FileBatchOpenV2 => {
            let v = FileBatchOpenV2::decode(body).ok()?;
            let fa = if v.mode == 1 {
                let s = FileTransferSendRequest {
                    id: v.batch_id as i32,
                    path: v.root_path.clone(),
                    file_num: v.entry_count as i32,
                    ..Default::default()
                };
                let mut fa = FileAction::new();
                fa.set_send(s);
                fa
            } else {
                let r = FileTransferReceiveRequest {
                    id: v.batch_id as i32,
                    path: v.root_path.clone(),
                    file_num: v.entry_count as i32,
                    total_size: v.total_bytes,
                    ..Default::default()
                };
                let mut fa = FileAction::new();
                fa.set_receive(r);
                fa
            };
            Some(msg_with(move |m| m.set_file_action(fa)))
        }

        // ---- 终端 ----
        RecordType::TermOpen => {
            let (_mode, _prog, cols, rows, _cwd) = dec_term_open(body)?;
            let o = OpenTerminal {
                cols: cols as u32,
                rows: rows as u32,
                ..Default::default()
            };
            let mut ta = TerminalAction::new();
            ta.set_open(o);
            Some(msg_with(move |m| m.set_terminal_action(ta)))
        }
        RecordType::TermData => {
            let raw = dec_term_data(body)?;
            let d = TerminalData {
                data: raw.into(),
                ..Default::default()
            };
            let mut ta = TerminalAction::new();
            ta.set_data(d);
            Some(msg_with(move |m| m.set_terminal_action(ta)))
        }
        RecordType::TermResize => {
            let (cols, rows) = dec_term_resize(body)?;
            let r = ResizeTerminal {
                cols: cols as u32,
                rows: rows as u32,
                ..Default::default()
            };
            let mut ta = TerminalAction::new();
            ta.set_resize(r);
            Some(msg_with(move |m| m.set_terminal_action(ta)))
        }
        RecordType::TermSignal => {
            let _sig = dec_term_signal(body)?;
            let c = CloseTerminal::new();
            let mut ta = TerminalAction::new();
            ta.set_close(c);
            Some(msg_with(move |m| m.set_terminal_action(ta)))
        }
        RecordType::TermLeaseGrantAckV2 => {
            let v = durt_wire::TermLeaseGrantAckV2::decode(body).ok()?;
            let o = TerminalOpened {
                terminal_id: v.pty_id as i32,
                success: true,
                ..Default::default()
            };
            let mut tr = TerminalResponse::new();
            tr.set_opened(o);
            Some(msg_with(move |m| m.set_terminal_response(tr)))
        }
        RecordType::TermOutputV2 => {
            let v = TermOutputV2::decode(body).ok()?;
            let d = TerminalData {
                terminal_id: v.pty_id as i32,
                data: v.chunk.clone().into(),
                ..Default::default()
            };
            let mut tr = TerminalResponse::new();
            tr.set_data(d);
            Some(msg_with(move |m| m.set_terminal_response(tr)))
        }
        RecordType::TermExitV2 => {
            let v = TermExitV2::decode(body).ok()?;
            let c = TerminalClosed {
                terminal_id: v.pty_id as i32,
                exit_code: v.exit_code,
                ..Default::default()
            };
            let mut tr = TerminalResponse::new();
            tr.set_closed(c);
            Some(msg_with(move |m| m.set_terminal_response(tr)))
        }
        RecordType::TermCloseV2 => {
            let v = TermCloseV2::decode(body).ok()?;
            let e = TerminalError {
                terminal_id: v.pty_id as i32,
                message: format!("closed reason={}", v.reason),
                ..Default::default()
            };
            let mut tr = TerminalResponse::new();
            tr.set_error(e);
            Some(msg_with(move |m| m.set_terminal_response(tr)))
        }

        // ---- 连接 / 显示 ----
        RecordType::ConnHello => {
            let (_caps, sid, _role, fp) = dec_conn_hello(body)?;
            let lr = LoginRequest {
                session_id: sid,
                hwid: fp.into(),
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_login_request(lr)))
        }
        RecordType::ChannelHello => {
            let (_kind, _inst, sid, _flags) = dec_channel_hello(body)?;
            let lr = LoginRequest {
                session_id: sid,
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_login_request(lr)))
        }
        RecordType::ChannelAck => {
            let (instance, _caps) = dec_channel_ack(body)?;
            let pi = PeerInfo {
                current_display: instance as i32,
                ..Default::default()
            };
            let mut lr = LoginResponse::new();
            lr.set_peer_info(pi);
            Some(msg_with(move |m| m.set_login_response(lr)))
        }
        RecordType::ConnClose => {
            let (reason, detail) = dec_conn_close(body)?;
            log::debug!("durt.bridge: ConnClose reason={reason}");
            // 关闭原因 → `misc.close_reason` (原 LoginResponse.error 口径不再用)。
            let mut ms = Misc::new();
            ms.set_close_reason(detail);
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        RecordType::DisplayDescriptorV2 => {
            let v = DisplayDescriptorV2::decode(body).ok()?;
            let d = DisplayInfo {
                x: v.left,
                y: v.top,
                width: v.right - v.left,
                height: v.bottom - v.top,
                scale: v.scale_x_milli as f64 / 1000.0,
                ..Default::default()
            };
            let pi = PeerInfo {
                displays: vec![d],
                current_display: display_from_id(&v.display_id),
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_peer_info(pi)))
        }
        RecordType::DisplayTopologyEndV2 => {
            let v = DisplayTopologyEndV2::decode(body).ok()?;
            let pi = PeerInfo {
                current_display: v.display_count as i32,
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_peer_info(pi)))
        }
        RecordType::DisplaySelectV2 => {
            let v = DisplaySelectV2::decode(body).ok()?;
            Some(msg_with(move |m| {
                m.set_cursor_id(v.request_id);
            }))
        }
        RecordType::DisplayTopologyChangedV2 => {
            // 拓扑变更通知 → peer_info 空壳 (具体 display 由 DisplayDescriptorV2 补)。
            let _ = DisplayTopologyChangedV2::decode(body).ok()?;
            let pi = PeerInfo::new();
            Some(msg_with(move |m| m.set_peer_info(pi)))
        }
        RecordType::CodecRenegotiate => {
            let (codec, _w, _h) = dec_codec_renegotiate(body)?;
            let vf = match codec {
                CODEC_RENEGOTIATE_RAW_RGB => {
                    let mut vf = VideoFrame::new();
                    vf.set_rgb(Default::default());
                    vf
                }
                CODEC_RENEGOTIATE_RAW_YUV => {
                    let mut vf = VideoFrame::new();
                    vf.set_yuv(Default::default());
                    vf
                }
                0 => {
                    let mut vf = VideoFrame::new();
                    vf.set_h264s(Default::default());
                    vf
                }
                1 => {
                    let mut vf = VideoFrame::new();
                    vf.set_h265s(Default::default());
                    vf
                }
                2 => {
                    let mut vf = VideoFrame::new();
                    vf.set_vp9s(Default::default());
                    vf
                }
                3 => {
                    let mut vf = VideoFrame::new();
                    vf.set_av1s(Default::default());
                    vf
                }
                _ => return None,
            };
            Some(msg_with(move |m| m.set_video_frame(vf)))
        }
        // ---- 控制面: 心跳 / 媒体编排 ----
        RecordType::Ping => {
            let v = Ping::decode(body).ok()?;
            let td = TestDelay {
                time: v.send_unix_ms as i64,
                from_client: true,
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_test_delay(td)))
        }
        RecordType::Pong => {
            let v = Pong::decode(body).ok()?;
            let td = TestDelay {
                time: v.send_unix_ms as i64,
                from_client: false,
                last_delay: v.recv_unix_ms.saturating_sub(v.send_unix_ms) as u32,
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_test_delay(td)))
        }
        RecordType::StartMedia => {
            let v = StartMedia::decode(body).ok()?;
            let mut ms = Misc::new();
            ms.set_auto_adjust_fps(v.max_fps as u32);
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        RecordType::StopMedia => {
            StopMedia::decode(body).ok()?;
            let mut ms = Misc::new();
            ms.set_client_record_status(false);
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        RecordType::VideoFeedback => {
            let v = VideoFeedback::decode(body).ok()?;
            let mut ms = Misc::new();
            match VideoFeedbackKind::from_repr(v.kind) {
                Some(VideoFeedbackKind::ReqIdr) => ms.set_refresh_video(true),
                Some(VideoFeedbackKind::Nack) => ms.set_video_received(false),
                _ => ms.set_video_received(true),
            }
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        RecordType::DisplayTopologyRequestV2 => {
            DisplayTopologyRequestV2::decode(body).ok()?;
            let mut ms = Misc::new();
            ms.set_switch_display(SwitchDisplay::default());
            Some(msg_with(move |m| m.set_misc(ms)))
        }
        // ---- 剪贴板 ClipV2 → clipboard (控制记录以 special_name 标角色, 有损) ----
        RecordType::ClipV2Offer => {
            ClipV2OfferBody::decode(body).ok()?;
            Some(msg_with(move |m| m.set_clipboard(Clipboard::default())))
        }
        RecordType::ClipV2Chunk => {
            let v = ClipV2ChunkBody::decode(body).ok()?;
            let cb = Clipboard {
                content: v.data.into(),
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_clipboard(cb)))
        }
        RecordType::ClipV2Ack | RecordType::ClipV2Cancel | RecordType::ClipV2Request => {
            let role = match ty {
                RecordType::ClipV2Ack => "clipv2.ack",
                RecordType::ClipV2Cancel => "clipv2.cancel",
                _ => "clipv2.request",
            };
            let cb = Clipboard {
                special_name: role.to_string(),
                ..Default::default()
            };
            Some(msg_with(move |m| m.set_clipboard(cb)))
        }
        // ---- Aux RPC (durt.idl §12) ----
        RecordType::RpcRequest => {
            let (_call_id, method, args) = dec_rpc_request(body)?;
            rpc_request_to_message(&method, &args)
        }
        RecordType::RpcResponse => {
            let (_call_id, status, result) = dec_rpc_response(body)?;
            rpc_response_to_message(status, &result)
        }
        // 未接线的记录类型显式忽略 (能力边界)。
        _ => {
            log::debug!("durt.bridge: 未映射 RecordType={record_type}");
            None
        }
    }
}

/// 屏幕 VideoDatagram → `video_frame`(h264s)。codec 由 StartMedia 带外协商,
/// datagram 本身不带, 见文件头。
fn video_datagram_to_message(bytes: &[u8]) -> Option<Message> {
    if bytes.len() < VIDEO_DATAGRAM_HDR_LEN as usize {
        return None;
    }
    let hdr = VideoDatagramHdr::decode(bytes, 0);
    let (magic, _ver) = durt_wire::unpack_magic_ver(hdr.magic_ver);
    if magic != DATAGRAM_MAGIC {
        return None;
    }
    let payload = bytes[VIDEO_DATAGRAM_HDR_LEN as usize..].to_vec();
    let f = EncodedVideoFrame {
        data: payload.into(),
        key: hdr.frame_type == VideoFrameType::IIdr.repr(),
        pts: hdr.frame_seq as i64,
        ..Default::default()
    };
    let frames = EncodedVideoFrames {
        frames: vec![f],
        ..Default::default()
    };
    let mut vf = VideoFrame {
        display: hdr.media_stream_id as i32,
        ..Default::default()
    };
    vf.set_h264s(frames);
    Some(msg_with(move |m| m.set_video_frame(vf)))
}

/// 组装 Message 的小工具 (闭包设 union, 返回值忽略)。
fn msg_with<T>(set: impl FnOnce(&mut Message) -> T) -> Message {
    let mut m = Message::new();
    let _ = set(&mut m);
    m
}

// ============ 单测 ============

#[cfg(test)]
mod tests {
    use super::*;
    use base::message_proto::{
        Auth2FA, CaptureDisplays, CliprdrFiles, CliprdrMonitorReady, CliprdrServerFormatDataRequest,
        CliprdrServerFormatDataResponse, CliprdrTryEmpty, CursorData, FileTransferCancel, Hash,
        MultiClipboards, OSLogin, OptionMessage, PluginRequest, PublicKey, Resolution, SignedId,
        SwitchSidesRequest, SwitchSidesResponse, ToggleVirtualDisplay, TouchEvent, TouchPanEnd,
        TouchPanStart, TouchPanUpdate, TouchScaleUpdate, VoiceCallRequest, VoiceCallResponse,
    };

    /// DurtOut → DurtIn (模拟线缆回环)。
    fn echo(o: DurtOut) -> DurtIn {
        match o {
            DurtOut::Record {
                record_type, body, ..
            } => DurtIn::Record { record_type, body },
            DurtOut::VideoDatagram(v) => DurtIn::VideoDatagram(v),
            DurtOut::AudioDatagram(v) => DurtIn::AudioDatagram(v),
        }
    }

    fn out_records(msg: &Message) -> Vec<(u16, Vec<u8>)> {
        message_to_durt(msg)
            .into_iter()
            .map(|o| match o {
                DurtOut::Record {
                    record_type, body, ..
                } => (record_type, body),
                _ => panic!("expected record"),
            })
            .collect()
    }

    fn roundtrip(msg: &Message) -> Message {
        let outs = message_to_durt(msg);
        assert_eq!(outs.len(), 1, "roundtrip expects exactly one output");
        durt_to_message(&echo(outs.into_iter().next().unwrap())).expect("reverse must map")
    }

    // ---- 鼠标 ----

    #[test]
    fn mouse_move_roundtrip() {
        let me = MouseEvent {
            mask: MOUSE_TYPE_MOVE | (MOUSE_BUTTON_LEFT << 3),
            x: 123,
            y: -45,
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_mouse_event(me);
        let back = roundtrip(&m);
        let got = back.mouse_event();
        assert_eq!(got.mask, MOUSE_TYPE_MOVE | (MOUSE_BUTTON_LEFT << 3));
        assert_eq!(got.x, 123);
        assert_eq!(got.y, -45);
    }

    #[test]
    fn mouse_down_up_roundtrip() {
        for (t, down) in [(MOUSE_TYPE_DOWN, 1u8), (MOUSE_TYPE_UP, 0u8)] {
            let me = MouseEvent {
                mask: t | (MOUSE_BUTTON_RIGHT << 3),
                x: 10,
                y: 20,
                ..Default::default()
            };
            let mut m = Message::new();
            m.set_mouse_event(me);
            let outs = message_to_durt(&m);
            assert_eq!(outs.len(), 1);
            let DurtOut::Record { record_type, body, .. } = &outs[0] else {
                panic!("expected record");
            };
            assert_eq!(*record_type, RecordType::PtrButtonV2 as u16);
            let v = PtrButtonV2::decode(body).unwrap();
            assert_eq!(v.button, 1); // RIGHT
            assert_eq!(v.down, down);
            let back = durt_to_message(&echo(outs[0].clone())).unwrap();
            let got = back.mouse_event();
            assert_eq!(got.mask, t | (MOUSE_BUTTON_RIGHT << 3));
            assert_eq!(got.x, 10);
            assert_eq!(got.y, 20);
        }
    }

    #[test]
    fn mouse_wheel_roundtrip() {
        let me = MouseEvent {
            mask: MOUSE_TYPE_WHEEL,
            x: 3,
            y: -2,
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_mouse_event(me);
        let back = roundtrip(&m);
        let got = back.mouse_event();
        assert_eq!(got.mask, MOUSE_TYPE_WHEEL);
        assert_eq!(got.x, 3);
        assert_eq!(got.y, -2);
    }

    // ---- 键盘 / 文本 ----

    #[test]
    fn key_chr_roundtrip() {
        let ke = KeyEvent {
            down: true,
            modifiers: vec![ControlKey::Control.into()],
            union: Some(key_event::Union::Chr(0x1E)),
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_key_event(ke);
        let back = roundtrip(&m);
        let got = back.key_event();
        assert_eq!(got.down, true);
        assert_eq!(got.union, Some(key_event::Union::Chr(0x1E)));
        assert_eq!(modifiers_to_bits(&got.modifiers), MOD_CTRL);
    }

    #[test]
    fn key_control_key_roundtrip() {
        let ke = KeyEvent {
            down: false,
            union: Some(key_event::Union::ControlKey(ControlKey::Alt.into())),
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_key_event(ke);
        let back = roundtrip(&m);
        let got = back.key_event();
        assert_eq!(
            got.union,
            Some(key_event::Union::ControlKey(ControlKey::Alt.into()))
        );
    }

    #[test]
    fn text_input_unicode_roundtrip() {
        let ke = KeyEvent {
            down: true,
            union: Some(key_event::Union::Unicode('A' as u32)),
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_key_event(ke);
        let outs = message_to_durt(&m);
        assert_eq!(outs.len(), 1);
        let DurtOut::Record { record_type, body, .. } = &outs[0] else {
            panic!("expected record");
        };
        assert_eq!(*record_type, RecordType::TextInputV2 as u16);
        let v = TextInputV2::decode(body).unwrap();
        assert_eq!(v.utf8, b"A".to_vec());
        let back = durt_to_message(&echo(outs[0].clone())).unwrap();
        assert_eq!(
            back.key_event().union,
            Some(key_event::Union::Unicode('A' as u32))
        );
    }

    #[test]
    fn text_input_seq_roundtrip() {
        let ke = KeyEvent {
            down: true,
            union: Some(key_event::Union::Seq("hi".to_string())),
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_key_event(ke);
        let back = roundtrip(&m);
        assert_eq!(
            back.key_event().union,
            Some(key_event::Union::Seq("hi".to_string()))
        );
    }

    // ---- 视频帧 ----

    #[test]
    fn video_frame_roundtrip_with_hdr_bytes() {
        let f = EncodedVideoFrame {
            data: vec![0xAA, 0xBB, 0xCC].into(),
            key: true,
            pts: 7,
            ..Default::default()
        };
        let frames = EncodedVideoFrames {
            frames: vec![f],
            ..Default::default()
        };
        let mut vf = VideoFrame {
            display: 3,
            ..Default::default()
        };
        vf.set_h264s(frames);
        let mut m = Message::new();
        m.set_video_frame(vf);
        let outs = message_to_durt(&m);
        assert_eq!(outs.len(), 1);
        let DurtOut::VideoDatagram(bytes) = &outs[0] else {
            panic!("expected video datagram");
        };
        // hdr 字节断言 (durt.idl §11.1 字段级偏移冻结)。
        assert_eq!(bytes.len(), 13 + 3);
        assert_eq!(bytes[0], pack_magic_ver(DATAGRAM_MAGIC, PROTO_VERSION));
        assert_eq!(bytes[1], VideoFrameType::IIdr.repr());
        assert_eq!(bytes[2], 3); // media_stream_id = display
        assert_eq!(&bytes[3..7], &0u32.to_le_bytes()); // frame_seq
        assert_eq!(&bytes[7..9], &0u16.to_le_bytes()); // slice_idx
        assert_eq!(&bytes[9..11], &1u16.to_le_bytes()); // slice_cnt
        assert_eq!(bytes[11], 0); // fec_group
        assert_eq!(bytes[12], 0); // fec_k
        assert_eq!(&bytes[13..], &[0xAA, 0xBB, 0xCC]);
        // 往返: data / key / display 保真 (codec 带外协商, 回 h264s)。
        let back = durt_to_message(&echo(outs[0].clone())).unwrap();
        let got = back.video_frame();
        assert_eq!(got.display, 3);
        let video_frame::Union::H264s(f) = got.union.as_ref().unwrap() else {
            panic!("expected h264s");
        };
        assert_eq!(f.frames.len(), 1);
        assert_eq!(f.frames[0].data.to_vec(), vec![0xAA, 0xBB, 0xCC]);
        assert!(f.frames[0].key);
    }

    #[test]
    fn video_frame_rgb_is_codec_renegotiate() {
        let mut vf = VideoFrame::new();
        vf.set_rgb(Default::default());
        let mut m = Message::new();
        m.set_video_frame(vf);
        let outs = message_to_durt(&m);
        let DurtOut::Record {
            record_type, body, ..
        } = &outs[0]
        else {
            panic!("expected record");
        };
        assert_eq!(*record_type, RecordType::CodecRenegotiate as u16);
        let (codec, _, _) = dec_codec_renegotiate(body).unwrap();
        assert_eq!(codec, CODEC_RENEGOTIATE_RAW_RGB);
        let back = durt_to_message(&echo(outs[0].clone())).unwrap();
        assert!(matches!(
            back.video_frame().union,
            Some(video_frame::Union::Rgb(_))
        ));
    }

    // ---- 文件 ----

    #[test]
    fn file_readdir_roundtrip() {
        let rd = ReadDir {
            path: "C:/tmp".to_string(),
            include_hidden: true,
            ..Default::default()
        };
        let mut fa = FileAction::new();
        fa.set_read_dir(rd);
        let mut m = Message::new();
        m.set_file_action(fa);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::FileOpen as u16);
        let (mode, path, _, _) = dec_file_open(&outs[0].1).unwrap();
        assert_eq!(mode, FILE_OPEN_MODE_LIST);
        assert_eq!(path, "C:/tmp");
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        let fa = back.file_action();
        let Some(file_action::Union::ReadDir(rd)) = fa.union.as_ref() else {
            panic!("expected read_dir");
        };
        assert_eq!(rd.path, "C:/tmp");
    }

    #[test]
    fn file_list_entry_roundtrip() {
        let entry = FileEntry {
            entry_type: FileType::Dir.into(),
            name: "sub".to_string(),
            size: 42,
            modified_time: 1700000000000,
            ..Default::default()
        };
        let fd = FileDirectory {
            entries: vec![entry],
            ..Default::default()
        };
        let mut fr = FileResponse::new();
        fr.set_dir(fd);
        let mut m = Message::new();
        m.set_file_response(fr);
        let outs = out_records(&m);
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].0, RecordType::FileListEntry as u16);
        let (name, size, mtime, kind, _) = dec_file_list_entry(&outs[0].1).unwrap();
        assert_eq!(name, "sub");
        assert_eq!(size, 42);
        assert_eq!(mtime, 1700000000000);
        assert_eq!(kind, FENTRY_KIND_DIR);
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        let fr = back.file_response();
        let Some(file_response::Union::Dir(fd)) = fr.union.as_ref() else {
            panic!("expected dir");
        };
        assert_eq!(fd.entries[0].name, "sub");
        assert_eq!(fd.entries[0].size, 42);
        assert_eq!(
            fd.entries[0].entry_type.enum_value().unwrap(),
            FileType::Dir
        );
    }

    #[test]
    fn file_chunk_roundtrip() {
        let b = FileTransferBlock {
            id: 5,
            file_num: 2,
            data: vec![1, 2, 3, 4].into(),
            blk_id: 3,
            ..Default::default()
        };
        let mut fr = FileResponse::new();
        fr.set_block(b);
        let mut m = Message::new();
        m.set_file_response(fr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::FileBatchEntryChunkV2 as u16);
        let v = FileBatchEntryChunkV2::decode(&outs[0].1).unwrap();
        assert_eq!(v.batch_id, 5);
        assert_eq!(v.entry_id, 2);
        assert_eq!(v.chunk_len, 4);
        assert_eq!(v.data, vec![1, 2, 3, 4]);
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        let fr = back.file_response();
        let Some(file_response::Union::Block(b)) = fr.union.as_ref() else {
            panic!("expected block");
        };
        assert_eq!(b.id, 5);
        assert_eq!(b.file_num, 2);
        assert_eq!(b.data.to_vec(), vec![1, 2, 3, 4]);
        assert_eq!(b.blk_id, 3);
    }

    #[test]
    fn file_op_request_mkdir_remove_rename() {
        // mkdir
        let c = FileDirCreate {
            id: 9,
            path: "a/b".to_string(),
            ..Default::default()
        };
        let mut fa = FileAction::new();
        fa.set_create(c);
        let mut m = Message::new();
        m.set_file_action(fa);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::FileOpRequest as u16);
        let (op, kind, path, path2, flags) = dec_file_op_request(&outs[0].1).unwrap();
        assert_eq!((op, kind, path.as_str(), path2.as_str()), (9, FOP_KIND_MKDIR, "a/b", ""));
        assert_eq!(flags, FOP_FLAG_RECURSIVE);
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        assert!(matches!(
            back.file_action().union,
            Some(file_action::Union::Create(_))
        ));

        // rename
        let r = FileRename {
            id: 11,
            path: "x".to_string(),
            new_name: "y".to_string(),
            ..Default::default()
        };
        let mut fa = FileAction::new();
        fa.set_rename(r);
        let mut m = Message::new();
        m.set_file_action(fa);
        let outs = out_records(&m);
        let (_, kind, path, path2, _) = dec_file_op_request(&outs[0].1).unwrap();
        assert_eq!(kind, FOP_KIND_RENAME);
        assert_eq!(path, "x");
        assert_eq!(path2, "y");
    }

    // ---- 终端 ----

    #[test]
    fn terminal_data_roundtrip() {
        let d = TerminalData {
            terminal_id: 4,
            data: vec![b'h', b'i'].into(),
            compressed: false,
            ..Default::default()
        };
        let mut tr = TerminalResponse::new();
        tr.set_data(d);
        let mut m = Message::new();
        m.set_terminal_response(tr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::TermOutputV2 as u16);
        let v = TermOutputV2::decode(&outs[0].1).unwrap();
        assert_eq!(v.pty_id, 4);
        assert_eq!(v.chunk, vec![b'h', b'i']);
        assert_eq!(v.event_length, 2);
        assert_ne!(v.flags & durt_wire::TERM_OUTPUT_FLAG_LAST, 0);
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        let tr = back.terminal_response();
        let Some(terminal_response::Union::Data(d)) = tr.union.as_ref() else {
            panic!("expected data");
        };
        assert_eq!(d.terminal_id, 4);
        assert_eq!(d.data.to_vec(), vec![b'h', b'i']);
    }

    #[test]
    fn terminal_action_open_resize_close_roundtrip() {
        // open
        let o = OpenTerminal {
            terminal_id: 1,
            rows: 24,
            cols: 80,
            ..Default::default()
        };
        let mut ta = TerminalAction::new();
        ta.set_open(o);
        let mut m = Message::new();
        m.set_terminal_action(ta);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::TermOpen as u16);
        let (_, _, cols, rows, _) = dec_term_open(&outs[0].1).unwrap();
        assert_eq!((cols, rows), (80, 24));
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        assert!(matches!(
            back.terminal_action().union,
            Some(terminal_action::Union::Open(_))
        ));

        // resize
        let r = ResizeTerminal {
            terminal_id: 1,
            rows: 30,
            cols: 100,
            ..Default::default()
        };
        let mut ta = TerminalAction::new();
        ta.set_resize(r);
        let mut m = Message::new();
        m.set_terminal_action(ta);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::TermResize as u16);
        assert_eq!(dec_term_resize(&outs[0].1).unwrap(), (100, 30));

        // close
        let c = CloseTerminal {
            terminal_id: 1,
            ..Default::default()
        };
        let mut ta = TerminalAction::new();
        ta.set_close(c);
        let mut m = Message::new();
        m.set_terminal_action(ta);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::TermSignal as u16);
        assert_eq!(dec_term_signal(&outs[0].1).unwrap(), TERM_SIG_SIGTERM);
    }

    #[test]
    fn terminal_closed_and_error_roundtrip() {
        let c = TerminalClosed {
            terminal_id: 2,
            exit_code: 3,
            ..Default::default()
        };
        let mut tr = TerminalResponse::new();
        tr.set_closed(c);
        let mut m = Message::new();
        m.set_terminal_response(tr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::TermExitV2 as u16);
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        let Some(terminal_response::Union::Closed(c)) = back.terminal_response().union.as_ref()
        else {
            panic!("expected closed");
        };
        assert_eq!(c.terminal_id, 2);
        assert_eq!(c.exit_code, 3);
    }

    // ---- 登录 / peer_info / 光标 ----

    #[test]
    fn login_request_emits_connhello_and_channelhello() {
        let lr = LoginRequest {
            session_id: 42,
            hwid: vec![9, 9].into(),
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_login_request(lr);
        let outs = out_records(&m);
        assert!(outs.len() >= 2);
        assert_eq!(outs[0].0, RecordType::ConnHello as u16);
        let (caps, sid, role, fp) = dec_conn_hello(&outs[0].1).unwrap();
        assert_eq!(sid, 42);
        assert_eq!(role, 1);
        assert_eq!(fp, vec![9, 9]);
        assert_ne!(caps, 0);
        assert_eq!(outs[1].0, RecordType::ChannelHello as u16);
    }

    #[test]
    fn peer_info_displays_to_topology() {
        let d = DisplayInfo {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.5,
            ..Default::default()
        };
        let pi = PeerInfo {
            displays: vec![d],
            current_display: 0,
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_peer_info(pi);
        let outs = out_records(&m);
        assert_eq!(outs.len(), 2);
        assert_eq!(outs[0].0, RecordType::DisplayDescriptorV2 as u16);
        let v = DisplayDescriptorV2::decode(&outs[0].1).unwrap();
        assert_eq!((v.left, v.top, v.right, v.bottom), (0, 0, 1920, 1080));
        assert_eq!(v.scale_x_milli, 1500);
        assert_ne!(v.flags & durt_wire::DISPLAY_DESCRIPTOR_FLAG_PRIMARY, 0);
        assert_eq!(outs[1].0, RecordType::DisplayTopologyEndV2 as u16);
        let e = DisplayTopologyEndV2::decode(&outs[1].1).unwrap();
        assert_eq!(e.display_count, 1);
    }

    #[test]
    fn cursor_id_maps_to_display_select() {
        let mut m = Message::new();
        m.set_cursor_id(77);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::DisplaySelectV2 as u16);
        let v = DisplaySelectV2::decode(&outs[0].1).unwrap();
        assert_eq!(v.request_id, 77);
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        assert_eq!(back.cursor_id(), 77);
    }

    #[test]
    fn cursor_data_is_ignored() {
        let cd = CursorData {
            id: 1,
            width: 4,
            height: 4,
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_cursor_data(cd);
        assert!(message_to_durt(&m).is_empty());
    }

    #[test]
    fn cursor_position_maps_to_ptr_move() {
        let cp = CursorPosition {
            x: 8,
            y: 9,
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_cursor_position(cp);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::PtrMoveV2 as u16);
        let v = PtrMoveV2::decode(&outs[0].1).unwrap();
        assert_eq!((v.physical_x, v.physical_y), (8, 9));
    }

    #[test]
    fn file_transfer_send_is_batch_open_get() {
        let s = FileTransferSendRequest {
            id: 6,
            path: "D:/a".to_string(),
            file_num: 2,
            ..Default::default()
        };
        let mut fa = FileAction::new();
        fa.set_send(s);
        let mut m = Message::new();
        m.set_file_action(fa);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::FileBatchOpenV2 as u16);
        let v = FileBatchOpenV2::decode(&outs[0].1).unwrap();
        assert_eq!(v.mode, 1);
        assert_eq!(v.batch_id, 6);
        assert_eq!(v.root_path, "D:/a");
        assert_eq!(v.entry_count, 2);
    }

    #[test]
    fn file_transfer_done_and_error() {
        let d = FileTransferDone {
            id: 3,
            file_num: 1,
            ..Default::default()
        };
        let mut fr = FileResponse::new();
        fr.set_done(d);
        let mut m = Message::new();
        m.set_file_response(fr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::FileBatchFinishV2 as u16);
        let v = FileBatchFinishV2::decode(&outs[0].1).unwrap();
        assert_eq!(v.action, 2); // Result
        assert_eq!(v.batch_id, 3);

        let e = FileTransferError {
            id: 4,
            error: "io".to_string(),
            ..Default::default()
        };
        let mut fr = FileResponse::new();
        fr.set_error(e);
        let mut m = Message::new();
        m.set_file_response(fr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::FileOpResult as u16);
        let (op, status, msg_text) = dec_file_op_result(&outs[0].1).unwrap();
        assert_eq!((op, status, msg_text.as_str()), (4, FOP_STATUS_IO_ERROR, "io"));
    }

    #[test]
    fn record_header_wrapper_is_varint() {
        // 引擎侧组帧: encode_record_header(type, len) + body。
        let body = enc_term_signal(1);
        let mut frame = Vec::new();
        encode_record_header(
            RecordType::TermSignal as u64,
            body.len() as u64,
            &mut frame,
        );
        frame.extend_from_slice(&body);
        let (hdr, n) = decode_record_header(&frame, 0);
        assert_eq!(hdr.r#type, RecordType::TermSignal as u64);
        assert_eq!(hdr.length, body.len() as u64);
        assert_eq!(&frame[n..], &body[..]);
    }

    // ---- 映射表补全: 逐条覆盖 ----

    #[test]
    fn video_frame_all_encoded_variants_are_datagrams() {
        // h265s / vp8s / vp9s / av1s 都走 VideoDatagram, 只是 codec 带外。
        let f = EncodedVideoFrame {
            data: vec![1].into(),
            key: false,
            pts: 0,
            ..Default::default()
        };
        let frames = EncodedVideoFrames {
            frames: vec![f],
            ..Default::default()
        };
        for set in [
            VideoFrame::set_h265s as fn(&mut VideoFrame, EncodedVideoFrames),
            VideoFrame::set_vp8s,
            VideoFrame::set_vp9s,
            VideoFrame::set_av1s,
        ] {
            let mut vf = VideoFrame::new();
            set(&mut vf, frames.clone());
            let mut m = Message::new();
            m.set_video_frame(vf);
            let outs = message_to_durt(&m);
            assert_eq!(outs.len(), 1);
            let DurtOut::VideoDatagram(bytes) = &outs[0] else {
                panic!("expected video datagram");
            };
            assert_eq!(bytes[1], VideoFrameType::P.repr()); // 非关键帧
            assert_eq!(bytes.len(), 13 + 1);
        }
    }

    #[test]
    fn video_frame_yuv_is_codec_renegotiate() {
        let mut vf = VideoFrame::new();
        vf.set_yuv(Default::default());
        let mut m = Message::new();
        m.set_video_frame(vf);
        let outs = message_to_durt(&m);
        let DurtOut::Record { body, .. } = &outs[0] else {
            panic!("expected record");
        };
        let (codec, _, _) = dec_codec_renegotiate(body).unwrap();
        assert_eq!(codec, CODEC_RENEGOTIATE_RAW_YUV);
    }

    #[test]
    fn file_remove_dir_and_file_map_to_delete() {
        // recursive=1 → Delete+recursive → 反向 RemoveDir。
        let rd = FileRemoveDir {
            id: 21,
            path: "d".to_string(),
            recursive: true,
            ..Default::default()
        };
        let mut fa = FileAction::new();
        fa.set_remove_dir(rd);
        let mut m = Message::new();
        m.set_file_action(fa);
        let outs = out_records(&m);
        let (op, kind, path, _, flags) = dec_file_op_request(&outs[0].1).unwrap();
        assert_eq!((op, kind, path.as_str(), flags), (21, FOP_KIND_DELETE, "d", FOP_FLAG_RECURSIVE));
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        assert!(matches!(
            back.file_action().union,
            Some(file_action::Union::RemoveDir(_))
        ));

        // recursive=0 → Delete → 反向 RemoveFile。
        let rf = FileRemoveFile {
            id: 22,
            path: "f".to_string(),
            ..Default::default()
        };
        let mut fa = FileAction::new();
        fa.set_remove_file(rf);
        let mut m = Message::new();
        m.set_file_action(fa);
        let outs = out_records(&m);
        let (_, kind, _, _, flags) = dec_file_op_request(&outs[0].1).unwrap();
        assert_eq!((kind, flags), (FOP_KIND_DELETE, 0));
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        assert!(matches!(
            back.file_action().union,
            Some(file_action::Union::RemoveFile(_))
        ));
    }

    #[test]
    fn file_receive_is_batch_open_put_with_entries() {
        let r = FileTransferReceiveRequest {
            id: 31,
            path: "E:/in".to_string(),
            files: vec![FileEntry {
                name: "a.bin".to_string(),
                ..Default::default()
            }],
            file_num: 1,
            total_size: 99,
            ..Default::default()
        };
        let mut fa = FileAction::new();
        fa.set_receive(r);
        let mut m = Message::new();
        m.set_file_action(fa);
        let outs = out_records(&m);
        assert_eq!(outs.len(), 2); // FileBatchOpenV2 + FileBatchEntryOpenV2
        assert_eq!(outs[0].0, RecordType::FileBatchOpenV2 as u16);
        let v = FileBatchOpenV2::decode(&outs[0].1).unwrap();
        assert_eq!((v.mode, v.batch_id, v.total_bytes), (FILE_OPEN_MODE_PUT, 31, 99));
        assert_eq!(outs[1].0, RecordType::FileBatchEntryOpenV2 as u16);
        let e = FileBatchEntryOpenV2::decode(&outs[1].1).unwrap();
        assert_eq!(e.rel_path, "a.bin");
        assert_eq!(e.phase, 0);
    }

    #[test]
    fn file_cancel_is_batch_abort() {
        let c = FileTransferCancel {
            id: 41,
            ..Default::default()
        };
        let mut fa = FileAction::new();
        fa.set_cancel(c);
        let mut m = Message::new();
        m.set_file_action(fa);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::FileBatchFinishV2 as u16);
        let v = FileBatchFinishV2::decode(&outs[0].1).unwrap();
        assert_eq!((v.action, v.batch_id), (3, 41)); // Abort
    }

    #[test]
    fn file_digest_is_batch_ack() {
        let d = FileTransferDigest {
            id: 51,
            file_num: 2,
            file_size: 1000,
            transferred_size: 256,
            ..Default::default()
        };
        let mut fr = FileResponse::new();
        fr.set_digest(d);
        let mut m = Message::new();
        m.set_file_response(fr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::FileBatchAckV2 as u16);
        let v = FileBatchAckV2::decode(&outs[0].1).unwrap();
        assert_eq!((v.batch_id, v.entry_id), (51, 2));
        assert_eq!((v.next_offset, v.credit_limit), (256, 1000));
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        let Some(file_response::Union::Digest(g)) = back.file_response().union.as_ref() else {
            panic!("expected digest");
        };
        assert_eq!(g.file_size, 1000);
        assert_eq!(g.transferred_size, 256);
    }

    #[test]
    fn terminal_action_data_is_termdata() {
        let d = TerminalData {
            terminal_id: 7,
            data: vec![b'x'].into(),
            ..Default::default()
        };
        let mut ta = TerminalAction::new();
        ta.set_data(d);
        let mut m = Message::new();
        m.set_terminal_action(ta);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::TermData as u16);
        assert_eq!(dec_term_data(&outs[0].1).unwrap(), vec![b'x']);
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        let Some(terminal_action::Union::Data(d)) = back.terminal_action().union.as_ref() else {
            panic!("expected data");
        };
        assert_eq!(d.data.to_vec(), vec![b'x']);
    }

    #[test]
    fn terminal_opened_and_error_roundtrip() {
        // opened → TermLeaseGrantAckV2
        let o = TerminalOpened {
            terminal_id: 8,
            success: true,
            ..Default::default()
        };
        let mut tr = TerminalResponse::new();
        tr.set_opened(o);
        let mut m = Message::new();
        m.set_terminal_response(tr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::TermLeaseGrantAckV2 as u16);
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        let Some(terminal_response::Union::Opened(o)) = back.terminal_response().union.as_ref()
        else {
            panic!("expected opened");
        };
        assert_eq!(o.terminal_id, 8);
        assert!(o.success);

        // error → TermCloseV2 (message 有损, 见文件头)
        let e = TerminalError {
            terminal_id: 9,
            message: "boom".to_string(),
            ..Default::default()
        };
        let mut tr = TerminalResponse::new();
        tr.set_error(e);
        let mut m = Message::new();
        m.set_terminal_response(tr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::TermCloseV2 as u16);
        let back = durt_to_message(&echo(
            message_to_durt(&m).into_iter().next().unwrap(),
        ))
        .unwrap();
        assert!(matches!(
            back.terminal_response().union,
            Some(terminal_response::Union::Error(_))
        ));
    }

    #[test]
    fn login_response_error_is_connclose_and_ok_is_connhello_plus_ack() {
        let mut lr = LoginResponse::new();
        lr.set_error("denied".to_string());
        let mut m = Message::new();
        m.set_login_response(lr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::ConnClose as u16);
        let (reason, detail) = dec_conn_close(&outs[0].1).unwrap();
        assert_eq!((reason, detail.as_str()), (5, "denied"));

        // 成功: ConnHello + ChannelAck + 显示拓扑。
        let pi = PeerInfo {
            displays: vec![DisplayInfo {
                width: 10,
                height: 10,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut lr = LoginResponse::new();
        lr.set_peer_info(pi);
        let mut m = Message::new();
        m.set_login_response(lr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::ConnHello as u16);
        assert_eq!(outs[1].0, RecordType::ChannelAck as u16);
        assert_eq!(outs[2].0, RecordType::DisplayDescriptorV2 as u16);
    }

    #[test]
    fn unknown_record_type_is_none() {
        // 本桥不处理的记录 → None, 交上层。
        assert!(durt_to_message(&DurtIn::Record {
            record_type: 0x78, // AuditEvent
            body: vec![],
        })
        .is_none());
    }

    // ---- 控制面 / Misc / 剪贴板 / 杂项 (Message 变体补齐) ----

    #[test]
    fn test_delay_roundtrip_both_directions() {
        let td = TestDelay {
            time: 1234,
            from_client: true,
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_test_delay(td);
        assert_eq!(out_records(&m)[0].0, RecordType::Ping as u16);
        let back = roundtrip(&m);
        assert!(back.test_delay().from_client);
        assert_eq!(back.test_delay().time, 1234);

        let td = TestDelay {
            time: 50,
            from_client: false,
            last_delay: 7,
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_test_delay(td);
        assert_eq!(out_records(&m)[0].0, RecordType::Pong as u16);
        let back = roundtrip(&m);
        assert!(!back.test_delay().from_client);
        assert_eq!(back.test_delay().time, 50);
        assert_eq!(back.test_delay().last_delay, 7);
    }

    #[test]
    fn misc_close_reason_maps_to_connclose() {
        let mut ms = Misc::new();
        ms.set_close_reason("bye".to_string());
        let mut m = Message::new();
        m.set_misc(ms);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::ConnClose as u16);
        let (reason, detail) = dec_conn_close(&outs[0].1).unwrap();
        assert_eq!((reason, detail.as_str()), (0, "bye"));
        let back = roundtrip(&m);
        assert_eq!(back.misc().close_reason(), "bye");
    }

    #[test]
    fn misc_media_bucket_maps_to_feedback_and_media() {
        // refresh_video → VideoFeedback(REQ_IDR), 反向回 refresh_video=true。
        let mut ms = Misc::new();
        ms.set_refresh_video(true);
        let mut m = Message::new();
        m.set_misc(ms);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::VideoFeedback as u16);
        let vf = VideoFeedback::decode(&outs[0].1).unwrap();
        assert_eq!(vf.kind, VideoFeedbackKind::ReqIdr.repr());
        assert!(roundtrip(&m).misc().refresh_video());

        // video_received 两向 round-trip。
        for ok in [true, false] {
            let mut ms = Misc::new();
            ms.set_video_received(ok);
            let mut m = Message::new();
            m.set_misc(ms);
            assert_eq!(roundtrip(&m).misc().video_received(), ok);
        }

        // auto_adjust_fps → StartMedia.max_fps, round-trip。
        let mut ms = Misc::new();
        ms.set_auto_adjust_fps(60);
        let mut m = Message::new();
        m.set_misc(ms);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::StartMedia as u16);
        assert_eq!(StartMedia::decode(&outs[0].1).unwrap().max_fps, 60);
        assert_eq!(roundtrip(&m).misc().auto_adjust_fps(), 60);

        // client_record_status=false → StopMedia round-trip; true → StartMedia (字节断言)。
        let mut ms = Misc::new();
        ms.set_client_record_status(false);
        let mut m = Message::new();
        m.set_misc(ms);
        assert_eq!(out_records(&m)[0].0, RecordType::StopMedia as u16);
        assert!(!roundtrip(&m).misc().client_record_status());

        let mut ms = Misc::new();
        ms.set_client_record_status(true);
        let mut m = Message::new();
        m.set_misc(ms);
        assert_eq!(out_records(&m)[0].0, RecordType::StartMedia as u16);

        // full_speed_fps → StartMedia (字节断言)。
        let mut ms = Misc::new();
        ms.set_full_speed_fps(120);
        let mut m = Message::new();
        m.set_misc(ms);
        let outs = out_records(&m);
        assert_eq!(StartMedia::decode(&outs[0].1).unwrap().max_fps, 120);

        // option → StartMedia (仅 quality/fps 标量过桥, 有损)。
        let om = OptionMessage {
            custom_image_quality: 3,
            custom_fps: 30,
            ..Default::default()
        };
        let mut ms = Misc::new();
        ms.set_option(om);
        let mut m = Message::new();
        m.set_misc(ms);
        let outs = out_records(&m);
        let sm = StartMedia::decode(&outs[0].1).unwrap();
        assert_eq!((sm.init_target_bps, sm.max_fps), (3, 30));
    }

    #[test]
    fn misc_display_bucket_maps_to_display_v2() {
        let mut ms = Misc::new();
        ms.set_switch_display(SwitchDisplay {
            display: 2,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_misc(ms);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::DisplaySelectV2 as u16);
        let v = DisplaySelectV2::decode(&outs[0].1).unwrap();
        assert_eq!(display_from_id(&v.display_id), 2);

        // change_resolution / capture_displays → DisplayTopologyRequestV2 (w/h 有损)。
        let mut ms = Misc::new();
        ms.set_change_resolution(Resolution {
            width: 800,
            height: 600,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_misc(ms);
        assert_eq!(out_records(&m)[0].0, RecordType::DisplayTopologyRequestV2 as u16);

        let mut ms = Misc::new();
        ms.set_capture_displays(CaptureDisplays::default());
        let mut m = Message::new();
        m.set_misc(ms);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::DisplayTopologyRequestV2 as u16);
        let v = DisplayTopologyRequestV2::decode(&outs[0].1).unwrap();
        assert_eq!(v.version, DISPLAY_TOPOLOGY_V2_VERSION);
        // 反向 → misc.switch_display (display 无载体)。
        let back = durt_to_message(&echo(DurtOut::Record {
            channel: 0,
            record_type: outs[0].0,
            body: outs[0].1.clone(),
        }))
        .unwrap();
        assert!(back.misc().has_switch_display());

        // toggle_virtual_display / follow_current_display → DisplaySelectV2。
        let mut ms = Misc::new();
        ms.set_toggle_virtual_display(ToggleVirtualDisplay {
            display: 1,
            on: true,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_misc(ms);
        assert_eq!(out_records(&m)[0].0, RecordType::DisplaySelectV2 as u16);

        let mut ms = Misc::new();
        ms.set_follow_current_display(3);
        let mut m = Message::new();
        m.set_misc(ms);
        let outs = out_records(&m);
        let v = DisplaySelectV2::decode(&outs[0].1).unwrap();
        assert_eq!(display_from_id(&v.display_id), 3);
    }

    #[test]
    fn misc_rpc_carried_roundtrips() {
        // chat_message → RpcRequest, round-trip。
        let mut ms = Misc::new();
        ms.set_chat_message(ChatMessage {
            text: "你好".to_string(),
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_misc(ms);
        assert_eq!(out_records(&m)[0].0, RecordType::RpcRequest as u16);
        assert_eq!(roundtrip(&m).misc().chat_message().text, "你好");

        for on in [true, false] {
            let mut ms = Misc::new();
            ms.set_uac(on);
            let mut m = Message::new();
            m.set_misc(ms);
            assert_eq!(out_records(&m)[0].0, RecordType::RpcRequest as u16);
            assert_eq!(roundtrip(&m).misc().uac(), on);

            let mut ms = Misc::new();
            ms.set_stop_service(on);
            let mut m = Message::new();
            m.set_misc(ms);
            assert_eq!(roundtrip(&m).misc().stop_service(), on);

            let mut ms = Misc::new();
            ms.set_portable_service_running(on);
            let mut m = Message::new();
            m.set_misc(ms);
            assert_eq!(roundtrip(&m).misc().portable_service_running(), on);

            let mut ms = Misc::new();
            ms.set_restart_remote_device(on);
            let mut m = Message::new();
            m.set_misc(ms);
            assert_eq!(roundtrip(&m).misc().restart_remote_device(), on);

            let mut ms = Misc::new();
            ms.set_foreground_window_elevated(on);
            let mut m = Message::new();
            m.set_misc(ms);
            assert_eq!(roundtrip(&m).misc().foreground_window_elevated(), on);
        }

        let mut ms = Misc::new();
        ms.set_selected_sid(9);
        let mut m = Message::new();
        m.set_misc(ms);
        assert_eq!(roundtrip(&m).misc().selected_sid(), 9);

        let mut ms = Misc::new();
        ms.set_message_query(MessageQuery {
            switch_display: 4,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_misc(ms);
        assert_eq!(roundtrip(&m).misc().message_query().switch_display, 4);

        let mut ms = Misc::new();
        ms.set_audio_format(AudioFormat {
            sample_rate: 48000,
            channels: 2,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_misc(ms);
        let back = roundtrip(&m);
        assert_eq!(
            (
                back.misc().audio_format().sample_rate,
                back.misc().audio_format().channels
            ),
            (48000, 2)
        );

        let mut ms = Misc::new();
        ms.set_elevation_response("uac ok".to_string());
        let mut m = Message::new();
        m.set_misc(ms);
        assert_eq!(roundtrip(&m).misc().elevation_response(), "uac ok");
    }

    #[test]
    fn misc_struct_rpc_roundtrips() {
        // permission_info。
        let mut ms = Misc::new();
        ms.set_permission_info(PermissionInfo {
            permission: permission_info::Permission::Clipboard.into(),
            enabled: true,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_misc(ms);
        let back = roundtrip(&m);
        let pi = &back.misc().permission_info();
        assert!(pi.enabled);
        assert_eq!(
            pi.permission.value(),
            permission_info::Permission::Clipboard as i32
        );

        // back_notification (privacy_mode_state + details/impl_key)。
        let mut bn = BackNotification {
            details: "d".to_string(),
            impl_key: "k".to_string(),
            ..Default::default()
        };
        bn.set_privacy_mode_state(back_notification::PrivacyModeState::PrvOnSucceeded);
        let mut ms = Misc::new();
        ms.set_back_notification(bn);
        let mut m = Message::new();
        m.set_misc(ms);
        let back = roundtrip(&m);
        let bn = &back.misc().back_notification();
        assert_eq!(bn.details, "d");
        assert_eq!(bn.impl_key, "k");
        assert_eq!(
            bn.privacy_mode_state(),
            back_notification::PrivacyModeState::PrvOnSucceeded
        );

        // elevation_request: logon + direct。
        let mut er = ElevationRequest::new();
        er.set_logon(ElevationRequestWithLogon {
            username: "u".to_string(),
            password: "p".to_string(),
            ..Default::default()
        });
        let mut ms = Misc::new();
        ms.set_elevation_request(er);
        let mut m = Message::new();
        m.set_misc(ms);
        let back = roundtrip(&m);
        assert!(back.misc().elevation_request().has_logon());
        assert_eq!(back.misc().elevation_request().logon().username, "u");

        let mut er = ElevationRequest::new();
        er.set_direct(true);
        let mut ms = Misc::new();
        ms.set_elevation_request(er);
        let mut m = Message::new();
        m.set_misc(ms);
        assert!(roundtrip(&m).misc().elevation_request().direct());

        // toggle_privacy_mode + supported_encoding。
        let mut ms = Misc::new();
        ms.set_toggle_privacy_mode(TogglePrivacyMode {
            impl_key: "imp".to_string(),
            on: true,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_misc(ms);
        let back = roundtrip(&m);
        assert_eq!(back.misc().toggle_privacy_mode().impl_key, "imp");
        assert!(back.misc().toggle_privacy_mode().on);

        let mut se = SupportedEncoding::default();
        se.h264 = true;
        se.vp8 = true;
        let mut ms = Misc::new();
        ms.set_supported_encoding(se);
        let mut m = Message::new();
        m.set_misc(ms);
        let back = roundtrip(&m);
        let se = &back.misc().supported_encoding();
        assert!(se.h264 && se.vp8 && !se.h265 && !se.av1);
    }

    #[test]
    fn clipboard_text_roundtrips_via_clipv2() {
        let payload: &[u8] = "hello 桥".as_bytes();
        let mut m = Message::new();
        m.set_clipboard(Clipboard {
            content: payload.to_vec().into(),
            ..Default::default()
        });
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::ClipV2Offer as u16);
        assert_eq!(outs[1].0, RecordType::ClipV2Chunk as u16);
        let offer = ClipV2OfferBody::decode(&outs[0].1).unwrap();
        assert_eq!(offer.size, payload.len() as u32);
        assert_eq!(offer.fmt, ClipboardV2Format::Utf8.repr());
        // Offer 是公告 (内容在 Chunk), Chunk round-trip 内容。
        let announce = durt_to_message(&echo(DurtOut::Record {
            channel: 0,
            record_type: outs[0].0,
            body: outs[0].1.clone(),
        }))
        .unwrap();
        assert_eq!(announce.clipboard().content.len(), 0);
        let back = durt_to_message(&echo(DurtOut::Record {
            channel: 0,
            record_type: outs[1].0,
            body: outs[1].1.clone(),
        }))
        .unwrap();
        assert_eq!(back.clipboard().content.to_vec(), payload);
    }

    #[test]
    fn clipboard_empty_and_large_split() {
        // 空内容只出 Offer。
        let mut m = Message::new();
        m.set_clipboard(Clipboard::default());
        let outs = out_records(&m);
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].0, RecordType::ClipV2Offer as u16);

        // 大内容按 61440 切块: 1 Offer + 3 Chunk。
        let big = vec![7u8; CLIPBOARD_V2_CHUNK_MAX as usize * 2 + 10];
        let mut m = Message::new();
        m.set_clipboard(Clipboard {
            content: big.into(),
            ..Default::default()
        });
        let outs = out_records(&m);
        assert_eq!(outs.len(), 4);
        let c1 = ClipV2ChunkBody::decode(&outs[2].1).unwrap();
        assert_eq!(c1.offset, CLIPBOARD_V2_CHUNK_MAX);
        let c2 = ClipV2ChunkBody::decode(&outs[3].1).unwrap();
        assert_eq!(c2.data.len(), 10);

        // multi_clipboards → 每个一条 Offer+Chunk 流, transfer_id 递增。
        let mc = MultiClipboards {
            clipboards: vec![
                Clipboard {
                    content: b"a".to_vec().into(),
                    ..Default::default()
                },
                Clipboard {
                    content: b"bb".to_vec().into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_multi_clipboards(mc);
        let outs = out_records(&m);
        assert_eq!(outs.len(), 4);
        let o1 = ClipV2OfferBody::decode(&outs[0].1).unwrap();
        let o2 = ClipV2OfferBody::decode(&outs[2].1).unwrap();
        assert_eq!((o1.transfer_id, o2.transfer_id), (1, 2));
    }

    #[test]
    fn cliprdr_maps_to_clipv2_family() {
        // FormatDataResponse → Offer+Chunk。
        let mut cr = Cliprdr::new();
        cr.set_format_data_response(CliprdrServerFormatDataResponse {
            msg_flags: 0,
            format_data: b"win".to_vec().into(),
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_cliprdr(cr);
        let outs = out_records(&m);
        assert_eq!(outs[0].0, RecordType::ClipV2Offer as u16);
        assert_eq!(outs[1].0, RecordType::ClipV2Chunk as u16);

        // Ready → ClipV2Ack; FormatDataRequest → ClipV2Request; TryEmpty → ClipV2Cancel。
        let mut cr = Cliprdr::new();
        cr.set_ready(CliprdrMonitorReady::default());
        let mut m = Message::new();
        m.set_cliprdr(cr);
        assert_eq!(out_records(&m)[0].0, RecordType::ClipV2Ack as u16);

        let mut cr = Cliprdr::new();
        cr.set_format_data_request(CliprdrServerFormatDataRequest {
            requested_format_id: 1,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_cliprdr(cr);
        assert_eq!(out_records(&m)[0].0, RecordType::ClipV2Request as u16);

        let mut cr = Cliprdr::new();
        cr.set_try_empty(CliprdrTryEmpty::default());
        let mut m = Message::new();
        m.set_cliprdr(cr);
        assert_eq!(out_records(&m)[0].0, RecordType::ClipV2Cancel as u16);

        // Files 清单 ClipV2 无 file 形态, 显式不承载。
        let mut cr = Cliprdr::new();
        cr.set_files(CliprdrFiles::default());
        let mut m = Message::new();
        m.set_cliprdr(cr);
        assert!(message_to_durt(&m).is_empty());
    }

    #[test]
    fn pointer_device_event_maps_to_ptr_v2() {
        let pde_with = |te: TouchEvent| {
            let mut pde = PointerDeviceEvent::new();
            pde.set_touch_event(te);
            let mut m = Message::new();
            m.set_pointer_device_event(pde);
            m
        };

        let mut te = TouchEvent::new();
        te.set_pan_start(TouchPanStart {
            x: 5,
            y: 6,
            ..Default::default()
        });
        let outs = out_records(&pde_with(te));
        assert_eq!(outs[0].0, RecordType::PtrButtonV2 as u16);
        let v = PtrButtonV2::decode(&outs[0].1).unwrap();
        assert_eq!((v.down, v.physical_x, v.physical_y), (1, 5, 6));

        let mut te = TouchEvent::new();
        te.set_pan_update(TouchPanUpdate {
            x: 7,
            y: 8,
            ..Default::default()
        });
        let outs = out_records(&pde_with(te));
        assert_eq!(outs[0].0, RecordType::PtrMoveV2 as u16);
        let v = PtrMoveV2::decode(&outs[0].1).unwrap();
        assert_eq!((v.physical_x, v.physical_y), (7, 8));

        let mut te = TouchEvent::new();
        te.set_pan_end(TouchPanEnd {
            x: 9,
            y: 10,
            ..Default::default()
        });
        let outs = out_records(&pde_with(te));
        assert_eq!(outs[0].0, RecordType::PtrButtonV2 as u16);
        assert_eq!(PtrButtonV2::decode(&outs[0].1).unwrap().down, 0);

        let mut te = TouchEvent::new();
        te.set_scale_update(TouchScaleUpdate {
            scale: -120,
            ..Default::default()
        });
        let outs = out_records(&pde_with(te));
        assert_eq!(outs[0].0, RecordType::PtrScrollV2 as u16);
        assert_eq!(PtrScrollV2::decode(&outs[0].1).unwrap().dy, -120);
    }

    #[test]
    fn message_box_and_screenshot_roundtrip() {
        let mb = MessageBox {
            msgtype: "n".to_string(),
            title: "t".to_string(),
            text: "x".to_string(),
            link: "l".to_string(),
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_message_box(mb);
        assert_eq!(out_records(&m)[0].0, RecordType::RpcRequest as u16);
        let back = roundtrip(&m);
        assert_eq!(back.message_box().title, "t");
        assert_eq!(back.message_box().text, "x");
        assert_eq!(back.message_box().link, "l");

        let sr = ScreenshotRequest {
            display: 3,
            sid: "s1".to_string(),
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_screenshot_request(sr);
        let back = roundtrip(&m);
        assert_eq!(back.screenshot_request().display, 3);
        assert_eq!(back.screenshot_request().sid, "s1");

        let sr = ScreenshotResponse {
            sid: "s1".to_string(),
            msg: "".to_string(),
            data: b"png".to_vec().into(),
            ..Default::default()
        };
        let mut m = Message::new();
        m.set_screenshot_response(sr);
        assert_eq!(out_records(&m)[0].0, RecordType::RpcResponse as u16);
        let back = roundtrip(&m);
        assert_eq!(back.screenshot_response().sid, "s1");
        assert_eq!(back.screenshot_response().data.to_vec(), b"png");
    }

    #[test]
    fn port_forward_channel_roundtrip() {
        let mut pf = PortForwardChannel::new();
        pf.set_open(PortForwardOpen {
            channel_id: 7,
            host: "h".to_string(),
            port: 22,
            window: 100,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_port_forward_channel(pf);
        assert_eq!(out_records(&m)[0].0, RecordType::RpcRequest as u16);
        let back = roundtrip(&m);
        assert_eq!(back.port_forward_channel().open().host, "h");
        assert_eq!(back.port_forward_channel().open().port, 22);

        let mut pf = PortForwardChannel::new();
        pf.set_data(PortForwardData {
            channel_id: 7,
            data: b"z".to_vec().into(),
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_port_forward_channel(pf);
        let back = roundtrip(&m);
        assert_eq!(back.port_forward_channel().data().data.to_vec(), b"z");

        let mut pf = PortForwardChannel::new();
        pf.set_window_update(PortForwardWindowUpdate {
            channel_id: 7,
            add: 64,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_port_forward_channel(pf);
        assert_eq!(roundtrip(&m).port_forward_channel().window_update().add, 64);

        let mut pf = PortForwardChannel::new();
        pf.set_close(PortForwardClose {
            channel_id: 7,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_port_forward_channel(pf);
        assert_eq!(roundtrip(&m).port_forward_channel().close().channel_id, 7);

        let mut pf = PortForwardChannel::new();
        pf.set_opened(PortForwardOpened {
            channel_id: 7,
            success: true,
            message: "ok".to_string(),
            window: 32,
            ..Default::default()
        });
        let mut m = Message::new();
        m.set_port_forward_channel(pf);
        let back = roundtrip(&m);
        assert!(back.port_forward_channel().opened().success);
        assert_eq!(back.port_forward_channel().opened().message, "ok");
    }

    #[test]
    fn auth_and_unsupported_variants_are_explicitly_skipped() {
        // 旧认证轨: 认证走 P-384 mTLS + omt1 票据 (durt.idl §3.2), 不承载。
        let mut m = Message::new();
        m.set_hash(Hash::default());
        assert!(message_to_durt(&m).is_empty());
        let mut m = Message::new();
        m.set_signed_id(SignedId::default());
        assert!(message_to_durt(&m).is_empty());
        let mut m = Message::new();
        m.set_public_key(PublicKey::default());
        assert!(message_to_durt(&m).is_empty());
        let mut m = Message::new();
        m.set_auth_2fa(Auth2FA::default());
        assert!(message_to_durt(&m).is_empty());
        let mut m = Message::new();
        m.set_os_login(OSLogin::default());
        assert!(message_to_durt(&m).is_empty());

        // 能力边界: 两侧互换 / 语音通话 / 插件面 DURT 不承载。
        let mut m = Message::new();
        m.set_switch_sides_response(SwitchSidesResponse::default());
        assert!(message_to_durt(&m).is_empty());
        let mut m = Message::new();
        m.set_voice_call_request(VoiceCallRequest::default());
        assert!(message_to_durt(&m).is_empty());
        let mut m = Message::new();
        m.set_voice_call_response(VoiceCallResponse::default());
        assert!(message_to_durt(&m).is_empty());

        let mut ms = Misc::new();
        ms.set_switch_sides_request(SwitchSidesRequest::default());
        let mut m = Message::new();
        m.set_misc(ms);
        assert!(message_to_durt(&m).is_empty());
        let mut ms = Misc::new();
        ms.set_plugin_request(PluginRequest::default());
        let mut m = Message::new();
        m.set_misc(ms);
        assert!(message_to_durt(&m).is_empty());
    }

    #[test]
    fn rpc_unknown_method_and_clipv2_control_reverse() {
        // 未映射 method 不还原。
        let body = enc_rpc_request(1, "unknown.method", b"");
        assert!(durt_to_message(&DurtIn::Record {
            record_type: RecordType::RpcRequest as u16,
            body,
        })
        .is_none());

        // ClipV2 控制记录反向以 special_name 标角色。
        let v = ClipV2AckBody {
            transfer_id: 1,
            sequence: 1,
            next_offset: 0,
            credit: CLIPBOARD_V2_CHUNK_MAX,
        };
        let back = durt_to_message(&DurtIn::Record {
            record_type: RecordType::ClipV2Ack as u16,
            body: v.encode().unwrap().to_vec(),
        })
        .unwrap();
        assert_eq!(back.clipboard().special_name, "clipv2.ack");
    }
}
