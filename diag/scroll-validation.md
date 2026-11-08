# macOS 精确像素滚轮验证

## 当前部署补记（2026-11-06）

reviewer 已核实以下结果；后文“未部署”和“未执行签名部署”仅记录历史实现阶段。

- **构建与测试：** Mac `/tmp/hidpipe-pixel-4RZFBx` 中 `cargo test --locked -p hidpipe-client` 的 6 个真实 CGEvent 测试通过，包含 288 组分片矩阵；使用下述 Apple clang／SDK 环境的 release 构建通过。
- **签名与安装：** 使用 `nix/macos/install-apps.sh` 和既有持久签名身份签署 release，经 `/tmp/hidpipe-pixel-4RZFBx/staged` 安装至 `/Users/woxqaq/Applications/Hidpipe.app`。`codesign` 严格验证通过，designated requirement 与既有身份一致。源码资源对应的 SHA-256 与未签名 release 一致：`2630f1521e00127a4c0dc8b3e1e532e7099e82fdeb7ee647133a8545f948e6d1`；staged 与已安装 app 的 CDHash 均为 `678be80111ec807722e7e1ca6ec9b301e4ec9eda`。CMS 签名时间可能导致完整签名二进制的 SHA-256 不同，此处不声称其相等。
- **生产运行：** 已 kickstart `gui/501/org.nix-community.home.hidpipe`，核实时运行 PID 为 `29055`；`21:36:18` 日志显示已连接 `woxqaq.local:24800`，保留原有 `--openlogi-socket`、`--clipboard`、`--input-feedback`、`--latency-stats` 参数。Linux `ss` 显示 `192.168.31.106:24800` 至 `192.168.31.240:65503` 的连接为 ESTABLISHED。
- **回滚与持久性：** 已验证先前签名 app 的回滚副本 `/tmp/hidpipe-pixel-4RZFBx/rollback/Hidpipe.app`。当前运行安装为临时手动签名 release；未做完整 Nix rebuild、未覆盖远端 dirty repo、未修改系统配置。Home Manager 若仍固定旧源码，后续 rebuild 可能覆盖本次安装，需先同步更新。

部署后尚未观察 Ghostty／微信聊天记录／微信公众号的主观滚动响应；新的 PIXEL HID tap live 矩阵也待执行。安装、连接和构造测试通过不代表这两层验收完成。

## 历史实现阶段状态（部署前）

状态：PIXEL 实现与 native CGEvent 构造测试已通过；HID tap 现场矩阵和 Ghostty／微信聊天／公众号消费观察待 reviewer 执行。此前 LINE/fixed 证据属于历史复现，不能作为当前版本验收。

wire 仍为 120 单位/格；目的端每格映射 10 Quartz point，再应用一次 scale/invert。每轴保留有符号亚像素余量，向零取整；默认 quarters 输出 2、3、2、3，tiny [1,119] 只输出一个 10 point 事件。ReleaseAll、Pointer Reset、断连与新连接清空余量。过大有限倍率饱和到 i32 边界并丢弃超范围量，不积压补偿队列。10 point 是明确的目的端基线，不能保证各应用屏幕位移相同。仅覆盖普通 Frame::Wheel 和 PointerEvent::Wheel；HID++ 拇指轮仍由目的端 OpenLogi profile 控制。

下方矩阵是当前 PIXEL 验收步骤；末尾保留早期 Linux 工具验证记录，历史 BLOCKED 和 LINE 描述不代表本次 native 构造测试结果。

## 历史 PIXEL 实现验证（真实 Mac，当时未部署）

SSH 主机为 `woxqaq@192.168.31.240`，`uname -sm` 为 `Darwin arm64`；`sw_vers` 输出 macOS `27.0.1`、BuildVersion `26A434`。全部远端源码和构建产物位于本次新建 `/tmp/hidpipe-pixel-4RZFBx`，复制当前 checkout 的 client/proto、workspace Cargo.toml/Cargo.lock 和 server manifest/source；没有覆盖原远端 dirty repo、已安装 app 或服务。未改锁文件、协议、server、OpenLogi、Nix 或 plans。

