// DURT legacy 控制/通道消息编解码 (durt.idl §2.1 + §3 CONTROL)。
// 定宽整数一律小端 (durt.idl §2.5); instance_id / session_id 走 varint (QUIC RFC9000 §16)。
// blob/str = varint len + bytes; decode 末尾必须 done() 否则 TrailingBytes。

/// 通道开线握手 (type=0x01), 每条非 CONTROL stream 的首记录。
/// flags: bit0=need_auth_check, bit1=resume, bit2=initiator_is_device。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelHello {
    pub channel_kind: u8,
    pub instance_id: u64,
    pub session_id: u64,
    pub flags: u8,
    pub auth_blob: Vec<u8>,
    pub resume_ctx: Vec<u8>,
}

impl ChannelHello {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u8(self.channel_kind);
        w.varint(self.instance_id);
        w.varint(self.session_id);
        w.u8(self.flags);
        w.blob(&self.auth_blob);
        w.blob(&self.resume_ctx);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            channel_kind: r.u8()?,
            instance_id: r.varint()?,
            session_id: r.varint()?,
            flags: r.u8()?,
            auth_blob: r.blob()?,
            resume_ctx: r.blob()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 通道开通确认 (type=0x02)。accepted_caps 为实际放行能力位, 可能小于请求值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAck {
    pub instance_id: u64,
    pub accepted_caps: u8,
}

impl ChannelAck {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.varint(self.instance_id);
        w.u8(self.accepted_caps);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            instance_id: r.varint()?,
            accepted_caps: r.u8()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 通道拒绝/复位 (type=0x03)。
/// reason: 1=UNAUTHORIZED 2=QUOTA 3=UNSUPPORTED_KIND 4=RESUME_FAILED 5=RATE_LIMITED 6=DEVICE_BUSY。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelReset {
    pub instance_id: u64,
    pub reason: u16,
}

impl ChannelReset {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.varint(self.instance_id);
        w.u16(self.reason);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            instance_id: r.varint()?,
            reason: r.u16()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 连接级 HELLO (type=0x05), CONTROL stream 首记录, 双向。
/// cap_bits 双方取交集; role: 0=device(被控端) 1=operator(主控)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnHello {
    pub durs_proto_min: u8,
    pub durs_proto_max: u8,
    pub cap_bits: u32,
    pub session_id: u64,
    pub role: u8,
    pub device_fp: Vec<u8>,
    pub ext: Vec<u8>,
}

impl ConnHello {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u8(self.durs_proto_min);
        w.u8(self.durs_proto_max);
        w.u32(self.cap_bits);
        w.varint(self.session_id);
        w.u8(self.role);
        w.blob(&self.device_fp);
        w.blob(&self.ext);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            durs_proto_min: r.u8()?,
            durs_proto_max: r.u8()?,
            cap_bits: r.u32()?,
            session_id: r.varint()?,
            role: r.u8()?,
            device_fp: r.blob()?,
            ext: r.blob()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 流控信用窗口推进 (type=0x06), 走 CONTROL 或各通道 stream。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowUpdate {
    pub target_channel_kind: u8,
    pub instance_id: u64,
    pub credit_bytes: u32,
}

impl FlowUpdate {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u8(self.target_channel_kind);
        w.varint(self.instance_id);
        w.u32(self.credit_bytes);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            target_channel_kind: r.u8()?,
            instance_id: r.varint()?,
            credit_bytes: r.u32()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 心跳保活 (type=0x07)。nonce 供回声, send_unix_ms 供 RTT 估算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ping {
    pub nonce: u64,
    pub send_unix_ms: u64,
}

impl Ping {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u64(self.nonce);
        w.u64(self.send_unix_ms);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            nonce: r.u64()?,
            send_unix_ms: r.u64()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 心跳回声 (type=0x08)。原样回 Ping.nonce / Ping.send_unix_ms, 另记 recv_unix_ms。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pong {
    pub nonce: u64,
    pub send_unix_ms: u64,
    pub recv_unix_ms: u64,
}

impl Pong {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u64(self.nonce);
        w.u64(self.send_unix_ms);
        w.u64(self.recv_unix_ms);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            nonce: r.u64()?,
            send_unix_ms: r.u64()?,
            recv_unix_ms: r.u64()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 优雅关闭通告 (type=0x09)。真正的连接关闭由传输层负责, 这里只做半连接通告。
/// reason: 0=NORMAL 1=IDLE_TIMEOUT 2=AUTH_EXPIRED 3=QUOTA_REVOKED 4=DEVICE_SHUTDOWN 5=PROTOCOL_ERROR 6=SUPERSEDED。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnClose {
    pub reason: u16,
    pub detail: String,
}

impl ConnClose {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u16(self.reason);
        w.str(&self.detail);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            reason: r.u16()?,
            detail: r.str()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::super::WireError;
    use super::*;

