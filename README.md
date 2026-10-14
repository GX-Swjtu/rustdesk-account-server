# RustDesk 个人账号服务

为原版 RustDesk 客户端提供账号、设备清单、个人地址簿和共享地址簿。独立 Rust/Axum 进程，SQLite 文件存储，不依赖 Redis、外部数据库或网页后台。此项目是独立的兼容实现，不是官方 RustDesk Server Pro。

账号服务不访问 hbbs 数据库，复用客户端原生账号界面，hbbr 和 P2P 协议无需修改。**登录后发起远控还需要 hbbs 支持客户端要求的加密握手**；仅有本 API 搭配未补齐握手的 OSS hbbs，会出现 `Failed to secure tcp`。公网令牌保护还需要下文说明的客户端安全修复。相邻服务端的独立兼容模块和部署开关见 [握手兼容说明](../rustdesk-server/docs/account-handshake.md)。ID/中继服务器仍负责发现、穿透与中继，账号 API 负责账号和管理数据。兼容接口依据官方 **1.5.0** 和当前本地 **1.5.1** 源码；实际验证范围见 [验证记录](docs/VALIDATION.md)。

## 能做什么

- 原生账号密码登录、保持登录、退出；账号由服务器 CLI 创建，没有公开注册。
- 成功登录后，本机出现在当前账号的原生设备清单。使用原生菜单将需要的设备加入地址簿。
- 个人地址簿同步设备、别名、备注、标签、颜色、连接设置和原生可复用连接凭据。
- 共享地址簿由管理员建立并授予只读/读写权限，日常条目编辑在原版客户端进行。只读授权仍允许使用保存的连接密码。
- 管理员查看/撤销会话、禁用账号、重置密码、解绑或转移设备、处理 ID/UUID 冲突、查询日志、备份和恢复。

新账号的个人地址簿初始为空，这是正常行为。登录后设备会自动出现在“设备列表”；要保存到地址簿，在设备卡片菜单选择“添加到地址簿”，或在地址簿菜单选择“增加 ID”。另一端刷新后可同步条目和别名。

账号登录**不会授予免密远控权限**。连接仍要求被控端密码或人工确认；从地址簿保存并同步凭据后，客户端可以复用这些凭据。个人凭据同步还需打开客户端的“与最近会话同步”选项（源码配置键 `sync-ab-with-recent-sessions`）。共享默认密码只在从该共享地址簿连接时使用，设置它不会改变被控端的实际密码。

暂不实现企业组织/设备组、连接级 ACL、集中策略、OIDC/TOTP、远程强制断开、Web 管理或旧版整本地址簿覆盖协议。设备在线状态仍由 RustDesk ID 服务判断，API 的设备状态字段不是在线证明。

## 构建与本地运行

需要 Rust 1.98 或更新版本、C 编译器。SQLite 随程序构建。交付的 Linux x86_64 文件和本地 Docker 镜像见 `dist/` 与验证记录；也可自行构建：

```sh
cargo build --release --locked
cargo test --release --locked
cargo clippy --all-targets --locked -- -D warnings
cp config.example.toml account.toml
./target/release/rustdesk-account-server --config account.toml init
./target/release/rustdesk-account-server --config account.toml user create myname --generate
./target/release/rustdesk-account-server --config account.toml serve
```

密码管理必须在交互终端执行。省略 `--generate` 则隐藏输入并确认；按个人部署需求，不强制最低长度或复杂度，密码仍须非空、最多 1024 字节。不接受命令行明文密码或重定向输入。`--generate` 生成 256 位随机密码，只在当前终端显示一次，请立即存入密码管理器。不要把示例测试账号用于真实服务。

默认仅监听 `127.0.0.1:21114`。本地测试客户端 API 地址可填 `http://127.0.0.1:21114`；公网必须使用 HTTPS，步骤见 [部署说明](docs/DEPLOYMENT.md)。不要在 API 地址末尾加 `/api`。

Windows 开发构建使用 `target\release\rustdesk-account-server.exe`。Linux 会检查密钥权限必须为 0600/0400。Windows 上还须用 NTFS ACL 限制数据和密钥目录只能由当前服务账号及管理员访问；提供 Linux 容器和原生 systemd 两种部署配置。

## 管理命令

以下用 `ras` 代表 `rustdesk-account-server --config account.toml`。容器执行方式见部署说明。命令中的用户名、设备 ID、会话 ID、地址簿 GUID 是不同的标识。

| 操作 | 命令 |
|---|---|
| 账号列表 | `ras user list` |
| 创建账号 | `ras user create alice` 或 `ras user create alice --generate` |
| 重置密码并撤销全部会话 | `ras user password alice --generate` |
| 禁用/启用 | `ras user disable alice` / `ras user enable alice` |
| 会话列表 | `ras session list --username alice` |
| 撤销单个/全部会话 | `ras session revoke SESSION_ID` / `ras session revoke-all alice` |
| 设备列表 | `ras device list` |
| 解绑设备并撤销其会话 | `ras device unbind DEVICE_ID` |
| 转移设备并撤销其全部会话 | `ras device transfer DEVICE_ID --owner bob` |
| 解决机器标识冲突 | `ras device resolve DEVICE_ID --uuid NEW_UUID --owner alice` |
| 创建共享地址簿，输出 GUID | `ras book create Family --owner alice` |
| 共享地址簿列表 | `ras book list` |
| 改名/删除 | `ras book rename GUID NewName` / `ras book delete GUID` |
| 只读/读写/取消授权 | `ras book grant GUID bob read` / `write` / `none` |
| 设置/清空默认连接密码 | `ras book password GUID` / `ras book password GUID --clear` |
| 可信服务端操作记录 | `ras logs --limit 100` |
| 未验证的客户端上报 | `ras logs --client-reports --limit 100` |
| 在线一致性备份 | `ras backup NEW_BACKUP_FILE` |
| 向新路径恢复 | `rustdesk-account-server --config restore.toml restore BACKUP_FILE` |