Mac 默认 Nix linker 首次报告 `ld: library not found for -liconv`。随后仅在当前构建进程设置 `CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/clang` 和 `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk`，无机器配置修改。以下命令在该临时目录实际执行：

| 检查 | 实际结果 |
|---|---|
| Linux `cargo test -p hidpipe-proto` | exit 0；11 个既有集成测试通过 |
| Mac `cargo check --locked -p hidpipe-client` | exit 0 |
| Mac `cargo test --locked -p hidpipe-client` | exit 0；本轮累计精度修正后 6 个真实 CGEvent 边界集成测试通过，0 个函数单元测试 |
| Mac `cargo clippy --locked -p hidpipe-client --tests -- -D warnings` | exit 0 |
| Mac `swiftc -typecheck diag/scroll-verify.swift` | exit 0 |
| Mac `swiftc diag/scroll-verify.swift -o scroll-verify` | exit 0 |
| Mac `./scroll-verify --help` 与 `swift diag/scroll-verify.swift --help` | 均 exit 0，无 tap 创建 |
| Mac verifier 六组非法 CLI | 均 exit 2：continuous=2、缺 continuous、fixed 缺 count、混合两种模式、非整数 point、负 count |
| Mac 临时 AppKit 构造探针 `swift appkit.swift` | exit 0；PIXEL 的 2/3/2/3/10/-2 转为 NSEvent 后 scrollingDeltaY 均等于输入，hasPreciseScrollingDeltas 均 true；没有 post |
| Rust 修改文件 rustfmt check 与 `git diff --check` | 均 exit 0 |
| Nushell 文档 | 六段 nu 代码均经 `nu-check --debug` 解析返回 true；实际生成 144 行，验证零倍率 point 期望均为 0，tiny/default 的参数字符串为 `10` |

CGEvent 测试直接包含生产 scroll 模块，使用真实 CGEventSource，构造后读取 point/continuous/flags/phase/momentum，不 post。断言覆盖 whole=10、quarters=2/3/2/3（两个普通 backend）、tiny [1,119]、0.1 倍率亚像素累计、负方向、两轴独立、反转、零倍率、正反抵消、ReleaseAll/Pointer Reset 与新连接丢弃余量、极大 finite 倍率饱和后继续正常处理零和双向输入。每个生成事件均断言 continuous=1、Shift|Command flags 保持、phase=momentum=0。

### 分片累计精度修正与边界证据

本轮在同一个 Mac 临时目录只同步 `client/src/scroll.rs` 和 `client/tests/scroll_events.rs`。先保留旧生产算法，执行 `cargo test --locked -p hidpipe-client --test scroll_events quartz_small_partitions_preserve_integer_totals -- --nocapture`，分别以正、负方向优先运行：单次 raw120 的 point 合计为 ±1；scale=0.1 的 raw±1×120 均得到 0，断言失败、exit 101。该测试直接走生产 `Scroll::event` 并读取实际 CGEvent，而非 Python 模拟或 helper 单元测试。

修正使用每轴 Kahan compensation（保存浮点加法丢失的低位），在整数提取后继续保留。换算中的除法和乘法自身仍可能把整数阈值落在略低的一侧，因此当最近整数非零、且距离不超过 `4 * f64::EPSILON * abs(total)` 时，按该整数提取；其余情况继续向零截断。约 1 point 处容差为 8.88e−16 point。容差随当前候选总量变化，在 i32 上界附近约为 1.91e−6 point；它不是固定的物理最小滚动量，也不归零极小输入。被提前提取的差额保留为反号余量，后续输入仍需偿还，避免每次边界处理都丢掉差额。这一策略有意将窗口内的真实缺口与舍入误差等同处理；不声称任意 f64 倍率、任意长序列都具有精确有理数运算或逐位相同的分批边界。

补偿和余量一起参与候选状态计算：无整数输出时提交；创建事件成功后提交；创建失败时两者均不提交。ReleaseAll／Pointer Reset 清零两者，新连接均从零开始；溢出饱和也清零两者，不计算或保存无穷大的补偿。

修正后 Mac `cargo test --locked -p hidpipe-client` 和 `cargo clippy --locked -p hidpipe-client --tests -- -D warnings` 均 exit 0，使用本文上述 clang／SDKROOT 环境变量。6 个边界测试包含：

