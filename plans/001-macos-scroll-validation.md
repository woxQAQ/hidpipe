# Plan 001: 建立 macOS 小数滚轮的真实注入验证基线

> 执行者：完整阅读本计划，按步骤验证。遇到 STOP 条件就记录证据并停止，不自行改生产滚轮算法。完成后更新 `plans/README.md`；只有实际取得 Mac 证据才可标记 DONE。
>
> Drift check：`git diff --stat 5e0f7ff..HEAD -- client/src/main.rs client/src/transport.rs proto/src/lib.rs proto/examples/scroll_fixture.rs diag/scroll.swift diag/scroll-verify.swift diag/scroll-validation.md`。若相关文件变化，逐项比较下方摘录；不一致则报告需要刷新计划。

## Status

- Priority: P1
- Effort: M
- Risk: LOW（诊断工具会真实滚动当前应用，必须显式启动）
- Depends on: none
- Category: tests / direction
- Planned at: commit `5e0f7ff`, 2026-11-06
- Status: BLOCKED（工具实现与 Linux 复核完成；缺少 Mac 实测）

## 执行与复核记录（2026-11-06）

- 隔离执行提交 `52e3fed`，新增且仅新增 scope 内三个工具/文档文件；审查后批准集成。
- reviewer 完整阅读 fixture、Swift verifier 和验证文档；独立复跑 `cargo test -p hidpipe-proto`（11 个集成测试）、fixture cargo check、Clippy `-D warnings`、rustfmt，全部通过。
- reviewer 独立执行临时 TCP boundary harness，35 组通过，覆盖所有序列、两轴、心跳/百万 Pong drain、EOF、断连、非法帧与阶段期限。该 harness 为临时验证记录，不是持久测试入口。
- 实现者已记录发送前延迟检查和 Nushell 文档检查的结果。fixture 额外支持 `--start-delay-ms` 和有限阶段超时，不改生产算法。
- 首次实现验证仅在 Linux 完成。后续已通过 SSH 连接 `woxqaq@192.168.31.240`（macOS 27.0.1、Swift 6.4），对当前 verifier 执行 `swiftc -typecheck -` 和 `swift - --help`，均 exit 0。
- 远程只读 HID tap 试运行返回 exit 1、summary `reasons=["tap is not enabled"]`，未进行滚轮注入。随后用户从 Mac 桌面终端运行并保存 `/tmp/hidpipe-scroll.i6IFkQ/log.log`；读取到 count=0、约 2 秒、`pass=true`，确认该桌面终端的监听启动检查通过，尚不证明真实滚轮事件到达。
- 已在 Mac 临时目录编译原生 fixture。SSH 启动同一路径的已签名 client 新实例被 macOS 拒绝注入权限，已停止；桌面终端测试实例也出现权限等待。后续使用独立临时 GUI launchd job 启动测试客户端，已验证其能回复 Ping；原有服务未重启。
- 首次真实 Mac 测试定位到 fixture 的跨平台错误：Darwin 接受的 socket 继承非阻塞状态，回复线程提前 WouldBlock/error35。执行者已在 accept 后显式恢复 blocking，未改生产代码。reviewer 在 Mac 编译修正版，并通过五轮 Enter 前 Ping/Pong、四次 `(0,30)`、ReleaseAll 和 exit0 的真实 TCP mock 验证。
- 首组真实 HID tap 输入验证通过：临时 GUI launchd client PID `98130`，原生 `Wheel`、Y 轴、scale=1，`quarters` `[30,30,30,30]`、100 ms 间隔。`/tmp/hidpipe-scroll.i6IFkQ/quarters-fixed.log` 捕获四次 fixedY=0.25，总 fixedY=1、fixedX=0，count=4，summary pass=true，无多余事件。四次 lineY/pointY 均为0，continuous=0、phase/momentum=0。fixture COMPLETE sent=4 total=120，正常退出。
- 此结果证明该序列的小数行到达真实 HID tap，不证明应用有响应，也不代表 whole 对照、倍率/方向/另一轴或完整矩阵已通过。应用消费等待用户观察；计划仍为 BLOCKED。用户此前的非零监听因输入源提前退出或 PID 错误而超时，不作为滚轮算法失败证据。
- Mac 已运行 `/Users/woxqaq/Applications/Hidpipe.app/Contents/MacOS/hidpipe-client`，其 `--help` 包含现有 scale/invert 参数。Mac checkout `/Users/woxqaq/git-for-agents/hidpipe` 为 `365bdd8` 且有未提交修改，未覆盖或部署。当前 Linux 的地址为 `192.168.31.106`，不是 Mac。首次工具交付的记录与完整复现流程仍见 `diag/scroll-validation.md`。

