# macOS 本地签名与辅助功能权限

## 问题与实现

Mac 上原来的 hidpipe-client 使用 linker ad-hoc 签名。`codesign -d -r-` 显示其 designated requirement（DR，系统用来识别程序的匹配规则）只有 `cdhash H"…"`。二进制变化即改变 DR，launchd 同时直接引用会随 rebuild 改变的 Nix-store 路径。

Home Manager 现在默认开启 `services.hidpipe.localSigning.enable`，在用户本地安装两个真实 bundle：

| 应用 | 固定 bundle ID | 可执行文件 |
|---|---|---|
| `~/Applications/Hidpipe.app` | `io.github.woxqaq.hidpipe.client` | `Contents/MacOS/hidpipe-client` |
| `~/Applications/OpenLogi Agent.app` | `io.github.woxqaq.hidpipe.openlogi-agent` | `Contents/MacOS/openlogi-agent` |

安装脚本 [`nix/macos/install-apps.sh`](../nix/macos/install-apps.sh) 在首次激活时生成本地自签名代码签名证书，导入独立 Keychain。状态保存在 `~/Library/Application Support/hidpipe/signing`，目录 0700，Keychain 与密码文件 0600。私钥在导入后通过临时目录清理；持久身份由 Keychain 保存，不进入 Nix store。脚本显式选择、临时解锁并最终锁定这个 Keychain，不要求登录 Keychain 解锁，也不修改系统信任设置。

DR 显式包含固定 bundle ID 与证书指纹，不包含代码哈希。两个应用共享证书，但 bundle ID 不同。需要保留整个签名状态目录；已有状态不完整时安装失败，不自动轮换身份。证书有效期十年，轮换需要单独安排权限迁移。

源码：[`nix/modules/home-manager.nix`](../nix/modules/home-manager.nix)。签名安装在 `writeBoundary` 之后、`setupLaunchAgents` 之前执行。应用先在私有目录中签名并验证，再复制到目标文件系统替换，替换失败时恢复旧 bundle。源码与安装脚本未变化且签名验证通过时不更新应用。launchd 环境中的 `HIDPIPE_SOURCE_PACKAGE` 保留源 package 路径，使 Home Manager 在 package 更新时重新加载固定路径的服务。

OpenLogi 的 executable watcher 原本会在二进制更新时通过 LaunchServices 自行重启，可能丢失 bridge 环境，并与 launchd 竞争。补丁 [`nix/openlogi-managed-restart.patch`](../nix/openlogi-managed-restart.patch) 在 `OPENLOGI_MANAGED_SERVICE` 存在时关闭该 watcher，由服务管理器重载。

## 已验证与未验证

在用户的 macOS 27.0.1 / arm64 上运行 [`nix/tests/macos-signing.sh`](../nix/tests/macos-signing.sh)，测试使用隔离的临时目录和证书：

- 两个不同 bundle ID 可以签名、验证。
- 相同版本重复安装不替换应用目录。
- 两个实际不同的 Mach-O 程序升级后，CDHash 改变、各自 DR 不变。
- 回滚保持原身份。
- 缺失已有证书时失败，不替换应用或生成新身份。

真实 Hidpipe 与 OpenLogi Agent 也已安装并通过 `codesign --verify --strict`；Hidpipe 的 `--help` 可运行。模块求值验证了启用时固定路径、激活顺序依赖，以及 Linux / 显式关闭签名时没有安装步骤。

这些验证证明代码身份稳定，不能替代 TCC 权限继承实测。首次从旧 executable 迁移到 bundle 时，用户仍需在辅助功能设置里授权两个新应用；之后再升级并观察 `CGPreflightPostEventAccess` 与实际注入，才能确认当前 macOS 版本上的权限复用。旧条目不由安装器重置或删除。

## 依据

Apple [Technical Note TN2206](https://developer.apple.com/library/archive/technotes/tn2206/_index.html) 的 Trust and Code Signing、Code Designated Requirement、Self-signed Identities 章节解释了子系统各自的信任策略、DR 与跨版本身份的关系，以及自签名身份的边界。它不是当前所有 macOS 版本 TCC 行为的保证，因此最终权限继承仍需运行时验证。