- 分片矩阵 288 组：scale=0.1／0.3／0.5／0.7／1／1.5，正负方向，两个普通 backend，x 反转开关，两个轴同时取相反方向；分别输入 `[120]`、`[1;120]`、`[2;60]`、`[3;40]`、`[7;17]+[1]`、`[11;10]+[10]`，point 合计均符合预期。所要求倍率 0.1／0.5／1／1.5 的单轴合计分别为 ±1／±5／±10／±15。
- 阈值下方：scale=`0.1 * (1 - deficit)`，deficit 为 1e−12 或 1e−14；raw120 或 raw1×120 在正负方向均不输出，追加 raw0 仍不输出，追加同方向 raw1 后才输出 ±1。有效缺口不会被容差提前提升为整点。
- 极小 scale=1e−20 的连续输入不产生事件；119 次 raw1 的低倍率累计经过两种 reset 后均被丢弃；原有方向、两轴、flags／continuous／phase、饱和与新连接覆盖保持通过。

### 历史实现阶段的现场缺项（部署状态已由顶部补记更新）

创建事件失败的状态处理采用先算候选、成功后提交；本次没有故意诱导 Quartz 分配失败。该历史阶段尚未执行 HID tap 的实际监听／权限失败路径、144 轮 live 矩阵、签名部署和 Ghostty／微信聊天记录／公众号观察；签名部署现已完成，见顶部补记，其余现场验收仍待执行。CGEvent 与 NSEvent 构造通过不能代替应用消费验收。现有 fixture 已足够覆盖本次基本序列，未增加 case。

## 工具与判定

`proto/examples/scroll_fixture.rs` 是一次连接的 loopback TCP 输入源，默认 `127.0.0.1:24802`。使用协议库的 `VERSION`（当前为 2）、`read_frame` 和 `write_frame`；要求客户端先发送 `Hello { version: VERSION, openlogi: false }`，不回发 Hello，不访问输入设备。启动它会在显式按 Enter 后真实滚动当前应用。

fixture 输出 `BOUND` 后等待连接；握手后输出 `READY`。只有在监听器输出 `LISTENING` 后才按 Enter。可用 `--start-delay-ms 2000` 留出切回目标应用的时间，默认 0。等待 Enter、发送前延迟、事件间隔和末尾 300 ms drain 期间每 250 ms 发送 Ping；后台持续读取 Pong / ReturnToLocal。只发送 Wheel、Ping 和末尾的 ReleaseAll；输入事件边界保持原样，不合并。`COMPLETE` 中的回复数量不是注入事件数量。

`--port` 范围 1..65535，`--axis` 为 x/y，`--interval-ms` 为 1..10000（默认 20），`--start-delay-ms` 为 0..10000。`--timeout-seconds` 为 1..3600，默认 120，分别限制 accept、READY、发送阶段（含发送前延迟和末尾 drain）；socket 读写超时各 3 秒。未回复心跳的 peer 会在读超时后失败；stdin EOF、断连、非法握手、非法回复或阶段超时也非零退出，不继续等待终端输入。stdin 读取线程不参与阻塞式 join；socket shutdown 后才 join 回复线程。末尾短暂 drain 不等同于 Quartz 注入确认。

`diag/scroll-verify.swift` 使用 listen-only HID tap，只统计 `eventSourceUnixProcessID` 与 `--pid` 相等的 scrollWheel。逐事件输出 JSON，包含 line/point/fixed、continuous、phase、momentum 和累计值；最后输出 `type=summary` JSON，含 count、fixedX/Y、lineX/Y、pointX/Y、pass 和 reasons。fixed 与 expected-fixed 的单位都是**行**，不是原始 120 单位，也不是 16.16 编码整数。

当前模式使用互斥参数 `--expected-point <整数合计> --expected-continuous <0|1>`，普通 PIXEL 的 continuous 期望为 1。逐事件验证 continuous，未选轴 point 必须始终为零；结束时选定轴 point 合计必须严格相等。默认不指定 `--expected-count`，完整观察 timeout 后判定；wire 数量不能作为 CGEvent 数量。可选 count 是独立显式断言，正 count 达标后再观察 0.5 秒，count=0 观察整个 timeout。reverse 合计为零仍可以有两个事件；零倍率额外指定 count=0 可证明没有事件。

