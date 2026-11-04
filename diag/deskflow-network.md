# Deskflow 网络路径

本次核对本地 Deskflow checkout `2877600facc0cf1b38b7f50b51cdc8fe20052bf2`，不代表上游最新版本。本文解释实现，不宣称已证明当前 hidpipe 卡顿的根因。

## 连接与协议

输入通过 TCP 长连接传输，默认端口 24800。客户端连接服务端，双方通过协议握手协商版本和屏幕信息。加密路径使用 SecureSocket/TLS，非加密路径使用 TCPSocket。TLS 实现禁止 TLS 1.2 之前的协议，含证书指纹验证逻辑；具体认证行为取决于安全配置。

来源：[协议常量](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/deskflow/ProtocolTypes.h)、[Client](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/client/Client.cpp)、[ClientListener](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/server/ClientListener.cpp)、[SecureSocket](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/net/SecureSocket.cpp)。

应用帧为 4 字节大端 payload 长度，随后是 payload；消息类型是 4 字节代码，例如 DMMV（绝对移动）、DMRM（相对移动）、DMWM（滚轮）、CNOP（空回复）。长度头解决 TCP 字节流的消息边界问题，不代表一次 write 就是一个 TCP 包。PacketStreamFilter 只有缓冲完整 payload 后才报告 ready，因此上层 drain 不必等半帧补齐。

来源：[PacketStreamFilter](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/deskflow/PacketStreamFilter.cpp)、[消息代码](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/deskflow/ProtocolTypes.cpp)。

## 异步读写

业务调用 TCPSocket::write 时，把字节加入 output buffer；缓冲原先为空时注册可写任务。独立 SocketMultiplexer 线程通过 pollSocket 等待 readiness，再执行实际 socket 读写。部分写成功时仅弹出已发送字节；读取时尽量吸收当前可读数据，通过事件队列通知上层。这种结构隔离业务处理与 socket readiness 等待，但发送缓冲仍可能积压，不能保证零延迟。

来源：[TCPSocket::write / doRead / doWrite / newJob](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/net/TCPSocket.cpp)、[SocketMultiplexer::serviceThread](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/net/SocketMultiplexer.cpp)。

## 延迟与积压处理

- TCPSocket::init 启用 TCP_NODELAY，关闭 Nagle；也启用 SO_KEEPALIVE。TCP_NODELAY 不会关闭对端 delayed ACK，也不会消除重传、拥塞或线程调度延迟。
- 客户端 ServerProxy::handleMessage 在正常消息解析处理成功后写 CNOP。源码注释明确说这是 Linux server → OS X client 的 delayed ACK workaround，并承认不清楚 TCP_NODELAY 开启后为何仍遇到该问题。服务端忽略 CNOP，发送输入不必等待逐条 CNOP，因而不是每个输入等待一次 RTT 的 stop-and-wait。
- 接收端 drain 当前完整消息。若移动后还有完整消息，绝对坐标只保留最新值，相对坐标累加；处理按键、按钮、滚轮等事件前或 drain 结束时 flush。没有固定等待 8/16 ms 凑批次，合并也不能消除已经发生的网络传输延迟。

来源：[TCPSocket::init](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/net/TCPSocket.cpp#L272-L291)、[ServerProxy](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/client/ServerProxy.cpp#L80-L215)、[移动合并](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/client/ServerProxy.cpp#L651-L728)、[服务端发送与 CNOP 处理](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/server/ClientProxy1_0.cpp)。

## 保活

较新协议路径 ClientProxy1_3 使用周期定时器发送 CALV，客户端回 CALV 并重置存活计时器。默认 keepalive 周期 3 秒，死亡阈值为周期的 3 倍；参数可由选项覆盖。这与每条消息的 CNOP 回复用途不同，也与内核 SO_KEEPALIVE 不同。

来源：[ClientProxy1_3](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/server/ClientProxy1_3.cpp)、[常量](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/deskflow/ProtocolTypes.h#L60-L108)、[客户端处理](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/client/ServerProxy.cpp)。

## 与 hidpipe 的对应

hidpipe 已启用 TCP_NODELAY，并有独立 writer 线程；Mac 的 inputFeedback 选项逐输入先写 Pong，再进行 resolver 和注入，借鉴的是主动回复思路，时序不同于 Deskflow 的 parseMessage 后回复。当前 hidpipe 尚未实现 Deskflow 式接收端移动合并。相关实现与证据边界见 [延迟对照调查](deskflow-latency.md)、[transport](../client/src/transport.rs)、[server net](../server/src/net.rs)。

本次读取到系统配置 `outputs/woxDarwin.nix` 中 inputFeedback = true、latencyStats = true；这只证明声明配置，不证明 Mac 上正在运行的进程已应用。
