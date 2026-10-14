# 0.1.0 验证记录

验证日期：2026-10-14。工作区：`D:\Code\rustdesk-p2p\rustdesk-account-server`。已按用户后续授权部署到 `https://47.108.237.195`，账号由隐藏输入命令建立；密码策略按用户要求改为非空且最多 1024 字节，无最低长度/复杂度。密码存储、限流、HTTPS 等措施保留。

## 已验证

| 项目 | 结果及证据 |
|---|---|
| Windows 服务端测试 | Rust 1.98.1，`cargo test --release --locked`：15 项集成测试全部通过。这是 API 服务端测试，不是 Windows RustDesk GUI 验收。 |
| Linux 服务端测试 | WSL Ubuntu-22.04，Rust 1.99.0：16 项全部通过，额外覆盖 Unix 密钥权限。 |
| 静态检查 | `cargo fmt --all -- --check`、`cargo clippy --all-targets --locked -- -D warnings` 通过。 |
| 公开客户端源码契约 | 官方 1.5.0 固定提交 + 本地 1.5.1，共 12 份源码文件的接口/字段标记核对通过，SHA-256 保存在 `test-results/client-source-contract.json`。 |
| Docker 镜像 | Debian bookworm / Linux amd64 构建成功。镜像 `rustdesk-account-server:0.1.0`。 |
| 容器实际运行 | 非 root `10001:10001`、只读根目录、只读独立密钥挂载、丢弃全部 capabilities、no-new-privileges 下，通过真实 HTTP 登录及地址簿读写。 |
| 管理 CLI | 容器内部真实终端验证账号创建/密码确认，密码输入未回显；在线一致性备份命令成功。 |
| 重启持久化 | 镜像停止/启动后同一令牌仍可刷新账号；服务端恢复测试也验证地址簿凭据可解密。 |
| 配置检查和 HTTPS | Compose 展开检查和 Caddy 2.11.7 `validate` 通过。公网使用原生 systemd；已获得 Let's Encrypt IP 证书，本机、max、学校 Windows 均以正常证书校验访问 `/healthz` 成功。 |
| 依赖审计 | `cargo-audit 0.22.2`，RustSec 数据库包含 1314 条公告，发现漏洞 0，警告 0。检查只表示当时数据库中未命中已知依赖漏洞，不是安全认证。 |
| 客户端与共享库 | 本次未改 `rustdesk`、`hbb_common` 源码；两端只配置 API 地址，原 ID/Relay/Key 保留。 |
| hbbs 兼容修复 | 用户授权后新增独立握手模块及测试。原文件仅 `lib.rs` +1 行、`rendezvous_server.rs` +17 行；未更新子模块。14 项库测试通过，4 项针对新模块。公网独立探针通过签名、KX v1、双向密钥及加密 NAT；传统 NAT 和 native P2P 协调/邀请通过。 |
| 真实 Windows 登录与连接 | 用户在两端原生账号窗口登录成功。补丁部署后用户确认学校电脑可解锁，中继与 P2P 均可连接；服务端接收到标记为客户端报告的连接记录。两端设备列表可见已获用户确认。 |
| 真实个人地址簿及文件传输 | 用户在本机添加学校设备并修改别名，确认学校端出现同一条目和别名；服务端可信操作记录包含 peer.add / peer.update。用户确认 P2P 文件传输成功，本次未独立核对传输文件的哈希。 |
| 公网 API 检查 | 登录/currentUser、空 POST、重启保留登录、退出后 401、未认证 401、超大请求 413、非 API 路径 404、伪造代理头与未知设备上报检查通过；原始状态码见 `test-results/public-deployment.json`。API-only 测试会话已撤销，只保留两端真实登录会话。 |

测试覆盖登录缺省及 text/plain Content-Type、空 POST、200 空写响应、分页、错误 JSON；账号隔离、猜测 GUID/条目 ID、所有共享只读写接口、权限撤销、密码重置/禁用/会话撤销；设备转移/冲突、刷新不能抢归属、私有凭据不迁移；未知上报、字段白名单、报告去重、心跳落库合并；代理头伪造、账号/IP/全局限流、缓存上限、超大请求、对象数量和日志保留；并发字段合并、事务回滚、备份恢复、错误/缺失密钥、AEAD 字段绑定、数据库/WAL/备份与日志不含测试密码/令牌/远控凭据明文。

## 资源实测

测试由 `scripts/measure.py` 完成，Linux release API 原生进程，WSL2 Ubuntu-22.04。临时 SQLite 位于 `/tmp`（此主机为 tmpfs），仅测 API 进程的 `/proc/PID/status` RSS，不含测试客户端、Caddy、hbbs、hbbr。5 ms 采样并记录内核 VmHWM。

负载：15 个测试设备依次登录；15 台每 3 秒发一次心跳，模拟活跃连接频率；每轮地址簿局部更新后由另一个会话读取验证。持续 **180 秒**，60 轮、900 次心跳，错误 **0**。