历史 LINE 复现保留 `--expected-fixed <行合计> --expected-count <数量>`，与 point/continuous 参数互斥。fixed 容差为 `max(1, 实际count)/65536` 行，未选轴 fixed 合计为零；非零 fixed 期望但没有事件始终失败。该模式不能用于当前 PIXEL 单位验收。

两个模式都会输出原始字段与 JSON summary；默认 timeout=10 秒，允许 >0.5 且 <=3600。tap 创建、启用、失效或断言失败均非零退出；参数错误在创建 tap 前退出 2。使用 executable PID，而非 cargo PID。

## 三终端操作（Nushell）

先在项目根目录执行预检；缺少 Rust 工具链可先 `nix develop`，不更新锁文件、不安装系统软件：

```nu
cargo test -p hidpipe-proto
cargo check -p hidpipe-proto --example scroll_fixture
cargo run -p hidpipe-proto --example scroll_fixture -- --help
cargo build -p hidpipe-client
swiftc -typecheck diag/scroll-verify.swift
swift diag/scroll-verify.swift --help
```

后面三条需要 Mac。先授予测试 executable Accessibility 权限，并为运行 Swift 的终端授予 Input Monitoring / Accessibility；不修改 launchd 配置。关闭旧测试 client 或记录 managed client 的 PID 以明确区分。每轮使用一个新 fixture 和测试 client；结束后在 client 终端按 Ctrl-C，避免自动重连抢占下一轮。

先生成完整待执行矩阵，共 144 轮：112 个 20 ms 基线组合（两轴 × 四倍率 × 两方向 × 七 case），另有 32 个 quarters 的 5/100 ms 组合。所有条目初始为 BLOCKED，生成条目不是执行证据。

```nu
let cases = [
  {name: whole, count: 1, raw: 120}
  {name: quarters, count: 4, raw: 120}
  {name: two-whole, count: 2, raw: 240}
  {name: double, count: 1, raw: 240}
  {name: tiny, count: 2, raw: 120}
  {name: reverse, count: 2, raw: 0}
  {name: negative, count: 4, raw: -120}
]
let matrix = (['x' 'y'] | each { |axis|
  [0.0 0.5 1.0 1.5] | each { |scale|
    ['normal' 'inverted'] | each { |direction|
      $cases | each { |case|
        let intervals = (if $case.name == 'quarters' { [20 5 100] } else { [20] })
        $intervals | each { |interval|
          let sign = (if $direction == 'inverted' { -1 } else { 1 })
          {
            axis: $axis, scale: $scale, direction: $direction, case: $case.name
            interval_ms: $interval
            expected_point: ($case.raw / 120 * 10 * $scale * $sign)
            expected_continuous: 1
            status: BLOCKED, summary: null
          }
        }
      } | flatten
    } | flatten
  } | flatten
} | flatten | enumerate | each { |row| $row.item | insert id $row.index })
$matrix | to json | save --force /tmp/hidpipe-scroll-matrix.json
$matrix | select id axis scale direction case interval_ms expected_point expected_continuous status
```

三个终端分别设置同一个 `trial_id`。首轮可以选 0（零倍率）；之后按表逐项执行。每次将终端 C 的原始 summary 和退出码记入记录，再把该项的 status 改成 PASS / FAIL；缺项保持 BLOCKED。

终端 A：先启动 fixture，等待 `BOUND`，然后启动 B。`READY` 后再准备 C。

```nu
let trial_id = 0
let trial = (open /tmp/hidpipe-scroll-matrix.json | where id == $trial_id | first)
cargo run -p hidpipe-proto --example scroll_fixture -- --port 24802 --case $trial.case --axis $trial.axis --interval-ms $trial.interval_ms --start-delay-ms 2000
```

终端 B：测试真实生产 client，至少一轮带 `--input-feedback`；下面所有轮都启用它。不给 `--openlogi-socket`。如果 foreground 应用正在底部／顶部，先移动到可滚动的中间位置。

