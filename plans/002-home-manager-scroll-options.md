# Plan 002: 将现有滚轮参数接入 Home Manager

> 执行者：按步骤执行并核对结果，遇 STOP 条件停止。完成后更新 `plans/README.md`，不部署到用户机器。
>
> Drift check：`git diff --stat 5e0f7ff..HEAD -- nix/modules/home-manager.nix client/src/main.rs README.md nix/tests/scroll-options.nix`。如有变化，先比较下方摘录；行为不匹配则刷新计划。

## Status

- Priority: P2
- Effort: S
- Risk: LOW
- Depends on: none；建议在 001 取得行为证据后使用新选项调参，但本项不改变默认行为，可独立实现。
- Category: dx
- Planned at: commit `5e0f7ff`, 2026-11-06
- Status: DONE

## 执行与复核记录（2026-11-06）

- 隔离执行提交：`baa55d3`，倍率序列化修正：`647981d`。已将批准的三文件差异集成到主工作区，未创建主分支提交或部署。
- `builtins.toJSON` 保留倍率精度；不使用会将 `0.0000001` 变成 `0.000000` 的 `toString`。
- 真实 `lib.evalModules` fixture：defaults、changed、zero、invalid、combined、linux、serialization 七组 PASS；reviewer 独立复跑通过，集成后再次通过。
- 两个 Nix 文件 parse、formatter check、diff check 通过。执行者还用 Rust `parse::<f64>()` 检查序列化值的数值往返。
- 未运行完整 HM activation、服务部署或 Mac 应用实测；本项只验证配置生成。

## 为什么做

macOS CLI 已支持每轴倍率和反向，但 Home Manager 的 launchd ProgramArguments 没有接入它们。使用托管服务时用户无法声明这些配置。只连接现有能力，保留默认倍率 1、反向 false；不改滚轮算法，也不把主滚轮参数误称为所有鼠标动作的全局参数。

## 当前状态与约定

- `nix/modules/home-manager.nix` 同时支持 Linux systemd 和 macOS launchd；`isDarwin = pkgs.stdenv.hostPlatform.isDarwin`。
- 同文件 `inputFeedback` 使用 `lib.mkOption { type = lib.types.bool; default = false; ... }`；launchd 参数使用结构化列表和 `lib.optionals` / `lib.optional`，不要拼接 shell 字符串。
- `nix/modules/home-manager.nix:151` 当前关键摘录：

```nix
ProgramArguments = [
  clientExecutable
  cfg.host
  "--port"
  (toString cfg.port)
]
# OpenLogi / clipboard optionals ...
++ lib.optional cfg.inputFeedback "--input-feedback"
++ lib.optional cfg.latencyStats "--latency-stats";
```

- `client/src/main.rs:637` 已提供：`--invert-scroll-x`、`--invert-scroll-y`、`--scroll-scale-x <f>`、`--scroll-scale-y <f>`。
- `client/src/main.rs:657` 默认 `scale_x/scale_y = 1.0`、`invert_x/invert_y = false`。
- `client/src/main.rs:734` 的 `parse_scale` 接受 finite 且非负的 f64，0 合法。Nix 类型不能无端拒绝整数写法 1 或允许负数。
- 普通 evdev Wheel 和 OpenLogi PointerEvent::Wheel 都使用 `Injector::wheel`。HID++ ThumbWheel 则由 `nix/openlogi-bridge.patch:678` 解析，`Effect::Scroll` 在该 patch:260 直接调用 OpenLogi injector；不受 CLI 滚轮参数控制，这是目的端 profile 解析的现有设计。
- README 的 module options reference 用表格描述选项；Scroll behavior 描述 120 单位/格和目的端处理。保留这个术语与架构。
- `flake.nix` 仅包含 nixpkgs input；不要为了 module 测试引入 Home Manager input 或更改锁文件。

## 范围

允许修改：

- `nix/modules/home-manager.nix`
- `README.md`（仅滚轮 CLI、module options、配置例子及主/拇指轮边界）
- `nix/tests/scroll-options.nix`（新增，实际 module evaluation 合约验证）
- `plans/README.md`（状态）；本计划只补充执行证据。