共享地址簿所有者具有完整条目编辑权限；管理创建/删除/成员授权仍只开放 CLI。共享名称必须唯一；创建和改名均拒绝客户端保留的“My address book”和“Legacy address book”，避免覆盖原生个人地址簿入口。命令不会输出现存密码、令牌或加密主密钥。失效会话在后续登录时清理，撤销历史保留在服务端操作记录中。

## 安全和数据行为

密码采用 Argon2id（19 MiB、2 次、并行度 1），默认同时校验 1 个。登录令牌为 256 位随机数，数据库只存 SHA-256 哈希；按本项目需求无时间过期。每次 API 请求检查当前账号、会话、设备关联和地址簿授权，禁用/重置/撤销立即阻止后续请求。没有永久账号锁定，限流窗口为 60 秒。

请求默认最多全局 32 个、每个来源 IP 4 个并发（`max_concurrent_requests_per_ip`），读取请求体前即检查额度；请求结束、取消或超时后释放。来源 IP 只由明确配置的可信代理头确定。代理部署必须正确设置 `trusted_proxies`，否则全部客户端会共享代理 IP 的额度。每 IP 上限应小于全局上限，保证单一来源的慢请求不会耗尽全部处理槽位；同一 NAT 后的客户端共享额度。

账号 API 的 HTTPS 不保护客户端与 hbbs 之间的独立信令连接。原版客户端的兼容握手允许服务器未完成加密时继续发送账号令牌；仅添加 hbbs 握手无法防止中间人利用该降级。本仓库配套客户端修复在打洞、中继申请、账号保活三处强制检查传输加密，拒绝明文 WebSocket；HTTPS 失败后的 TCP API 代理备用路径也须确认加密后才能发送密码或令牌。安全使用账号需要安装包含此修复的客户端，并使用匹配的 hbbs 公钥；未更新的原版客户端仍有此限制。

同一 ID + UUID 用另一个账号成功输入密码登录时，设备清单归属转移，并撤销该设备原有账号的会话。仅带旧令牌刷新、上报或心跳不能转移归属。ID 相同但 UUID 不同时返回冲突，需管理员核对后 `device resolve`。ID/UUID 是客户端自报的清单标识，不是密码学设备身份证明；掌握有效账号密码的人可能伪造它们，不能将设备清单当作远控授权。

转移设备不会迁移旧账号的地址簿、连接凭据和历史记录。注销账号会话或撤销共享授权无法收回客户端已经同步的远控凭据，也不会中断已经建立的远控会话。需要撤销远控能力时，必须更改被控端密码，并处理被控端已认可的连接方式。

个人地址簿 `hash`、共享条目密码和共享默认密码使用 XChaCha20-Poly1305 加密，绑定地址簿、条目和字段作为 AAD。每次加密使用随机 192 位 nonce。主密钥独立于数据库保存，缺少/错误密钥会拒绝启动。用户名、设备元数据、别名、备注和标签不是加密字段；备份应整体作为敏感数据保护。

原版心跳、sysinfo 和客户端日志通常不带 Bearer 令牌。因此它们只能为已登记且账号未禁用的 ID+UUID 提交有限展示字段，不能注册设备、改归属或访问地址簿。未知标识被忽略，sysinfo 仍返回兼容的成功文本；登记后 heartbeat 会请求一次 sysinfo。只保存主机名、系统用户名、OS、版本，不保存 CPU、内存、预置配置或原始请求。上报可被伪造，不用于授权或安全审计。

服务端操作记录与客户端报告分表保存。客户端报告仅保留白名单字段、最多 10 个文件摘要，带 nonce 的重复报告去重。默认日志保留 30 天且每类最多 20000 行；每小时及写入时清理。心跳最近时间最多每台每分钟写入一次。SQLite 使用 WAL、FULL 同步、外键、2 MiB 页缓存，数据库作业和请求并发均有上限。

请求默认最多 64 KiB，分页每页最多 100 条；账号/设备/共享簿/每簿条目/标签/会话数量可以在配置中调整。默认每 IP 600 请求/分钟；登录每 IP 12 次、每账号 8 次、全局 30 次/分钟。首次集中登录 15 台设备时请分批进行；遇到 429 等待约一分钟，避免反复快速重试。限流缓存满时拒绝新键，不无限分配内存。只部署一个 API 进程，否则每进程的限流额度会相加。

## 进一步说明

- [接口与兼容基线](docs/PROTOCOL.md)
- [公网部署、升级、备份与恢复](docs/DEPLOYMENT.md)
- [实际验证记录和待验证事项](docs/VALIDATION.md)
- 原生权限语义：[官方地址簿说明](https://rustdesk.com/docs/en/self-host/rustdesk-server-pro/address-book/)
- 密码参数依据：[OWASP Password Storage Cheat Sheet](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html)

2026-10-14 已按用户授权部署到公网，以原生 systemd 运行 API 和 Caddy。两端 Windows 1.5.1 已使用原生账号窗口登录，用户确认登录后中继、P2P 均可连接。现有客户端带独立 P2P 扩展，本次未改客户端代码；不能把它们的通过结果等同于官方 1.5.0 安装包和 Android 真机通过。完整范围见验证记录。
