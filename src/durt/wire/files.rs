// DURT legacy 文件传输消息编解码 (durt.idl §7 FILES + codecs.ts §7.1)。
// 定宽整数一律小端; blob/str = varint 长度 + 字节; sha256 为 32 字节裸字段 (无长度前缀)。
// FileOpRequest.op_id / FileOpResult.op_id 在线为 varint; FileOpResult.stat 为可选块 (has_stat 门)。
// decode 末尾必须 done() 否则 TrailingBytes。

/// 文件打开 (type=0x30)。mode: 0=put, 1=get, 2=list。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOpen {
    /// 0=put, 1=get, 2=list
    pub mode: u8,
    pub path: String,
    pub total_size: u64,
    pub chunk_size: u32,
}

impl FileOpen {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u8(self.mode);
        w.str(&self.path);
        w.u64(self.total_size);
        w.u32(self.chunk_size);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            mode: r.u8()?,
            path: r.str()?,
            total_size: r.u64()?,
            chunk_size: r.u32()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}
/// 文件数据块 (type=0x31)。sha256 = 32 字节裸字段; data = blob (varint 长度 + 字节)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChunk {
    pub offset: u64,
    /// data 字节数 (与 data blob 长度同义; 解码只读 blob, 不校验两者一致)
    pub len: u32,
    pub sha256: [u8; 32],
    pub flags: u8,
    pub data: Vec<u8>,
}

impl FileChunk {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u64(self.offset);
        w.u32(self.len);
        w.raw(&self.sha256);
        w.u8(self.flags);
        w.blob(&self.data);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let offset = r.u64()?;
        let len = r.u32()?;
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(r.raw(32)?);
        let flags = r.u8()?;
        let data = r.blob()?;
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(Self { offset, len, sha256, flags, data })
    }
}
/// 文件应答 (type=0x32), 定宽 9 字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileAck {
    pub next_offset: u64,
    pub status: u8,
}

impl FileAck {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u64(self.next_offset);
        w.u8(self.status);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self { next_offset: r.u64()?, status: r.u8()? };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 目录项 (type=0x34)。kind: 0=file, 1=dir, 2=symlink。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListEntry {
    pub name: String,
    pub size: u64,
    pub mtime_unix_ms: u64,
    /// 0=file, 1=dir, 2=symlink
    pub kind: u8,
    pub mode_bits: u32,
}

impl FileListEntry {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.str(&self.name);
        w.u64(self.size);
        w.u64(self.mtime_unix_ms);
        w.u8(self.kind);
        w.u32(self.mode_bits);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            name: r.str()?,
            size: r.u64()?,
            mtime_unix_ms: r.u64()?,
            kind: r.u8()?,
            mode_bits: r.u32()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}
/// 取消 (type=0x35), 定宽 2 字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCancel {
    pub reason: u16,
}

impl FileCancel {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u16(self.reason);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self { reason: r.u16()? };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 文件操作结果里的 stat 可选块 (codecs.ts §7.1, has_stat==1 时才在线)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOpStat {
    pub is_dir: u8,
    pub size: u64,
    pub mtime_unix_sec: u64,
}

/// 文件操作请求 (type=0x38)。op_id 在线为 varint; path2 无第二路径时传空串。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOpRequest {
    pub op_id: u32,
    pub op_kind: u8,
    pub path: String,
    pub path2: String,
    pub flags: u8,
}

impl FileOpRequest {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.varint(self.op_id as u64);
        w.u8(self.op_kind);
        w.str(&self.path);
        w.str(&self.path2);
        w.u8(self.flags);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            op_id: r.varint()? as u32,
            op_kind: r.u8()?,
            path: r.str()?,
            path2: r.str()?,
            flags: r.u8()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}
/// 文件操作结果 (type=0x39)。op_id 在线为 varint; stat 由 has_stat (0/1) 门控。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOpResult {
    pub op_id: u32,
    pub status: u8,
    pub message: String,
    pub stat: Option<FileOpStat>,
}

