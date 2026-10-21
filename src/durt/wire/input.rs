// DURT legacy 输入消息编解码 (durt.idl §6 INPUT)。
// 定宽整数一律小端; 定宽之外无长度前缀, decode 末尾必须 done() 否则 TrailingBytes。

/// 鼠标/指针移动 (type=0x20), 定宽 9 字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtrMove {
    pub x: i32,
    pub y: i32,
    pub buttons: u8,
}

impl PtrMove {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.i32(self.x);
        w.i32(self.y);
        w.u8(self.buttons);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            x: r.i32()?,
            y: r.i32()?,
            buttons: r.u8()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 鼠标按键 (type=0x21), 定宽 10 字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtrButton {
    pub button: u8,
    pub down: u8,
    pub x: i32,
    pub y: i32,
}

impl PtrButton {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u8(self.button);
        w.u8(self.down);
        w.i32(self.x);
        w.i32(self.y);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            button: r.u8()?,
            down: r.u8()?,
            x: r.i32()?,
            y: r.i32()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 滚轮 (type=0x22), 定宽 4 字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtrScroll {
    pub dx: i16,
    pub dy: i16,
}

impl PtrScroll {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.i16(self.dx);
        w.i16(self.dy);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            dx: r.i16()?,
            dy: r.i16()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 按键 (type=0x23), 定宽 6 字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    pub keycode: u32,
    pub down: u8,
    pub modifiers: u8,
}

impl KeyEvent {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.u32(self.keycode);
        w.u8(self.down);
        w.u8(self.modifiers);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self {
            keycode: r.u32()?,
            down: r.u8()?,
            modifiers: r.u8()?,
        };
        if !r.done() {
            return Err(super::WireError::TrailingBytes);
        }
        Ok(v)
    }
}

/// 文本输入 (type=0x24), utf8 = blob (varint 长度前缀 + UTF-8 字节)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextInput {
    pub utf8: Vec<u8>,
}

impl TextInput {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = super::WireWriter::new();
        w.blob(&self.utf8);
        w.finish()
    }

    pub fn decode(buf: &[u8]) -> Result<Self, super::WireError> {
        let mut r = super::WireReader::new(buf);
        let v = Self { utf8: r.blob()? };
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
    fn roundtrip_ptr_move() {
        let v = PtrMove { x: 100, y: -200, buttons: 0b101 };
        assert_eq!(PtrMove::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_ptr_button() {
        let v = PtrButton { button: 2, down: 1, x: -1, y: 70000 };
        assert_eq!(PtrButton::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_ptr_scroll() {
        let v = PtrScroll { dx: -3, dy: 120 };
        assert_eq!(PtrScroll::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_key_event() {
        let v = KeyEvent { keycode: 0x1234_5678, down: 0, modifiers: 0x0F };
        assert_eq!(KeyEvent::decode(&v.encode()).unwrap(), v);
    }

    #[test]
    fn roundtrip_text_input() {
        let v = TextInput { utf8: "你好, world".as_bytes().to_vec() };
        assert_eq!(TextInput::decode(&v.encode()).unwrap(), v);
    }

    // ---- 手算字节向量 (期望值由人工按小端推导, 不经 encode() 生成) ----

    // PtrMove{x:1, y:-2, buttons:3} 推导:
    //   x=1        -> i32 小端 [0x01, 0x00, 0x00, 0x00]
    //   y=-2       -> 补码 0xFFFFFFFE, 小端 [0xFE, 0xFF, 0xFF, 0xFF]
    //   buttons=3  -> u8 [0x03]
    // 合计 9 字节
    #[test]
    fn ptr_move_hand_computed_bytes() {
        let v = PtrMove { x: 1, y: -2, buttons: 3 };
        assert_eq!(
            v.encode(),
            vec![0x01, 0x00, 0x00, 0x00, 0xFE, 0xFF, 0xFF, 0xFF, 0x03]
        );
        // 反向: 手算字节解码回同一值
        assert_eq!(
            PtrMove::decode(&[0x01, 0x00, 0x00, 0x00, 0xFE, 0xFF, 0xFF, 0xFF, 0x03]).unwrap(),
            v
        );
    }

    // PtrButton{button:2, down:1, x:256, y:-1} 推导:
    //   button=2   -> u8 [0x02]
    //   down=1     -> u8 [0x01]
    //   x=256      -> 0x00000100, 小端 [0x00, 0x01, 0x00, 0x00]
    //   y=-1       -> 补码 0xFFFFFFFF, 小端 [0xFF, 0xFF, 0xFF, 0xFF]
    // 合计 10 字节
    #[test]
    fn ptr_button_hand_computed_bytes() {
        let v = PtrButton { button: 2, down: 1, x: 256, y: -1 };
        assert_eq!(
            v.encode(),
            vec![0x02, 0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF]
        );
    }

    // TextInput{utf8:"hi"} 推导 (blob = varint 长度 + 原始字节):
    //   len=2      -> varint 单字节 (2 < 2^6) [0x02]
    //   b'h'=0x68, b'i'=0x69
    // 合计 3 字节
    #[test]
    fn text_input_hand_computed_bytes() {
        let v = TextInput { utf8: b"hi".to_vec() };
        assert_eq!(v.encode(), vec![0x02, 0x68, 0x69]);
    }

    // ---- 错误路径: 截断 / 尾部多余字节 ----
    #[test]
    fn decode_truncated() {
        // 少于 9 字节读不完 x/y/buttons
        assert_eq!(PtrMove::decode(&[0x01, 0x00]).unwrap_err(), WireError::Truncated);
        // 少于 4 字节读不完 dx/dy
        assert_eq!(PtrScroll::decode(&[0x01]).unwrap_err(), WireError::Truncated);
        // blob 声明 len=5 但只给 2 字节
        assert_eq!(
            TextInput::decode(&[0x05, 0x61, 0x62]).unwrap_err(),
            WireError::Truncated
        );
    }

    #[test]
    fn decode_trailing_bytes() {
        // 9 字节合法 PtrMove 后多 1 字节
        let mut buf = vec![0x01, 0x00, 0x00, 0x00, 0xFE, 0xFF, 0xFF, 0xFF, 0x03];
        buf.push(0xAA);
        assert_eq!(PtrMove::decode(&buf).unwrap_err(), WireError::TrailingBytes);
    }
}
