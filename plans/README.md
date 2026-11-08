# 滚轮调整执行计划

由 improve 技能于 2026-11-06 基于 hidpipe `5e0f7ff` 编写。用户随后要求两项全部实现，执行者的变更经审查后集成到当前工作区，保留为未提交修改。

执行者应完整阅读对应计划，在隔离分支中工作，遵守 scope 与 STOP conditions，记录实际验证结果。计划内区分已有仓库命令和需要实现的新工具接口；规划阶段没有运行构建、测试或 Mac 实测。

## 执行顺序与状态

| Plan | 内容 | 优先级 | 工作量 | 硬依赖 | 状态 |
|---|---|---|---|---|---|
| [001](001-macos-scroll-validation.md) | 建立 macOS 实际客户端小数滚轮验证基线 | P1 | M | 无 | BLOCKED；工具实现及 Linux 复核通过，Mac 实测待完成 |
| [002](002-home-manager-scroll-options.md) | 将现有倍率与反向参数接入 Home Manager | P2 | S | 无 | DONE；已集成，七组模块求值检查通过 |
| [003](003-precise-pixel-scroll.md) | 普通滚轮改为精确像素事件，消除逐事件放大 | P1 | M | 已取得 Ghostty 原因证据 | DONE；native 验证和 Mac 安装完成，三应用反馈待确认 |

状态值：TODO、IN PROGRESS、DONE、BLOCKED（附原因）、REJECTED（附理由）。

## 依赖与决策

推荐先执行 001，再据实测调整个人配置。002 只是接入已有 CLI，不改变默认倍率或算法，因此无需等待 001 才能独立实现；其 Nix 求值验证也不能替代 001 的 Mac 证据。

001 可在 Linux 开发协议 fixture，但完成状态要求真实 Mac 的 Quartz 与应用证据。缺少 Mac、权限或证据时标记 BLOCKED，不把工具完成记为行为验证通过。若发现小数丢失或分批不等价，先保留失败证据，再由 advisor 制定生产修正计划；当前计划没有授权顺手修改注入函数。

## 调查结论与证据

- hidpipe 原生采集保留每格 120 单位，在支持高精度轴时忽略 legacy companion；见 `server/src/main.rs:384`。
- 普通两种后端共用小数行注入、目的端倍率与方向；见 `client/src/main.rs:253`、`:328`、`:434`、`:606`。
- CLI 已有参数，Home Manager 尚未生成对应参数；见 `client/src/main.rs:637`、`nix/modules/home-manager.nix:151`。
- HID++ 拇指轮由目的端 OpenLogi profile 解析并直接产生效果；见 `nix/openlogi-bridge.patch:260`、`:678`。
- Deskflow 本地 checkout `2877600` 的 EI 路径先累计整格，Mac 整数行缩放会丢余量；见该仓库 `src/lib/platform/EiComputer.cpp:813`、`src/lib/platform/OSXComputer.mm:625`、`src/lib/deskflow/ISecondaryComputer.h:66`。只代表本地版本，不宣称为上游最新状态。

## 已考虑但未采纳

- 默认复制 Deskflow 的整格输出：会损失 hidpipe 已保存的亚格输入，且 Deskflow 的整数缩放受事件分批影响。新证据已确认现有小数LINE在Ghostty被逐事件放大，003采用完整PIXEL事件与亚像素余量修正。
- 根据 Deskflow EI 路径直接翻转 evdev 方向：两者输入约定不同，不能据此推出 hidpipe 必须取反。
- 在没有应用证据时将主滚轮切为PIXEL：此前暂缓；后续Ghostty同版本源码和Mac AppKit探针已提供依据，用户已授权003。不引入惯性动画。
- 统一拇指轮与普通滚轮所有参数：当前分工符合目的端 OpenLogi 绑定架构，另做协议/注入设计的成本尚无需求支持。
- 将 fixed-point 字段存在等同于所有应用都平滑：代码静态事实不足以证明应用响应，需要实测。

## 覆盖边界

只调查滚轮采集、协议单位、macOS 注入、OpenLogi 主/拇指轮分工及 Nix 参数生成。实现阶段已验证 Nix 服务参数生成、协议 fixture 的 Linux/Darwin 边界行为及真实 Mac CGEvent 构造。003已用原签名身份安装到Mac并重启客户端、确认生产连接恢复，三应用新版反馈待确认。未进行全仓安全审计，未发布 issue 或向主分支提交；未修改远端dirty源码或Nix系统配置。
