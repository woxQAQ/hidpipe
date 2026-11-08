# Plan 003: 使用精确像素事件注入普通滚轮

- Planned at: `5e0f7ff` 加当前未提交的001/002实现，2026-11-06。
- Status: DONE（代码、native 验证和 Mac 安装完成；三应用现场反馈待确认）。
- Priority: P1；Effort: M；Risk: MED（改变普通滚轮的macOS表示与手感）。
- Depends on: Ghostty现场分析已取得；001完整应用矩阵仍不算通过。

## 执行与部署记录

- 已实现 `client/src/scroll.rs` 并统一两种普通Wheel backend；完整PIXEL事件、每格10 point、每轴余量与Kahan补偿、整数边界4×epsilon容差，重置时清空全部累计状态。
- 初版累计在scale0.1、raw1×120场景少发1point，真实Mac测试复现后修正。最终6个真实CGEvent集成测试、288组分片对照通过；阈值下方1e-12/1e-14point缺口不提前提升。reviewer独立复跑Mac native tests通过。
- Mac release构建通过；使用既有本地签名身份安装 `/Users/woxqaq/Applications/Hidpipe.app`，未变更系统配置、协议或远端dirty源码。签名严格验证通过；staged/installed代码目录哈希相同，source-sha256与release原文件匹配。
- 已重启GUI managed client，PID29055；日志21:36:18连接 `woxqaq.local:24800`，原有OpenLogi、clipboard、inputFeedback、latencyStats参数保持。Linux ss确认ESTABLISHED。
- 回滚备份 `/tmp/hidpipe-pixel-4RZFBx/rollback/Hidpipe.app` 已验证签名。当前是手工签名的运行态版本；后续须同步Nix源，避免旧锁定版本重建覆盖。
- Ghostty、微信聊天记录和公众号的新版实际响应尚待用户观察；不声称已完成live HID tap矩阵或三个应用验收。

## 问题与证据

生产 `client/src/main.rs::Injector::wheel` 把wire增量除120，使用LINE整数构造事件，再覆写fixed-point小数行。Mac实际CGEvent→NSEvent探针证实此组合为 `hasPreciseScrollingDeltas=false`、小数scrollingDeltaY。Ghostty1.3.1非精确分支会把每个非零小数提升到至少1，再乘默认discrete=3，导致分批越细放大越大。四次.25到达HID tap且浏览器会移动，不能据此认为Ghostty正常。用户报告Ghostty、微信聊天记录异常，公众号正常；微信算法未确认。

完整证据见本目录 `scroll-application-findings.md`，但实现不依赖应用名称，不改Ghostty或微信。

## 当前实现与约束

- `Injector`持有 `scroll: ScrollModifier`，`wheel(&self, dx:f64,dy:f64)`；Frame::Wheel和PointerEvent::Wheel进入前均 `/120.0`。
- ScrollModifier在main.rs底部，CLI支持axis scale/invert，默认scale1/false，非负finite且0合法。
- `release_all`处理ReleaseAll、PointerEvent::Reset、Drop；连接新建Injector，不保留跨连接余量。
- `diag/scroll-verify.swift` 当前验证fixed累计并假设事件数等于wire数；像素累计后此假设不成立。
- proto VERSION2、wire120单位/格不变。OpenLogi HID++ ThumbWheel由目标profile独立注入，不在范围。
- 所有既有未提交实现必须保留；不更新锁文件，不全局格式化。

## 目标行为

1. 普通Wheel统一使用 `ScrollEventUnit::PIXEL`，由Quartz产生一致的line/fixed/point/continuous字段。移除fixed字段覆写，不能只改continuous位。
2. 目的端固定映射每格10像素，现有scale/invert继续在同一边界应用一次。10为Mac LINE1→point10探针的基线选择，不是所有应用屏幕位移保证；不增加新CLI或应用特例。
3. 每轴连接内保存不足一像素的有符号余量。向零取整数，静止时不发送；反向自然抵消，无人为方向特例。ReleaseAll/Reset/Drop与新连接清零。
4. 对有限但过大scale导致的乘法溢出先饱和到可注入i32范围，余量始终有限且绝对值<1；不把inf带入未来事件。饱和丢弃超范围量，不排无限补偿队列。
5. 正常量级、相同有符号输入序列仅改变分批时，总point输出+余量一致。不要求正负抵消序列的中间输出或事件数相同。
6. 创建CGEvent失败不可静默消耗已经提取出的整数像素；可以候选计算后成功才提交状态。亚像素无事件时仍保存余量。

