//! DURT 连接准入/撮合层 (控制端 host_role=operator_console)。
//!
//! 把「按设备 uid + JWT 发起连接」翻译成可执行连接计划 (endpoint / TLS 材料 /
//! AuthTicketV2 票据 / conn_id / session_id), 供 `durt::session` 建 QUIC。本层不做 QUIC。
//!
//! 权威规格: `src/static/Durt/proto/IPC_CONTRACT.md` §4.12–§4.15 (prepare_admission /
//! verify_admission / open_authorized_conn / drop_admission) + §8.1 digest canonical。
//! 验签唯一实现 = `durt_sidecar::admission::AdmissionManager`, 本文件只搬运材料与
//! 对齐字段, 不重写任何签名校验。
//!
//! token / 私钥 / community key / om_ticket 只经内存: 不进日志、不进 argv、不进错误串。

use durt_sidecar::admission::{unix_time_ms, AdmissionManager, HostRole};
use durt_sidecar::error::{Result, SidecarError};
use durt_sidecar::ipc::proto::{
    AdmissionCredentialsArgs, AuthorityKeyArgs, AuthTicketV2KeyArgs, OpenConnArgs, TlsArgs,
    VerifyAdmissionArgs,
};
use durt_sidecar::quic::tls::TlsParams;
use durt_wire::{
    NativeAdmissionAddressFamily, NativeAdmissionEnvelopeV1, AUTH_DIRECTION_KNOWN,
    CAP_AUTH_TICKET_V2, CAP_DATAGRAM_E2E,
    CAP_DISPLAY_TOPOLOGY_V2, CAP_FILE_BATCH_V2, CAP_INPUT_ACTION_RECEIPT_V2, CAP_INPUT_V2,
    ChannelKind, DEVICE_VIP_UDP_PORT, NATIVE_ADMISSION_V1_SUPERNODE_PORT,
};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;

/// om-master 撮合基址。来源: 被控端 `Service/Config/OmConstants.cs` `VipBaseUrl` 与
/// 控制端 `windows/Harness/LiveDurtHost.cs` `VipBase` 同值 (web `api/http.ts` VIP_BASE 同源)。
pub const OM_MASTER_API_BASE: &str = "https://om.darra.xyz/api";

/// 撮合路径 (uid 需 URI 转义)。来源: om-master `src/om/routes.mjs`
/// `router.post("/devices/:uid/durt/connect")` — 唯一连接入口, path 必须显式 native。
pub const OM_DURT_CONNECT_PATH: &str = "/om/devices/%s/durt/connect";

/// 撮合 HTTP 预算 (native 冷启动)。来源: Console `App/lib/api.ts` durtConnect `timeoutMs=60_000`。
pub const OM_DURT_CONNECT_TIMEOUT_MS: u64 = 60_000;

/// supernode 候选重连退避阶梯 (ms)。来源: `控制端/web/lib/api/session.ts`
/// `DURT_CONNECT_409_RETRY_DELAYS_MS = [300, 600, 1_000]`。
pub const SUPERNODE_RETRY_BACKOFF_MS: [u64; 3] = [300, 600, 1_000];

/// 会话用途 (决定初连 AuthTicketV2 意图; FILES 走懒刷新, 不进初连 batch)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// 远程桌面 (SCREEN_CTRL 意图)。
    Remote,
    /// 文件通道 (初连不发 FILES 意图, 票据走后续懒刷新)。
    Files,
    /// 终端通道 (TERMINAL 意图)。
    Terminal,
}

/// 发起连接的目标: 设备 uid + 登录 JWT + 会话用途。
pub struct ConnectTarget {
    pub uid: String,
    /// 登录态 Bearer JWT, 只经内存。
    pub token: String,
    pub kind: SessionKind,
}

/// supernode 候选节点 (覆盖网信令; QUIC 对端仍是 device_vip)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupernodeCandidate {
    pub addr: IpAddr,
    pub port: u16,
}

