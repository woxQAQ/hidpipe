# Mac 输入处理延迟实测

2026-11-04，在运行中的 Mac 客户端加入可选慢事件计时后，已确认屏幕信息查询反复产生数毫秒尖峰；另捕获一次键盘事件创建 32.422 ms，以及首次移动的提交 6.739 ms。当前证据足以说明本地输入路径存在可优化的停顿，但不能把此前全部卡顿归结于这些调用。

## 方法与部署

计时随现有 `--latency-stats` 启用，每次输入超过 2 ms 时输出 `[slow-inject]`，记录类别和进程内序号，不记录按键、按钮或位移内容。阶段分别是屏幕查询、状态锁、事件创建、属性设置、提交、对象释放；这些是墙钟时间，包含调度等待，未使用 System Trace 进一步区分调用内部等待和线程被抢占。

Mac 原生 release 构建、Clippy（warnings 为错误）、现有 6 项集成测试通过。使用现有 `nix/macos/install-apps.sh` 和同一签名身份安装，codesign 验证通过，SIGTERM 后由 launchd 自动重启。15:35:56 日志确认重新连接且 input feedback / latency stats 均启用。诊断客户端 PID 为 61336。

这是直接安装的诊断构建，尚未通过 Nix rebuild 固化；以后 Home Manager 激活可能替换它。远端源码、构建产物和原应用备份位于 `/tmp/hidpipe-latency.48xQeT`，备份为 `Hidpipe.previous.app`。

## 真实输入证据

从 `~/Library/Logs/hidpipe.log` 读取：

```text
[15:36:02] [slow-inject] epoch_us=1793777762608080 seq=1 kind=motion total_us=8692 bounds_us=1923 state_us=3 create_us=16 configure_us=1 post_us=6739 drop_us=2 other_us=5
[15:36:02] [slow-inject] epoch_us=1793777762642445 seq=16 kind=motion total_us=5433 bounds_us=5397 state_us=0 create_us=6 configure_us=0 post_us=26 drop_us=0 other_us=1
[15:36:03] [slow-inject] epoch_us=1793777763332366 seq=51 kind=key total_us=32461 bounds_us=0 state_us=0 create_us=32422 configure_us=0 post_us=31 drop_us=1 other_us=5
[15:36:07] [slow-inject] epoch_us=1793777767014083 seq=404 kind=motion total_us=7859 bounds_us=7824 state_us=0 create_us=6 configure_us=1 post_us=25 drop_us=0 other_us=1
```

15:36:04–07 还有多次主要由 bounds 阶段贡献的 2–4 ms 慢移动。该阶段当前覆盖 `CGDisplay::main().bounds()` 的两个调用，因此用下述独立采样继续拆分。

同一窗口 Linux journal：

```text
15:36:07 server.queue: n=466 mean_us=59.6 max_us=171.8
15:36:07 server.write: n=469 mean_us=19.0 max_us=71.4
```

Mac 对应统计：

```text
client.resolve: n=457 mean_us=29.5 max_us=216.0
client.inject: n=442 mean_us=486.4 max_us=32733.0
client.feedback: n=471 mean_us=29.9 max_us=143.8
client.process: n=474 mean_us=512.2 max_us=32761.8
```

阶段样本数独立，窗口边界也不是严格同步，不能逐帧拼接或相加最大值。慢事件打印开销计入外层 inject 汇总，因此它比慢事件内部 total 略大。

## 被动 Quartz 采样

脚本：[injection-probe.swift](injection-probe.swift)。只查询显示信息、创建并释放移动事件，**不调用 CGEventPost，不注入输入**。每轮 sleep 8 ms，执行 2500 轮，实际约 24.4 秒。

第一次采样把主屏 ID 和 bounds 一起计时：平均 197.296 us、最大 7815.792 us；创建事件最大 256.167 us。

第二次分开计时：

| 阶段 | 平均 us | 最大 us |
| --- | ---: | ---: |
| CGMainDisplayID | 120.719 | 7477.666 |
| CGDisplayBounds | 50.857 | 6110.958 |
| 创建鼠标事件 | 5.845 | 24.833 |
| 属性设置 | 3.787 | 20.833 |
| 对象释放 | 1.714 | 16.500 |

尖峰主要落在获取主屏 ID，也有 bounds 查询本身的尖峰。因此仅缓存 display ID 不能去掉整个问题。独立脚本不能代表可信应用进程内的全部注入行为，但与真实输入的屏幕查询尖峰相互印证。

## 后续方向

优先把主屏 ID 与几何信息移出逐移动路径，缓存后通过显示配置变更通知更新；需处理回调线程与输入线程之间的状态同步，以及主屏切换、缩放、分辨率和热插拔，不能永久缓存启动时尺寸。

键盘创建尖峰目前只抓到一次，出现在新诊断进程启动后很早的输入阶段；首次移动 post 也慢。这支持继续检查初始化路径，但不足以断言是冷启动，更不足以把所有键盘事件都视为慢。若重复发生，应针对事件创建阶段用 System Trace / Time Profiler 区分线程调度与 Quartz 内部等待。

首次调查仅增加测量并定位，没有测出应用显示端的完整输入延迟。后续缓存改动见下节。

## 显示信息缓存已实现

主屏 ID 和 bounds 改为进程级快照，由独立 `display-geometry` 线程在启动及显示配置完成通知后查询。CoreGraphics 回调只调用 unpark；通知可合并，查询期间收到的通知会使后台线程再刷新一次。查询在写锁之外进行，输入路径只短暂读锁并复制快照。回调和状态均存活至进程退出，没有 user-info 裸指针生命周期问题。

刷新得到无效 ID、非有限坐标或小于 1 的尺寸时保留原快照并记录错误。主屏 ID 或范围变化后，输入线程清空旧的边缘返回累计值，并继续按最新范围 clamp 光标。

实现：[display_cache.rs](../client/src/display_cache.rs)、[输入路径](../client/src/main.rs)。原慢事件的 bounds 阶段现在测量快照读取，不再调用显示查询。

Mac 原生 release 构建、全 target Clippy 和现有 6 项集成测试通过。15:59:35 安装后日志打印主屏 ID 3、1920×1080，15:59:36 已连接，input feedback / latency stats 均开启；新进程 PID 为 63183。仍为直接安装的构建，Home Manager 激活可能替换它。缓存改动前的诊断应用另备份为 `/tmp/hidpipe-latency.48xQeT/Hidpipe.before-display-cache.app`。

被动验证程序 [display_cache example](../client/examples/display_cache.rs) 使用同一缓存实现，不创建或注入事件，采样 2500 次：快照读取平均 **0.231 us**，最大 **11.625 us**。该数值只表示缓存访问开销，不能与端到端输入延迟等同。可在 Mac 运行 `cargo run --release -p hidpipe-client --example display_cache`；实际显示配置变化时也会打印新快照。

安装后截至本轮核对，真实连接只有心跳处理样本，尚无新一批鼠标输入可用于前后对照；没有通过改变用户的分辨率或热插拔显示器来验证真实系统回调。回调注册成功、启动快照以及稳定配置下的缓存读取已实测，显示变更后的端到端更新仍需现场验证。键盘创建尖峰、首次 post、移动合并和网络协议未在此改动中处理。
