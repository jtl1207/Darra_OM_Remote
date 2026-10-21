// DURT legacy 终端消息编解码 (durt.idl §5 TERMINAL)。
// 定宽整数一律小端; blob/str = varint len + bytes; decode 末尾必须 done() 否则 TrailingBytes。

/// 终端数据流 (type=0x10): blob = varint len + 原始字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermData {
    pub raw: Vec<u8>,
}

impl TermData {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.blob(&self.raw);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self { raw: r.blob()? };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 终端窗口尺寸 (type=0x11): 定宽 4 字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermResize {
    pub cols: u16,
    pub rows: u16,
}

impl TermResize {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u16(self.cols);
        w.u16(self.rows);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self { cols: r.u16()?, rows: r.u16()? };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 终端信号 (type=0x12): 定宽 1 字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermSignal {
    pub sig: u8,
}

impl TermSignal {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u8(self.sig);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self { sig: r.u8()? };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 终端退出 (type=0x13): 定宽 4 字节, 退出码小端有符号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermExit {
    pub exit_code: i32,
}

impl TermExit {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.i32(self.exit_code);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self { exit_code: r.i32()? };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 终端分离 (type=0x15): pty_id 走 varint。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermDetach {
    pub pty_id: u64,
}

impl TermDetach {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.varint(self.pty_id);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self { pty_id: r.varint()? };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 打开终端 (type=0x14): mode(1) + program(str) + cols(u16) + rows(u16) + cwd(str)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermOpen {
    /// 0=默认 shell, 1=指定程序
    pub mode: u8,
    pub program: String,
    pub cols: u16,
    pub rows: u16,
    pub cwd: String,
}

impl TermOpen {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u8(self.mode);
        w.str(&self.program);
        w.u16(self.cols);
        w.u16(self.rows);
        w.str(&self.cwd);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            mode: r.u8()?,
            program: r.str()?,
            cols: r.u16()?,
            rows: r.u16()?,
            cwd: r.str()?,
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

    // ---- 往返: decode(encode(x)) == x ----

    #[test]
    fn roundtrip_term_data() {
        let x = TermData { raw: vec![0x00, 0xFF, 0x41, 0x42, 0x43] };
        assert_eq!(TermData::decode(&x.encode()).unwrap(), x);
        let empty = TermData { raw: Vec::new() };
        assert_eq!(TermData::decode(&empty.encode()).unwrap(), empty);
    }

    #[test]
    fn roundtrip_term_resize() {
        let x = TermResize { cols: 80, rows: 25 };
        assert_eq!(TermResize::decode(&x.encode()).unwrap(), x);
        let big = TermResize { cols: 65535, rows: 1 };
        assert_eq!(TermResize::decode(&big.encode()).unwrap(), big);
    }

    #[test]
    fn roundtrip_term_signal() {
        let x = TermSignal { sig: 2 };
        assert_eq!(TermSignal::decode(&x.encode()).unwrap(), x);
    }

    #[test]
    fn roundtrip_term_exit() {
        let x = TermExit { exit_code: 0 };
        assert_eq!(TermExit::decode(&x.encode()).unwrap(), x);
        let neg = TermExit { exit_code: -1 };
        assert_eq!(TermExit::decode(&neg.encode()).unwrap(), neg);
        let min = TermExit { exit_code: i32::MIN };
        assert_eq!(TermExit::decode(&min.encode()).unwrap(), min);
    }

    #[test]
    fn roundtrip_term_open() {
        let x = TermOpen {
            mode: 1,
            program: "bash".to_string(),
            cols: 120,
            rows: 40,
            cwd: "/home/用户".to_string(),
        };
        assert_eq!(TermOpen::decode(&x.encode()).unwrap(), x);
        let def = TermOpen {
            mode: 0,
            program: String::new(),
            cols: 0,
            rows: 0,
            cwd: String::new(),
        };
        assert_eq!(TermOpen::decode(&def.encode()).unwrap(), def);
    }

