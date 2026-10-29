# hidpipe

跨机 HID 事件管道:Linux 捕获 → macOS 注入。名字即架构——**HID usage ID 在管道里流动**,仅此而已。

为解决 deskflow/synergy 系的三个结构性痛点而生:

1. **换键薛定谔**:keysym 翻译 + 跨机 keymap 协商是两个会漂移的状态机;
2. **延迟**:翻译层与事件循环排队;
3. **异常补救弱**:无心跳自愈、无防卡键安全阀。

## 架构

```mermaid
flowchart LR
    subgraph Linux["Linux (server: 键盘鼠标所在机)"]
        K[物理键鼠] --> KD[keyd: 改键唯一发生地]
        KD -->|evdev 原始事件| S[hidpipe-server<br/>EVIOCGRAB + 热键切换]
    end
    subgraph Wire["线上协议: 无语义"]
        S -->|"HID usage ID 帧<br/>TCP_NODELAY 明文"| C
    end
    subgraph macOS["macOS (client)"]
        C[hidpipe-client<br/>CGEventPost @ HID tap] --> SYS[系统]
        KB[Karabiner] -.->|驱动层, 看不到注入事件| SYS
    end
```

三条设计不变量:

1. **协议无语义**:线上只有原始 HID 事件。没有 keymap 协商,没有修饰键状态机,没有缓存——没有能漂移的东西。
2. **换键恰好一次,位置固定**:改键全部发生在捕获侧的 keyd(支持 `overload(control, esc)` 这类状态性改键,解析完才上线)。macOS 注入在 Quartz 层,结构性绕过 Karabiner,不会双重映射,也不依赖对端任何配置。
3. **断线即释放**:连接死亡 ⇒ server 立即 ungrab;client Drop 时释放所有按住的键。grab/按键状态随连接生灭,不留残渣。

## 构建

```bash
# Linux(捕获端)
cargo build --release -p hidpipe-server

# macOS(注入端,在 Mac 上构建)
cargo build --release -p hidpipe-client
```

也可以用 `nix develop` 拿到 Rust 工具链。

## 运行

**Linux**(需要读 `/dev/input/event*` 的权限,`input` 组即可):

```bash
hidpipe-server                    # 自动探测设备,keyd 虚拟设备优先
hidpipe-server --keyboard "keyd" --mouse "MX Master" --port 24800
```

**macOS**:

```bash
hidpipe-client <linux-ip>         # 断线每秒重试,直到成功
```

首次运行需在 **系统设置 → 隐私与安全性 → 辅助功能** 授权运行它的终端(iTerm/Terminal 等)。

**使用**：把光标推到 Linux 屏幕**右缘**即切入远程(Layer-shell 屏障,compositor 背书的精确检测);在 Mac 上把光标**顶住左缘继续左推**即交还控制。`ScrollLock` 等热键(`--switch-key`)作为备用始终有效。远程模式下 Linux 侧光标冻结(设备被独占),所有输入发往 Mac。

## 参数

| server | 默认 | 说明 |
|---|---|---|
| `--bind` / `--port` | 0.0.0.0 / 24800 | 监听地址与端口 |
| `--keyboard` / `--mouse` | 自动探测 | 设备名子串过滤 |
| `--switch-key` | scrolllock | 备用切换键,可选名见 `--help` |
| `--no-barrier` | 关 | 禁用屏幕右缘屏障,只用热键 |
| `--probe` | 关 | 探键模式:按什么打印什么,用于选切换键 |
| `--swap-caps-esc` | 关 | 捕获侧补做 caps↔esc(仅当互换不在 keyd 层时) |

| client | 默认 | 说明 |
|---|---|---|
| `--port` | 24800 | 服务器端口 |
| `--flip-scroll` | 关 | 反转滚轮方向 |

## 已知限制(v0,周末原型)

- **明文 TCP**,仅可信 LAN;TLS 在 roadmap。
- 媒体键(音量/播放)未映射——它们在 HID consumer page,不在 keyboard page。
- macOS 的 **SecureEventInput**:密码框聚焦时系统丢弃一切合成事件。这是平台策略,任何注入式方案(deskflow 同)都绕不过。
- 鼠标钳制在主屏;多屏支持在 roadmap。
- 边缘屏障建在**每个输出的右缘**;多显示器且右侧还有显示器时,内接缝也会触发(指定输出的开关在 roadmap)。
- 返程方向(Mac → Linux)只检测 Mac **主屏左缘**。
- 键鼠热插拔未处理(读取失败会回本地模式并打日志,重启 server 恢复)。
- 鼠标侧键(前进/后退)未转发。

## Roadmap

- [ ] TLS(自签证书对,fingerprint 认证)
- [ ] WoL / mDNS 心跳——server 主动唤醒 client 方向
- [ ] nix 模块:systemd service(Linux)+ launchd agent(macOS)
- [ ] 多屏:屏障按输出名配置;Mac 侧多屏钳制
- [ ] 切入远程时按 Y 比例映射 Mac 光标初始位置
- [ ] 媒体键(consumer page)
- [ ] 剪贴板同步(谨慎,Worst case 先不做)

**为什么又是边缘切换了**：最初否决的是“位移累加估算光标位置”的伪边缘检测(必漂移)。现在的实现是两条确定性路径：正向靠 layer-shell 屏障(compositor 亲自发 enter 事件),返程靠 client 端精确的光标钳制状态。没有任何估算成分。热键作为备用通道保留。