impl FileOpResult {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.varint(self.op_id as u64);
        w.u8(self.status);
        w.str(&self.message);
        match &self.stat {
            Some(s) => {
                w.u8(1);
                w.u8(s.is_dir);
                w.varint(s.size);
                w.varint(s.mtime_unix_sec);
            }
            None => w.u8(0),
        }
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let op_id = r.varint()? as u32;
        let status = r.u8()?;
        let message = r.str()?;
        let has_stat = r.u8()?;
        let stat = match has_stat {
            0 => None,
            1 => Some(FileOpStat {
                is_dir: r.u8()?,
                size: r.varint()?,
                mtime_unix_sec: r.varint()?,
            }),
            _ => return Err(super::WireError::InvalidField("has_stat")),
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(Self { op_id, status, message, stat })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use super::super::WireError;

    // ---- 往返: decode(encode(x)) == x, 每个结构各一 ----
    #[test]
    fn roundtrip_file_open() {
        let v = FileOpen {
            mode: 1,
            path: "/tmp/a.bin".to_string(),
            total_size: 4096,
            chunk_size: 1024,
        };
        assert_eq!(FileOpen::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_file_chunk() {
        let v = FileChunk {
            offset: 1 << 40,
            len: 3,
            sha256: [7u8; 32],
            flags: 0x01,
            data: vec![0xAA, 0xBB, 0xCC],
        };
        assert_eq!(FileChunk::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_file_ack() {
        let v = FileAck { next_offset: u64::MAX, status: 2 };
        assert_eq!(FileAck::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_file_list_entry() {
        let v = FileListEntry {
            name: "目录取样".to_string(),
            size: 0,
            mtime_unix_ms: 1_700_000_000_000,
            kind: 2,
            mode_bits: 0o755,
        };
        assert_eq!(FileListEntry::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_file_cancel() {
        let v = FileCancel { reason: 0x0102 };
        assert_eq!(FileCancel::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_file_op_request() {
        let v = FileOpRequest {
            op_id: 300,
            op_kind: 5,
            path: "a/b.txt".to_string(),
            path2: String::new(),
            flags: 0x11,
        };
        assert_eq!(FileOpRequest::decode(&v.encode()).unwrap(), v);
    }
    #[test]
    fn roundtrip_file_op_result_no_stat() {
        let v = FileOpResult {
            op_id: 7,
            status: 0,
            message: "ok".to_string(),
            stat: None,
        };
        assert_eq!(FileOpResult::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_file_op_result_with_stat() {
        let v = FileOpResult {
            op_id: 0xFFFF_FFFF,
            status: 1,
            message: "详细消息".to_string(),
            stat: Some(FileOpStat { is_dir: 1, size: 12345, mtime_unix_sec: 1_700_000_000 }),
        };
        assert_eq!(FileOpResult::decode(&v.encode()).unwrap(), v);
    }

    // ---- 手算字节 (不调 encode 生成期望值) ----
    // FileCancel{reason:0x0102} 推导:
    //   reason=0x0102 -> u16 小端 [0x02, 0x01]
    // 合计 2 字节
    #[test]
    fn file_cancel_hand_computed_bytes() {
        let v = FileCancel { reason: 0x0102 };
        assert_eq!(v.encode(), vec![0x02, 0x01]);
        // 反向: 手算字节解码回同一值
        assert_eq!(FileCancel::decode(&[0x02, 0x01]).unwrap(), v);
    }

    // FileAck{next_offset:256, status:1} 推导:
    //   next_offset=256 -> u64 小端 [0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    //   status=1        -> u8 [0x01]
    // 合计 9 字节
    #[test]
    fn file_ack_hand_computed_bytes() {
        let v = FileAck { next_offset: 256, status: 1 };
        assert_eq!(
            v.encode(),
            vec![0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01]
        );
    }
    // FileOpRequest{op_id:300, op_kind:5, path:"ab", path2:"", flags:0x11} 推导:
    //   op_id=300   -> varint 2 字节 (64 <= 300 < 2^14): 首字节 0x40|(300>>8)=0x41, 次字节 300&0xFF=0x2C
    //   op_kind=5   -> u8 [0x05]
    //   path="ab"   -> str: varint len=2 [0x02] + b'a'=0x61, b'b'=0x62
    //   path2=""    -> str: varint len=0 [0x00]
    //   flags=0x11  -> u8 [0x11]
    // 合计 8 字节
    #[test]
    fn file_op_request_hand_computed_bytes() {
        let v = FileOpRequest {
            op_id: 300,
            op_kind: 5,
            path: "ab".to_string(),
            path2: String::new(),
            flags: 0x11,
        };
        assert_eq!(v.encode(), vec![0x41, 0x2C, 0x05, 0x02, 0x61, 0x62, 0x00, 0x11]);
    }

    // ---- 错误路径: 截断 / 尾部多余字节 / has_stat 越界 ----
    #[test]
    fn decode_truncated() {
        // u64 读不完
        assert_eq!(FileAck::decode(&[0x01]).unwrap_err(), WireError::Truncated);
        // str 声明 len=5 但只给 1 字节
        assert_eq!(FileOpen::decode(&[0x01, 0x05, 0x61]).unwrap_err(), WireError::Truncated);
        // sha256 32 字节裸字段不足
        assert_eq!(FileChunk::decode(&[0u8; 10]).unwrap_err(), WireError::Truncated);
    }

    #[test]
    fn decode_trailing_bytes() {
        let mut buf = FileCancel { reason: 1 }.encode();
        buf.push(0xAA);
        assert_eq!(FileCancel::decode(&buf).unwrap_err(), WireError::TrailingBytes);
    }

    #[test]
    fn decode_file_op_result_bad_has_stat() {
        // op_id=1 varint [0x01], status=0, message="" (len 0), has_stat=2 (非法)
        assert_eq!(
            FileOpResult::decode(&[0x01, 0x00, 0x00, 0x02]).unwrap_err(),
            WireError::InvalidField("has_stat")
        );
    }
}