    #[test]
    fn roundtrip_term_detach() {
        let x = TermDetach { pty_id: 200 };
        assert_eq!(TermDetach::decode(&x.encode()).unwrap(), x);
        let small = TermDetach { pty_id: 63 };
        assert_eq!(TermDetach::decode(&small.encode()).unwrap(), small);
        let big = TermDetach { pty_id: 1 << 40 };
        assert_eq!(TermDetach::decode(&big.encode()).unwrap(), big);
    }

    // ---- 手工推算字节向量 (期望字节不许用 encode() 生成) ----

    #[test]
    fn hand_term_resize_80x25() {
        // TermResize{cols:80, rows:25}
        //   cols = u16 LE(80)  = 0x0050 → [0x50, 0x00]
        //   rows = u16 LE(25)  = 0x0019 → [0x19, 0x00]
        // 期望 = [0x50, 0x00, 0x19, 0x00]
        let x = TermResize { cols: 80, rows: 25 };
        assert_eq!(x.encode(), vec![0x50, 0x00, 0x19, 0x00]);
        assert_eq!(TermResize::decode(&[0x50, 0x00, 0x19, 0x00]).unwrap(), x);
    }

    #[test]
    fn hand_term_detach_200() {
        // TermDetach{pty_id:200}
        //   varint(200): 200>=64 且 200<16384 → 2 字节形式
        //     首字节 = 0x40 | (200>>8) = 0x40 | 0x00 = 0x40
        //     次字节 = 200 & 0xFF = 0xC8
        // 期望 = [0x40, 0xC8]
        let x = TermDetach { pty_id: 200 };
        assert_eq!(x.encode(), vec![0x40, 0xC8]);
        assert_eq!(TermDetach::decode(&[0x40, 0xC8]).unwrap(), x);
    }

    #[test]
    fn hand_term_open_sh() {
        // TermOpen{mode:1, program:"sh", cols:120, rows:40, cwd:"/tmp"}
        //   mode = u8(1)                    → [0x01]
        //   program = str "sh" (len 2)      → [0x02] + [0x73 's', 0x68 'h']
        //   cols = u16 LE(120 = 0x78)       → [0x78, 0x00]
        //   rows = u16 LE(40  = 0x28)       → [0x28, 0x00]
        //   cwd = str "/tmp" (len 4)        → [0x04] + [0x2F '/', 0x74 't', 0x6D 'm', 0x70 'p']
        // 期望 = [0x01, 0x02, 0x73, 0x68, 0x78, 0x00, 0x28, 0x00, 0x04, 0x2F, 0x74, 0x6D, 0x70]
        let x = TermOpen {
            mode: 1,
            program: "sh".to_string(),
            cols: 120,
            rows: 40,
            cwd: "/tmp".to_string(),
        };
        assert_eq!(
            x.encode(),
            vec![0x01, 0x02, 0x73, 0x68, 0x78, 0x00, 0x28, 0x00, 0x04, 0x2F, 0x74, 0x6D, 0x70]
        );
    }

    #[test]
    fn hand_term_exit_minus_one() {
        // TermExit{exit_code:-1}
        //   i32 LE(-1) = 0xFFFFFFFF → [0xFF, 0xFF, 0xFF, 0xFF]
        let x = TermExit { exit_code: -1 };
        assert_eq!(x.encode(), vec![0xFF, 0xFF, 0xFF, 0xFF]);
    }

    // ---- 错误路径 ----

    #[test]
    fn trailing_bytes_rejected() {
        assert_eq!(
            TermResize::decode(&[0x50, 0x00, 0x19, 0x00, 0x00]),
            Err(super::super::WireError::TrailingBytes)
        );
        assert_eq!(
            TermDetach::decode(&[0x40, 0xC8, 0x00]),
            Err(super::super::WireError::TrailingBytes)
        );
    }

    #[test]
    fn truncated_rejected() {
        assert_eq!(
            TermResize::decode(&[0x50, 0x00, 0x19]),
            Err(super::super::WireError::Truncated)
        );
        assert_eq!(TermSignal::decode(&[]), Err(super::super::WireError::Truncated));
    }

    #[test]
    fn invalid_utf8_rejected() {
        // str 长度 2, 字节 [0xFF, 0xFF] 不是合法 UTF-8
        assert_eq!(
            TermOpen::decode(&[0x00, 0x02, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00]),
            Err(super::super::WireError::InvalidUtf8)
        );
    }
}
