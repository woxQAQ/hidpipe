//! 单客户端 TCP 连接的读写线程。
//!
//! 保活策略:写线程每 2s 发 Ping;读线程 6s 读不到任何帧即判死,
//! 上报 `NetMsg::Dead(连接代数)`。主循环凭代数忽略旧连接迟到的死讯。

use std::io;
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use hidpipe_proto as proto;
use proto::Frame;

const PING_INTERVAL: Duration = Duration::from_secs(2);
const READ_TIMEOUT: Duration = Duration::from_secs(6);

pub enum NetMsg {
    /// 候选连接的首帧是版本匹配的 Hello,可上岗。
    Hello(u64),
    /// client 请求交还控制(光标顶到 Mac 左缘)。
    ReturnToLocal(u64),
    /// 连接已死(读超时/写失败/对端关闭/握手非法),携带连接代数。
    Dead(u64),
}

pub struct Connection {
    pub id: u64,
    tx: Sender<Frame>,
}

impl Connection {
    pub fn spawn(stream: TcpStream, net_tx: Sender<NetMsg>, id: u64) -> io::Result<Self> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        let reader = stream.try_clone()?;
        let (tx, rx) = mpsc::channel::<Frame>();
        thread::spawn({
            let net_tx = net_tx.clone();
            move || writer_loop(stream, rx, net_tx, id)
        });
        thread::spawn({
            let tx = tx.clone();
            move || reader_loop(reader, tx, net_tx, id)
        });
        Ok(Self { id, tx })
    }

    /// 发送失败说明对端已死,读/写线程会随后上报 Dead,这里静默丢弃。
    pub fn send(&self, frame: Frame) {
        let _ = self.tx.send(frame);
    }
}

// 主循环丢弃 Connection 即关闭 channel,写线程随之退出;
// 读线程最迟在一个读超时(6s)后退出。

fn writer_loop(mut stream: TcpStream, rx: Receiver<Frame>, net_tx: Sender<NetMsg>, id: u64) {
    loop {
        let frame = match rx.recv_timeout(PING_INTERVAL) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Timeout) => Frame::Ping,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if proto::write_frame(&mut stream, &frame).is_err() {
            let _ = net_tx.send(NetMsg::Dead(id));
            return;
        }
    }
}

fn reader_loop(mut stream: TcpStream, tx: Sender<Frame>, net_tx: Sender<NetMsg>, id: u64) {
    // 首帧必须是版本匹配的 Hello,否则立即判死。
    // 未通过握手的连接无权顶掉已上岗的连接——这是协议级准入控制,
    // 防止同端口的旧协议客户端(如 deskflow 的重试循环)挤掉正常连接。
    match proto::read_frame(&mut stream) {
        Ok(Frame::Hello { version }) if version == proto::VERSION => {
            let _ = net_tx.send(NetMsg::Hello(id));
        }
        _ => {
            let _ = net_tx.send(NetMsg::Dead(id));
            return;
        }
    }
    loop {
        match proto::read_frame(&mut stream) {
            Ok(Frame::Ping) => {
                let _ = tx.send(Frame::Pong);
            }
            Ok(Frame::ReturnToLocal) => {
                let _ = net_tx.send(NetMsg::ReturnToLocal(id));
            }
            // Hello / Pong 等其余帧不影响主循环,忽略。
            Ok(_) => {}
            Err(_) => {
                let _ = net_tx.send(NetMsg::Dead(id));
                return;
            }
        }
    }
}