```nu
let trial_id = 0
let trial = (open /tmp/hidpipe-scroll-matrix.json | where id == $trial_id | first)
let inversion = (if $trial.direction == 'inverted' { [$"--invert-scroll-($trial.axis)"] } else { [] })
cargo run -p hidpipe-client -- 127.0.0.1 --port 24802 --scroll-scale-x $trial.scale --scroll-scale-y $trial.scale --input-feedback ...$inversion
```

终端 C：确认 PID 为 **hidpipe-client executable**，不能取 cargo 的 PID。`pgrep -x` 列出的候选可能包括 managed client，必须根据进程表核对测试实例。

```nu
let trial_id = 0
let trial = (open /tmp/hidpipe-scroll-matrix.json | where id == $trial_id | first)
let candidates = (^pgrep -x hidpipe-client | lines | into int)
ps | where pid in $candidates
let client_pid = (input '输入已核对的测试 hidpipe-client PID: ' | into int)
if $client_pid not-in $candidates { error make {msg: 'PID 不在 executable 候选中'} }
let log = $"/tmp/hidpipe-scroll-($trial_id).jsonl"
let verify_exit = (try {
  swift diag/scroll-verify.swift --pid $client_pid --axis $trial.axis --expected-point $trial.expected_point --expected-continuous $trial.expected_continuous --timeout-seconds 15 | save --force $log
  0
} catch { |err|
  $err.exit_code? | default 1
})
open --raw $log
print $"verify_exit=($verify_exit)"
```

这里只把 stdout JSON 保存到文件，stderr 的 `LISTENING` 会即时显示。`try/catch` 保留非零外部退出码，使失败后仍能展示原始日志；不能把捕获错误当作通过。看到它后回到 A 按 Enter，并在两秒内切回目标应用；不在客户端主机上再滚动鼠标。结束后把 `verify_exit` 和完整原始 summary 一起保存。超出 15 秒操作窗口可增大 timeout 重跑；不得把晚启动监听器的漏事件当成算法失败。全部日志保留到完成记录后再清理。

## 期望矩阵和记录方式

固定序列和未反转时的 expected-point 如下；反转时总量取相反数。scale=0 均不输出事件；tiny 的首个 raw1 在这些倍率下不会输出事件。需要数量断言时按像素序列计算，不照搬 wire 数量。

| case | 原始序列 | scale 0 | scale 0.5 | scale 1 | scale 1.5 |
|---|---|---:|---:|---:|---:|
| whole | [120] | 0 | 5 | 10 | 15 |
| quarters | [30,30,30,30] | 0 | 5 | 10 | 15 |
| two-whole | [120,120] | 0 | 10 | 20 | 30 |
| double | [240] | 0 | 10 | 20 | 30 |
| tiny | [1,119] | 0 | 5 | 10 | 15 |
| reverse | [30,-30] | 0 | 0 | 0 | 0 |
| negative | [-30,-30,-30,-30] | 0 | -5 | -10 | -15 |

每个轴、倍率、符号下分别比较 quarters/whole、two-whole/double 的实测 point 合计，必须严格相等；默认 quarters 单事件 point 为 2、3、2、3，continuous 均为 1。比较 quarters 在 5/20/100 ms 的合计；屏幕位移可能受应用行为影响，不要求应用间相同。

当前 **144/144 个 macOS 矩阵条目均 BLOCKED，原始 summary 均未取得**。不得用下表中的 Linux mock 成功填充 Mac 矩阵。完成真实执行后，在本文追加机器型号、OS 版本、测试客户端 commit、完整 CLI、实际 PID 与筛选方式，以及矩阵每项 PASS/FAIL 和未经改写的 summary。失败时保留日志；两次合理调整仍不能观测，或发现任一小数／分批失败，就停止生产修正并提交证据。

## 实际应用消费（全部待执行）

在 Ghostty、微信聊天记录、微信公众号（可补浏览器）分别使用足够长、可重复且未到滚动边界的内容。按上述延迟切回目标应用后测试 quarters 的单独小步响应、正负方向、无输入后停止、0.5/1.5 倍率可感知以及 tiny 的小输入是否全部丢失。记录应用名称／版本、内容条件、case、倍率、方向和实际观察。point 总量相同不能替代这些观察；若窗口焦点或 PID 不可靠，标为 BLOCKED。