## 为什么做

hidpipe 保留每格 120 单位的高精度输入，在 macOS 换算为小数行。代码会先以整数构造 Quartz LINE 事件，再写 fixed-point 字段；尚无证据证明所有目标应用都正确消费小数行。需要将传输、Quartz 到达和应用响应分开验证，才能判断后续是否应修正注入或增加离散模式。

Deskflow 本地 checkout `2877600` 的 EI 路径先累计整格，macOS 再转整数行并截断缩放结果。它不是高精度正确性的基准：倍率 1.5 下两个单格事件得到 2 行，一个双格事件得到 3 行。本计划验证 hidpipe 自己的累计量，不复制该行为。

## 当前状态与约束

- Rust workspace：`proto`、Linux-only `server`、macOS-only `client`；不要在 Linux 声称验证了 Quartz。
- `README.md` 约定协议只携带原始输入，目的端解析鼠标绑定；滚轮单位保持每格 120，不新增协议字段。
- `proto/src/lib.rs:27` 当前 `VERSION = 2`，须引用常量，不从 README 的旧版本文字推导协议。
- `proto/src/lib.rs:31`：`Hello { version: u8, openlogi: bool }`；`Wheel { dx: i32, dy: i32 }`；提供 `read_frame`、`write_frame`。
- `client/src/main.rs:153` 连接后先发送 Hello，不等待服务端 Hello；随后进入实际 `transport::run`。
- `client/src/main.rs:434` 的生产路径摘录：

```rust
let (dx, dy) = self.scroll.apply(dx, dy);
CGEvent::new_scroll_event(
    self.source.clone(), ScrollEventUnit::LINE, 2,
    dy as i32, dx as i32, 0,
)
```

随后 `set_double_value_field` 设置 `SCROLL_WHEEL_EVENT_FIXED_POINT_DELTA_AXIS_1/2`，带上 held modifier flags 并 post 到 HID tap。`Frame::Wheel` 与普通 OpenLogi `PointerEvent::Wheel` 都进入该函数；HID++ `ThumbWheel` 由 OpenLogi 独立处理，不在本次范围。

- `client/src/main.rs:637` 已有 `--scroll-scale-x/-y`、`--invert-scroll-x/-y`。倍率要求 finite、非负；0 合法。
- `client/src/transport.rs:44` 接收 Ping 回复 Pong；`--input-feedback` 会对每个输入也回 Pong，fixture 不可因没读回复而阻塞。
- 现有诊断风格：`server/examples/scroll.rs` 是显式运行的 example，`io::Result` 错误向上返回；`diag/scroll.swift` 用 listen-only HID event tap，输出 line/point/fixed/continuous/phase/momentum/PID。复用这种风格。
- `proto/tests/client_transport.rs` 用真实 TCP peer 驱动客户端 transport，是边界测试范例。不要为数值转换函数编写镜像式单元测试。

## 范围

只允许新增/修改：

- `proto/examples/scroll_fixture.rs`（新增）
- `diag/scroll-verify.swift`（新增）
- `diag/scroll-validation.md`（新增，记录程序用法及实测结论）
- `plans/README.md`（状态）；本计划只可补充执行证据。

只读参考：`client/src/main.rs`、`client/src/transport.rs`、`proto/src/lib.rs`、`diag/scroll.swift`。

禁止修改生产注入、server、协议、Cargo 依赖/锁文件、OpenLogi patches、系统配置、服务权限。不要增加平滑动画、惯性或离散输出模式。

## 命令与环境

以下为独立外部命令，可在 Nushell 运行；不要让操作者临时改 shell。

