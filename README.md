# tgState

基于 Telegram 的私有文件存储系统，使用 Rust 构建，单文件部署，开箱即用。

将 Telegram 频道作为无限容量的文件存储后端，通过 Web 界面管理上传、下载和分享文件。

## 功能

- 通过 Web 界面或 API 上传文件到 Telegram 频道
- 大文件自动分块上传（>19.5MB），下载时流式拼接
- 短链接分享，支持在线预览（图片、视频、PDF、文本等）
- 图床模式，兼容 PicGo API
- Telegram Bot 自动同步频道文件变动
- SSE 实时推送文件列表更新
- 网页引导式配置，无需预填环境变量
- 全站安全加固（CSP、CSRF、Rate Limiting、会话超时）

## 快速开始

### 方式一：直接下载（推荐）

从 [Releases](https://github.com/buyi06/tgstate-rust/releases) 下载预编译的二进制文件：

```bash
# 下载最新版
wget https://github.com/buyi06/tgstate-rust/releases/latest/download/tgstate-linux-amd64.tar.gz

# 解压
tar xzf tgstate-linux-amd64.tar.gz

# 运行
cd tgstate
./tgstate
```

先配置 Authelia OIDC 环境变量，再访问 `http://你的IP:8000`，使用 Authelia 登录后在设置页配置 Bot。

### 方式二：Docker

```bash
docker run -d --name tgstate -p 8000:7860 -v tgstate_data:/app/data \
  $(docker build -q .)
```

或使用 Docker Compose：

```yaml
services:
  tgstate:
    build: .
    ports:
      - "8000:7860"
    volumes:
      - tgstate_data:/app/data
    restart: unless-stopped

volumes:
  tgstate_data:
```

### 方式三：从源码编译

```bash
# 需要 Rust 1.75+
cargo build --release
./target/release/tgstate
```

## 配置流程

1. 先确定公开访问地址 `BASE_URL`，例如 `https://你的图床域名`
2. 在 Authelia 创建 OIDC 客户端，回调地址设为 `BASE_URL/api/auth/callback`
3. 通过环境变量配置 `BASE_URL`、`OIDC_ISSUER_URL`、`OIDC_CLIENT_ID`、`OIDC_CLIENT_SECRET`
4. 启动后访问 Web 界面，使用 Authelia 登录
5. 进入「系统设置」页面，填写 Bot Token（从 [@BotFather](https://t.me/BotFather) 获取）和频道名
6. 点击「保存并应用」

Telegram 和 PicGo 配置保存在本地数据库中；OIDC 统一身份认证配置来自环境变量。

## 环境变量

OIDC 是管理界面的强制登录方式，必须先配置后才能使用后台。

| 变量 | 说明 | 默认值 |
|---|---|---|
| `OIDC_ISSUER_URL` | Authelia issuer URL，例如 `https://auth.example.com` | 必填 |
| `OIDC_CLIENT_ID` | Authelia OIDC 客户端 ID | 必填 |
| `OIDC_CLIENT_SECRET` | Authelia OIDC 客户端密钥 | 必填 |
| `BOT_TOKEN` | Telegram Bot Token | - |
| `CHANNEL_NAME` | 目标频道 `@name` 或 `-100xxx` | - |
| `PICGO_API_KEY` | PicGo 上传 API 密钥 | - |
| `BASE_URL` | 公开访问 URL，也用于生成 OIDC callback，例如 `https://your-domain.example` | `http://127.0.0.1:8000` |
| `DATA_DIR` | 数据目录 | `app/data` |
| `LOG_LEVEL` | 日志级别 | `info` |
| `SESSION_MAX_AGE_SECS` | 登录会话 Cookie 有效期（秒） | `604800` (7天) |
| `COOKIE_SECURE` | 强制使用 `Secure` Cookie（反向代理 TLS 场景） | 自动推断 |
| `TRUST_FORWARDED_FOR` | 信任 `X-Forwarded-For` / `X-Real-IP` 识别客户端 IP | `0` |

> ⚠️ **如果你使用反向代理**，必须同时设置 `COOKIE_SECURE=1` 和 `TRUST_FORWARDED_FOR=1`，
> 否则 Cookie 可能缺失 `Secure` 标志，限流也会对所有请求在代理层合并后统一限流。请确保
> `TRUST_FORWARDED_FOR` 只在前置代理可信的网络拓扑下开启。

## API

### 文件操作

| 方法 | 路径 | 说明 |
|---|---|---|
| `POST` | `/api/upload` | 上传文件（multipart，字段名 `file`） |
| `GET` | `/api/files` | 获取文件列表 |
| `DELETE` | `/api/files/:file_id` | 删除文件 |
| `POST` | `/api/batch_delete` | 批量删除 |
| `GET` | `/d/:short_id` | 短链接下载/预览 |
| `GET` | `/api/file-updates` | SSE 实时更新 |

### 认证

| 方法 | 路径 | 说明 |
|---|---|---|
| `GET` | `/api/auth/login` | 发起 OIDC 登录跳转 |
| `GET` | `/api/auth/callback` | OIDC 授权码回调 |
| `GET` | `/api/auth/session` | 当前登录状态 |
| `POST` | `/api/auth/logout` | 退出 |

### 配置

| 方法 | 路径 | 说明 |
|---|---|---|
| `GET` | `/api/app-config` | 获取配置 |
| `POST` | `/api/app-config/apply` | 保存并应用 |
| `POST` | `/api/reset-config` | 重置配置 |
| `POST` | `/api/verify/bot` | 验证 Bot Token |
| `POST` | `/api/verify/channel` | 验证频道 |

### PicGo 兼容

```bash
curl -X POST http://your-host:8000/api/upload \
  -H "X-Api-Key: your_picgo_api_key" \
  -F "file=@image.png"
```

## 安全

- CSRF 防护（Origin 头校验）
- 登录限流（5 次/分钟/IP）
- Content Security Policy
- Cookie 加固（HttpOnly、SameSite=Strict、24h 超时）
- API 白名单认证
- 输入验证与错误脱敏
- 安全头（X-Frame-Options、X-XSS-Protection 等）

## 技术栈

| 组件 | 技术 |
|---|---|
| Web 框架 | Axum 0.7 |
| 异步运行时 | Tokio |
| 模板引擎 | Tera |
| 数据库 | SQLite (WAL) |
| HTTP 客户端 | reqwest |
| CI/CD | GitHub Actions |

## 项目结构

```
├── src/
│   ├── main.rs                 # 入口
│   ├── config.rs               # 配置管理
│   ├── database.rs             # SQLite 操作
│   ├── auth.rs                 # 认证 & Cookie
│   ├── state.rs                # 应用状态
│   ├── middleware/
│   │   ├── auth.rs             # 认证 & CSRF 中间件
│   │   ├── rate_limit.rs       # 限流中间件
│   │   └── security_headers.rs # 安全头
│   ├── routes/                 # API 路由
│   └── telegram/               # Telegram Bot 服务
├── app/
│   ├── templates/              # HTML 模板
│   └── static/                 # CSS/JS
├── .github/workflows/          # CI/CD
├── Dockerfile
└── Cargo.toml
```

## License

MIT