| 应用 | 名称／版本 | 内容条件 | 小步／正负／停止／倍率观察 | 状态 |
|---|---|---|---|---|
| Ghostty | 现场版本 1.3.1；本次待复核 | 未取得 | 未执行 | BLOCKED |
| 微信聊天记录 | 未取得 | 未取得 | 未执行 | BLOCKED |
| 微信公众号 | 未取得 | 未取得 | 未执行 | BLOCKED |

## 历史 Linux 执行证据（LINE 工具阶段）

以下结果针对本次工具代码；不是实际 Mac client 注入证据。主机 `uname -s` 为 `Linux`，`cargo --version` 为 `cargo 1.98.0 (797e8a9bc 2026-08-05)`。隔离 worktree 为 `/home/woxQAQ/.t3/worktrees/hidpipe/t3-f9b385a7`，使用 T3 已分配的 `t3/f9b385a7` 分支。

| 命令／检查 | 实际结果 |
|---|---|
| 基线 drift check | `git diff --stat 5e0f7ff..HEAD -- …` 开始执行时无输出，HEAD 等于基线 |
| `cargo check -p hidpipe-proto --example scroll_fixture` | exit 0 |
| `cargo test -p hidpipe-proto` | exit 0；11 个既有集成测试通过（bridge 2、client_transport 2、clipboard 5、roundtrip 2），没有新增函数单元测试 |
| `cargo run -p hidpipe-proto --example scroll_fixture -- --help` | exit 0，显示参数、七个 case、READY 门控和滚动警告 |
| `cargo build -p hidpipe-proto --example scroll_fixture` | exit 0 |
| `cargo clippy -p hidpipe-proto --example scroll_fixture -- -D warnings` | exit 0 |
| `rustfmt --edition 2021 --check proto/examples/scroll_fixture.rs` | 最初发现格式差异，修正后 exit 0 |
| `swiftc -typecheck diag/scroll-verify.swift` | exit 127，`swiftc: command not found`；未完成 |
| `swift diag/scroll-verify.swift --help` | exit 127，`swift: command not found`；未完成 |
| Mac `cargo build -p hidpipe-client` | 未执行：当前为 Linux，尚无 Mac 构建证据 |

临时外部 harness `/tmp/hidpipe-001-boundary.py` 以 subprocess 启动构建后的 fixture，通过真实 loopback TCP 作为 mock client，设置 socket、进程和队列超时，直接按 VERSION=2 的 tag 与大端 i32 解析字节。执行命令（Nushell）：

```nu
cargo build -p hidpipe-proto --example scroll_fixture
python3 /tmp/hidpipe-001-boundary.py
```

该 harness 是本次执行的临时证据，不作为仓库持久测试入口；路径在其他主机上不会自动存在。检查的是进程和 TCP 边界，不调用数值函数，不复制 Injector 的 Quartz 行为。既有 `client_transport` 测试另外覆盖了真实生产 transport 的 feedback / Ping / ReturnToLocal 行为。

harness 实际输出 `BOUNDARY RESULT: 35 groups passed; no Quartz or application claims`：

| 检查 | 结果 |
|---|---|
| 七个 case × 两轴，20 ms；quarters × 两轴，5/100 ms | 18 轮 exit 0；每个原始 dx/dy、事件边界、未选轴为零、间隔下界和唯一末尾 ReleaseAll 均断言 |
| READY 等待 | 每轮 Enter 前只收到至少三个 Ping，无服务端 Hello 或 Wheel；回复心跳后再按 Enter |
| 异步 drain | quarters/x/20 ms 期间额外发送 1,000,000 个 Pong 和 ReturnToLocal；每个输入及 ReleaseAll 也回 Pong；断言 COMPLETE 计数包含这些回复，exit 0，无阻塞 |
| 错误版本、openlogi=true、畸形 Hello、首帧非 Hello、未知首 tag | 5 轮均 exit 1 |
| 未知回复 tag、客户端反向发送 Wheel | 2 轮均 exit 1 |
| Enter 前断连、空 stdin EOF、未换行文本后 stdin EOF、序列中途断连 | 4 轮均 exit 1，有界结束，不等待 stdin；未换行文本不会触发发送 |
| accept / READY / 部分 Hello / 无回复 peer / 发送阶段超时 | 5 轮均 exit 1；socket 读超时在此 Linux 表现为 WouldBlock / OS error 11 |
| 非法 CLI | 一组 12 项均非零退出且没有 BOUND：端口 0/65536/-1、未知 case、轴 z、间隔 0/10001/NaN、timeout 0/3601、未知参数、缺值 |

