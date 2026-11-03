# hidpipe 与 Deskflow 的输入延迟处理对照

## 结论与证据边界

Deskflow 值得借鉴的两项具体机制是：收到输入消息后回 Noop，缓解历史上 Linux → macOS 路径的 delayed ACK 问题；在接收端已有积压时合并鼠标移动，减少重复注入。hidpipe 已经启用 TCP_NODELAY，但没有这两项机制。启用 OpenLogi 时，hidpipe 还增加了逐事件的同步 Unix socket 往返。

以上是源码确认的差异，不能直接认定为用户当前延迟的根因。本次没有测量 Mac 注入耗时或应用显示延迟，也没有修改输入处理、网络协议或运行中的服务。

调查使用本地 Deskflow checkout，commit `2877600facc0cf1b38b7f50b51cdc8fe20052bf2`；未声称它是上游最新版本。

## Deskflow 如何处理

### 1. TCP_NODELAY 之外，每条正常消息都回 Noop

`ServerProxy::handleMessage` 在解析成功后写入 `kMsgCNoop`。作者明确解释：Linux server → OS X client 曾出现 delayed ACK 引起的延迟，因此用应用层回复提供可携带 ACK 的数据包；注释也明确承认尚不清楚为什么 TCP_NODELAY 没能避免问题。因此这是有历史经验支持的 workaround，不是已经证明适用于所有网络的因果解释。