禁止修改：`client`、`server`、`proto`、其他 Nix modules、OpenLogi patches、flake inputs/lock、包版本、签名逻辑、任何 `/home/woxQAQ/nixos-config` 文件。不要切换系统、重启服务、修改 Accessibility 设置。

## Git workflow

使用隔离分支 `advisor/002-scroll-options`。仓库历史是简短描述，例如 `update readme`；若需要提交可用 `add Home Manager scroll options`。未经指示不 push、开 PR 或部署。

## 命令

| 目的 | 命令 | 成功标准 |
|---|---|---|
| 查看基线模块 | `nix-instantiate --parse nix/modules/home-manager.nix` | exit 0；现有模块语法检查 |
| 检查新增评估 fixture | `nix-instantiate --parse nix/tests/scroll-options.nix` | exit 0 |
| 真实模块评估 | `nix eval --impure --json --expr 'let f = builtins.getFlake (toString ./.); in import ./nix/tests/scroll-options.nix { nixpkgs = f.inputs.nixpkgs; }'` | exit 0，JSON 列出所有用例 PASS |
| 格式检查 | `nix fmt -- --check nix/modules/home-manager.nix nix/tests/scroll-options.nix` | exit 0；使用 flake 指定的 nixfmt |
| 差异检查 | `git diff --check` | exit 0 |

新增 fixture 的评估命令是待实现接口；现有 formatter 来自 flake.nix。规划阶段未运行上述验证。`--impure` 仅用于解析当前 checkout 的路径及锁定的 getFlake，不应引入未锁定远程源码。以上都是可在 Nushell 执行的独立命令。

## 步骤

### 1. 添加四个有明确边界的 Home Manager 选项

在 `options.services.hidpipe` 内加入：

| 选项 | 类型 / 默认 | 描述 |
|---|---|---|
| `scroll.scaleX` | integer 或 float，非负，默认 `1.0` | macOS 普通横向 Wheel 倍率；0 禁用该轴 |
| `scroll.scaleY` | 同上，默认 `1.0` | macOS 普通纵向 Wheel 倍率 |
| `scroll.invertX` | bool，false | 翻转普通横向 Wheel |
| `scroll.invertY` | bool，false | 翻转普通纵向 Wheel |

使用 `lib.types.addCheck (lib.types.either lib.types.int lib.types.float)` 或锁定 nixpkgs 已有的等价类型，明确非负检查；不要引入自建配置解析器。类型对负数、字符串、null 拒绝。配置可以在 Linux HM 求值，但不生成 server 滚轮参数；说明该组选项只在 Mac 注入生效。保持类型与现有 CLI 接受值一致；Nix 可表达的非有限值若存在也要拒绝，不能新增 CLI 不接受的值。

**Verify**：模块 parse exit 0；步骤 3 的负值/类型验证通过后此步才算完成。

### 2. 将非默认配置加入 macOS 参数列表

使用结构化 optionals，仅在 `scaleX != 1.0` / `scaleY != 1.0` 时加入各自 `[ "--scroll-scale-x" (toString cfg.scroll.scaleX) ]` 和 y。使用 `lib.optional` 加反向 flag。这样默认 ProgramArguments 完全不变；scale=0 不可被当作 false 省略。顺序固定且每个 flag 最多出现一次。

不要改 signed bundle 路径、OpenLogi/clipboard 参数、inputFeedback/latencyStats 的语义；Linux server ExecStart 不接入这些参数。

**Verify**：模块 parse exit 0；步骤 3 比较实际 ProgramArguments，不以文本 rg 代替结果验证。

### 3. 用模块求值验证生成的服务配置

新增 `nix/tests/scroll-options.nix`，接口 `{ nixpkgs }:`。使用传入的锁定 nixpkgs，分别构造 `aarch64-darwin` 与 `x86_64-linux` pkgs，运行 `lib.evalModules` 并导入真实的 `../modules/home-manager.nix`。

