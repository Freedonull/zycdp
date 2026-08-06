# 04 - 使用指南

> 在业务项目中集成 zycdp 的完整指南。包含依赖配置、两种使用模式（自驱动 / 连接指纹浏览器）、API 红线。

## 一、依赖配置

### 方式一：Git 依赖（推荐，fork 场景最佳）

```toml
[dependencies]
zycdp = { git = "https://github.com/Freedonull/zycdp.git", branch = "main" }
tokio = { version = "1", features = ["full"] }
futures = "0.3"
anyhow = "1"
serde_json = "1"
```

锁定版本的三种写法：
```toml
# 锁分支（跟随主干）
zycdp = { git = "https://github.com/Freedonull/zycdp.git", branch = "main" }

# 锁 tag（最稳定，推荐发布节奏）
zycdp = { git = "https://github.com/Freedonull/zycdp.git", tag = "v0.3.0" }

# 锁 commit（最严格，CI 推荐）
zycdp = { git = "https://github.com/Freedonull/zycdp.git", rev = "a1b2c3d" }
```

### 方式二：本地 path（开发联调期）

```toml
zycdp = { path = "../zycdp" }
```

⚠️ path 依赖不能跨机器，部署/CI 必须换成 git。

### 私有仓库（git 依赖支持 token）

```toml
zycdp = { git = "https://<token>@github.com/Freedonull/zycdp.git" }
```

## 二、两种使用模式

### 模式 A：自驱动（zycdp 管理 Chrome 生命周期）

zycdp 自己启动 Chrome 并应用 stealth 指纹。适合**单机、不需要指纹浏览器**的场景。

```rust
use zycdp::{Browser, BrowserConfig, ChaserPage};
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (browser, mut handler) = Browser::launch(
        BrowserConfig::builder()
            .new_headless_mode()
            .build()
            .map_err(|e| anyhow::anyhow!(e))?,
    ).await?;

    tokio::spawn(async move { while let Some(_) = handler.next().await {} });

    let page = browser.new_page("about:blank").await?;
    let chaser = ChaserPage::new(page);

    // Native 模式：用真实 OS/Chrome 版本/RAM，只修 HeadlessChrome 等 headless 特征
    chaser.apply_native_profile().await?;

    chaser.goto("https://example.com").await?;
    let title = chaser.evaluate("document.title").await?;
    println!("{:?}", title);

    Ok(())
}
```

### 模式 B：连接指纹浏览器（推荐用于反爬场景）

连接已运行的指纹浏览器（如 AdsPower、BitBrowser、Multilogin 等），指纹由指纹浏览器负责，zycdp 只做操作。

```rust
use zycdp::{Browser, ChaserPage};
use futures::StreamExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 指纹浏览器会暴露一个 debugging 端口（如 127.0.0.1:64233）
    let (browser, mut handler) = Browser::connect("http://127.0.0.1:64233").await?;

    tokio::spawn(async move { while let Some(_) = handler.next().await {} });

    let page = browser.new_page("about:blank").await?;
    let chaser = ChaserPage::new(page);

    // ⚠️ 模式 B 绝对不要调用 apply_profile / apply_native_profile / enable_stealth_mode
    // 指纹浏览器已经配置好指纹，这些方法会覆盖它

    chaser.goto("https://example.com").await?;

    // 只读校验指纹浏览器的配置是否生效（安全）
    let ua = chaser.evaluate("navigator.userAgent").await?;
    let platform = chaser.evaluate("navigator.platform").await?;
    println!("UA: {:?}, Platform: {:?}", ua, platform);

    Ok(())
}
```

## 三、为什么连指纹浏览器是"干净模式"

`Browser::connect`（`src/browser/mod.rs:80`）路径下：
- `config: None`、`child: None` → **不启动任何 Chrome 进程、不传任何启动参数**
- 每个 page 创建时自动跑 4 组 init 命令链（Frame/Network/Page/Emulation），其中：
  - Frame：`Page.enable` + 只读查询 → **无影响**
  - Network：`Network.enable` + `ignoreHttpsErrors` → 轻微（如需严格证书校验要关）
  - Page：事件订阅类 → **无影响**
  - **Emulation：`setDeviceMetricsOverride`** → 🔴 关键风险点，但只在 viewport 不为 None 时触发