## 范围

- `client/src/main.rs`，可新增 `client/src/scroll.rs` 收拢像素累计及真实事件构造。
- 可新增 `client/tests/scroll_events.rs`：边界集成测试真实模块构造CGEvent并转换/读取字段，不post，不是转换函数单元测试。
- `diag/scroll-verify.swift`、`diag/scroll-validation.md`、`README.md`。
- 如边界验证需要，`proto/examples/scroll_fixture.rs`只加明确的细粒度/Reset序列，不改已有fixture修正。
- `plans/README.md`及本计划由reviewer维护。

禁止：协议、server、OpenLogi patches、Nix参数名/默认、锁文件、其他系统配置、原有远端dirty源码。不上游push，不向主分支commit。

## 执行步骤与验证

1. 将真实事件构造和余量生命周期组织成最小模块，main统一调用。保持已有修饰键flags与diagnostics stage测量有意义。检查两种普通backend都只经过一次换算。
   - Verify: Linux协议 `cargo test -p hidpipe-proto` exit0；native Mac `cargo check -p hidpipe-client` exit0。Linux不能验证CoreGraphics。
2. 编写真实Quartz边界集成测试（不post）。输入raw120与raw30x4均累计point10，continuous1；10像素选择下quarters应为2,3,2,3。验证tiny1+119、低倍率产生亚像素、正负方向、两轴、0倍率、反向、ReleaseAll/Reset生命周期、超大finite倍率不会毒化后续事件。使用真实生产模块；不复制算法，不为helper写函数单元测试。
   - Verify: Mac `cargo test -p hidpipe-client` exit0，断言真实CGEvent的point及continuous、flags；NSEvent精确识别可通过Swift探针补证。
3. 诊断器引入明确的point/continuous断言接口，与旧fixed模式互斥并保留旧历史复现用途。point模式不假设fixed=wire/120、事件数=wire数。更新新矩阵期望及旧证据标签。文档解释10像素、亚像素累计、拇指轮边界和当前未验证项，用户命令用Nushell。
   - Verify: Mac `swiftc -typecheck diag/scroll-verify.swift` / `--help` exit0。formatter/diff检查exit0。
4. reviewer将当前工作树源码复制到Mac新的临时构建目录，避免覆盖 `/Users/woxqaq/git-for-agents/hidpipe` 的dirty源码。native build/test后，用已授权签名流程准备测试二进制；不在未取得原签名方式与回滚备份前替换正在用的App。
   - Verify: 记录实际命令、Mac构建与测试输出、事件字段；没有应用观察不宣称全部问题解决。
5. Ghostty、微信聊天记录、公众号实测对照，先whole/quarters同总point，再倍率0.5。保留失败证据；若需要权限，沿用已验证的桌面监听与GUI launchd测试身份，不再反复手动启动未授权client。

## 检查命令

- Linux: `cargo test -p hidpipe-proto`。
- Mac: `cargo check -p hidpipe-client`；`cargo test -p hidpipe-client`；`cargo clippy -p hidpipe-client --tests -- -D warnings`。
- Mac: `swiftc -typecheck diag/scroll-verify.swift`；`swift diag/scroll-verify.swift --help`。
- 格式：只对修改的Rust文件运行rustfmt check；`git diff --check`。Nix未改无需重复全套。

## Done criteria与STOP

代码、真实Quartz边界验证通过且当前checkout集成后，可标实现完成；三应用未观察时独立标实测待完成。真实验证失败时不强行写PASS。若必须改协议/鼠标绑定架构，或10像素基线无法形成一致事件，停止并报告。不要通过修改Ghostty配置掩盖hidpipe问题。现场部署只能用可回滚的已有签名身份，禁止暴露证书/私钥内容。