另外执行 `python3 /tmp/hidpipe-001-delay.py`，exit 0：650 ms 发送前延迟期间没有 Wheel，收到 4 个 Ping，延迟后仅发送一个 `(0,120)` Wheel 和一个 ReleaseAll；10 秒延迟配 1 秒阶段期限收到 5 个 Ping 后 exit 1，全程没有 Wheel。发送前延迟 -1/10001/NaN 均被拒绝。

执行 `python3 /tmp/hidpipe-001-doc-check.py`，exit 0：使用 Nushell 0.116.1 解析本文六段 nu 代码，实际运行矩阵生成代码并断言 144 个唯一条目、112/32 的覆盖分布、零倍率 count、reverse count/total 与反转符号。另用真实外部进程验证日志 `save` 加 `try/catch` 的命令可分别记录退出码 0 和 1。这些检查不包含 Mac 终端中的实际 PID 发现、Swift 或 client 执行。

## macOS accepted socket 修正

后续 Mac 验证已确认 fixture 的平台差异：为限制 accept 等待时间而设置的 listener nonblocking 模式会被 BSD/macOS 的 accepted `TcpStream` 继承；Linux 不继承。`set_read_timeout` 不会恢复 blocking 模式，因此原 fixture 即使读到 Hello，也可能在一次 Ping 后立即以 `WouldBlock` / OS error 35 结束回复读取。真实 client 和立即回复 Pong 的 Python peer 都复现了该失败；独立的最小 Python server 则确认 GUI / launchd client 能正常回复 Pong。这是 fixture 的 socket 模式问题，不能据此判断生产 client 心跳失败。

修正是在 accept 完成并丢弃 listener 后、设置 TCP_NODELAY、读写超时和读取 Hello 之前，显式调用 `stream.set_nonblocking(false)?`，让握手和后台 drain 使用有超时的 blocking I/O。修正后的 Mac 编译、mock 复测和真实 client 验证尚待 reviewer 执行；这里不声称 Mac 已通过，也不改变 HID tap 或应用消费的 BLOCKED 状态。

修正后在当前共享 checkout 的 Linux 复核：fixture 的 cargo check / build、Clippy `-D warnings`、rustfmt 检查均 exit 0；`cargo test -p hidpipe-proto` 的 11 个既有集成测试通过。重新执行上述 boundary harness，35 组全部通过（包括 READY 门控、心跳、百万 Pong drain 和错误退出）；delay harness 也通过，650 ms 延迟期间收到 4 个 Ping 后发送一个 Wheel，1 秒阶段期限下收到 5 个 Ping 后失败且没有 Wheel。这些 Linux 结果不验证 BSD/macOS 的模式继承或修正后的 Mac 行为。

## 历史三层结论与缺项（本次 PIXEL 证据见本文开头）

1. **协议层：Linux 工具与 TCP mock 已验证。** VERSION=2 握手、原始序列、心跳、feedback drain、门控和错误退出已有上述证据；尚未驱动真实 macOS client 完成矩阵。
2. **HID tap 层：BLOCKED。** Swift 未 typecheck、帮助未运行、权限失败路径未实测；尚无 Mac 型号／OS、Mac client build、实际 source PID、144 轮原始 summary 或分批等价实测。
3. **应用消费层：BLOCKED。** 三个应用的名称／版本、内容与交互观察均缺失。

未修改生产注入算法、协议、依赖、系统配置或计划文件。与原计划的执行差异：保持 T3 分配的隔离分支；按本次授权不更新 plans；缺 Mac 时继续完成可实现的工具和 Linux 验证，整体不标 DONE。工具额外提供有限阶段超时和可选发送前延迟，以支持有界自动检查和前台应用切换。