/// 可执行连接计划: 字段全部来自已验签的冻结计划, 调用方不可覆盖
/// (对齐 §4.14 `open_authorized_conn` "不得接受 peer_addr/TLS/CIDR/node/epoch 覆盖")。
pub struct ConnectPlan {
    /// QUIC 对端 = device_vip:18901 (AdmissionPlan::into_open_conn_args 的 peer_addr)。
    pub endpoint: SocketAddr,
    /// mTLS 材料 (Drop 自动清零 PEM)。
    pub tls: TlsParams,
    /// 母服 p2p_conn 连接号 (与 session_id 同源, 见 routes.mjs buildDurtConnectResponseData)。
    pub conn_id: u64,
    pub session_id: u64,
    /// CA 台账下发的期望对端指纹 (sha256 hex), 与 tls.expect_peer_fp 同值。
    pub expect_peer_fp: String,
    /// AuthTicketV2 envelope 原始字节 (内存驻留; drop_plan 清零)。
    pub tickets: Vec<Vec<u8>>,
    /// supernode 候选 (信封内固定 endpoint)。
    pub supernode: Vec<SupernodeCandidate>,
    /// 重连退避阶梯 (ms), 见 SUPERNODE_RETRY_BACKOFF_MS。
    pub retry_backoff_ms: [u64; 3],
    /// 计划过期时刻 (unix ms)。
    pub expires_unix_ms: u64,
    /// 一次性 opaque handle (已消费; 仅留作 drop_admission 语义对账, 不可复用)。
    admission_handle: String,
}

impl std::fmt::Debug for ConnectPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // ★ 私钥/证书/票据/handle 绝不进日志 (IPC_CONTRACT §8.1) ★
        f.debug_struct("ConnectPlan")
            .field("endpoint", &self.endpoint)
            .field("conn_id", &self.conn_id)
            .field("session_id", &self.session_id)
            .field("expect_peer_fp", &self.expect_peer_fp)
            .field("ticket_count", &self.tickets.len())
            .field("supernode", &self.supernode)
            .field("expires_unix_ms", &self.expires_unix_ms)
            .field("tls", &self.tls)
            .field("admission_handle", &"<opaque>")
            .finish()
    }
}

/// 登录态/目标前置校验 (撮合前 fail-closed; 缺 token 必须明确报错)。
fn validate_target(target: &ConnectTarget) -> Result<()> {
    if target.uid.trim().is_empty() {
        return Err(SidecarError::InvalidArgs("uid 为空".into()));
    }
    if target.token.trim().is_empty() {
        return Err(SidecarError::BadToken);
    }
    Ok(())
}

/// 进程内唯一 AdmissionManager (host_role=OperatorConsole)。
/// IPC token 只用于 manager 内部 IPC binding 摘要, 每进程随机, 不外传。
fn shared_manager() -> Result<std::sync::MutexGuard<'static, AdmissionManager>> {
    lazy_static::lazy_static! {
        static ref ADMISSION: Mutex<AdmissionManager> = {
            let token = uuid::Uuid::new_v4().to_string();
            let manager = AdmissionManager::production(HostRole::OperatorConsole, &token)
                .expect("durt 内置 authority roots 必须可解析");
            Mutex::new(manager)
        };
    }
    ADMISSION
        .lock()
        .map_err(|_| SidecarError::Internal("admission manager 锁中毒".into()))
}

