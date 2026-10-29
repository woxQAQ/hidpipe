//! hidpipe 线上协议 v0。
//!
//! 协议只承载原始 HID 事件:键盘为 USB HID keyboard page (0x07) 的
//! usage ID,鼠标为相对位移与按键编号。协议不理解 keymap / keysym,
//! 也不做 keymap 协商——翻译只发生在两端本地的静态查表。
//! 这是整个系统确定性的来源:线上没有任何会漂移的状态机。
//!
//! 帧格式:`[tag: u8][payload]`,多字节字段大端。v0 为明文 TCP,
//! 仅用于可信 LAN。

#![forbid(unsafe_code)]

use std::io::{self, Read, Write};

/// 协议版本。握手帧携带,不一致即拒绝,避免沉默的语义错位。
pub const VERSION: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// 客户端连上后发送的第一帧。
    Hello { version: u8 },
    /// 键盘事件。usage 为 USB HID page 0x07 usage ID。
    Key { usage: u16, state: KeyState },
    /// 一个上报周期内累加的相对位移。
    Motion { dx: i16, dy: i16 },
    Button { button: MouseButton, state: ButtonState },
    /// 滚轮。dy > 0 表示滚轮向远离用户方向滚动(内容上滚)。
    Wheel { dx: i8, dy: i8 },
    /// 要求对端释放所有按住的键与按钮。
    /// 切回本地或断线前发送,防止对端残留卡住的修饰键。
    ReleaseAll,
    Ping,
    Pong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyState {
    Release = 0,
    Press = 1,
    /// 长按产生的自动重复。对端据此打 autorepeat 标记,
    /// 与真实按键重复行为对齐。
    Repeat = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MouseButton {
    Left = 0,
    Right = 1,
    Middle = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ButtonState {
    Release = 0,
    Press = 1,
}

const TAG_HELLO: u8 = 0x00;
const TAG_KEY: u8 = 0x01;
const TAG_MOTION: u8 = 0x02;
const TAG_BUTTON: u8 = 0x03;
const TAG_WHEEL: u8 = 0x04;
const TAG_RELEASE_ALL: u8 = 0x05;
const TAG_PING: u8 = 0x06;
const TAG_PONG: u8 = 0x07;

/// 各 tag 对应的定长 payload 长度;未知 tag 返回 None。
pub const fn payload_len(tag: u8) -> Option<usize> {
    match tag {
        TAG_HELLO => Some(1),
        TAG_KEY => Some(3),
        TAG_MOTION => Some(4),
        TAG_BUTTON | TAG_WHEEL => Some(2),
        TAG_RELEASE_ALL | TAG_PING | TAG_PONG => Some(0),
        _ => None,
    }
}

pub fn encode(frame: &Frame) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    match *frame {
        Frame::Hello { version } => out.extend([TAG_HELLO, version]),
        Frame::Key { usage, state } => {
            out.push(TAG_KEY);
            out.extend(usage.to_be_bytes());
            out.push(state as u8);
        }
        Frame::Motion { dx, dy } => {
            out.push(TAG_MOTION);
            out.extend(dx.to_be_bytes());
            out.extend(dy.to_be_bytes());
        }
        Frame::Button { button, state } => out.extend([TAG_BUTTON, button as u8, state as u8]),
        Frame::Wheel { dx, dy } => out.extend([TAG_WHEEL, dx as u8, dy as u8]),
        Frame::ReleaseAll => out.push(TAG_RELEASE_ALL),
        Frame::Ping => out.push(TAG_PING),
        Frame::Pong => out.push(TAG_PONG),
    }
    out
}

pub fn decode(tag: u8, payload: &[u8]) -> Option<Frame> {
    if payload_len(tag)? != payload.len() {
        return None;
    }
    Some(match tag {
        TAG_HELLO => Frame::Hello { version: payload[0] },
        TAG_KEY => Frame::Key {
            usage: u16::from_be_bytes([payload[0], payload[1]]),
            state: match payload[2] {
                0 => KeyState::Release,
                1 => KeyState::Press,
                _ => KeyState::Repeat,
            },
        },
        TAG_MOTION => Frame::Motion {
            dx: i16::from_be_bytes([payload[0], payload[1]]),
            dy: i16::from_be_bytes([payload[2], payload[3]]),
        },
        TAG_BUTTON => Frame::Button {
            button: match payload[0] {
                0 => MouseButton::Left,
                1 => MouseButton::Right,
                _ => MouseButton::Middle,
            },
            state: match payload[1] {
                1 => ButtonState::Press,
                _ => ButtonState::Release,
            },
        },
        TAG_WHEEL => Frame::Wheel {
            dx: payload[0] as i8,
            dy: payload[1] as i8,
        },
        TAG_RELEASE_ALL => Frame::ReleaseAll,
        TAG_PING => Frame::Ping,
        TAG_PONG => Frame::Pong,
        _ => return None,
    })
}

fn invalid(msg: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// 从流中读一帧。读超时或连接中断以 Err 上抛,由调用方判死。
pub fn read_frame(r: &mut impl Read) -> io::Result<Frame> {
    let mut tag = [0u8; 1];
    r.read_exact(&mut tag)?;
    let len = payload_len(tag[0]).ok_or(invalid("unknown frame tag"))?;
    let mut payload = [0u8; 8];
    r.read_exact(&mut payload[..len])?;
    decode(tag[0], &payload[..len]).ok_or(invalid("malformed frame"))
}

pub fn write_frame(w: &mut impl Write, frame: &Frame) -> io::Result<()> {
    w.write_all(&encode(frame))
}
