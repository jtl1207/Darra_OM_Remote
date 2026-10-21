// DURT legacy 记录编解码 — 供连接引擎使用。
// 记法严格对齐 durt.idl §2/§3/§5/§6/§7/§8/§11。
// varint 用 durt_wire::encode_varint/decode_varint (QUIC RFC9000 §16)。
// blob/str = varint len + bytes; str 解码时校验 UTF-8。
// 定宽整数一律小端 (durt.idl §2.5)。

pub mod control;
pub mod files;
pub mod input;
pub mod media;
pub mod term;

// ---- 错误 ----
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// 缓冲区不够读完字段
    Truncated,
    /// str 字段不是合法 UTF-8
    InvalidUtf8,
    /// decode 后仍有剩余字节
    TrailingBytes,
    /// 字段值不合法 (枚举越界 / blob len 与声明 len 不符等)
    InvalidField(&'static str),
}

impl core::fmt::Display for WireError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WireError::Truncated => write!(f, "truncated"),
            WireError::InvalidUtf8 => write!(f, "invalid utf-8"),
            WireError::TrailingBytes => write!(f, "trailing bytes"),
            WireError::InvalidField(s) => write!(f, "invalid field: {s}"),
        }
    }
}

impl std::error::Error for WireError {}

// ---- WireWriter: 追加式小端写入 ----
pub struct WireWriter {
    buf: Vec<u8>,
}

impl WireWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i16(&mut self, v: i16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn varint(&mut self, v: u64) {
        durt_wire::encode_varint(v, &mut self.buf);
    }
    /// varint len + bytes
    pub fn blob(&mut self, v: &[u8]) {
        self.varint(v.len() as u64);
        self.buf.extend_from_slice(v);
    }
    /// varint len + UTF-8 bytes (与 blob 同布局, 仅语义不同)
    pub fn str(&mut self, v: &str) {
        self.blob(v.as_bytes());
    }
    /// 固定长度原始字节 (sha256 等), 无长度前缀
    pub fn raw(&mut self, v: &[u8]) {
        self.buf.extend_from_slice(v);
    }
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

// ---- WireReader: 顺序小端读取 ----
pub struct WireReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> WireReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        // 先界检查再切片; 用减法比较, 避免 pos + n 溢出
        if n > self.buf.len() - self.pos {
            return Err(WireError::Truncated);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn i16(&mut self) -> Result<i16, WireError> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32, WireError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn varint(&mut self) -> Result<u64, WireError> {
        // RFC9000 §16: 首字节高 2 bit 决定 1/2/4/8 字节。
        // 先界检查再调 decode_varint —— 后者按声明长度直取 buf[offset+i],
        // 缓冲只剩首字节时会越界 panic 而不是返回 Truncated。
        if self.pos >= self.buf.len() {
            return Err(WireError::Truncated);
        }
        let n = 1usize << (self.buf[self.pos] >> 6);
        if n > self.buf.len() - self.pos {
            return Err(WireError::Truncated);
        }
        let (v, n) = durt_wire::decode_varint(self.buf, self.pos);
        self.pos += n;
        Ok(v)
    }
    /// varint len + bytes
    pub fn blob(&mut self) -> Result<Vec<u8>, WireError> {
        let len = self.varint()? as usize;
        Ok(self.take(len)?.to_vec())
    }
    /// varint len + UTF-8 bytes
    pub fn str(&mut self) -> Result<String, WireError> {
        String::from_utf8(self.blob()?).map_err(|_| WireError::InvalidUtf8)
    }
    /// 固定长度原始字节, 无长度前缀
    pub fn raw(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        self.take(n)
    }
    pub fn done(&self) -> bool {
        self.pos == self.buf.len()
    }
}

// ---- 回归: 截断 varint 必须 Err(Truncated), 不许 panic ----
#[cfg(test)]
mod tests {
    use super::*;

    fn decode(buf: &[u8]) -> Result<u64, WireError> {
        WireReader::new(buf).varint()
    }

    #[test]
    fn varint_truncated_single_byte_prefix() {
        // 0x40/0x80/0xC0 = 首字节声明 2/4/8 字节, 缓冲只剩首字节
        assert_eq!(decode(&[0x40]), Err(WireError::Truncated));
        assert_eq!(decode(&[0x80]), Err(WireError::Truncated));
        assert_eq!(decode(&[0xC0]), Err(WireError::Truncated));
    }

    #[test]
    fn varint_truncated_short_tail() {
        // 声明 8 字节只给 2 字节; 声明 4 字节只给 3 字节
        assert_eq!(decode(&[0xC0, 0x01]), Err(WireError::Truncated));
        assert_eq!(decode(&[0x80, 0x01, 0x02]), Err(WireError::Truncated));
    }

    #[test]
    fn varint_full_decodes_ok() {
        // 对照: 完整 varint 正常解码, 不误报 Truncated
        assert_eq!(decode(&[0x25]), Ok(0x25));
        assert_eq!(decode(&[0x40, 0x01]), Ok(1));
        assert_eq!(decode(&[0x80, 0x00, 0x00, 0x02]), Ok(2));
        assert_eq!(
            decode(&[0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03]),
            Ok(3)
        );
    }
}

