# Ghostty 滚轮放大：现场定位记录

日期：2026-11-06。此记录补充 plans/001，不代表完整 macOS 滚轮验证完成。

## 用户观察

Mac Ghostty、微信聊天记录滚动过于灵敏，微信公众号正常。此前浏览器的 quarters 小步观察只确认页面移动，没有证明滚动距离正确。微信内部闭源实现尚未核实，不能把 Ghostty 的具体算法直接归因给微信。

## 当前运行态与源码证据

- Mac 地址 `192.168.31.240`，Ghostty 的 CFBundleShortVersionString 为 `1.3.1`。
- Mac Ghostty 配置未检出 `mouse-scroll-multiplier` 显式覆盖；同版本上游默认 precision=1、discrete=3。
- [v1.3.1 macOS 前端](https://github.com/ghostty-org/ghostty/blob/v1.3.1/macos/Sources/Ghostty/Surface%20View/SurfaceView_AppKit.swift#L1034) 读取 NSEvent.scrollingDeltaY 与 hasPreciseScrollingDeltas，转交 core；精确事件另有2倍缩放。
- [v1.3.1 Surface.zig](https://github.com/ghostty-org/ghostty/blob/v1.3.1/src/Surface.zig#L3391) 的非精确分支会先把非零 `abs(yoff)<1` 提升到1，再乘 cell height 与 discrete multiplier；随后按单元格换算行数。
- [v1.3.1 Config.zig](https://github.com/ghostty-org/ghostty/blob/v1.3.1/src/config/Config.zig#L976) 记录 discrete 默认3、precision默认1。
- 当前系统配置 `home/public/terminal/ghostty-fractional-scroll.patch` 在 Linux 取消上述提升并修正余数处理；`ghostty.nix` 仅 Linux 源码包应用该补丁。Darwin 使用官方 ghostty-bin，未应用补丁。此为当前代码事实，不推断补丁最初的讨论意图。
- 当前 hidpipe 生产 wheel 注入为 LINE，`delta/120` 后修改 fixed-point 字段，不标连续。真实四次 dy=30 的 HID tap 验证为4个 fixedY=.25、lineY=0、pointY=0、continuous=0，总 fixedY=1。

## 不发送输入的 Mac AppKit 转换探针

使用 `CGEvent(scrollWheelEvent2Source:units:wheelCount:wheel1:wheel2:wheel3:)` 构造事件，再调用 `NSEvent(cgEvent:)`；没有调用 post，不改变应用。

| 构造 | lineY | fixedY | pointY | NSEvent.scrollingDeltaY | precise |
|---|---:|---:|---:|---:|---|
| LINE整数0后覆写fixed为1/120 | 0 | 0.008331298828125 | 0 | 0.008331298828125 | false |
| LINE整数0后覆写fixed为.25 | 0 | .25 | 0 | .25 | false |
| LINE整数0后覆写fixed为.5 | 0 | .5 | 0 | .5 | false |
| LINE1后覆写fixed为1 | 1 | 1 | 10 | 1 | false |
| LINE2后覆写fixed为2 | 2 | 2 | 20 | 2 | false |
| PIXEL1，字段不覆写 | 1 | 0.0999908447265625 | 1 | 1 | true |
| PIXEL2，字段不覆写 | 1 | 0.1999969482421875 | 2 | 2 | true |
| PIXEL10，字段不覆写 | 1 | 1 | 10 | 10 | true |

## 可确认的结论

Ghostty 存在事件分批相关放大：默认设置下，原始120单位分成4个30时，4个.25非精确事件各被提升为1、各产生3行，总12行；同总量一次120会产生3行。事件越细、数量越多，放大越严重。单纯降低 hidpipe scale，只要结果仍是非零小数，该提升行为仍存在，不能保证改善。

这不是凭空假设字段读取方式：AppKit 探针已证明当前CGEvent转为 non-precise + fractional scrollingDeltaY，Ghostty同版本源码明确处理该组合的方式。不过尚未在Ghostty中做受控whole/quarters屏幕行数对照，完整现场量化仍待验证。

## 修正方向（尚未实施）

优先评估在 macOS 注入边界生成完整的 PIXEL 事件，由 Quartz 生成一致字段，协议仍用120单位/格。若像素字段需要整数，则在连接作用域按轴累计亚像素余量，避免逐事件截断；测试同总量不同分批在输出像素总量上等价，断连/ReleaseAll清空余量。避免只翻 continuous 标记却保留原先单位和字段，也避免应用名称特例或全局倍率补偿隐藏取整问题。

像素/格比例是目的端的单位映射选择，不由现有数据证明唯一值；Quartz的LINE1探针对应point10可作为明确的候选基线，但Ghostty精确路径另有2倍偏好，不能假设所有应用实际像素距离一致。应以Ghostty、微信聊天记录、公众号/浏览器作实际对照后决定。

## 未覆盖

未取得普通物理滚轮在各应用下的 live-scroll.log；未核实微信聊天记录控件算法；未改生产注入、倍率默认、系统配置或应用权限。独立临时 GUI launchd 测试client已bootout，原有生产client继续运行。