/// 撮合 + 准入 → 连接计划。
/// 语义 = §4.12 prepare_admission → om-master `POST /om/devices/:uid/durt/connect` (native)
/// → §4.13 verify_admission → §4.14 取冻结计划 (等价 open_authorized_conn 的材料面)。
pub async fn request_plan(target: &ConnectTarget) -> Result<ConnectPlan> {
    validate_target(target)?;
    let (challenge_id, challenge_b64url) = {
        let mut manager = shared_manager()?;
        let prepared = manager.prepare(unix_time_ms()?)?;
        (prepared.challenge_id, prepared.challenge_b64url)
    };
    let body = build_match_body(&challenge_b64url, target.kind);
    let data = post_match(target, &body).await?;
    let (verify_args, meta) = parse_match_response(&data, &challenge_id)?;
    let mut manager = shared_manager()?;
    let now = unix_time_ms()?;
    let verified = manager.verify(verify_args, now)?;
    if meta.conn_id != 0 && meta.conn_id != verified.conn_id {
        return Err(SidecarError::AdmissionInvalid);
    }
    let plan = manager.take_plan(&verified.admission_handle, now)?;
    let open = plan.into_open_conn_args()?;
    ConnectPlan::from_open_conn_args(
        open,
        verified.conn_id,
        verified.session_id,
        verified.expires_unix_ms,
        verified.admission_handle,
        meta.tickets,
        meta.supernode,
    )
}

/// §4.15 drop_admission 语义: 销毁未消费 handle + 清零计划内敏感材料。
/// handle 已被 take_plan 一次性消费时 drop_handle 返回 admission_handle_invalid, 视为正常。
pub fn drop_plan(mut plan: ConnectPlan) {
    if let Ok(mut manager) = shared_manager() {
        let now = unix_time_ms().unwrap_or(0);
        let _ = manager.drop_handle(&plan.admission_handle, now);
    }
    for ticket in plan.tickets.iter_mut() {
        for byte in ticket.iter_mut() {
            *byte = 0;
        }
    }
    plan.tickets.clear();
    let mut handle = std::mem::take(&mut plan.admission_handle).into_bytes();
    for byte in handle.iter_mut() {
        *byte = 0;
    }
    drop(handle);
}

/// 撮合响应解析出的非签名旁路信息 (仅用于对齐/计划装配, 不参与验签)。
struct MatchMeta {
    conn_id: u64,
    tickets: Vec<Vec<u8>>,
    supernode: Vec<SupernodeCandidate>,
}

/// 初连 AuthTicketV2 意图 (来源: LiveDurtHost.BuildConnectIntents 同口径;
/// FILES/NETWORK 不进初连 batch)。
fn channel_intents(kind: SessionKind) -> Vec<Value> {
    let (channel, instance) = match kind {
        SessionKind::Remote => (ChannelKind::ScreenCtrl, 3u32),
        SessionKind::Terminal => (ChannelKind::Terminal, 2),
        // FILES 票据走懒刷新, 初连不发意图 (LiveDurtHost: FILES 不得进初连 batch)。
        SessionKind::Files => return Vec::new(),
    };
    vec![json!({
        "kind": channel.repr(),
        "instance": instance,
        "direction": AUTH_DIRECTION_KNOWN,
        "requested_caps": 0x03,
    })]
}

/// 连接级能力位 (来源: web session.ts clientCapsForDurtConnect + LiveDurtHost native 口径)。
fn client_cap_bits(kind: SessionKind) -> u32 {
    let _ = kind;
    CAP_DISPLAY_TOPOLOGY_V2
        | CAP_INPUT_V2
        | CAP_INPUT_ACTION_RECEIPT_V2
        | CAP_FILE_BATCH_V2
        | CAP_AUTH_TICKET_V2
        | CAP_DATAGRAM_E2E
}

/// §4.13 撮合请求体 (native 路径必须带 sidecar 生成的 32B challenge)。
fn build_match_body(challenge_b64url: &str, kind: SessionKind) -> Value {
    let mut body = json!({
        "path": "native",
        "client_cap_bits": client_cap_bits(kind),
        "native_admission_challenge": challenge_b64url,
    });
    let intents = channel_intents(kind);
    if !intents.is_empty() {
        body["channel_intents"] = Value::Array(intents);
    }
    body
}