    // ---- 往返: decode(encode(x)) == x, 每个结构各一 ----
    #[test]
    fn roundtrip_channel_hello() {
        // varint 2 字节起点 instance_id=64, 4 字节起点 session_id=2^30, 空 resume_ctx
        let v = ChannelHello {
            channel_kind: 3,
            instance_id: 64,
            session_id: 1 << 30,
            flags: 0x05,
            auth_blob: vec![0xAA, 0xBB, 0xCC],
            resume_ctx: vec![],
        };
        assert_eq!(ChannelHello::decode(&v.encode()).unwrap(), v);
        // 8 字节 varint 上限 2^62-1, 空 auth_blob
        let v = ChannelHello {
            channel_kind: 0,
            instance_id: 0,
            session_id: (1u64 << 62) - 1,
            flags: 0,
            auth_blob: vec![],
            resume_ctx: vec![1],
        };
        assert_eq!(ChannelHello::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_channel_ack() {
        // varint 4 字节起点 2^14
        let v = ChannelAck { instance_id: 16384, accepted_caps: 0x3F };
        assert_eq!(ChannelAck::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_channel_reset() {
        // varint 2 字节上限 16383
        let v = ChannelReset { instance_id: 16383, reason: 3 };
        assert_eq!(ChannelReset::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_conn_hello() {
        // session_id=2^30 走 8 字节 varint, ext 空 blob
        let v = ConnHello {
            durs_proto_min: 1,
            durs_proto_max: 7,
            cap_bits: 0x0000_8802,
            session_id: 1 << 30,
            role: 1,
            device_fp: vec![0xDE, 0xAD, 0xBE],
            ext: vec![],
        };
        assert_eq!(ConnHello::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_flow_update() {
        // varint 1 字节上限 63, credit_bytes 全 F
        let v = FlowUpdate { target_channel_kind: 2, instance_id: 63, credit_bytes: 0xFFFF_FFFF };
        assert_eq!(FlowUpdate::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_ping() {
        let v = Ping { nonce: 0xDEAD_BEEF_CAFE_F00D, send_unix_ms: 1_729_000_000_000 };
        assert_eq!(Ping::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_pong() {
        let v = Pong {
            nonce: 0xDEAD_BEEF_CAFE_F00D,
            send_unix_ms: 1_729_000_000_000,
            recv_unix_ms: 1_729_000_000_042,
        };
        assert_eq!(Pong::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_conn_close() {
        // 中文 UTF-8 detail
        let v = ConnClose { reason: 5, detail: "对端协议错误".to_string() };
        assert_eq!(ConnClose::decode(&v.encode()).unwrap(), v);
        // 空 detail
        let v = ConnClose { reason: 0, detail: String::new() };
        assert_eq!(ConnClose::decode(&v.encode()).unwrap(), v);
    }

    // ---- 畸形输入: 截断 / 尾随 / 非法 UTF-8 ----
    #[test]
    fn decode_truncated() {
        // ChannelAck 只有 varint 没有 accepted_caps
        assert_eq!(ChannelAck::decode(&[0x40, 0x64]).unwrap_err(), WireError::Truncated);
        // ConnClose 只有 reason 没有 detail
        assert_eq!(ConnClose::decode(&[0x07, 0x00]).unwrap_err(), WireError::Truncated);
    }

    #[test]
    fn decode_trailing_bytes() {
        // ChannelAck 合法 3 字节后多 1 字节
        assert_eq!(
            ChannelAck::decode(&[0x40, 0x64, 0xFF, 0x00]).unwrap_err(),
            WireError::TrailingBytes
        );
        // Ping 定宽 16 字节后多 1 字节
        assert_eq!(Ping::decode(&[0u8; 17]).unwrap_err(), WireError::TrailingBytes);
    }

    // ---- 手推字节: 期望值逐字节人工推导, 禁止用 encode() 生成 ----
    #[test]
    fn channel_hello_hand_computed_bytes() {
        // channel_kind = 0x01        -> 01
        // instance_id  = 0x02 (<64)  -> varint 1 字节: 02
        // session_id   = 0x03 (<64)  -> varint 1 字节: 03
        // flags        = 0x00        -> 00
        // auth_blob    = [AA BB]     -> varint len=2 = 02, 再 AA BB
        // resume_ctx   = []          -> varint len=0 = 00
        let v = ChannelHello {
            channel_kind: 0x01,
            instance_id: 0x02,
            session_id: 0x03,
            flags: 0x00,
            auth_blob: vec![0xAA, 0xBB],
            resume_ctx: vec![],
        };
        let expect = vec![0x01, 0x02, 0x03, 0x00, 0x02, 0xAA, 0xBB, 0x00];
        assert_eq!(v.encode(), expect);
        assert_eq!(ChannelHello::decode(&expect).unwrap(), v);
    }

    #[test]
    fn channel_ack_hand_computed_bytes() {
        // instance_id   = 100 (64<=100<16384) -> varint 2 字节: 首字节 0x40|(100>>8) = 0x40|0x00 = 0x40, 次字节 100&0xFF = 0x64
        // accepted_caps = 0xFF                -> FF
        let v = ChannelAck { instance_id: 100, accepted_caps: 0xFF };
        let expect = vec![0x40, 0x64, 0xFF];
        assert_eq!(v.encode(), expect);
        assert_eq!(ChannelAck::decode(&expect).unwrap(), v);
    }

    #[test]
    fn flow_update_hand_computed_bytes() {
        // target_channel_kind = 0x02         -> 02
        // instance_id  = 64 (=2^6, 首个 2 字节 varint) -> 首字节 0x40|(64>>8) = 0x40|0x00 = 0x40, 次字节 64&0xFF = 0x40
        // credit_bytes = 1024 = 0x00000400   -> u32 小端: 00 04 00 00
        let v = FlowUpdate { target_channel_kind: 0x02, instance_id: 64, credit_bytes: 1024 };
        let expect = vec![0x02, 0x40, 0x40, 0x00, 0x04, 0x00, 0x00];
        assert_eq!(v.encode(), expect);
        assert_eq!(FlowUpdate::decode(&expect).unwrap(), v);
    }

    #[test]
    fn ping_hand_computed_bytes() {
        // nonce        = 0x0102030405060708 -> u64 小端: 08 07 06 05 04 03 02 01
        // send_unix_ms = 0x1122334455667788 -> u64 小端: 88 77 66 55 44 33 22 11
        let v = Ping { nonce: 0x0102_0304_0506_0708, send_unix_ms: 0x1122_3344_5566_7788 };
        let expect = vec![
            0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33,
            0x22, 0x11,
        ];
        assert_eq!(v.encode(), expect);
        assert_eq!(Ping::decode(&expect).unwrap(), v);
    }

    #[test]
    fn conn_close_hand_computed_bytes() {
        // reason = 7 = 0x0007 -> u16 小端: 07 00
        // detail = "hi"       -> varint len=2 = 02, 再 68('h') 69('i')
        let v = ConnClose { reason: 7, detail: "hi".to_string() };
        let expect = vec![0x07, 0x00, 0x02, 0x68, 0x69];
        assert_eq!(v.encode(), expect);
        assert_eq!(ConnClose::decode(&expect).unwrap(), v);
    }

    #[test]
    fn decode_invalid_utf8() {
        // ConnClose: reason=0 -> 00 00, str len=2 -> 02, 载荷 FF FE 非法 UTF-8
        assert_eq!(
            ConnClose::decode(&[0x00, 0x00, 0x02, 0xFF, 0xFE]).unwrap_err(),
            WireError::InvalidUtf8
        );
    }
}