- `connect` 默认 viewport = `None`（`HandlerConfig::default()`）→ **Emulation 阶段被整个跳过，不发 setDeviceMetricsOverride** ✅

**结论**：用 `Browser::connect` 默认路径是干净的，不会破坏指纹浏览器配置。

## 四、API 红线（连指纹浏览器时）

### ❌ 不要调用（会覆盖指纹浏览器配置）

| 方法 | 危害 |
|---|---|
| `ChaserPage::apply_profile()` | 覆盖 UA/platform/WebGL/全部指纹 |
| `ChaserPage::apply_native_profile()` | 覆盖 UA（替换 HeadlessChrome） |
| `Page::enable_stealth_mode()` | 注入 webdriver/plugins/WebGL patch，与指纹浏览器冲突 |
| `BrowserConfig::builder().viewport()` | 触发 Emulation 覆盖屏幕 |
| `raw_page().evaluate()` | 触发 `Runtime.enable` 泄漏（任何模式都不要用） |

### ✅ 可以使用（只读或操作，不覆盖指纹）

- `ChaserPage::evaluate()` —— 走 isolated world，不触发 `Runtime.enable`，只读安全
- `goto` / `content` / `url` / `screenshot` / cookies
- `move_mouse_human` / `click_human` / `type_text*` / `scroll_human`（行为模拟）
- `enable_request_interception` / `fulfill_request_html` / `continue_request`（请求拦截）
- `create_incognito_context_with_proxy`（多 context 代理，见下）

## 五、代理配置

### 无认证代理

```rust
// 方式一：CDP context 级代理（每个 context 独立，可运行时切换）
let ctx_id = browser
    .create_incognito_context_with_proxy("socks5://127.0.0.1:1080")
    .await?;

// 方式二：启动参数级代理（整个浏览器统一，launch 模式才可用）
let config = BrowserConfig::builder()
    .arg("--proxy-server=socks5://127.0.0.1:1080")
    .build()?;
```

支持的格式（Chrome 命令行规范）：
- `socks5://host:port`、`socks4://host:port`、`http://host:port`

### 带认证代理

⚠️ Chrome 不支持 `user:pass@host:port` 内嵌认证（`src/browser/mod.rs:479` 注释明示）。两种解法：
1. **本地转发器**（推荐）：用 gost/pproxy 把"有认证代理"转成"本地无认证端口"
2. **Fetch.continueWithAuth 封装**：zycdp 待实现（见 [改进路线 P2-1](./02-improvement-roadmap.md#p2-1代理认证支持)）

## 六、人类行为模拟

```rust
// 鼠标：贝塞尔曲线移动 + 点击
chaser.move_mouse_human(500.0, 300.0).await?;
chaser.click_human(100.0, 200.0).await?;  // 移动+停顿+点击+停顿

// 键盘：变速打字，3% 概率打错字回退（最拟人）
chaser.type_text_with_typos("hello world").await?;

// 滚动：多步 + ease-in/out
chaser.scroll_human(500).await?;  // 正数向下，负数向上
```

## 七、请求拦截（Turnstile/captcha 场景）

```rust
use zycdp::cdp::browser_protocol::network::ResourceType;

// 拦截所有 Document 请求
chaser.enable_request_interception("*", Some(ResourceType::Document)).await?;

// 在事件循环中处理 EventRequestPaused：
// - 想替换内容：chaser.fulfill_request_html(request_id, "<html>...</html>", 200).await?
// - 想放行：chaser.continue_request(request_id).await?

chaser.disable_request_interception().await?;
```

## 八、常见错误排查

| 错误 | 原因 | 解决 |
|---|---|---|
| `find_element` 返回 Error | 元素未加载（无自动等待） | 手写 wait 循环，或等 P1-3 Locator 完成 |
| `Runtime.enable` 检测告警 | 误用 `raw_page().evaluate()` | 改用 `chaser.evaluate()` |
| 连指纹浏览器后指纹错乱 | 调用了 `apply_profile` 等 | 模式 B 只用操作类 API（见红线表） |
| Windows native 内存恒为 8GB | P0-2 缺陷未修 | 临时：显式 `.memory_gb(真实值)` |