| 环境 / 目的 | 命令 | 成功标准 |
|---|---|---|
| Linux/macOS，协议回归 | `cargo test -p hidpipe-proto` | exit 0；这是既有仓库命令，本次规划未运行 |
| Linux/macOS，编译新增 fixture | `cargo check -p hidpipe-proto --example scroll_fixture` | exit 0 |
| macOS，编译实际客户端 | `cargo build -p hidpipe-client` | exit 0；现有 README 的 native build 路径 |
| macOS，Swift 类型检查 | `swiftc -typecheck diag/scroll-verify.swift` | exit 0 |
| fixture 帮助 | `cargo run -p hidpipe-proto --example scroll_fixture -- --help` | exit 0，列出参数、case 和启动后会滚动当前应用 |
| 新监听器帮助 | `swift diag/scroll-verify.swift --help` | exit 0，无需创建 event tap |

后四种新增工具命令是本计划要求实现的接口，目前尚不存在。缺少工具链时可使用仓库 `nix develop`，不要安装系统软件或更新 flake.lock。

## Git workflow

执行应使用隔离分支 `advisor/001-scroll-validation`。现有 commit history 是简短描述，如 `update readme`；若需要提交，使用简短描述 `add scroll validation probe`。未经授权不 push、不创建 PR、不合并。

## 步骤

### 1. 增加可控的真实协议输入源

在 `proto/examples/scroll_fixture.rs` 实现一个一次运行、一次连接的 TCP server，只绑定 `127.0.0.1`，默认端口 24802；必须允许 `--port`、`--case`、`--axis x|y`、`--interval-ms`，以及 `--help`。case 名称与序列固定为：

- `whole`: `[120]`
- `quarters`: `[30, 30, 30, 30]`
- `two-whole`: `[120, 120]`
- `double`: `[240]`
- `tiny`: `[1, 119]`
- `reverse`: `[30, -30]`
- `negative`: `[-30, -30, -30, -30]`

默认 interval 20 ms，允许 5/100 ms 对照，输入非法时非零退出。每次连接读并校验 `Hello { version: VERSION, openlogi: false }`；不向 client 发 Hello。握手后打印 READY 并等待操作者在 fixture 终端按 Enter，此时可取得 client PID 并启动监听器；等待期间每 250 ms 发送 Ping，后台持续 drain 回复，避免客户端 read timeout。Enter 后用库的 `write_frame` 写 `Frame::Wheel`，未选轴为 0；序列结束后发送 ReleaseAll 并关闭。设置 TCP_NODELAY 和有限读写超时，接收方向需持续 drain Pong/ReturnToLocal 等合法帧，避免 feedback 开启时堆积；peer 非法帧或中途断开应失败。不得发键盘、Motion 或按钮事件，也不抓取任何设备。

记录 case、每个发送量、合计、间隔，禁止把 client 回复计作输入。测试分批等价时要保留原始事件边界，不在 fixture 里合并。

**Verify**：运行 fixture 的 check 和 `--help` 命令，均 exit 0。用 Linux/macOS 实际客户端前不得声称注入已通过。

### 2. 增加结构化监听与累计检查

在 `diag/scroll-verify.swift` 实现 listen-only HID event tap，沿用现有 `diag/scroll.swift` 的创建、失效重启和退出错误处理。实现参数：`--pid <client-pid>`、`--axis x|y`、`--expected-count <n>`、`--expected-fixed <signed-total>`、`--timeout-seconds <n>`、`--help`。

只统计指定 PID 的 scrollWheel 事件。输出每个匹配事件的 line/point/fixed、continuous、phase/momentum 与累计字段；结束时输出 JSON summary，至少含 count、fixedX/fixedY、lineX/lineY、pointX/pointY 和 pass。非零 expected-fixed 未收到事件必定失败；非零预期按 count 和 fixed 合计检查，容差最多 `max(1, expected-count) / 65536` 行，以容纳 16.16 量化。未选轴 fixed 合计应为零。达到 count 后保持短暂观察窗口抓多余事件，而非立即退出；若超时、tap 失效、事件数不符或总量不符，非零退出并记录原因。

`reverse` 会发两次非零事件，expected count 是 2、total 是 0；倍率为 0 时生产路径会不发事件，使用 count=0 和整个观察窗口断言没有该 PID 的滚轮事件。不要将总量为零自动判为没有事件。

**Verify**：Swift typecheck 和 `--help` exit 0。启动权限不足时工具必须明确失败，不可输出 PASS。

### 3. 通过实际客户端执行矩阵