/// om-master 撮合 HTTP 调用 (Bearer JWT; 响应恒为 {success, data} 信封)。
async fn post_match(target: &ConnectTarget, body: &Value) -> Result<Value> {
    let url = format!(
        "{}{}",
        OM_MASTER_API_BASE,
        OM_DURT_CONNECT_PATH.replacen("%s", &utf8_percent_encode(&target.uid), 1)
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(OM_DURT_CONNECT_TIMEOUT_MS))
        .build()
        .map_err(|e| SidecarError::Internal(format!("撮合客户端构建失败: {}", e)))?;
    let resp = client
        .post(&url)
        .bearer_auth(&target.token)
        .json(body)
        .send()
        .await
        .map_err(|e| SidecarError::Internal(format!("撮合请求失败: {}", e)))?;
    let status = resp.status().as_u16();
    if status == 401 {
        return Err(SidecarError::BadToken);
    }
    let text = resp
        .text()
        .await
        .map_err(|e| SidecarError::Internal(format!("撮合响应读取失败: {}", e)))?;
    let root: Value = serde_json::from_str(&text)
        .map_err(|_| SidecarError::Internal("撮合响应非 JSON".into()))?;
    if !(200..300).contains(&status) || root.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(map_match_error(status, &root));
    }
    root.get("data")
        .cloned()
        .filter(Value::is_object)
        .ok_or_else(|| SidecarError::Internal("撮合响应缺少 data 信封".into()))
}

/// 错误映射: 只取 error_code/message, 正文截断, 绝不回带 token。
fn map_match_error(status: u16, root: &Value) -> SidecarError {
    let code = root
        .get("error_code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if code.contains("TOKEN") || code.contains("UNAUTH") || code.contains("LOGIN") {
        return SidecarError::BadToken;
    }
    let message = root
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(120)
        .collect::<String>();
    SidecarError::Internal(format!("撮合失败 status={status} code={code} msg={message}"))
}

/// 把 {success,data} 的 data 投影成 §4.13 verify_admission 材料 + 旁路元信息。
/// 字段名与 om-master routes.mjs native_admission 投影 / LiveDurtHost 逐字一致。
fn parse_match_response(data: &Value, challenge_id: &str) -> Result<(VerifyAdmissionArgs, MatchMeta)> {
    let native = data
        .get("native_admission")
        .filter(|value| value.is_object())
        .ok_or_else(|| SidecarError::AdmissionInvalid)?;
    let envelope_b64 = native
        .get("envelope_b64")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(SidecarError::AdmissionInvalid)?
        .to_string();
    let authority_keys = parse_authority_keys(native.get("authority_keys"))?;
    let auth_ticket_v2_keys = parse_ticket_keys(native.get("auth_ticket_v2_keys"))?;
    let (auth_ticket_v2_envelopes_b64, tickets) = parse_ticket_envelopes(
        native.get("auth_ticket_v2_envelopes_b64"),
    )?;
    let credentials = parse_credentials(native.get("credentials"))?;
    let supernode = supernode_from_envelope(&envelope_b64)?;
    let conn_id = data
        .get("conn_id")
        .and_then(Value::as_u64)
        .or_else(|| data.get("session_id").and_then(Value::as_u64))
        .unwrap_or(0);
    Ok((
        VerifyAdmissionArgs {
            challenge_id: challenge_id.to_string(),
            envelope_b64,
            authority_keys,
            auth_ticket_v2_keys,
            auth_ticket_v2_envelopes_b64,
            credentials,
        },
        MatchMeta {
            conn_id,
            tickets,
            supernode,
        },
    ))
}

fn parse_authority_keys(value: Option<&Value>) -> Result<Vec<AuthorityKeyArgs>> {
    let items = value.and_then(Value::as_array).ok_or(SidecarError::AdmissionInvalid)?;
    let mut keys = Vec::with_capacity(items.len());
    for entry in items {
        keys.push(AuthorityKeyArgs {
            key_id_hex: str_field(entry, "key_id_hex")?,
            p256_spki_der_b64: str_field(entry, "p256_spki_der_b64")?,
        });
    }
    Ok(keys)
}

