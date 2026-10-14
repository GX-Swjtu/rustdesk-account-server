# 客户端接口契约

基线为官方 RustDesk 1.5.0，提交 `fada664df7a294d1d1a9ca3e7cd3637069122f17`，以及工作区中的 1.5.1 客户端（`a9be639d9ca1319359c25872a3ada7c24d034d55`）。实现来自公开协议和客户端源码分析，不使用 Pro 服务端代码。代码位置：

- [用户模型](https://github.com/rustdesk/rustdesk/blob/1.5.0/flutter/lib/models/user_model.dart)
- [设备清单](https://github.com/rustdesk/rustdesk/blob/1.5.0/flutter/lib/models/group_model.dart)
- [地址簿模型](https://github.com/rustdesk/rustdesk/blob/1.5.0/flutter/lib/models/ab_model.dart)
- [载荷模型](https://github.com/rustdesk/rustdesk/blob/1.5.0/flutter/lib/common/hbbs/hbbs.dart)
- [心跳与系统信息](https://github.com/rustdesk/rustdesk/blob/1.5.0/src/hbbs_http/sync.rs)

`scripts/check-client-contract.py` 保存源码路径、SHA-256 和接口标记核对结果。它用于复查版本变化，不能代替 GUI 真机验收。升级客户端时先在隔离账号上完成本文件和验收表中的检查。

| 方法 | 路径 | 正常响应 |
|---|---|---|
| GET | `/api/login-options` | `[]` |
| POST | `/api/login` | `{access_token,type:"access_token",user:{...}}` |
| POST | `/api/logout` | 200，空响应体 |
| POST | `/api/currentUser` | 用户对象 |
| GET | `/api/users` | 当前用户一项，`{total,data}` |
| GET | `/api/peers` | 当前账号登记的设备，`{total,data}` |
| GET | `/api/device-group/accessible` | `{total:0,data:[]}` |
| POST | `/api/ab/personal` | `{guid}` |
| POST | `/api/ab/settings` | `{max_peer_one_ab}` |
| POST | `/api/ab/shared/profiles` | `{total,data:[{guid,name,owner,note,rule,info:{password}}]}` |
| POST | `/api/ab/peers?ab=GUID` | `{total,data:[Peer]}` |
| POST | `/api/ab/tags/GUID` | `[{name,color}]`，直接数组 |
| POST | `/api/ab/peer/add/GUID` | 200，空响应体 |
| PUT | `/api/ab/peer/update/GUID` | 200，空响应体 |
| DELETE | `/api/ab/peer/GUID` | 200，空响应体 |
| POST | `/api/ab/tag/add/GUID` | 200，空响应体 |
| PUT | `/api/ab/tag/rename/GUID` | 200，空响应体 |
| PUT | `/api/ab/tag/update/GUID` | 200，空响应体 |
| DELETE | `/api/ab/tag/GUID` | 200，空响应体 |
| POST | `/api/sysinfo` | 文本 `SYSINFO_UPDATED` |
| POST | `/api/heartbeat` | `{}`，需要 sysinfo 时 `{sysinfo:true}` |
| POST | `/api/audit/conn`、`/file`、`/alarm` | 200，空响应体 |
| GET | `/healthz` | `ok` |

登录使用 JSON 字节，不要求 Content-Type；兼容 Dart 自动设置的 `text/plain; charset=utf-8`。登录之外的用户/设备/地址簿接口需要 `Authorization: Bearer TOKEN`。原版的空 POST 查询允许无 body，不强制填 `{}`。旧版整本 `/api/ab`、`/api/switch-grant` 和策略接口不提供，返回 404。

分页参数 `current` 从 1 开始，`pageSize` 为 1–100，默认 100。保留原生 `accessible`、`status` 查询参数的兼容性，但 API 始终只返回当前账号可见范围。`status:1` 表示登记设备可用，不表示在线。

Peer 的字段为 `id,hash,password,username,hostname,platform,alias,tags,forceAlwaysRelay,rdpPort,rdpUsername,loginName,device_group_name,note,same_server`。`forceAlwaysRelay` 为字符串 `"true"/"false"`；tag 颜色为无符号 32 位数；`same_server` 返回 false，允许原生最近会话补充展示字段。个人仅接受/返回 hash，共享仅接受/返回 password，另一类凭据忽略并返回空串。

更新缺省字段保持原值，显式空串清除该字段；不接受 null 替代字符串。条目删除 body 为 ID 字符串数组，最多 100 项；标签删除 body 为名称数组。标签重命名 body 为 `{old,new}`，颜色更新为 `{name,color}`。标签重命名和删除通过数据库外键同步成员关系。

写入使用 `BEGIN IMMEDIATE`，在同一事务内验证会话、读取旧字段、验证授权、合并变更和写审计。并发更新不同字段保留双方变更；同一字段最后提交的值生效。创建同一条目/标签返回 409，删除不存在的条目/标签幂等成功。

错误为非 200 状态和 `{error:"..."}`：400 无效参数或容量已满；401 无效密码/令牌、禁用或撤销；403 无地址簿权限；409 设备标识或条目冲突；413 超大请求；429 限流/过载并附 `Retry-After:60`；500 内部故障。错误不含底层 SQL、密码、令牌或原始请求。登录不存在账号和错误密码统一响应，并使用相同 Argon2 参数进行校验。

设备绑定支持没有本机 ID 的纯控制端登录；这种会话仍可查看地址簿，但不会登记设备。当前用户刷新和退出只操作令牌所指的会话，不相信 body 中给出的身份。