在 Mac 上使用三个独立终端：fixture、客户端、监听器；先启动 fixture，再启动 client，等待 fixture 输出 READY。取得实际 client PID 后启动监听器；监听器成功创建 tap 后输出 LISTENING，此后才在 fixture 终端按 Enter，确保不漏首事件。文档必须给出完整、可复现的 Nushell 命令。客户端命令示例：

```nu
cargo run -p hidpipe-client -- 127.0.0.1 --port 24802 --scroll-scale-x 0.5 --scroll-scale-y 0.5
```

监听器按实际 client 进程 PID 过滤；不要误用 cargo 的 PID。关闭原来 managed client 或使用能明确区分的 PID，避免两个 client 混淆；不修改 launchd 配置。各轮结束后终止测试客户端，避免自动重连消费下一轮。

矩阵至少包含：两个轴；倍率 0、0.5、1、1.5；各轴 inversion；上述七种序列；20 ms 基线。对 quarters/whole 和 two-whole/double 分组比较同倍率同符号的 fixed 总量。对 quarters 追加 5/100 ms 观察，确认输入总量不受分批与间隔影响。scale=0 的 count=0；其余非零 case 的 count 等于输入事件数。expected-fixed = 原始单位总和 / 120 × 对应 scale × inversion sign。

将记录保存在 `diag/scroll-validation.md`：Mac 型号/OS、客户端 commit、CLI、PID 筛选方式、每个 case 的 PASS/FAIL 与原始 summary，不记录无关键盘输入。至少一轮开启 `--input-feedback`，确认 fixture drain 正常。

**Verify**：每轮监听命令 exit 0，矩阵无缺项；发现任何失败，保留证据并按 STOP 条件报告，不更改生产函数。

### 4. 验证实际应用消费

在浏览器、终端、用户常用的第三个应用中分别确认：quarters 单独小步有响应；正负方向正确；无输入时不继续滚动；0.5/1.5 倍率可感知且小输入不全部丢失。使用可重复的足够长内容，避免到达边界；记录应用名称/版本、内容条件、观察结果。应用可能按事件速度加速，不能要求屏幕像素在不同事件间隔下严格一致，更不能用 fixed 总量相同代替应用验证。

文档结论需区分：协议成功、HID tap 成功、应用消费成功。未取得 Mac 或应用证据时标记 BLOCKED，并列出缺项；允许工具交付，不允许写成验证完成。

**Verify**：`cargo test -p hidpipe-proto` exit 0；文档具备三层证据和完整矩阵。若测试失败或应用小步无响应，停止并提出后续生产修正需要的证据，不擅自修复。

## Test plan / Done criteria

- [ ] fixture 驱动真实生产客户端，不复制 `Injector::wheel` 实现。
- [ ] 两个轴、正负、反向、零倍率、0.5/1/1.5 和分批等价均完成上述边界验证。
- [ ] 收到的事件数和 fixed 累计量由工具检查，失败返回非零。
- [ ] Mac `cargo build -p hidpipe-client`、Swift typecheck、协议回归 exit 0。
- [ ] 三个应用的观察证据已写入文档；字段通过不能替代这项。
- [ ] `git diff --name-only` / `git status --short` 显示修改均在 scope 内。
- [ ] 状态更新：有完整证据才 DONE；缺 Mac/权限/观察证据则 BLOCKED。

不新增针对 `ScrollModifier::apply` 的函数单元测试。这里需要的是实际 TCP → 客户端 → Quartz → 应用链路证据。

## STOP conditions

- 源码漂移导致摘录或握手不同；尤其不要沿用旧 README 的协议版本。
- 无 Mac、无 Accessibility/Input Monitoring 权限、source PID 不可可靠区分、无法观测应用响应：记录 BLOCKED。
- 两次合理调整后工具仍无法采到预期事件，或任何分批/小数 case 失败：保留失败证据，停止生产修正。
- 需要修改 scope 外文件、引入依赖或改变协议才能运行。

## 维护说明

未来改变 Quartz 单位、字段、倍率或事件合并时重跑本矩阵。默认只验证原生 Wheel；OpenLogi 普通 Wheel 共用同一注入函数，但桥接链路和 HID++ 拇指轮不据此算已测。Deskflow 默认整数行可用于观察对照，不能用它的丢余量行为作为通过标准。