fn parse_ticket_keys(value: Option<&Value>) -> Result<Vec<AuthTicketV2KeyArgs>> {
    let items = value.and_then(Value::as_array).ok_or(SidecarError::AdmissionInvalid)?;
    let mut keys = Vec::with_capacity(items.len());
    for entry in items {
        keys.push(AuthTicketV2KeyArgs {
            key_id_hex: str_field(entry, "key_id_hex")?,
            p384_spki_der_b64: str_field(entry, "p384_spki_der_b64")?,
        });
    }
    Ok(keys)
}

/// 票据集合: b64 原样交 verify (deny_unknown_fields 逐字段), 另解码留计划内使用。
fn parse_ticket_envelopes(value: Option<&Value>) -> Result<(Vec<String>, Vec<Vec<u8>>)> {
    let items = value.and_then(Value::as_array).ok_or(SidecarError::AdmissionInvalid)?;
    let mut b64_list = Vec::with_capacity(items.len());
    let mut raw_list = Vec::with_capacity(items.len());
    for entry in items {
        let text = entry.as_str().filter(|s| !s.is_empty()).ok_or(SidecarError::AdmissionInvalid)?;
        let raw = b64_decode(text).map_err(|_| SidecarError::AdmissionInvalid)?;
        b64_list.push(text.to_string());
        raw_list.push(raw);
    }
    Ok((b64_list, raw_list))
}

/// §4.13 credentials 五组件 (opaque UTF-8 / PEM; 只经内存)。
fn parse_credentials(value: Option<&Value>) -> Result<AdmissionCredentialsArgs> {
    let obj = value.filter(|value| value.is_object()).ok_or(SidecarError::AdmissionInvalid)?;
    Ok(AdmissionCredentialsArgs {
        community_key_utf8: str_field(obj, "community_key_utf8")?,
        om_ticket_utf8: str_field(obj, "om_ticket_utf8")?,
        cert_chain_pem: str_field(obj, "cert_chain_pem")?,
        private_key_pem: str_field(obj, "private_key_pem")?,
        ca_pem: str_field(obj, "ca_pem")?,
    })
}

fn str_field(entry: &Value, key: &str) -> Result<String> {
    entry
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or(SidecarError::AdmissionInvalid)
}

/// 从已签信封取 supernode 候选 (只读字段, 不做验签 — 验签在 AdmissionManager::verify)。
fn supernode_from_envelope(envelope_b64: &str) -> Result<Vec<SupernodeCandidate>> {
    let bytes = b64_decode(envelope_b64).map_err(|_| SidecarError::AdmissionInvalid)?;
    let envelope = NativeAdmissionEnvelopeV1::decode(&bytes)
        .map_err(|_| SidecarError::AdmissionInvalid)?;
    let body = &envelope.body;
    if body.supernode_port != NATIVE_ADMISSION_V1_SUPERNODE_PORT {
        return Err(SidecarError::AdmissionInvalid);
    }
    if body.address_family != NativeAdmissionAddressFamily::Ipv4.repr()
        && body.address_family != NativeAdmissionAddressFamily::Ipv6.repr()
    {
        return Err(SidecarError::AdmissionInvalid);
    }
    Ok(vec![SupernodeCandidate {
        addr: ip_from_16(&body.supernode_address),
        port: body.supernode_port,
    }])
}

/// 16B IPv4-mapped/IPv6 字节 → IpAddr (durt.idl: IPv4 用 16B IPv4-mapped 表示)。
fn ip_from_16(bytes: &[u8; 16]) -> IpAddr {
    let v6 = Ipv6Addr::from(*bytes);
    match v6.to_ipv4_mapped() {
        Some(v4) => IpAddr::V4(v4),
        None => IpAddr::V6(v6),
    }
}

#[allow(dead_code)]
fn ip_to_16(addr: IpAddr) -> [u8; 16] {
    match addr {
        IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
        IpAddr::V6(v6) => v6.octets(),
    }
}

