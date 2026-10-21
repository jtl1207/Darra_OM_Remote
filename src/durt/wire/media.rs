// DURT legacy 媒体编排消息编解码 (durt.idl §8 SCREEN_CTRL)。
// 定宽整数一律小端; media_stream_id 用 varint (QUIC 风格, durt_wire::encode_varint)。
// decode 末尾必须 done() 否则 TrailingBytes。

/// 启动媒体流 (type=0x40)。
/// media_kind 只允许 0=VIDEO, 其他值 fail-closed; media_stream_id 归属键 ≤256。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartMedia {
    /// 媒体种类: 只允许 0=VIDEO
    pub media_kind: u8,
    /// 归属键 (varint, ≤256)
    pub media_stream_id: u64,
    /// 初始目标码率 (bps)
    pub init_target_bps: u32,
    /// 帧率上限
    pub max_fps: u16,
    /// 画面宽
    pub w: u16,
    /// 画面高
    pub h: u16,
    /// 编解码偏好 (CodecPref)
    pub codec_pref: u8,
}

impl StartMedia {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u8(self.media_kind);
        w.varint(self.media_stream_id);
        w.u32(self.init_target_bps);
        w.u16(self.max_fps);
        w.u16(self.w);
        w.u16(self.h);
        w.u8(self.codec_pref);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let media_kind = r.u8()?;
        // 只允许 0=VIDEO, 其他值 fail-closed (durt.idl §8)
        if media_kind != 0 {
            return Err(super::WireError::InvalidField("media_kind"));
        }
        let v = Self {
            media_kind,
            media_stream_id: r.varint()?,
            init_target_bps: r.u32()?,
            max_fps: r.u16()?,
            w: r.u16()?,
            h: r.u16()?,
            codec_pref: r.u8()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 停止媒体流 (type=0x41)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopMedia {
    /// 归属键 (varint)
    pub media_stream_id: u64,
}

impl StopMedia {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.varint(self.media_stream_id);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            media_stream_id: r.varint()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 视频反向反馈 (type=0x42), 接收端→发送端。
/// kind: 0=NACK关键片 1=请求IDR 2=LTR确认 3=CC统计上报。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFeedback {
    /// 反馈种类 (VideoFeedbackKind)
    pub kind: u8,
    /// 帧序号
    pub frame_seq: u32,
    /// 片索引
    pub slice_idx: u16,
    /// 接收端实测吞吐 (bps)
    pub recv_delivery_rate: u32,
    /// 丢包率 (百万分率)
    pub recv_loss_ppm: u32,
    /// 抖动 (微秒)
    pub recv_jitter_us: u32,
}

impl VideoFeedback {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u8(self.kind);
        w.u32(self.frame_seq);
        w.u16(self.slice_idx);
        w.u32(self.recv_delivery_rate);
        w.u32(self.recv_loss_ppm);
        w.u32(self.recv_jitter_us);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            kind: r.u8()?,
            frame_seq: r.u32()?,
            slice_idx: r.u16()?,
            recv_delivery_rate: r.u32()?,
            recv_loss_ppm: r.u32()?,
            recv_jitter_us: r.u32()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::WireError;

    // ---- 往返: decode(encode(x)) == x ----

    #[test]
    fn start_media_round_trip() {
        let x = StartMedia {
            media_kind: 0,
            media_stream_id: 200,
            init_target_bps: 5_000_000,
            max_fps: 60,
            w: 1920,
            h: 1080,
            codec_pref: 1,
        };
        assert_eq!(StartMedia::decode(&x.encode()).unwrap(), x);
    }

    #[test]
    fn start_media_round_trip_varint_boundaries() {
        // 0/63/64/256: 1 字节与 2 字节 varint 分界 + 归属键上界
        for id in [0u64, 63, 64, 256] {
            let x = StartMedia {
                media_kind: 0,
                media_stream_id: id,
                init_target_bps: u32::MAX,
                max_fps: 0,
                w: 1,
                h: 65535,
                codec_pref: 3,
            };
            assert_eq!(StartMedia::decode(&x.encode()).unwrap(), x);
        }
    }

    #[test]
    fn stop_media_round_trip() {
        for id in [0u64, 1, 200, 256] {
            let x = StopMedia { media_stream_id: id };
            assert_eq!(StopMedia::decode(&x.encode()).unwrap(), x);
        }
    }

    #[test]
    fn video_feedback_round_trip() {
        let x = VideoFeedback {
            kind: 3,
            frame_seq: 70_000,
            slice_idx: 65535,
            recv_delivery_rate: 8_000_000,
            recv_loss_ppm: 1234,
            recv_jitter_us: 4567,
        };
        assert_eq!(VideoFeedback::decode(&x.encode()).unwrap(), x);
    }

    // ---- 手算字节向量 (期望字节逐字段推得, 不用 encode() 生成) ----

    #[test]
    fn stop_media_hand_bytes() {
        // media_stream_id=200 → varint 2 字节型:
        //   字节0 = 0x40 | (200 >> 8) = 0x40 | 0x00 = 0x40
        //   字节1 = 200 & 0xFF = 0xC8
        // 合计 2 字节
        let expected: Vec<u8> = vec![0x40, 0xC8];
        assert_eq!(StopMedia { media_stream_id: 200 }.encode(), expected);
        assert_eq!(StopMedia::decode(&expected).unwrap(), StopMedia { media_stream_id: 200 });
    }

    #[test]
    fn video_feedback_hand_bytes() {
        // kind=1                      = [0x01]
        // frame_seq u32 LE(256)       = [0x00, 0x01, 0x00, 0x00]
        // slice_idx u16 LE(2)         = [0x02, 0x00]
        // recv_delivery_rate u32 LE(1000) = [0xE8, 0x03, 0x00, 0x00]
        // recv_loss_ppm u32 LE(0)     = [0x00, 0x00, 0x00, 0x00]
        // recv_jitter_us u32 LE(50)   = [0x32, 0x00, 0x00, 0x00]
        // 合计 1+4+2+4+4+4 = 19 字节
        let expected: Vec<u8> = vec![
            0x01,
            0x00, 0x01, 0x00, 0x00,
            0x02, 0x00,
            0xE8, 0x03, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00,
            0x32, 0x00, 0x00, 0x00,
        ];
        let x = VideoFeedback {
            kind: 1,
            frame_seq: 256,
            slice_idx: 2,
            recv_delivery_rate: 1000,
            recv_loss_ppm: 0,
            recv_jitter_us: 50,
        };
        assert_eq!(x.encode(), expected);
        assert_eq!(VideoFeedback::decode(&expected).unwrap(), x);
    }

    #[test]
    fn start_media_hand_bytes() {
        // media_kind=0                    = [0x00]
        // media_stream_id=200 → varint    = [0x40, 0xC8]
        // init_target_bps u32 LE(5000000) = 0x004C4B40 → [0x40, 0x4B, 0x4C, 0x00]
        // max_fps u16 LE(60)              = [0x3C, 0x00]
        // w u16 LE(1920) = 0x0780         = [0x80, 0x07]
        // h u16 LE(1080) = 0x0438         = [0x38, 0x04]
        // codec_pref=1                    = [0x01]
        // 合计 1+2+4+2+2+2+1 = 14 字节
        let expected: Vec<u8> = vec![
            0x00,
            0x40, 0xC8,
            0x40, 0x4B, 0x4C, 0x00,
            0x3C, 0x00,
            0x80, 0x07,
            0x38, 0x04,
            0x01,
        ];
        let x = StartMedia {
            media_kind: 0,
            media_stream_id: 200,
            init_target_bps: 5_000_000,
            max_fps: 60,
            w: 1920,
            h: 1080,
            codec_pref: 1,
        };
        assert_eq!(x.encode(), expected);
        assert_eq!(StartMedia::decode(&expected).unwrap(), x);
    }

    // ---- 非法/截断路径 ----

    #[test]
    fn start_media_rejects_non_video_kind() {
        // media_kind 只允许 0=VIDEO, 其他值 fail-closed
        let x = StartMedia {
            media_kind: 1,
            media_stream_id: 200,
            init_target_bps: 5_000_000,
            max_fps: 60,
            w: 1920,
            h: 1080,
            codec_pref: 1,
        };
        assert_eq!(StartMedia::decode(&x.encode()), Err(WireError::InvalidField("media_kind")));
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let mut buf = StopMedia { media_stream_id: 200 }.encode();
        buf.push(0x00);
        assert_eq!(StopMedia::decode(&buf), Err(WireError::TrailingBytes));
    }

    #[test]
    fn decode_rejects_truncated() {
        // 空缓冲读 varint: Truncated
        assert_eq!(StopMedia::decode(&[]), Err(WireError::Truncated));
        // VideoFeedback 定宽 19 字节, 砍掉末字节后读 recv_jitter_us 时 Truncated
        let buf = vec![0u8; 18];
        assert_eq!(VideoFeedback::decode(&buf), Err(WireError::Truncated));
    }
}