来源：[ServerProxy.cpp:200–215](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/client/ServerProxy.cpp#L200-L215)。TCP_NODELAY 设置见 [TCPSocket.cpp:285](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/net/TCPSocket.cpp#L285)。

### 2. 有积压才合并移动，不固定等待一批事件

接收循环处理所有当前可读消息，结束时 `flushCompressedMouse`。移动处理发现后面还有输入，就暂存移动：绝对坐标保留最新位置，相对坐标累计 dx/dy。按键、鼠标按钮、滚轮等处理前也会 flush，保留操作顺序。没有为了凑一批而固定等待 8/16 ms。

来源：[接收循环](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/client/ServerProxy.cpp#L80-L123)、[flush](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/client/ServerProxy.cpp#L373-L385)、[移动与滚轮](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/client/ServerProxy.cpp#L651-L728)。

### 3. macOS 仍然通过 Quartz HID tap 注入

鼠标移动创建 CGEvent、设置 flags 和 delta，随后 `CGEventPost(kCGHIDEventTap, event)`。绝对移动维护自己的光标位置；相对移动路径会读取系统位置。hidpipe 已经自行跟踪位置，并使用相同的 HID tap 注入，没有证据表明换一个注入层就能解决延迟。

Deskflow 还设置 local event suppression interval，但相关注释针对本地硬件事件被抑制、交还本机时光标迟疑，不应把这个设置理解为远程注入队列的通用加速开关。

来源：[OSXComputer.mm:435–615](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/platform/OSXComputer.mm#L435-L615)、[suppression 设置与注释](https://github.com/deskflow/deskflow/blob/2877600facc0cf1b38b7f50b51cdc8fe20052bf2/src/lib/platform/OSXComputer.mm#L1903-L1935)。

## hidpipe 中的对应问题

| 环节 | 当前行为 | 延迟含义 |
|---|---|---|
| Linux 主循环 | 每轮 drain 后 sleep 1 ms | 引入轮询等待及调度抖动；sleep 不保证恰好 1 ms |
| 网络 writer | 无界 mpsc 队列，逐帧写出，TCP_NODELAY | 阻塞时可能积压旧输入；目前未测得队列长度 |
| Mac 接收 | 逐帧读取并立即处理 | 已积压的移动也逐个解析、请求 agent、注入 |
| OpenLogi resolver | 每个 PointerEvent 写给本地 agent，再阻塞读一字节 disposition | 每个事件增加一次同步 IPC 往返；期间后续键盘、鼠标和 Ping 都无法处理 |
| Mac motion | 每次获取主屏 bounds | 可测量并考虑缓存，需保留显示配置变化处理；尚无耗时证据 |

源码：[`server/src/main.rs`](../server/src/main.rs) 主循环与 `drain_mouse`；[`server/src/net.rs`](../server/src/net.rs) `Connection::spawn` / `writer_loop`；[`client/src/main.rs`](../client/src/main.rs) `session` / `Injector::motion`；[`proto/src/bridge.rs`](../proto/src/bridge.rs) `Resolver::resolve`。

OpenLogi agent 的实际构建来源是 [`nix/openlogi-package.nix`](../nix/openlogi-package.nix) 应用的 [`nix/openlogi-bridge.patch`](../nix/openlogi-bridge.patch)，其中 `resolve_connection` 同样确认处理一个事件后回复一字节。agent 的 effects 另行排队执行，不能据此声称 resolver 等待所有动作执行完成。500 ms 是 resolver 的超时上限，并不是正常事件固定延迟；capture 的 20 ms recv_timeout 在事件到达时会被唤醒，也不是每个事件固定等待 20 ms。

不能直接绕过所有 Motion 的 resolver：agent 用普通移动识别可配置按钮手势并决定是否吞掉移动。若优化普通移动路径，需明确定义手势状态由谁维护、何时切换，以及事件顺序。

## 当前运行时观察

调查时 `systemctl --user status hidpipe` 显示服务正在运行，参数包含 `--openlogi-socket /home/woxQAQ/.cache/openlogi-hidpipe/pointer.sock`，因此上面的额外 IPC 路径与当前配置有关。Nix store 二进制是否逐字对应当前工作树，本次未验证。

`ss -tinp '( sport = :24800 )'` 的单次快照：

- 连接 Linux `192.168.31.106` → Mac `192.168.31.240`。
- `rtt:9.691/8.286`，`minrtt:2.311`，单位 ms；这是 TCP 估算值，不是端到端输入延迟。
- `bytes_retrans:1137`，`retrans:0/12`，`dsack_dups:11`。这些说明有重传/重复接收相关记录，不能单独区分丢包、重排或伪重传。
- 当时 `Recv-Q=0`、`Send-Q=0`；这不能排除间歇积压或应用层队列积压。
- `ato:40` 是 Linux 本地接收侧 ACK 定时信息，不能拿它证明 Mac 的 delayed ACK 是 40 ms。

## 建议实施顺序

1. 先加低开销、按时间窗口汇总的测量：Linux 捕获到写出的排队时间、Mac resolver 往返时间、CGEvent 创建/注入调用耗时、处理事件数。跨机器单向耗时不能直接相减未同步的时钟；CGEventPost 返回也不代表应用已经处理事件。
2. 对照测试 Deskflow 的逐消息回复策略。hidpipe 已有 Pong，server 会忽略它，可考虑实验性复用而不新增 wire tag；明确它的语义，并检查回复流量与持续输入下的效果。只有对照证据支持后才作为正式修复。
3. 接收端对当前已就绪的连续移动做有界合并，不增加等待窗口；相对移动累计位移，遇到控制事件及时 flush。需要验证边缘返回、屏幕边界 clamp 和手势路径：简单累加可能改变往返越界轨迹，手势输入也不能未经论证就合并。读取就绪不等于完整帧到齐，避免依赖 peek 后直接阻塞读取不完整帧。
4. 通过测量决定是否重构 OpenLogi resolver 为有序批处理/流水线，或共享明确的路由状态。保留绑定决定在注入之前、断线释放和手势一致性。
5. 将 Linux 的固定 sleep 轮询替换为 readiness 等待，并保留 heartbeat、barrier 与连接通知的及时唤醒。它能去掉固定轮询等待，但未证明是当前主要瓶颈。

## 已实现的测量与对照开关

后续实现新增了 `--latency-stats`（两端）和 `--input-feedback`（Mac），对应 Nix 选项为 `latencyStats` 和 `inputFeedback`，均默认关闭。逐输入回复使用现有 Pong，在 OpenLogi 解析和注入之前写出；不回复 Pong，不改变协议版本。客户端 write timeout 为 6 秒，避免新增回复路径无限阻塞。

统计分开记录 Linux writer 排队/写入、Mac resolver 往返/输入处理/回复写入/整帧处理，按 5 秒窗口输出样本数、均值和最大值。没有跨机器相减时钟，也不把 CGEventPost 返回解释为应用已经收到输入。Linux 的统计从 enqueue 开始，尚不覆盖捕获前的轮询等待。

客户端网络循环已提取到 `client/src/transport.rs`，通过 TCP 和 Unix socket 集成测试运行同一实现：验证先回复再等待 resolver/注入、关闭回复开关、忽略收到的 Pong、绑定吞掉移动以及 ReleaseAll 顺序。Linux release 构建、7 项测试、Clippy、Nix 原生包构建均通过。完整 macOS 客户端交叉检查受限于本机缺少 Darwin Rust 标准库，Quartz 路径仍需 Mac 构建和实际体验验证。

本轮未实施移动合并、resolver 流水线或 Linux readiness 重构，也未重启运行中的服务。使用说明见 [README 的 Latency diagnostics](../README.md#latency-diagnostics)。

README 关于 Deskflow“翻译层和事件循环增加延迟”的陈述没有附实测数据；本次源码调查不能支持 hidpipe 天然更快的结论。