/// 最小 base64 解码 (标准 + url-safe 无填充; envelope/challenge 材料只经内存)。
fn b64_decode(input: &str) -> std::result::Result<Vec<u8>, ()> {
    fn sextet(c: u8) -> std::result::Result<u8, ()> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' | b'-' => Ok(62),
            b'/' | b'_' => Ok(63),
            _ => Err(()),
        }
    }
    let mut data = Vec::with_capacity(input.len() * 3 / 4 + 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in input.as_bytes() {
        if c == b'=' {
            break;
        }
        acc = (acc << 6) | sextet(c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            data.push((acc >> bits) as u8);
        }
    }
    Ok(data)
}

/// uid → URI path 转义 (只放行 unreserved, 其余按 %XX)。
fn utf8_percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

impl ConnectPlan {
    /// 把冻结计划 (AdmissionPlan::into_open_conn_args 产物) 对齐成连接计划。
    /// 字段映射钉死: endpoint ← peer_addr; tls ← TlsArgs + alpn; session_id ← session_id;
    /// expect_peer_fp ← tls.expect_peer_fp (CA 台账, 不许对端自报)。
    pub fn from_open_conn_args(
        open: OpenConnArgs,
        conn_id: u64,
        session_id: u64,
        expires_unix_ms: u64,
        admission_handle: String,
        tickets: Vec<Vec<u8>>,
        supernode: Vec<SupernodeCandidate>,
    ) -> Result<Self> {
        if open.role != "client" {
            return Err(SidecarError::AdmissionInvalid);
        }
        let peer = open
            .peer_addr
            .as_deref()
            .and_then(|text| text.parse::<SocketAddr>().ok())
            .ok_or(SidecarError::AdmissionInvalid)?;
        // 对齐 into_open_conn_args: QUIC 对端必须是固定 device_vip:18901。
        if peer.port() != DEVICE_VIP_UDP_PORT {
            return Err(SidecarError::AdmissionInvalid);
        }
        let TlsArgs {
            cert_chain_pem,
            private_key_pem,
            ca_pem,
            require_client_cert,
            expect_peer_fp,
            key_curve,
        } = open.tls;
        if expect_peer_fp.trim().is_empty() || session_id == 0 {
            return Err(SidecarError::AdmissionInvalid);
        }
        Ok(ConnectPlan {
            endpoint: peer,
            tls: TlsParams {
                cert_chain_pem,
                private_key_pem,
                ca_pem,
                require_client_cert,
                expect_peer_fp: expect_peer_fp.clone(),
                key_curve,
                alpn: open.alpn,
            },
            conn_id,
            session_id,
            expect_peer_fp,
            tickets,
            supernode,
            retry_backoff_ms: SUPERNODE_RETRY_BACKOFF_MS,
            expires_unix_ms,
            admission_handle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use durt_wire::{
        NativeAdmissionHostRole, NativeAdmissionV1Algorithm, NativeAdmissionV1Body,
        NATIVE_ADMISSION_V1_CANDIDATE_DOMAIN, NATIVE_ADMISSION_V1_SIGNING_DOMAIN,
        NATIVE_ADMISSION_V1_VERSION,
    };
    use sha2::{Digest, Sha256};
    use std::net::Ipv4Addr;

    const NOW: u64 = 1_800_000_000_000;

    /// 固定向量: 字段值全部钉死 (RFC5737 文档地址 198.51.100.1 / 覆盖网 100.64.0.2)。
    fn fixed_body() -> NativeAdmissionV1Body {
        let mut supernode_address = [0u8; 16];
        supernode_address[10] = 0xff;
        supernode_address[11] = 0xff;
        supernode_address[12..16].copy_from_slice(&[198, 51, 100, 1]);
        let mut device_vip = [0u8; 16];
        device_vip[10] = 0xff;
        device_vip[11] = 0xff;
        device_vip[12..16].copy_from_slice(&[100, 64, 0, 2]);
        NativeAdmissionV1Body {
            version: NATIVE_ADMISSION_V1_VERSION,
            algorithm: NativeAdmissionV1Algorithm::Es256P1363.repr(),
            host_role: NativeAdmissionHostRole::OperatorConsole.repr(),
            address_family: NativeAdmissionAddressFamily::Ipv4.repr(),
            key_id: [0x11; 32],
            admission_id: [0x22; 16],
            challenge: [0x33; 32],
            issued_unix_ms: NOW,
            not_before_unix_ms: NOW - 30_000,
            expires_unix_ms: NOW + 60_000,
            ttl_ms: 60_000,
            conn_id: 41,
            session_id: 41,
            binding_epoch: 7,
            route_epoch: 9,
            supernode_address,
            supernode_port: NATIVE_ADMISSION_V1_SUPERNODE_PORT,
            device_vip_port: DEVICE_VIP_UDP_PORT,
            overlay_prefix: 24,
            target_cidr_present: 0,
            target_prefix: 0,
            operator_vip: [0x44; 16],
            device_vip,
            overlay_network: [0x55; 16],
            target_network: [0u8; 16],
            device_tls_fingerprint: [0x22; 32],
            operator_tls_fingerprint: [0x66; 32],
            auth_ticket_keyset_digest: [0x77; 32],
            auth_ticket_set_digest: [0x88; 32],
            credential_digest: [0x99; 32],
            candidate_digest: [0; 32],
            issuer: "darra-om".into(),
            device_uid: "dev-1".into(),
            operator_account: "acct-1".into(),
            operator_subject: "subject-1".into(),
            node_id: "sn-1".into(),
            community: "comm-1".into(),
        }
    }

    /// §8.1 candidate_digest 复算 (测试向量专用; 生产验签在 sidecar)。
    fn candidate_digest_recompute(body: &NativeAdmissionV1Body) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(NATIVE_ADMISSION_V1_CANDIDATE_DOMAIN.as_bytes());
        hash.update([body.address_family]);
        hash.update(body.supernode_address);
        hash.update(body.supernode_port.to_le_bytes());
        hash.update(body.route_epoch.to_le_bytes());
        hash.update((body.node_id.as_bytes().len() as u16).to_le_bytes());
        hash.update(body.node_id.as_bytes());
        hash.finalize().into()
    }

    /// ① NativeAdmissionV1 envelope 解析 / digest 复算固定向量。
    #[test]
    fn native_admission_envelope_parse_and_digest_vector() {
        let mut body = fixed_body();
        let candidate = candidate_digest_recompute(&body);
        assert_eq!(
            hex::encode(candidate),
            "e323350f66d61200d44ed7e82e57642971ce2f5568e5f1fe4b78f334ebe41d35",
            "candidate_digest 固定向量漂移"
        );
        body.candidate_digest = candidate;

        let signing_input = body.signing_input().expect("canonical 签名输入");
        assert_eq!(
            hex::encode(Sha256::digest(&signing_input)),
            "9e67660c5c8531e486680e7cce4bdcc3eee1971c9107175d472fa905290104a6",
            "signing_input digest 固定向量漂移"
        );
        assert_eq!(
            &signing_input[..NATIVE_ADMISSION_V1_SIGNING_DOMAIN.as_bytes().len()],
            NATIVE_ADMISSION_V1_SIGNING_DOMAIN.as_bytes(),
            "签名域前缀"
        );

        let mut envelope = durt_wire::NativeAdmissionEnvelopeV1 {
            body: body.clone(),
            signature: [0x5a; 64],
        };
        envelope.body.candidate_digest = candidate;
        let encoded = envelope.encode().expect("信封编码");
        let decoded = durt_wire::NativeAdmissionEnvelopeV1::decode(&encoded).expect("信封解析");
        assert_eq!(decoded, envelope, "encode/decode 往返必须无损");
        assert_eq!(decoded.body.conn_id, 41);
        assert_eq!(decoded.body.supernode_port, NATIVE_ADMISSION_V1_SUPERNODE_PORT);
        assert_eq!(decoded.body.device_vip_port, DEVICE_VIP_UDP_PORT);
    }

    /// 与 AdmissionPlan::into_open_conn_args 的字段映射对齐 (peer=device_vip:18901,
    /// expect_peer_fp=hex(device_tls_fingerprint), alpn="durt/1", session_id 直传)。
    fn open_conn_args_fixture() -> OpenConnArgs {
        let body = fixed_body();
        let device_vip = ip_from_16(&body.device_vip);
        assert_eq!(device_vip, IpAddr::V4(Ipv4Addr::new(100, 64, 0, 2)));
        OpenConnArgs {
            role: "client".into(),
            bind_addr: None,
            peer_addr: Some(SocketAddr::new(device_vip, DEVICE_VIP_UDP_PORT).to_string()),
            session_id: body.session_id,
            alpn: "durt/1".into(),
            tls: TlsArgs {
                cert_chain_pem: "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n".into(),
                private_key_pem: "-----BEGIN PRIVATE KEY-----\ntest\n-----END PRIVATE KEY-----\n".into(),
                ca_pem: "-----BEGIN CERTIFICATE-----\nca\n-----END CERTIFICATE-----\n".into(),
                require_client_cert: true,
                expect_peer_fp: hex::encode(body.device_tls_fingerprint),
                key_curve: "P384".into(),
            },
            idle_timeout_ms: 30_000,
            max_datagram_size: 1350,
            enable_datagram: true,
            cc_stats_interval_ms: 0,
        }
    }

    /// ② 连接计划字段与 into_open_conn_args 产物逐项对齐。
    #[test]
    fn connect_plan_aligns_with_open_conn_args() {
        let body = fixed_body();
        let open = open_conn_args_fixture();
        let expect_fp = hex::encode(body.device_tls_fingerprint);
        let plan = ConnectPlan::from_open_conn_args(
            open,
            body.conn_id,
            body.session_id,
            NOW + 60_000,
            "handle-opaque".into(),
            vec![vec![0xAB; 85]],
            vec![SupernodeCandidate {
                addr: IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)),
                port: NATIVE_ADMISSION_V1_SUPERNODE_PORT,
            }],
        )
        .expect("计划装配");
        assert_eq!(
            plan.endpoint,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 2)), DEVICE_VIP_UDP_PORT)
        );
        assert_eq!(plan.session_id, 41);
        assert_eq!(plan.conn_id, 41);
        assert_eq!(plan.expect_peer_fp, expect_fp);
        assert_eq!(plan.tls.expect_peer_fp, expect_fp);
        assert_eq!(plan.tls.key_curve, "P384");
        assert_eq!(plan.tls.alpn, "durt/1");
        assert!(plan.tls.require_client_cert);
        assert_eq!(plan.tickets.len(), 1);
        assert_eq!(plan.supernode[0].port, NATIVE_ADMISSION_V1_SUPERNODE_PORT);
        assert_eq!(plan.retry_backoff_ms, SUPERNODE_RETRY_BACKOFF_MS);
        let debug = format!("{plan:?}");
        assert!(!debug.contains("PRIVATE KEY"), "Debug 不得泄私钥");
        assert!(!debug.contains("handle-opaque"), "Debug 不得泄 handle");
    }

    /// ③ 未登录 / 缺 token 时 request_plan 必须明确报错, 不发起撮合。
    #[tokio::test]
    async fn request_plan_rejects_missing_token() {
        let target = ConnectTarget {
            uid: "dev-1".into(),
            token: String::new(),
            kind: SessionKind::Remote,
        };
        let err = request_plan(&target).await.expect_err("缺 token 必须拒绝");
        assert!(
            matches!(err, SidecarError::BadToken),
            "缺 token 应报 BadToken, 实际: {err:?}"
        );

        let blank_uid = ConnectTarget {
            uid: "  ".into(),
            token: "jwt".into(),
            kind: SessionKind::Files,
        };
        let err = request_plan(&blank_uid).await.expect_err("空 uid 必须拒绝");
        assert!(matches!(err, SidecarError::InvalidArgs(_)), "实际: {err:?}");
    }
}