| 指标 | 实测 |
|---|---:|
| 空闲 RSS | 6876 KiB，约 6.7 MiB |
| 依次登录阶段 RSS 峰值 | 26468 KiB，约 25.8 MiB |
| 持续负载 RSS 峰值 / 结束 RSS | 26952 KiB，约 26.3 MiB |
| 整个进程 VmHWM | 26952 KiB |
| 15 次串行登录耗时 | 229 ms |
| 心跳请求 P95 | 8.194 ms |
| 持久化设备数 / SQLite 完整性 | 15 / ok |
| 空闲 ≤64 MiB、登录 ≤128 MiB | 均达到本次测试目标 |

基准只把登录限额调高到 60，Argon2 参数、密码并发 1、请求/DB 并发及数据行为保持默认。生产默认每账号 8 次登录/分钟。机器性能、文件系统、容器限制和后续使用会影响延迟及 RSS；此次是 3 分钟稳定性检查，未做数日运行或公网压测。原始数据见 `test-results/resources-linux.json`。

公网服务器原生进程快照：API RSS 25740 KiB（25.1 MiB），Caddy RSS 50248 KiB（49.1 MiB），新增握手的 hbbs RSS 11136 KiB（10.9 MiB）。该快照是登录后的驻留值，不是公网登录峰值测量；登录峰值仍以上述隔离基准为依据。API 21114 只监听 127.0.0.1。

## 交付物

- `dist/rustdesk-account-server-linux-x86_64`：4376848 字节，Docker bookworm 构建产物；Linux 动态链接 glibc/libm/libgcc_s，无需安装 SQLite。已在 WSL Ubuntu-22.04 和公网 Ubuntu 运行。
- `dist/rustdesk-account-server-0.1.0-image.tar`：30398464 字节，可用 `docker load -i` 导入。
- 镜像 ID：`sha256:59c8c2e695a131378f7a315d89991a3f984e826c0b91353c8a8ad2348f6f9290`。
- 已部署 API SHA-256：`dfcbdb9358911d8ce73ad701a5b2f718b0696ceb15c700305e0b96d45cc62309`；hbbs SHA-256：`fb27f23f6a66af2fde641a5e16a8a73b300f8fda00323002c7a66700595aa2c8`。
- `dist/SHA256SUMS`：导出物 SHA-256；源码、部署示例、文档和验证数据另附源代码压缩包。
- `test-results/container-smoke.json`、`test-results/dependency-audit.json`：容器与依赖检查结果。

Docker 冒烟测试临时容器、数据卷已清理，基准 API 已退出。公网保留正式账号、API、Caddy 和启用兼容握手的 hbbs。hbbs 原二进制保留，新的 systemd 服务启用开机启动；原有 21116 服务和 hbbr 未改。

本地 P2P 探针曾失败：回环地址被原协调器的公网来源约束拒绝；旧探针还使用过时协议。改为当前源码构建的探针并使用公网入口后，双方注册、明文 TCP 协调和 UDP 邀请均成功，见 `test-results/hbbs-native-public.json`。这项探针本身没有验证跨网数据传输；真实连接结果单独依据用户实测记录。

验收后以 SQLite Backup API 生成 `/var/lib/rustdesk-account/backup-accepted-20261014.sqlite3`，下载到本机受限 ACL 目录 `C:\Users\Gaoxi\.rustdesk-account-backups\20261014-accepted\database\accounts.sqlite3`。主密钥另存同级 `key` 子目录，未放进源码、镜像或交付压缩包。两者虽然分文件存放，当前仍在同一台机器，不等同于异地容灾。备份含两端仍有效会话；恢复后撤销策略见部署说明。

## 尚未验证，不计为通过

| 项目 | 当前状态和所需验证 |
|---|---|
| 官方安装包 / GUI 重启退出 | 两台已测 Windows 为当前带 P2P 扩展的 1.5.1，复用原生账号和管理 UI；不能等同于未修改官方 1.5.0 安装包验收。真实 GUI 重启保持登录、退出未单独验证，API 层已验证。 |
| 更多地址簿操作 | 设备清单、个人条目及别名双端同步已通过。标签/备注和共享只读/读写的真实 GUI 验收尚未完成，API 测试已覆盖。 |
| 保存密码后的真实远控 | 未实测。需被控端设置测试密码，从个人及共享地址簿发起连接，核对原版凭据同步行为。不能以 API 返回正确密码代替这一验收。 |
| 文件传输完整性 | 用户确认 P2P 文件传输成功；独立哈希比对和中继文件传输未单独验证。HTTP 文件操作上报测试不等于传输测试。 |
| Android 真机 | 未实测，本次没有接入隔离 Android 真机。须验证登录、设备/地址簿同步、连接和文件传输。 |
| 证书自动续期与长时间稳定性 | 初次签发及多端受信任 TLS 已通过；证书自动续期尚未经历一个完整周期，多日持续运行也未验证。 |

真实客户端验收时，另外检查：换账号登录后旧会话 401、旧地址簿秘密仍只属于旧账号；共享授权撤销后重新拉取失败，但已缓存密码仍需更改被控端密码才能撤销远控；遇到 ID/UUID 冲突不会自动覆盖，需要 CLI 明确处理。