不要复制参数生成逻辑或只测 helper。由于仓库没有 HM input，fixture 可只声明外部模块需要的容器选项：`home.homeDirectory`、`home.packages`、`home.activation`、`launchd.agents`、`systemd.user.services`、`assertions`；容器可用 attrsOf anything，真实 hidpipe 选项保持原类型。明确注释这是 service 输出边界的测试，不能替代完整 HM 集成。

测试配置设 host、homeDirectory，关闭 `localSigning.enable`，用 `pkgs.hello` 等轻量已有 package 显式覆盖 hidpipe 与 OpenLogi package，避免构建真实程序。保持 package 的 mainProgram 可由 lib.getExe 解析。使用 `builtins.deepSeq` 强制目标输出；错误用 `throw`，返回结构化的用例名/PASS JSON。

至少验证：

1. Darwin 默认参数与改动前列表一致，无新 flags。
2. Darwin scaleX=0.5、scaleY=2（整数写法）和两个 invert=true，flag/value 正确、无重复。
3. scale=0 的参数保留，未改轴不添加 flag。
4. 负数、字符串、null 通过 `builtins.tryEval` + 强制参数求值被拒绝；错误不可悄悄回落默认。
5. OpenLogi、clipboard、feedback、latency 一起开启，新旧参数都存在且次数正确；fixture 不启动服务或 socket。
6. Linux 即使配置同样 scroll 值，真实 server ExecStart 中没有任何 scroll flags；不会额外生成 Mac hidpipe job。

**Verify**：完整 `nix eval` 命令 exit 0，JSON 六组均 PASS。不允许为通过 fixture 而削弱生产选项类型或更改其他 modules。

### 4. 更新用户文档

在 README 的 client CLI 表补上已有四个参数；在 module options 表添加四项，注明 macOS、默认值及 0 的含义。示例：

```nix
services.hidpipe.scroll = {
  scaleX = 1.0;
  scaleY = 0.5;
  invertX = false;
  invertY = false;
};
```

只把 0.5 描述为个人调参例子，不宣称是新推荐默认。Scroll behavior 区分普通 Wheel 与 OpenLogi HID++ 拇指轮；后者仍在 Mac OpenLogi profile 中调 sensitivity/绑定。fixed-point 保存小数这一实现事实可以写，不能在未取得应用证据时声称所有应用都平滑。

**Verify**：运行 format check、module eval 和 `git diff --check`，全部 exit 0；检查 README 四个名称与评估后的 options 一致。

## Test plan / Done criteria

- [ ] Darwin 默认参数逐项等于旧列表。
- [ ] 小数、整数、零倍率与反向正确序列化，旧功能组合不丢参数。
- [ ] 无效类型与负值在 Nix 求值时失败。
- [ ] Linux ExecStart 不收到 Mac-only flags。
- [ ] 新增 fixture 使用真实模块求值，没有镜像参数构造函数的测试。
- [ ] 两个 Nix 文件 parse、module eval、nixfmt check、diff check 全部 exit 0。
- [ ] README 描述拇指轮不受此组选项控制。
- [ ] `git diff --name-only` 与 `git status --short` 仅含 scope 内修改。
- [ ] 更新计划索引状态并记录实际执行命令与结果。

不增加 Rust 函数单元测试，不做无关构建或部署。配置连线测试不能用来声称滚轮应用表现验证已完成。

## STOP conditions

- CLI 参数或 ScrollModifier 当前行为与摘录不一致。
- 锁定 nixpkgs 的求值/类型行为导致难以安全表达非负有限值，或必须引入 HM input、改 flake.lock 才能验证。
- fixture 需要实际签名、启动服务、设备或访问用户系统配置。
- 两次合理修正后仍无法通过检查，或必须修改 scope 外文件。

## Maintenance notes

未来修改 CLI 名称或默认倍率，须同步 Nix 映射和实际 module evaluation。不要把 scaleY 当 DPI，也不要混入 thumbwheel sensitivity。若后续统一拇指轮与主滚轮注入，应另做设计并验证目的端绑定架构，不在本项顺手改桥接。
