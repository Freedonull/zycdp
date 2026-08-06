use crate::page::Page;
use crate::profiles::ChaserProfile;
use anyhow::{Result, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD};
use chromiumoxide_cdp::cdp::browser_protocol::emulation::{
    SetDeviceMetricsOverrideParams, SetFocusEmulationEnabledParams,
    SetUserAgentOverrideParams as EmulationSetUserAgentOverrideParams, UserAgentBrandVersion,
    UserAgentMetadata,
};
use chromiumoxide_cdp::cdp::browser_protocol::fetch::{
    ContinueRequestParams, DisableParams as FetchDisableParams, EnableParams as FetchEnableParams,
    FulfillRequestParams, HeaderEntry, RequestPattern,
};
use chromiumoxide_cdp::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType,
};
use chromiumoxide_cdp::cdp::browser_protocol::network::ResourceType;
use chromiumoxide_cdp::cdp::browser_protocol::page::{
    AddScriptToEvaluateOnNewDocumentParams, CreateIsolatedWorldParams,
};
use chromiumoxide_cdp::cdp::js_protocol::runtime::EvaluateParams;
use rand::Rng;
use serde_json::Value;
use std::future::Future;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// 页面加载状态（用于 [`ChaserPage::wait_for_load_state`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadState {
    /// DOM 树解析完成（早于 load）。
    DomContentLoaded,
    /// 所有资源（图片/样式等）加载完成——`goto` 默认等待的状态。
    Load,
    /// 500ms 内无网络请求（SPA 数据加载完毕的标志）。
    NetworkIdle,
}

impl LoadState {
    fn as_str(self) -> &'static str {
        match self {
            LoadState::DomContentLoaded => "DOMContentLoaded",
            LoadState::Load => "load",
            LoadState::NetworkIdle => "networkIdle",
        }
    }
}

/// JavaScript 弹窗类型（alert / confirm / prompt / beforeunload）。
///
/// 在 [`ChaserPage::on_dialog`] 的回调里用于区分弹窗种类，决定如何处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogType {
    Alert,
    Confirm,
    Prompt,
    Beforeunload,
}

impl DialogType {
    /// 从 CDP 的 DialogType 转换（内部使用）。
    fn from_cdp(d: &chromiumoxide_cdp::cdp::browser_protocol::page::DialogType) -> Self {
        use chromiumoxide_cdp::cdp::browser_protocol::page::DialogType as D;
        match d {
            D::Alert => DialogType::Alert,
            D::Confirm => DialogType::Confirm,
            D::Prompt => DialogType::Prompt,
            D::Beforeunload => DialogType::Beforeunload,
        }
    }
}

/// Stealth browser page with human-like input simulation.
///
/// # Stealth JavaScript Execution
///
/// ```rust,ignore
/// // Safe - uses isolated world, no Runtime.enable leak
/// let title = chaser.evaluate("document.title").await?;
///
/// // Dangerous - only use raw_page().evaluate() if you know what you're doing
/// let val = chaser.raw_page().evaluate("...").await?;  // Triggers Runtime.enable!
/// ```
///
/// All other `raw_page()` methods (get_cookies, screenshot, goto, etc.) are safe.
///
/// # Features
///
/// - Zero-footprint JS execution via `Page.createIsolatedWorld`
/// - Bezier curve mouse movements with jitter
/// - Realistic typing with variable delays
#[derive(Clone, Debug)]
pub struct ChaserPage {
    page: Page,
    mouse_pos: Arc<Mutex<Point>>,
}

impl ChaserPage {
    /// Create a new ChaserPage wrapping the given Page.
    pub fn new(page: Page) -> Self {
        Self {
            page,
            mouse_pos: Arc::new(Mutex::new(Point { x: 0.0, y: 0.0 })),
        }
    }

    // ========== SAFE PAGE ACCESS ==========

    /// Access the underlying Page.
    ///
    /// Most methods are safe, **except `raw_page().evaluate()`** which
    /// triggers `Runtime.enable` detection. Use `chaser.evaluate()` instead.
    #[doc(alias = "inner")]
    pub fn raw_page(&self) -> &Page {
        &self.page
    }

    /// Deprecated: Use `raw_page()` instead.
    ///
    /// This method is kept for backwards compatibility but will be removed in a future version.
    #[deprecated(since = "0.1.1", note = "Use `raw_page()` instead for clarity")]
    pub fn inner(&self) -> &Page {
        &self.page
    }

    // ========== STEALTH-SAFE PAGE OPERATIONS ==========

    /// Navigate to a URL (stealth-safe).
    ///
    /// This is equivalent to `raw_page().goto()` but provided for convenience.
    pub async fn goto(&self, url: &str) -> Result<()> {
        self.page.goto(url).await.map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// 等待页面到达指定生命周期状态（比 `goto` 默认的 `load` 更细粒度）。
    ///
    /// SPA 站点 `load` 触发时数据还没加载完，常需等 `NetworkIdle`（500ms 无网络
    /// 请求）。在 `goto` 之后调用本方法补充等待。
    ///
    /// # 示例
    /// ```rust,ignore
    /// chaser.goto("https://spa-site.com").await?;
    /// chaser.wait_for_load_state(LoadState::NetworkIdle, Duration::from_secs(30)).await?;
    /// // 此时 SPA 数据已加载完毕
    /// ```
    pub async fn wait_for_load_state(
        &self,
        state: LoadState,
        timeout: std::time::Duration,
    ) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::page::EventLifecycleEvent;
        use futures::StreamExt;

        let target = state.as_str();
        // 只匹配主 frame 的生命周期事件——iframe 的 networkIdle 可能先于主 frame 触发，
        // 若不过滤会误判完成。mainframe() 返回主 frame id。
        let main_frame = self
            .page
            .mainframe()
            .await
            .map_err(|e| anyhow!("获取主 frame 失败: {}", e))?
            .map(|id| id.into());
        let mut stream = self
            .page
            .event_listener::<EventLifecycleEvent>()
            .await
            .map_err(|e| anyhow!("订阅 lifecycle 事件失败: {}", e))?;

        // 用绝对截止时间，而非每次事件的 timeout——否则持续到来的非目标事件会
        // 反复重置超时，导致永远不超时。
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .ok_or_else(|| {
                    anyhow!("wait_for_load_state 超时（{:?}）等待 {}", timeout, target)
                })?;
            match tokio::time::timeout(remaining, stream.next()).await {
                Err(_) => {
                    return Err(anyhow!(
                        "wait_for_load_state 超时（{:?}）等待 {}",
                        timeout,
                        target
                    ));
                }
                Ok(None) => return Err(anyhow!("lifecycle 事件流关闭")),
                Ok(Some(ev)) => {
                    // 只认主 frame 的事件；frame_id 为 None 时也接受（兼容性）
                    let ev_frame: Option<String> = Some(ev.frame_id.clone().into());
                    let is_main = ev_frame == main_frame || main_frame.is_none();
                    if ev.name == target && is_main {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Get the page HTML content (stealth-safe).
    pub async fn content(&self) -> Result<String> {
        self.page.content().await.map_err(|e| anyhow!("{}", e))
    }

    /// Get the current page URL (stealth-safe).
    pub async fn url(&self) -> Result<Option<String>> {
        self.page.url().await.map_err(|e| anyhow!("{}", e))
    }

    // ========== LOCATOR（自动等待 + 抗 stale） ==========
    //
    // 与 `raw_page().find_element()` 的关键区别：find_element 找不到元素立刻报错，
    // 异步加载的网页上用户被迫手写 sleep + 重试。Locator 层封装"轮询等待元素出现"。
    //
    // 查询走 DOM 域（QuerySelector / PerformSearch），不触发 Runtime.enable，
    // 对 stealth 无影响——路线图里"必须用 isolated world 查询"是想当然，DOM 域查询
    // 本身就不碰 Runtime domain。

    /// 等待匹配 CSS selector 的第一个元素出现，最多等 `timeout`。
    ///
    /// 每 100ms 轮询一次 `querySelector`（DOM 域，stealth-safe），元素出现即返回。
    /// 超时返回 Err。这是相比 `find_element` 的核心增强（后者无等待，元素未加载就 fail）。
    ///
    /// # 示例
    /// ```rust,ignore
    /// // 等待登录按钮出现（最多 10 秒）
    /// let el = chaser.wait_for_selector("#login-btn", Duration::from_secs(10)).await?;
    /// el.click().await?;
    /// ```
    pub async fn wait_for_selector(
        &self,
        selector: &str,
        timeout: std::time::Duration,
    ) -> Result<crate::element::Element> {
        let interval = std::time::Duration::from_millis(100);
        let start = std::time::Instant::now();
        loop {
            // find_element 内部走 DOM.querySelector，失败说明元素尚未存在
            match self.page.find_element(selector).await {
                Ok(el) => return Ok(el),
                Err(_) => {
                    if start.elapsed() >= timeout {
                        return Err(anyhow!(
                            "等待 selector '{}' 超时（{:?}）",
                            selector,
                            timeout
                        ));
                    }
                    tokio::time::sleep(interval).await;
                }
            }
        }
    }

    /// 按可见文本查找元素（XPath `//*[contains(text(), "...")]`），常用于爬虫定位
    /// 没有 id/class 的按钮/链接。返回第一个匹配。
    pub async fn find_by_text(&self, text: &str) -> Result<crate::element::Element> {
        // 转义 XPath 字符串里的单引号：文本里的 ' 拆成 '+...'
        let escaped = text.replace('\'', "',\"'\",'");
        let xpath = format!("//*[contains(text(), '{}')]", escaped);
        self.page
            .find_xpath(xpath)
            .await
            .map_err(|e| anyhow!("{}", e))
    }

    /// 找到含指定文本的元素并点击（自动等待 + scroll into view）。
    pub async fn click_by_text(&self, text: &str) -> Result<()> {
        let el = self.find_by_text(text).await?;
        el.click().await.map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// 创建一个 Locator 句柄，后续每次操作前重新查询元素（抗 stale element）。
    ///
    /// 适合"同一个元素要多次操作、中间页面可能重渲染"的场景：
    /// ```rust,ignore
    /// let btn = chaser.locator("#submit");
    /// btn.click().await?;           // 第一次：等待+点击
    /// btn.wait(Duration::from_secs(5)).await?;  // 重新等待它再次出现
    /// ```
    pub fn locator(&self, selector: impl Into<String>) -> ZyLocator {
        ZyLocator {
            chaser: self.clone(),
            selector: selector.into(),
        }
    }

    /// Execute JavaScript using **stealth execution** (no Runtime.enable leak).
    ///
    /// This is the safe way to run JavaScript on protected sites.
    /// Under the hood, it uses `Page.createIsolatedWorld` to avoid detection.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Get page title
    /// let title: String = chaser.evaluate("document.title").await?;
    ///
    /// // Check a value
    /// let ua: String = chaser.evaluate("navigator.userAgent").await?;
    /// ```
    pub async fn evaluate(&self, script: &str) -> Result<Option<Value>> {
        self.evaluate_stealth(script).await
    }

    /// Apply a ChaserProfile to this page in one clean call.
    ///
    /// This method:
    /// 1. Sets the User-Agent HTTP header
    /// 2. Injects the profile's bootstrap script for JS-level spoofing
    ///
    /// **IMPORTANT:** Call this BEFORE navigating to the target site.
    ///
    /// # Example
    /// ```rust,ignore
    /// let profile = ChaserProfile::windows().build();
    /// let page = browser.new_page("about:blank").await?;
    /// let chaser = ChaserPage::new(page);
    /// chaser.apply_profile(&profile).await?;
    /// chaser.inner().goto("https://example.com").await?;
    /// ```
    pub async fn apply_profile(&self, profile: &ChaserProfile) -> Result<()> {
        if profile.native_ua_data() {
            // Native profile: call setUserAgentOverride only to replace
            // "HeadlessChrome" with "Chrome" in the UA string (headless Chrome
            // uses "HeadlessChrome" by default, which is an obvious bot signal).
            // We do NOT set userAgentMetadata so Sec-CH-UA headers stay as the
            // browser's real values — no mismatch between HTTP headers and JS.
            let ua = profile.user_agent();
            // Fix "HeadlessChrome" → "Chrome" (new headless mode should do this
            // too, but some Chrome versions on Linux still emit "HeadlessChrome").
            let ua = ua.replace("HeadlessChrome/", "Chrome/");
            self.page
                .execute(
                    EmulationSetUserAgentOverrideParams::builder()
                        .user_agent(ua)
                        .accept_language(profile.locale().to_string())
                        .platform(profile.os().platform().to_string())
                        .build()
                        .map_err(|e| anyhow!("{}", e))?,
                )
                .await
                .map_err(|e| anyhow!("{}", e))?;

            // Only inject bootstrap if it's non-empty — native profiles use a
            // minimal script; if that ever becomes a detection vector we can
            // turn it off here entirely.
            let boot = profile.bootstrap_script();
            if !boot.trim().is_empty() {
                self.page
                    .execute(AddScriptToEvaluateOnNewDocumentParams {
                        source: boot,
                        world_name: None,
                        include_command_line_api: None,
                        run_immediately: None,
                    })
                    .await
                    .map_err(|e| anyhow!("{}", e))?;
            }

            // Fix screen.width/height for headless mode.
            // Headless Chrome reports screen.width=800, screen.height=600 by default
            // while --window-size sets the window to 1920x1080. The mismatch
            // (innerWidth=1920 > screen.width=800) is physically impossible and is a
            // known headless detection signal. setDeviceMetricsOverride with
            // screenWidth/screenHeight overrides these at the browser level, so
            // screen.width returns the correct native value (not a JS wrapper).
            let sw = profile.screen_width() as i64;
            let sh = profile.screen_height() as i64;
            let _ = self
                .page
                .execute(
                    SetDeviceMetricsOverrideParams::builder()
                        .width(0)
                        .height(0)
                        .device_scale_factor(0.0)
                        .mobile(false)
                        .screen_width(sw)
                        .screen_height(sh)
                        .build()
                        .map_err(|e| anyhow!("{}", e))?,
                )
                .await;

            self.page
                .execute(
                    SetFocusEmulationEnabledParams::builder()
                        .enabled(true)
                        .build()
                        .map_err(|e| anyhow!("{}", e))?,
                )
                .await
                .map_err(|e| anyhow!("{}", e))?;

            return Ok(());
        }

        let ua_params = {
            let ver = profile.chrome_version().to_string();
            let full_ver = format!("{}.0.0.0", ver);

            let brand = |name: &str, v: &str| UserAgentBrandVersion {
                brand: name.to_string(),
                version: v.to_string(),
            };

            let metadata = UserAgentMetadata {
                brands: Some(vec![
                    brand("Google Chrome", &ver),
                    brand("Chromium", &ver),
                    brand("Not=A?Brand", "24"),
                ]),
                full_version_list: Some(vec![
                    brand("Google Chrome", &full_ver),
                    brand("Chromium", &full_ver),
                    brand("Not=A?Brand", "24.0.0.0"),
                ]),
                platform: profile.os().hints_platform().to_string(),
                platform_version: profile.os().platform_version().to_string(),
                architecture: profile.os().architecture().to_string(),
                model: String::new(),
                mobile: false,
                bitness: Some("64".to_string()),
                wow64: Some(false),
                form_factors: None,
            };

            EmulationSetUserAgentOverrideParams::builder()
                .user_agent(profile.user_agent())
                .accept_language(profile.locale().to_string())
                .platform(profile.os().platform().to_string())
                .user_agent_metadata(metadata)
                .build()
                .map_err(|e| anyhow!("{}", e))?
        };

        // Set UA + Sec-CH-UA-* headers
        self.page
            .execute(ua_params)
            .await
            .map_err(|e| anyhow!("{}", e))?;

        // Inject the bootstrap script to run on every new document
        self.page
            .execute(AddScriptToEvaluateOnNewDocumentParams {
                source: profile.bootstrap_script(),
                world_name: None,
                include_command_line_api: None,
                run_immediately: None,
            })
            .await
            .map_err(|e| anyhow!("{}", e))?;

        // Simulate a focused, active page so document.hasFocus() and
        // document.visibilityState return the same values as a real user session.
        // Without this, headless Chrome reports hasFocus=false / visibilityState='hidden'.
        self.page
            .execute(
                SetFocusEmulationEnabledParams::builder()
                    .enabled(true)
                    .build()
                    .map_err(|e| anyhow!("{}", e))?,
            )
            .await
            .map_err(|e| anyhow!("{}", e))?;

        Ok(())
    }

    /// Apply a profile derived from the actual running browser.
    ///
    /// Reads the Chrome version from the connected browser via CDP so it's
    /// always accurate — even when using chromiumoxide_fetcher's downloaded
    /// binary whose version differs from any system Chrome installation.
    /// OS and RAM are still detected from the host machine.
    ///
    /// Call this BEFORE navigating to the target site.
    pub async fn apply_native_profile(&self) -> Result<()> {
        let ua = self.page.user_agent().await.map_err(|e| anyhow!("{}", e))?;
        let chrome_version = parse_chrome_major(&ua).unwrap_or(131);
        let profile = crate::profiles::ChaserProfile::native()
            .chrome_version(chrome_version)
            .build();
        self.apply_profile(&profile).await
    }

    // ========== REQUEST INTERCEPTION API ==========

    /// Enable request interception for specific URL patterns.
    ///
    /// This uses the Fetch domain to intercept requests before they are sent.
    /// After enabling, use `fulfill_request` or `continue_request` to handle
    /// intercepted requests.
    ///
    /// # Arguments
    /// * `url_pattern` - Glob pattern to match URLs (e.g., "*", "https://example.com/*")
    /// * `resource_type` - Optional resource type filter (Document, Script, etc.)
    ///
    /// # Example
    /// ```rust,ignore
    /// // Intercept all document requests
    /// chaser.enable_request_interception("*", Some(ResourceType::Document)).await?;
    /// ```
    pub async fn enable_request_interception(
        &self,
        url_pattern: &str,
        resource_type: Option<ResourceType>,
    ) -> Result<()> {
        let mut pattern_builder = RequestPattern::builder().url_pattern(url_pattern);
        if let Some(rt) = resource_type {
            pattern_builder = pattern_builder.resource_type(rt);
        }

        self.page
            .execute(
                FetchEnableParams::builder()
                    .handle_auth_requests(false)
                    .pattern(pattern_builder.build())
                    .build(),
            )
            .await
            .map_err(|e| anyhow!("{}", e))?;

        Ok(())
    }

    /// Disable request interception.
    pub async fn disable_request_interception(&self) -> Result<()> {
        self.page
            .execute(FetchDisableParams::default())
            .await
            .map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// Fulfill an intercepted request with custom HTML content.
    ///
    /// This is useful for Turnstile/captcha solving where you want to
    /// serve a minimal page that only loads the challenge widget.
    ///
    /// # Arguments
    /// * `request_id` - The request ID from the EventRequestPaused event
    /// * `html` - The HTML content to serve
    /// * `status_code` - HTTP status code (usually 200)
    ///
    /// # Example
    /// ```rust,ignore
    /// let fake_html = r#"
    ///     <!DOCTYPE html>
    ///     <html>
    ///     <head>
    ///         <script src="https://challenges.cloudflare.com/turnstile/v0/api.js"></script>
    ///     </head>
    ///     <body>
    ///         <div class="cf-turnstile" data-sitekey="your-sitekey"></div>
    ///     </body>
    ///     </html>
    /// "#;
    /// chaser.fulfill_request_html(request_id, fake_html, 200).await?;
    /// ```
    pub async fn fulfill_request_html(
        &self,
        request_id: impl Into<String>,
        html: &str,
        status_code: i64,
    ) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::fetch::RequestId;

        let body_base64 = STANDARD.encode(html);

        self.page
            .execute(
                FulfillRequestParams::builder()
                    .request_id(RequestId::from(request_id.into()))
                    .response_code(status_code)
                    .body(body_base64)
                    .response_header(HeaderEntry {
                        name: "content-type".to_string(),
                        value: "text/html; charset=utf-8".to_string(),
                    })
                    .build()
                    .map_err(|e| anyhow!("{}", e))?,
            )
            .await
            .map_err(|e| anyhow!("{}", e))?;

        Ok(())
    }

    /// Continue an intercepted request without modification.
    ///
    /// Use this when you intercept a request but decide not to modify it.
    pub async fn continue_request(&self, request_id: impl Into<String>) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::fetch::RequestId;

        self.page
            .execute(
                ContinueRequestParams::builder()
                    .request_id(RequestId::from(request_id.into()))
                    .build()
                    .map_err(|e| anyhow!("{}", e))?,
            )
            .await
            .map_err(|e| anyhow!("{}", e))?;

        Ok(())
    }

    // ========== 响应体捕获（Network.getResponseBody） ==========
    //
    // 与请求拦截（Fetch 域）的区别：请求拦截只能修改/拒绝请求，拿不到响应 body。
    // 响应捕获订阅 Network 域事件，在响应加载完成后读取 body——SPA 采集核心模式
    // （等接口返回 → 解析 JSON）。Network 域默认已 enable（非 stealth 检测点）。

    /// 阻塞等待匹配 `url_pattern`（子串匹配）的响应完成，返回其 body。
    ///
    /// 典型用法：导航前调用，等 SPA 的 XHR 接口返回数据。
    /// ```rust,ignore
    /// // 等待 API 接口返回 JSON
    /// let body = chaser.wait_for_response("/api/users", Duration::from_secs(15)).await?;
    /// let users: Vec<User> = serde_json::from_str(&body)?;
    /// ```
    pub async fn wait_for_response(
        &self,
        url_pattern: &str,
        timeout: std::time::Duration,
    ) -> Result<String> {
        use chromiumoxide_cdp::cdp::browser_protocol::network::{
            EventLoadingFinished, EventResponseReceived, GetResponseBodyParams,
        };
        use futures::StreamExt;

        let mut resp_stream = self
            .page
            .event_listener::<EventResponseReceived>()
            .await
            .map_err(|e| anyhow!("订阅 responseReceived 失败: {}", e))?;
        let mut finish_stream = self
            .page
            .event_listener::<EventLoadingFinished>()
            .await
            .map_err(|e| anyhow!("订阅 loadingFinished 失败: {}", e))?;

        // 收集已见响应的 requestId（按 url 匹配）
        let matched_ids: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let matched_ids_resp = Arc::clone(&matched_ids);

        // 任务 1：监听响应到达，按 url 匹配记录 requestId
        let pattern = url_pattern.to_string();
        let resp_task = tokio::spawn(async move {
            while let Some(ev) = resp_stream.next().await {
                if ev.response.url.contains(&pattern) {
                    matched_ids_resp
                        .lock()
                        .unwrap()
                        .push(ev.request_id.clone().into());
                }
            }
        });

        // 任务 2：监听加载完成，若该 requestId 已匹配则读取 body
        let page = self.page.clone();
        let finish_task = tokio::spawn(async move {
            while let Some(ev) = finish_stream.next().await {
                let rid: String = ev.request_id.clone().into();
                let is_matched = matched_ids.lock().unwrap().iter().any(|r| r == &rid);
                if is_matched {
                    // body 此时已就绪，读取返回。base64_encoded=true 时 body 是
                    // base64 字符串（二进制/gzip 响应），需解码否则调用方拿到损坏数据。
                    let cmd = GetResponseBodyParams::new(ev.request_id.clone());
                    match page.execute(cmd).await {
                        Ok(resp) => {
                            if resp.result.base64_encoded {
                                // 解码 base64，损失地从字节转 String（二进制可能非 UTF-8，
                                // 但本 API 语义是采集文本 body；纯二进制建议直接用 CDP）
                                match STANDARD.decode(&resp.result.body) {
                                    Ok(bytes) => return Some(String::from_utf8_lossy(&bytes).into_owned()),
                                    Err(_) => return Some(resp.result.body),
                                }
                            }
                            return Some(resp.result.body);
                        }
                        Err(_) => continue,
                    }
                }
            }
            None
        });

        // 带超时等待 body
        let result = tokio::time::timeout(timeout, finish_task).await;
        resp_task.abort();
        match result {
            Ok(Ok(Some(body))) => Ok(body),
            Ok(Ok(None)) => Err(anyhow!(
                "响应流关闭，未捕获到匹配 '{}' 的 body",
                url_pattern
            )),
            Ok(Err(e)) => Err(anyhow!("wait_for_response 任务失败: {}", e)),
            Err(_) => Err(anyhow!(
                "wait_for_response 超时（{:?}）未匹配到 '{}'",
                timeout,
                url_pattern
            )),
        }
    }

    // ========== 文件下载（Browser.setDownloadBehavior） ==========

    /// 配置下载目录并开启下载事件。调用后，页面触发的下载会自动保存到 `dir`。
    ///
    /// 配合 [`wait_for_download`] 等待下载完成。
    ///
    /// # 示例
    /// ```rust,ignore
    /// chaser.enable_downloads("/tmp/downloads").await?;
    /// chaser.click_by_text("导出 CSV").await?;  // 触发下载
    /// let info = chaser.wait_for_download(Duration::from_secs(60)).await?;
    /// println!("已下载: {} -> {:?}", info.filename, info.filepath);
    /// ```
    pub async fn enable_downloads(&self, dir: impl AsRef<str>) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::browser::{
            SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
        };

        self.page
            .execute(
                SetDownloadBehaviorParams::builder()
                    .behavior(SetDownloadBehaviorBehavior::Allow)
                    .download_path(dir.as_ref().to_string())
                    .events_enabled(true)
                    .build()
                    .unwrap(),
            )
            .await
            .map_err(|e| anyhow!("setDownloadBehavior 失败: {}", e))?;
        Ok(())
    }

    /// 阻塞等待一次下载完成（需先调 `enable_downloads`），返回文件名和落盘路径。
    ///
    /// `filepath` 来自 CDP 的 `EventDownloadProgress`（state=Completed 时提供），
    /// 取决于平台不一定保证已设置，建议用 `filename` 在下载目录自行拼接校验。
    pub async fn wait_for_download(&self, timeout: std::time::Duration) -> Result<DownloadInfo> {
        use chromiumoxide_cdp::cdp::browser_protocol::browser::{
            DownloadProgressState, EventDownloadProgress, EventDownloadWillBegin,
        };
        use futures::StreamExt;

        let mut begin_stream = self
            .page
            .event_listener::<EventDownloadWillBegin>()
            .await
            .map_err(|e| anyhow!("订阅 downloadWillBegin 失败: {}", e))?;
        let mut progress_stream = self
            .page
            .event_listener::<EventDownloadProgress>()
            .await
            .map_err(|e| anyhow!("订阅 downloadProgress 失败: {}", e))?;

        // 先记录下载开始的文件名（按 guid 关联）
        let filenames: Arc<Mutex<std::collections::HashMap<String, String>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let filenames_begin = Arc::clone(&filenames);

        let begin_task = tokio::spawn(async move {
            while let Some(ev) = begin_stream.next().await {
                filenames_begin
                    .lock()
                    .unwrap()
                    .insert(ev.guid.clone(), ev.suggested_filename.clone());
            }
        });

        // 等进度事件 state=Completed
        let progress_task = tokio::spawn(async move {
            while let Some(ev) = progress_stream.next().await {
                if ev.state == DownloadProgressState::Completed {
                    let filename = filenames
                        .lock()
                        .unwrap()
                        .get(&ev.guid)
                        .cloned()
                        .unwrap_or_default();
                    return Some(DownloadInfo {
                        guid: ev.guid.clone(),
                        filename,
                        filepath: ev.file_path.clone(),
                    });
                }
            }
            None
        });

        let result = tokio::time::timeout(timeout, progress_task).await;
        begin_task.abort();
        match result {
            Ok(Ok(Some(info))) => Ok(info),
            Ok(Ok(None)) => Err(anyhow!("下载事件流关闭，未捕获到完成事件")),
            Ok(Err(e)) => Err(anyhow!("wait_for_download 任务失败: {}", e)),
            Err(_) => Err(anyhow!("wait_for_download 超时（{:?}）", timeout)),
        }
    }

    /// **Stealth Execution（隔离世界方案）**
    ///
    /// 通过 `Page.createIsolatedWorld` 创建隔离 JS 上下文执行脚本，**从不调用
    /// `Runtime.enable`**——这是反爬用来识别 CDP 自动化的关键信号。
    ///
    /// 这等价于 rebrowser-patches 的 `alwaysIsolated` 模式（在隔离世界执行 JS），
    /// 而非其默认的 `addBinding` 模式（拿主世界 context id 在主世界执行）。
    /// 两者各有取舍：
    /// - 本方案（隔离世界）：网站主世界看不到注入的变量，对网站隐身；
    ///   但无法访问/修改主世界闭包内的变量。
    /// - addBinding（主世界）：能操作主世界，但执行特征暴露在主世界。
    ///
    /// 因此此前注释里"100% stealth parity with Rebrowser"的说法不准确——本实现
    /// 并非 rebrowser 默认模式的等价物。详见 docs/01-architecture.md 第五节。
    pub async fn evaluate_stealth(&self, script: &str) -> Result<Option<Value>> {
        // Get the main frame ID
        let frame_id = self
            .page
            .mainframe()
            .await
            .map_err(|e| anyhow!("{}", e))?
            .ok_or_else(|| anyhow!("No main frame available"))?;

        // Create an isolated world - Chrome returns the Context ID in the response!
        // This is the key insight: we get a context ID without touching Runtime domain
        let isolated_world = self
            .page
            .execute(
                CreateIsolatedWorldParams::builder()
                    .frame_id(frame_id)
                    .world_name("chaser") // Our stealth world
                    .grant_univeral_access(true) // Access to page DOM
                    .build()
                    .unwrap(),
            )
            .await
            .map_err(|e| anyhow!("{}", e))?;

        let ctx_id = isolated_world.result.execution_context_id;

        // Execute in the isolated world using the captured context ID
        let params = EvaluateParams::builder()
            .expression(script)
            .context_id(ctx_id)
            .await_promise(true)
            .return_by_value(true)
            .build()
            .unwrap();

        let res = self
            .page
            .execute(params)
            .await
            .map_err(|e| anyhow!("{}", e))?;
        Ok(res.result.result.value)
    }

    /// 在指定 frame（含 iframe）的隔离世界执行 JS（stealth-safe）。
    ///
    /// 与 `evaluate` 的区别：`evaluate` 只在主 frame 执行；本方法接受任意 FrameId，
    /// 可在 `<iframe>` 内执行脚本——`evaluate` 无法穿透 iframe 文档边界。
    /// 实现与 `evaluate_stealth` 同路：在目标 frame 上 `createIsolatedWorld` →
    /// `Runtime.evaluate(context_id)`，从不调用 `Runtime.enable`。
    ///
    /// 用 `frames()` / `frame()` 拿到 FrameId 后传入。
    pub async fn evaluate_in_frame(
        &self,
        frame_id: impl Into<String>,
        script: &str,
    ) -> Result<Option<Value>> {
        use chromiumoxide_cdp::cdp::browser_protocol::page::FrameId;
        let frame_id = FrameId::from(frame_id.into());
        let isolated_world = self
            .page
            .execute(
                CreateIsolatedWorldParams::builder()
                    .frame_id(frame_id)
                    .world_name("chaser-frame")
                    .grant_univeral_access(true)
                    .build()
                    .unwrap(),
            )
            .await
            .map_err(|e| anyhow!("createIsolatedWorld 失败: {}", e))?;
        let ctx_id = isolated_world.result.execution_context_id;
        let params = EvaluateParams::builder()
            .expression(script)
            .context_id(ctx_id)
            .await_promise(true)
            .return_by_value(true)
            .build()
            .unwrap();
        let res = self
            .page
            .execute(params)
            .await
            .map_err(|e| anyhow!("frame evaluate 失败: {}", e))?;
        Ok(res.result.result.value)
    }

    /// 列出页面所有 frame 的 ID（含主 frame 和所有 iframe）。
    ///
    /// 配合 [`evaluate_in_frame`] 或 [`frame`] 在 iframe 内执行操作。
    /// 想知道每个 frame 是什么，用 `raw_page().frame_url(id)` 查 URL。
    pub async fn frame_ids(&self) -> Result<Vec<String>> {
        let ids = self.page.frames().await.map_err(|e| anyhow!("{}", e))?;
        Ok(ids.into_iter().map(|id| id.into()).collect())
    }

    // ========== Shadow DOM 内操作（穿透 shadow root） ==========
    //
    // 普通 querySelector 不能穿透 shadow boundary。反爬会把关键内容藏进 shadow root
    // （含 closed 模式）让传统爬虫失效。本组方法走 CDP DOM 域的 describeNode(pierce=true)
    // 拿到 shadow root 的 nodeId，再在其内部 querySelector——对 open/closed shadow root
    // 都有效（closed 封装是 JS 层限制，CDP 协议层无视），且不触发 Runtime.enable（stealth-safe）。

    /// 在指定宿主元素的 shadow root 内查找元素（单层穿透）。
    ///
    /// `host_selector` 定位带 shadow root 的宿主元素（如自定义组件），
    /// `inner_selector` 在 shadow root 内部按 CSS 查找。对 open 和 closed shadow root
    /// 都有效。
    ///
    /// # 示例
    /// ```rust,ignore
    /// // 在 <my-widget> 的 shadow root 内找 .price
    /// let el = chaser.find_in_shadow("my-widget", ".price").await?;
    /// ```
    pub async fn find_in_shadow(
        &self,
        host_selector: &str,
        inner_selector: &str,
    ) -> Result<crate::element::Element> {
        // 1. 找到宿主元素
        let host = self
            .page
            .find_element(host_selector)
            .await
            .map_err(|e| anyhow!("找不到宿主元素 '{}': {}", host_selector, e))?;
        // 2. describeNode(pierce=true) 拿 shadow root 的 nodeId
        let shadow_root_id = self.first_shadow_root_id(host.node_id).await?;
        // 3. 在 shadow root 内 querySelector
        self.page
            .find_element_in_root(inner_selector, shadow_root_id)
            .await
            .map_err(|e| anyhow!("shadow root 内找不到 '{}': {}", inner_selector, e))
    }

    /// 多层穿透查找：按 `>>>` 分隔的选择器链逐层进入 shadow root。
    ///
    /// 适合嵌套的 shadow DOM 结构（组件套组件）。
    ///
    /// # 示例
    /// ```rust,ignore
    /// // 三层穿透：host >>> inner-host >>> target
    /// let el = chaser.find_in_shadow_deep("outer-widget >>> inner-widget >>> .btn").await?;
    /// ```
    pub async fn find_in_shadow_deep(&self, selector: &str) -> Result<crate::element::Element> {
        let parts: Vec<&str> = selector.split(">>>").map(|s| s.trim()).collect();
        if parts.len() < 2 {
            return Err(anyhow!(
                "find_in_shadow_deep 选择器需含至少一个 '>>>' 分隔符，如 'host >>> .inner'"
            ));
        }
        // 第一段：在主文档找宿主
        let mut host = self
            .page
            .find_element(parts[0])
            .await
            .map_err(|e| anyhow!("找不到 '{}': {}", parts[0], e))?;
        // 中间各段：在当前宿主的 shadow root 内找下一层宿主
        for part in &parts[1..parts.len() - 1] {
            let shadow_root_id = self.first_shadow_root_id(host.node_id).await?;
            host = self
                .page
                .find_element_in_root(part.to_string(), shadow_root_id)
                .await
                .map_err(|e| anyhow!("shadow root 内找不到 '{}': {}", part, e))?;
        }
        // 最后一段：在最深 shadow root 内找目标
        let final_sel = parts[parts.len() - 1];
        let shadow_root_id = self.first_shadow_root_id(host.node_id).await?;
        self.page
            .find_element_in_root(final_sel.to_string(), shadow_root_id)
            .await
            .map_err(|e| anyhow!("最深 shadow root 内找不到 '{}': {}", final_sel, e))
    }

    /// 用 describeNode(pierce=true) 获取指定节点的第一个 shadow root 的 nodeId。
    async fn first_shadow_root_id(
        &self,
        node_id: chromiumoxide_cdp::cdp::browser_protocol::dom::NodeId,
    ) -> Result<chromiumoxide_cdp::cdp::browser_protocol::dom::NodeId> {
        use chromiumoxide_cdp::cdp::browser_protocol::dom::DescribeNodeParams;
        let resp = self
            .page
            .execute(
                DescribeNodeParams::builder()
                    .node_id(node_id)
                    .depth(1)
                    .pierce(true)
                    .build(),
            )
            .await
            .map_err(|e| anyhow!("describeNode 失败: {}", e))?;
        let shadow_roots = resp.result.node.shadow_roots.ok_or_else(|| {
            anyhow!("元素没有 shadow root（可能不是 shadow host，或 shadow root 尚未挂载）")
        })?;
        let first = shadow_roots
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("元素声明了 shadow_roots 但为空数组"))?;
        Ok(first.node_id)
    }

    /// 按匹配条件找到第一个 iframe，返回 [`ZyFrame`] 句柄。
    ///
    /// `matcher` 是闭包，接收 (frame_url, frame_name)，返回 true 即匹配。
    /// 常见用法：
    /// ```rust,ignore
    /// // 按 URL 子串匹配
    /// let f = chaser.frame(|url, _| url.contains("recaptcha")).await?;
    /// // 按 name 匹配
    /// let f = chaser.frame(|_, name| name == "payment").await?;
    /// ```
    pub async fn frame<F>(&self, matcher: F) -> Result<Option<ZyFrame>>
    where
        F: Fn(&str, &str) -> bool,
    {
        let ids = self.page.frames().await.map_err(|e| anyhow!("{}", e))?;
        // 排除主 frame——用户调 frame() 是找 iframe，宽松匹配器（如 url.contains("api")）
        // 会误匹配到主 frame（其 url 是页面 url）。要操作主 frame 用 evaluate() 即可。
        let main_id: Option<String> = self
            .page
            .mainframe()
            .await
            .map_err(|e| anyhow!("{}", e))?
            .map(|id| id.into());
        for id in ids {
            let id_str: String = id.clone().into();
            if Some(&id_str) == main_id.as_ref() {
                continue; // 跳过主 frame
            }
            let url = self
                .page
                .frame_url(id.clone())
                .await
                .map_err(|e| anyhow!("{}", e))?
                .unwrap_or_default();
            let name = self
                .page
                .frame_name(id.clone())
                .await
                .map_err(|e| anyhow!("{}", e))?
                .unwrap_or_default();
            if matcher(&url, &name) {
                return Ok(Some(ZyFrame {
                    chaser: self.clone(),
                    frame_id: id.into(),
                    url,
                    name,
                }));
            }
        }
        Ok(None)
    }

    /// Moves the mouse to the target coordinates using a human-like Bezier curve path.
    ///
    /// The path includes:
    /// - Randomized control points for natural arcs
    /// - 20% chance of slight overshoot
    /// - Target jitter (±2px)
    /// - Variable delays between movements (5-15ms)
    pub async fn move_mouse_human(&self, x: f64, y: f64) -> Result<()> {
        let start = { *self.mouse_pos.lock().unwrap() };
        let end = Point { x, y };

        // Target Selection Jitter: don't land exactly on the pixel
        let jitter_x = rand::thread_rng().gen_range(-2.0..2.0);
        let jitter_y = rand::thread_rng().gen_range(-2.0..2.0);
        let target_with_jitter = Point {
            x: end.x + jitter_x,
            y: end.y + jitter_y,
        };

        let path = BezierPath::generate(start, target_with_jitter, 25);

        for point in path {
            self.page
                .move_mouse(crate::layout::Point {
                    x: point.x,
                    y: point.y,
                })
                .await
                .map_err(|e| anyhow!("{}", e))?;
            *self.mouse_pos.lock().unwrap() = point;
            // Tiny delay to simulate physical movement
            let delay = rand::thread_rng().gen_range(5..15);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
        }

        Ok(())
    }

    /// Perform a click at the current mouse position.
    pub async fn click(&self) -> Result<()> {
        let pos = { *self.mouse_pos.lock().unwrap() };
        self.page
            .click(crate::layout::Point { x: pos.x, y: pos.y })
            .await
            .map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// Move to target and click with full human-like behavior.
    ///
    /// Combines Bezier curve mouse movement with a natural click, including:
    /// - Human-like path to target
    /// - Small random delay before clicking (50-150ms)
    /// - Variable click duration
    pub async fn click_human(&self, x: f64, y: f64) -> Result<()> {
        // Move to target with bezier curve
        self.move_mouse_human(x, y).await?;

        // Small pause before clicking (humans don't click instantly after arriving)
        let delay1 = rand::thread_rng().gen_range(50..150);
        tokio::time::sleep(tokio::time::Duration::from_millis(delay1)).await;

        // Click
        self.click().await?;

        // Small pause after clicking
        let delay2 = rand::thread_rng().gen_range(30..80);
        tokio::time::sleep(tokio::time::Duration::from_millis(delay2)).await;

        Ok(())
    }

    /// Type text with human-like delays between keystrokes.
    ///
    /// Simulates realistic typing with:
    /// - Variable delay between keys (50-150ms by default)
    /// - Occasional longer pauses (5% chance of 200-400ms pause)
    pub async fn type_text(&self, text: &str) -> Result<()> {
        self.type_text_with_delay(text, 50, 150).await
    }

    /// Type text with custom delay range (in milliseconds).
    ///
    /// # Arguments
    /// * `text` - The text to type
    /// * `min_delay_ms` - Minimum delay between keystrokes
    /// * `max_delay_ms` - Maximum delay between keystrokes
    pub async fn type_text_with_delay(
        &self,
        text: &str,
        min_delay_ms: u64,
        max_delay_ms: u64,
    ) -> Result<()> {
        for c in text.chars() {
            // Send keyDown with the character
            let key_down = DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyDown)
                .text(c.to_string())
                .build()
                .unwrap();

            self.page
                .execute(key_down)
                .await
                .map_err(|e| anyhow!("{}", e))?;

            // Send keyUp
            let key_up = DispatchKeyEventParams::builder()
                .r#type(DispatchKeyEventType::KeyUp)
                .build()
                .unwrap();

            self.page
                .execute(key_up)
                .await
                .map_err(|e| anyhow!("{}", e))?;

            // Random delay between keystrokes
            let delay = rand::thread_rng().gen_range(min_delay_ms..max_delay_ms);

            // 5% chance of a longer "thinking" pause
            let actual_delay = if rand::thread_rng().gen_bool(0.05) {
                rand::thread_rng().gen_range(200..400)
            } else {
                delay
            };

            tokio::time::sleep(tokio::time::Duration::from_millis(actual_delay)).await;
        }

        Ok(())
    }

    /// Press a specific key (e.g., "Enter", "Tab", "Escape").
    pub async fn press_key(&self, key: &str) -> Result<()> {
        // Map common key names to their key codes
        let (key_str, code) = match key {
            "Enter" => ("Enter", "Enter"),
            "Tab" => ("Tab", "Tab"),
            "Escape" => ("Escape", "Escape"),
            "Backspace" => ("Backspace", "Backspace"),
            "Delete" => ("Delete", "Delete"),
            "ArrowUp" => ("ArrowUp", "ArrowUp"),
            "ArrowDown" => ("ArrowDown", "ArrowDown"),
            "ArrowLeft" => ("ArrowLeft", "ArrowLeft"),
            "ArrowRight" => ("ArrowRight", "ArrowRight"),
            _ => (key, key),
        };

        let key_down = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::RawKeyDown)
            .key(key_str)
            .code(code)
            .build()
            .unwrap();

        self.page
            .execute(key_down)
            .await
            .map_err(|e| anyhow!("{}", e))?;

        let key_up = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyUp)
            .key(key_str)
            .code(code)
            .build()
            .unwrap();

        self.page
            .execute(key_up)
            .await
            .map_err(|e| anyhow!("{}", e))?;

        Ok(())
    }

    /// Press Enter key with a small random delay before pressing.
    pub async fn press_enter(&self) -> Result<()> {
        let mut rng = rand::thread_rng();
        tokio::time::sleep(tokio::time::Duration::from_millis(rng.gen_range(100..300))).await;
        self.press_key("Enter").await
    }

    /// Press Tab key to move to next field.
    pub async fn press_tab(&self) -> Result<()> {
        let mut rng = rand::thread_rng();
        tokio::time::sleep(tokio::time::Duration::from_millis(rng.gen_range(50..150))).await;
        self.press_key("Tab").await
    }

    /// Scroll the page with human-like physics (smooth, variable speed).
    ///
    /// Simulates realistic scrolling with:
    /// - Multiple small scroll steps rather than one jump
    /// - Variable scroll distances per step
    /// - Easing at start and end (deceleration)
    ///
    /// # Arguments
    /// * `delta_y` - Total pixels to scroll (positive = down, negative = up)
    pub async fn scroll_human(&self, delta_y: i32) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
        };

        let mut rng = rand::thread_rng();
        let pos = { *self.mouse_pos.lock().unwrap() };

        // Number of scroll steps (more steps = smoother)
        let steps = (delta_y.abs() / 50).clamp(3, 15) as usize;
        let mut remaining = delta_y;

        for i in 0..steps {
            // Ease-in/ease-out: scroll less at start and end
            let progress = i as f64 / steps as f64;
            let ease = if progress < 0.3 {
                progress / 0.3 * 0.5 + 0.5
            } else if progress > 0.7 {
                (1.0 - progress) / 0.3 * 0.5 + 0.5
            } else {
                1.0
            };

            let base_step = remaining / (steps - i) as i32;
            let jitter = rng.gen_range(-10..10);
            let step = ((base_step as f64 * ease) as i32 + jitter).clamp(-200, 200);

            if step == 0 {
                continue;
            }

            let scroll = DispatchMouseEventParams::builder()
                .r#type(DispatchMouseEventType::MouseWheel)
                .x(pos.x)
                .y(pos.y)
                .button(MouseButton::None)
                .delta_x(0.0)
                .delta_y(step as f64)
                .build()
                .unwrap();

            self.page
                .execute(scroll)
                .await
                .map_err(|e| anyhow!("{}", e))?;
            remaining -= step;

            // Variable delay between scroll events (16-50ms for 60-20 FPS feel)
            tokio::time::sleep(tokio::time::Duration::from_millis(rng.gen_range(16..50))).await;
        }

        Ok(())
    }

    // ========== 人性化高级交互（select / upload / drag / idle） ==========

    /// 仿真从 `<select>` 下拉框选择一个选项（按 value 或可见文本匹配）。
    ///
    /// 真实用户选下拉框的流程：点击展开 → 移动到目标项 → 点击。这里用 JS 在
    /// 隔离世界设置 `select.value` 并派发 `change`/`input` 事件触发框架监听器，
    /// 配合点击焦点仿真，行为覆盖绝大多数站点（含 React/Vue 等受控组件）。
    ///
    /// # 参数
    /// - `selector`: select 元素的 CSS selector
    /// - `value`: 要选中的 option 的 value 属性（精确匹配）
    pub async fn select_option(&self, selector: &str, value: &str) -> Result<()> {
        // 先点击 select 让它获焦（仿真真人交互，部分站点靠 focus 状态判定）
        let el = self
            .page
            .find_element(selector)
            .await
            .map_err(|e| anyhow!("{}", e))?;
        el.click().await.map_err(|e| anyhow!("{}", e))?;

        // 在隔离世界设置 value 并触发事件链。注意脚本里不能用模板字面量 ${}
        // （此处是直接通过 evaluate_stealth 发送，不嵌套，但为一致仍用拼接）。
        let script = format!(
            r#"(function() {{
                var sel = document.querySelector({sel_q});
                if (!sel) return false;
                var ok = false;
                for (var i = 0; i < sel.options.length; i++) {{
                    if (sel.options[i].value === {val_q}) {{
                        sel.value = {val_q};
                        sel.selectedIndex = i;
                        ok = true;
                        break;
                    }}
                }}
                if (ok) {{
                    sel.dispatchEvent(new Event('input', {{ bubbles: true }}));
                    sel.dispatchEvent(new Event('change', {{ bubbles: true }}));
                }}
                return ok;
            }})()"#,
            // 将参数字符串 JSON.stringify 进去，安全转义引号/反斜杠
            sel_q = serde_json::to_string(selector).unwrap_or_else(|_| "''".into()),
            val_q = serde_json::to_string(value).unwrap_or_else(|_| "''".into()),
        );
        let ok = self
            .evaluate(&script)
            .await
            .map_err(|e| anyhow!("select_option evaluate 失败: {}", e))?
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !ok {
            return Err(anyhow!(
                "select_option: 在 '{}' 中未找到 value='{}' 的 option",
                selector,
                value
            ));
        }
        Ok(())
    }

    /// 仿真给 `<input type="file">` 设置文件（支持多文件）。
    ///
    /// 走 `DOM.setFileInputFiles`（DOM 域，stealth-safe，不触发 Runtime.enable）。
    /// 文件路径必须是**浏览器所在机器的本机绝对路径**（CDP 直接读取，不经 JS）。
    ///
    /// # 参数
    /// - `selector`: input[type=file] 的 CSS selector
    /// - `file_paths`: 要上传的文件的绝对路径列表
    pub async fn set_input_files(&self, selector: &str, file_paths: &[String]) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::dom::SetFileInputFilesParams;

        let el = self
            .page
            .find_element(selector)
            .await
            .map_err(|e| anyhow!("{}", e))?;
        // input[type=file] 必须有 backend_node_id 才能设置文件
        let backend = el.backend_node_id;
        let cmd = SetFileInputFilesParams::builder()
            .files(file_paths.to_vec())
            .backend_node_id(backend)
            .build()
            .map_err(|e| anyhow!("{}", e))?;
        self.page.execute(cmd).await.map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// 仿真拖拽：用贝塞尔曲线把鼠标从当前位置移到目标，按下→移动→释放。
    ///
    /// 适用于监听 mousedown/mousemove/mouseup 的拖拽（绝大多数滑块、可拖动元素）。
    /// 对监听原生 HTML5 drag-and-drop 事件（dragstart/drop）的站点无效——那种需用
    /// `Page.setInterceptDrags` + `Input.dispatchDragEvent`，本库暂未封装。
    ///
    /// # 参数
    /// - `to_x`/`to_y`: 目标坐标（CSS 像素，相对主框架 viewport）
    pub async fn drag_human(&self, to_x: f64, to_y: f64) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
        };

        // 1. 用贝塞尔曲线仿真移动到目标（带抖动/过冲）
        self.move_mouse_human(to_x, to_y).await?;

        // 2. 按下鼠标左键
        self.page
            .execute(
                DispatchMouseEventParams::builder()
                    .r#type(DispatchMouseEventType::MousePressed)
                    .x(to_x)
                    .y(to_y)
                    .button(MouseButton::Left)
                    .click_count(1)
                    .build()
                    .unwrap(),
            )
            .await
            .map_err(|e| anyhow!("{}", e))?;

        // 3. 小幅停顿后缓慢移动一小段（仿真"按住拖动"）
        let mut rng = rand::thread_rng();
        tokio::time::sleep(tokio::time::Duration::from_millis(rng.gen_range(50..150))).await;
        // 在目标附近做几次微小位移，触发 mousemove 监听器
        let steps = rng.gen_range(3..7);
        for i in 0..steps {
            let jx = to_x + rng.gen_range(-3.0..3.0);
            let jy = to_y + rng.gen_range(-3.0..3.0) + (i as f64) * 2.0;
            self.page
                .execute(
                    DispatchMouseEventParams::builder()
                        .r#type(DispatchMouseEventType::MouseMoved)
                        .x(jx)
                        .y(jy)
                        .button(MouseButton::Left)
                        .build()
                        .unwrap(),
                )
                .await
                .map_err(|e| anyhow!("{}", e))?;
            tokio::time::sleep(tokio::time::Duration::from_millis(rng.gen_range(20..60))).await;
        }

        // 4. 释放鼠标（drop）
        let end_x = to_x + rng.gen_range(2.0..6.0);
        let end_y = to_y + rng.gen_range(2.0..6.0);
        self.page
            .execute(
                DispatchMouseEventParams::builder()
                    .r#type(DispatchMouseEventType::MouseReleased)
                    .x(end_x)
                    .y(end_y)
                    .button(MouseButton::Left)
                    .click_count(1)
                    .build()
                    .unwrap(),
            )
            .await
            .map_err(|e| anyhow!("{}", e))?;

        // 同步内部鼠标位置
        *self.mouse_pos.lock().unwrap() = Point { x: end_x, y: end_y };
        Ok(())
    }

    /// 仿真"无意图等待"——人类在页面上的自然停顿（看内容、思考）。
    ///
    /// 行为分析型反爬（DataDome/Akamai）会检测"操作间隔是否过于规律"。在自动化
    /// 流程里穿插此方法，引入随机长度的人类式停顿，降低被判定为 bot 的概率。
    ///
    /// 默认随机 800ms~2.5s。可通过 `min_ms`/`max_ms` 自定义区间。
    pub async fn human_idle(&self, min_ms: u64, max_ms: u64) -> Result<()> {
        let mut rng = rand::thread_rng();
        let (lo, hi) = if max_ms <= min_ms {
            (min_ms, min_ms + 1)
        } else {
            (min_ms, max_ms)
        };
        let delay = rng.gen_range(lo..hi);
        tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
        Ok(())
    }

    /// 仿真"无意图等待"的便捷形式：固定随机区间 800~2500ms。
    pub async fn idle(&self) -> Result<()> {
        self.human_idle(800, 2500).await
    }

    // ========== Dialog（alert/confirm/prompt/beforeunload）处理 ==========

    /// 注册一个自动 dialog 处理器：每次页面弹出 alert/confirm/prompt/beforeunload
    /// 对话框时，调用 `handler` 获取处理决策（accept 还是 dismiss、prompt 输入文本）。
    ///
    /// 不注册处理器时，弹出对话框会**阻塞页面执行**（headless 下无人能点），
    /// 导致后续操作全部挂起——这是自动化里极常见的卡死原因。
    ///
    /// `handler` 接收 dialog 类型与消息文本，返回 `(accept, prompt_text)`：
    /// - `accept=true` 接受对话框（confirm/prompt 点"确定"，alert 点"好"）
    /// - `prompt_text` 仅 prompt 对话框生效，作为输入文本
    ///
    /// # 示例
    /// ```rust,ignore
    /// chaser.on_dialog(|dtype, msg| async move {
    ///     println!("dialog: {:?} {}", dtype, msg);
    ///     (true, None)  // 全部接受
    /// }).await?;
    /// ```
    pub async fn on_dialog<F, Fut>(&self, handler: F) -> Result<()>
    where
        F: Fn(DialogType, String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = (bool, Option<String>)> + Send + 'static,
    {
        use chromiumoxide_cdp::cdp::browser_protocol::page::{
            EventJavascriptDialogOpening, HandleJavaScriptDialogParams,
        };
        use futures::StreamExt;

        let mut stream = self
            .page
            .event_listener::<EventJavascriptDialogOpening>()
            .await
            .map_err(|e| anyhow!("订阅 dialog 事件失败: {}", e))?;
        let page = self.page.clone();

        tokio::spawn(async move {
            while let Some(ev) = stream.next().await {
                let dtype = DialogType::from_cdp(&ev.r#type);
                let msg = ev.message.clone();
                let (accept, prompt_text) = handler(dtype, msg).await;

                let mut cmd = HandleJavaScriptDialogParams::builder().accept(accept);
                if let Some(txt) = prompt_text {
                    cmd = cmd.prompt_text(txt);
                }
                // 处理 dialog 失败只能记日志——回调里没法回传错误给调用方，
                // 且 dialog 未处理会一直阻塞页面，尽力而为。
                if let Err(e) = page.execute(cmd.build().unwrap()).await {
                    tracing::warn!("handleJavaScriptDialog 失败: {}", e);
                }
            }
        });
        Ok(())
    }

    /// 简化的 dialog 处理：自动接受（accept=true）或自动忽略（accept=false）所有
    /// 弹出的对话框，不做任何区分。适合"我只想让弹框别挡路"的场景。
    ///
    /// 等价于 `on_dialog(|_, _| async move { (accept, None) })`。
    pub async fn auto_handle_dialogs(&self, accept: bool) -> Result<()> {
        self.on_dialog(move |_dtype, _msg| async move { (accept, None) })
            .await
    }

    // ========== 代理认证（Fetch.continueWithAuth） ==========

    /// 注册代理 HTTP 认证凭据，自动响应所有 407 Proxy Authentication Required 挑战。
    ///
    /// Chrome 原生不支持 `user:pass@host:port` 形式的代理认证（这是 Chrome 的限制，
    /// 非 zycdp 问题）。本方法通过 `Fetch` 域拦截认证请求并自动填入凭据，让带认证的
    /// HTTP/SOCKS5 代理可直接使用，无需本地转发器。
    ///
    /// **必须在导航到目标站点前调用**（认证处理器需先就位）。调用时已自动开启
    /// Fetch 域的 `handleAuthRequests`。
    ///
    /// # 示例
    /// ```rust,ignore
    /// let config = BrowserConfig::builder()
    ///     .proxy_server("http://10.0.0.1:8080")  // 代理地址（不带认证）
    ///     .build()?;
    /// // ... launch, new_page, ChaserPage ...
    /// chaser.enable_proxy_auth("user", "pass").await?;  // 注册认证
    /// chaser.goto("https://example.com").await?;        // 之后正常导航
    /// ```
    pub async fn enable_proxy_auth(&self, username: &str, password: &str) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::fetch::{
            AuthChallengeResponse, AuthChallengeResponseResponse, AuthChallengeSource,
            ContinueWithAuthParams, EventAuthRequired,
        };
        use futures::StreamExt;

        // 开启 Fetch 域：拦截所有请求 + 要求处理认证。
        // 注意 handle_auth_requests(true) 才会让 authRequired 事件发出来。
        self.page
            .execute(
                FetchEnableParams::builder()
                    .handle_auth_requests(true)
                    .pattern(RequestPattern::builder().url_pattern("*").build())
                    .build(),
            )
            .await
            .map_err(|e| anyhow!("Fetch.enable 失败: {}", e))?;

        let mut stream = self
            .page
            .event_listener::<EventAuthRequired>()
            .await
            .map_err(|e| anyhow!("订阅 authRequired 事件失败: {}", e))?;
        let page = self.page.clone();
        let user = username.to_string();
        let pass = password.to_string();

        tokio::spawn(async move {
            while let Some(ev) = stream.next().await {
                // 关键安全过滤：只响应代理认证（source == Proxy）。
                // source == Server 是站点本身的 401 basic auth，此时绝不能把代理
                // 用户名/密码当站点凭据发出去——既是凭据泄露，也会破坏站点登录。
                // 对 Server 认证用 Default（交给浏览器默认行为，不提供凭据）。
                let is_proxy = ev.auth_challenge.source == Some(AuthChallengeSource::Proxy);
                let mut resp = AuthChallengeResponse::builder();
                if is_proxy {
                    resp = resp
                        .response(AuthChallengeResponseResponse::ProvideCredentials)
                        .username(user.clone())
                        .password(pass.clone());
                } else {
                    resp = resp.response(AuthChallengeResponseResponse::Default);
                }
                let cmd = ContinueWithAuthParams::builder()
                    .request_id(ev.request_id.clone())
                    .auth_challenge_response(resp.build().unwrap())
                    .build()
                    .unwrap();
                if let Err(e) = page.execute(cmd).await {
                    tracing::warn!("Fetch.continueWithAuth 失败: {}", e);
                }
            }
        });
        Ok(())
    }

    // ========== 地理位置伪造 + 权限授予 ==========

    /// 一站式启用地理位置伪造：设置坐标 + 授予 geolocation 权限。
    ///
    /// 单独 `emulate_geolocation` 不授予权限时，站点调 `getCurrentPosition` 会被拒。
    /// 本方法组合坐标 override + 权限授予，让站点直接拿到伪造位置。
    ///
    /// # 示例
    /// ```rust,ignore
    /// chaser.enable_geolocation(37.7749, -122.4194).await?;  // 旧金山坐标
    /// ```
    pub async fn enable_geolocation(&self, latitude: f64, longitude: f64) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::browser::{
            PermissionDescriptor, PermissionSetting, SetPermissionParams,
        };
        use chromiumoxide_cdp::cdp::browser_protocol::emulation::SetGeolocationOverrideParams;

        // 1. 授予 geolocation 权限（Browser 域）
        self.page
            .execute(
                SetPermissionParams::builder()
                    .permission(
                        PermissionDescriptor::builder()
                            .name("geolocation")
                            .build()
                            .unwrap(),
                    )
                    .setting(PermissionSetting::Granted)
                    .build()
                    .unwrap(),
            )
            .await
            .map_err(|e| anyhow!("setPermission 失败: {}", e))?;

        // 2. 设置坐标 override（Emulation 域）
        self.page
            .execute(
                SetGeolocationOverrideParams::builder()
                    .latitude(latitude)
                    .longitude(longitude)
                    .build(),
            )
            .await
            .map_err(|e| anyhow!("setGeolocationOverride 失败: {}", e))?;
        Ok(())
    }

    /// 批量授予站点权限，避免首次访问弹权限框导致卡住。
    ///
    /// 常用值：`"geolocation"`、`"clipboard-read"`、`"clipboard-write"`、
    /// `"notifications"`、`"camera"`、`"microphone"`。
    pub async fn grant_permissions(&self, permissions: &[&str]) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::browser::{
            PermissionDescriptor, PermissionSetting, SetPermissionParams,
        };

        for &perm in permissions {
            self.page
                .execute(
                    SetPermissionParams::builder()
                        .permission(PermissionDescriptor::builder().name(perm).build().unwrap())
                        .setting(PermissionSetting::Granted)
                        .build()
                        .unwrap(),
                )
                .await
                .map_err(|e| anyhow!("grant {} 失败: {}", perm, e))?;
        }
        Ok(())
    }

    // ========== 键盘组合键 + 鼠标右键/双击 ==========

    /// 按下修饰键 + 普通键的组合（如 Ctrl+A、Shift+Tab、Ctrl+Enter）。
    ///
    /// `modifiers` 先按下，`key` 最后按，全部释放。修饰键：Control/Shift/Alt/Meta。
    ///
    /// # 示例
    /// ```rust,ignore
    /// chaser.press_key_combo(&["Control"], "a").await?;  // 全选
    /// chaser.press_key_combo(&["Control", "Shift"], "Tab").await?;  // 反向切焦点
    /// ```
    pub async fn press_key_combo(&self, modifiers: &[&str], key: &str) -> Result<()> {
        // CDP 的按键事件必须带 modifiers 位标志，浏览器才识别为组合键。
        // 位值：Alt=1, Ctrl=2, Meta=4, Shift=8（CDP 文档）。
        let modifier_bits: i64 = modifiers.iter().map(|m| match *m {
            "Alt" => 1,
            "Control" => 2,
            "Meta" => 4,
            "Shift" => 8,
            _ => 0,
        }).sum();

        // 跟踪已按下的修饰键，任何步骤失败时尽力释放已按下的，避免键盘状态卡住
        // （修饰键没释放会导致后续所有操作都带 Ctrl/Shift）。
        let mut held_count = 0usize;
        let result: Result<()> = async {
            // 1. 按下所有修饰键（每个带自己的 modifiers 标志）
            for m in modifiers {
                self.hold_key(m).await?;
                held_count += 1;
            }
            // 2. 按目标键（关键：必须用 KeyDefinition 的 virtual key code + modifiers
            //    位标志，浏览器才识别为组合键快捷键。光有 key/code 字符串不够——
            //    windows_virtual_key_code 是浏览器判断快捷键的依据）。
            let key_definition = crate::keys::get_key_definition(key)
                .ok_or_else(|| anyhow!("Key not found: {key}"))?;
            let mut cmd = DispatchKeyEventParams::builder()
                .key(key_definition.key)
                .code(key_definition.code)
                .windows_virtual_key_code(key_definition.key_code)
                .native_virtual_key_code(key_definition.key_code)
                .modifiers(modifier_bits);
            // 单字符键带 text（产生输入），功能键用 RawKeyDown
            if key_definition.key.len() == 1 {
                cmd = cmd.text(key_definition.key);
                cmd = cmd.r#type(DispatchKeyEventType::KeyDown);
            } else {
                cmd = cmd.r#type(DispatchKeyEventType::RawKeyDown);
            }
            self.page
                .execute(cmd.build().unwrap())
                .await
                .map_err(|e| anyhow!("{}", e))?;
            self.page
                .execute(
                    DispatchKeyEventParams::builder()
                        .r#type(DispatchKeyEventType::KeyUp)
                        .key(key_definition.key)
                        .code(key_definition.code)
                        .windows_virtual_key_code(key_definition.key_code)
                        .native_virtual_key_code(key_definition.key_code)
                        .modifiers(modifier_bits)
                        .build()
                        .unwrap(),
                )
                .await
                .map_err(|e| anyhow!("{}", e))?;
            // 3. 反序释放修饰键
            for m in modifiers.iter().rev() {
                self.release_key(m).await?;
            }
            Ok(())
        }
        .await;
        if result.is_err() && held_count > 0 {
            // 失败清理：反序释放已按下的修饰键，尽力而为
            for m in modifiers[..held_count].iter().rev() {
                let _ = self.release_key(m).await;
            }
        }
        result
    }

    /// 按住一个键不释放（配合 release_key 实现组合键）。stealth-safe（走 Input 域）。
    async fn hold_key(&self, key: &str) -> Result<()> {
        let (key_str, code) = match key {
            "Control" => ("Control", "ControlLeft"),
            "Shift" => ("Shift", "ShiftLeft"),
            "Alt" => ("Alt", "AltLeft"),
            "Meta" => ("Meta", "MetaLeft"),
            _ => (key, key),
        };
        let key_down = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::RawKeyDown)
            .key(key_str)
            .code(code)
            .build()
            .unwrap();
        self.page
            .execute(key_down)
            .await
            .map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// 释放一个按住的键（配合 hold_key）。
    pub async fn release_key(&self, key: &str) -> Result<()> {
        let (key_str, code) = match key {
            "Control" => ("Control", "ControlLeft"),
            "Shift" => ("Shift", "ShiftLeft"),
            "Alt" => ("Alt", "AltLeft"),
            "Meta" => ("Meta", "MetaLeft"),
            _ => (key, key),
        };
        let key_up = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyUp)
            .key(key_str)
            .code(code)
            .build()
            .unwrap();
        self.page
            .execute(key_up)
            .await
            .map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// 在指定坐标右键点击（contextmenu）。
    pub async fn right_click(&self, x: f64, y: f64) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
        };
        self.page
            .execute(
                DispatchMouseEventParams::builder()
                    .r#type(DispatchMouseEventType::MousePressed)
                    .x(x)
                    .y(y)
                    .button(MouseButton::Right)
                    .click_count(1)
                    .build()
                    .unwrap(),
            )
            .await
            .map_err(|e| anyhow!("{}", e))?;
        self.page
            .execute(
                DispatchMouseEventParams::builder()
                    .r#type(DispatchMouseEventType::MouseReleased)
                    .x(x)
                    .y(y)
                    .button(MouseButton::Right)
                    .click_count(1)
                    .build()
                    .unwrap(),
            )
            .await
            .map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// 在指定坐标双击。
    ///
    /// 真实双击序列：pressed(1) released(1) pressed(2) released(2)，第二次按下
    /// 的 click_count=2 触发浏览器的 dblclick 事件。
    pub async fn double_click(&self, x: f64, y: f64) -> Result<()> {
        use chromiumoxide_cdp::cdp::browser_protocol::input::{
            DispatchMouseEventParams, DispatchMouseEventType, MouseButton,
        };
        for count in 1..=2 {
            self.page
                .execute(
                    DispatchMouseEventParams::builder()
                        .r#type(DispatchMouseEventType::MousePressed)
                        .x(x)
                        .y(y)
                        .button(MouseButton::Left)
                        .click_count(count)
                        .build()
                        .unwrap(),
                )
                .await
                .map_err(|e| anyhow!("{}", e))?;
            self.page
                .execute(
                    DispatchMouseEventParams::builder()
                        .r#type(DispatchMouseEventType::MouseReleased)
                        .x(x)
                        .y(y)
                        .button(MouseButton::Left)
                        .click_count(count)
                        .build()
                        .unwrap(),
                )
                .await
                .map_err(|e| anyhow!("{}", e))?;
        }
        Ok(())
    }

    // ========== 弹窗（popup）捕获 ==========

    /// 阻塞等待由本页面打开的新窗口/popup 完成，返回新页面的 target_id。
    ///
    /// 底层 `Target.setAutoAttach` 已开启（zycdp 默认），子 target（新 tab、popup）
    /// 会自动 attach。本方法订阅 `Target.attachedToTarget` 事件，按 openerId 过滤
    /// 出"由当前页面打开的"新 target。
    ///
    /// 用返回的 target_id 调 `browser.get_page(target_id)` 拿到新 Page 句柄。
    ///
    /// # 示例
    /// ```rust,ignore
    /// // 先订阅，再点击触发 popup（避免时序）
    /// let popup_task = tokio::spawn({
    ///     let chaser = chaser.clone();
    ///     async move { chaser.wait_for_popup(Duration::from_secs(10)).await }
    /// });
    /// chaser.click_by_text("在新窗口打开").await?;
    /// let target_id = popup_task.await.unwrap()??.unwrap();
    /// ```
    pub async fn wait_for_popup(&self, timeout: std::time::Duration) -> Result<Option<String>> {
        use chromiumoxide_cdp::cdp::browser_protocol::target::EventAttachedToTarget;
        use futures::StreamExt;

        let my_target: String = self.page.target_id().clone().into();
        let mut stream = self
            .page
            .event_listener::<EventAttachedToTarget>()
            .await
            .map_err(|e| anyhow!("订阅 attachedToTarget 失败: {}", e))?;

        loop {
            match tokio::time::timeout(timeout, stream.next()).await {
                Err(_) => {
                    return Err(anyhow!(
                        "wait_for_popup 超时（{:?}）未捕获到 popup",
                        timeout
                    ))
                }
                Ok(None) => return Ok(None),
                Ok(Some(ev)) => {
                    // 过滤：openerId 等于当前页面 target_id 的才是本页打开的 popup
                    let info = &ev.target_info;
                    let opener_matches = info
                        .opener_id
                        .as_ref()
                        .map(|s| -> String { s.clone().into() })
                        == Some(my_target.clone());
                    if opener_matches {
                        return Ok(Some(info.target_id.clone().into()));
                    }
                }
            }
        }
    }

    /// mimicking how real humans type.
    pub async fn type_text_with_typos(&self, text: &str) -> Result<()> {
        let mut rng = rand::thread_rng();
        let typo_chars = ['q', 'w', 'e', 'r', 't', 'a', 's', 'd', 'f', 'g'];

        for c in text.chars() {
            // 3% chance of typo
            if rng.gen_bool(0.03) && c.is_alphabetic() {
                // Type wrong character
                let typo = typo_chars[rng.gen_range(0..typo_chars.len())];
                self.type_single_char(typo).await?;

                // Brief pause to "notice" the mistake
                tokio::time::sleep(tokio::time::Duration::from_millis(rng.gen_range(100..300)))
                    .await;

                // Backspace to correct
                self.press_key("Backspace").await?;
                tokio::time::sleep(tokio::time::Duration::from_millis(rng.gen_range(30..80))).await;
            }

            // Type the correct character
            self.type_single_char(c).await?;

            // Random delay
            let delay = rng.gen_range(50..150);
            let actual_delay = if rng.gen_bool(0.05) {
                rng.gen_range(200..400) // thinking pause
            } else {
                delay
            };
            tokio::time::sleep(tokio::time::Duration::from_millis(actual_delay)).await;
        }

        Ok(())
    }

    /// Helper to type a single character
    async fn type_single_char(&self, c: char) -> Result<()> {
        let key_down = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyDown)
            .text(c.to_string())
            .build()
            .unwrap();

        self.page
            .execute(key_down)
            .await
            .map_err(|e| anyhow!("{}", e))?;

        let key_up = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyUp)
            .build()
            .unwrap();

        self.page
            .execute(key_up)
            .await
            .map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }
}

/// 下载完成信息（由 [`ChaserPage::wait_for_download`] 返回）。
#[derive(Debug, Clone)]
pub struct DownloadInfo {
    /// 下载的全局唯一 guid。
    pub guid: String,
    /// 建议的文件名（来自 Content-Disposition）。
    pub filename: String,
    /// 落盘路径（平台相关，不保证已设置）。
    pub filepath: Option<String>,
}

/// iframe 句柄，封装"在指定 iframe 内执行操作"的语义。
///
/// 通过 [`ChaserPage::frame`] 获取。所有方法都在该 iframe 的隔离世界执行
/// （stealth-safe，不触发 `Runtime.enable`）。
///
/// # 示例
/// ```rust,ignore
/// // 找到 reCAPTCHA iframe 并在它里面执行 JS
/// if let Some(f) = chaser.frame(|url, _| url.contains("recaptcha")).await? {
///     let title = f.evaluate("document.title").await?;
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ZyFrame {
    chaser: ChaserPage,
    frame_id: String,
    url: String,
    name: String,
}

impl ZyFrame {
    /// 该 iframe 的 URL。
    pub fn url(&self) -> &str {
        &self.url
    }

    /// 该 iframe 的 name 属性。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 在该 iframe 的隔离世界执行 JS（stealth-safe）。
    pub async fn evaluate(&self, script: &str) -> Result<Option<Value>> {
        self.chaser.evaluate_in_frame(&self.frame_id, script).await
    }

    /// 在该 iframe 内按 CSS selector 点击元素（通过 JS dispatchEvent）。
    ///
    /// 走 JS 而非 DOM 域，因为 DOM 域的 querySelector 以主文档为根，无法穿透
    /// iframe 边界；在 iframe 自己的隔离世界里跑 querySelector 才能找到 iframe
    /// 内的元素。
    pub async fn click_in(&self, selector: &str) -> Result<()> {
        let sel = serde_json::to_string(selector).unwrap_or_else(|_| "''".into());
        let script = format!(
            r#"(function() {{
                var el = document.querySelector({sel});
                if (!el) return false;
                el.click();
                return true;
            }})()"#,
            sel = sel
        );
        let ok = self
            .evaluate(&script)
            .await
            .map_err(|e| anyhow!("frame click_in 失败: {}", e))?
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !ok {
            return Err(anyhow!(
                "frame click_in: 在 iframe 内未找到 selector '{}'",
                selector
            ));
        }
        Ok(())
    }

    /// 在该 iframe 内按 CSS selector 读取元素的 innerText。
    pub async fn text_in(&self, selector: &str) -> Result<Option<String>> {
        let sel = serde_json::to_string(selector).unwrap_or_else(|_| "''".into());
        let script = format!(
            r#"(function() {{
                var el = document.querySelector({sel});
                return el ? el.innerText : null;
            }})()"#,
            sel = sel
        );
        self.evaluate(&script)
            .await
            .map_err(|e| anyhow!("frame text_in 失败: {}", e))
            .map(|v| v.and_then(|x| x.as_str().map(|s| s.to_string())))
    }
}

/// 轻量 Locator 句柄，封装"按 selector 反复查询元素"的语义。
///
/// 与直接 `wait_for_selector` 一次拿 Element 的区别：ZyLocator 每次调用 `click`/
/// `wait`/`text` 时都重新查询，元素因页面重渲染变成 stale 后仍可继续使用。
///
/// # 示例
/// ```rust,ignore
/// let btn = chaser.locator("#submit");
/// btn.click().await?;                       // 等待出现并点击（默认 30s 超时）
/// let label = btn.text().await?;            // 重新查询并读取文本
/// ```
#[derive(Debug, Clone)]
pub struct ZyLocator {
    chaser: ChaserPage,
    selector: String,
}

impl ZyLocator {
    /// 默认等待超时。绝大多数页面元素应在 30 秒内出现。
    const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    /// 等待元素出现，最多 `timeout`，返回 Element（每次调用都重新查询，抗 stale）。
    pub async fn wait_with_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<crate::element::Element> {
        self.chaser.wait_for_selector(&self.selector, timeout).await
    }

    /// 用默认超时（30s）等待元素出现。
    pub async fn wait(&self) -> Result<crate::element::Element> {
        self.wait_with_timeout(Self::DEFAULT_TIMEOUT).await
    }

    /// 等待元素出现并点击。每次调用重新查询，页面重渲染后仍可用。
    pub async fn click(&self) -> Result<()> {
        let el = self.wait().await?;
        el.click().await.map_err(|e| anyhow!("{}", e))?;
        Ok(())
    }

    /// 等待元素出现并读取其 inner_text。
    pub async fn text(&self) -> Result<Option<String>> {
        let el = self.wait().await?;
        el.inner_text().await.map_err(|e| anyhow!("{}", e))
    }
}

#[derive(Debug)]
pub struct BezierPath;

impl BezierPath {
    /// Generates a path of points from start to end using a cubic Bezier curve.
    ///
    /// The curve includes randomized control points to create natural, human-like arcs.
    pub fn generate(start: Point, end: Point, steps: usize) -> Vec<Point> {
        let mut rng = rand::thread_rng();
        let mut path = Vec::with_capacity(steps);

        // Calculate distance for offset scaling
        let dist = ((end.x - start.x).powi(2) + (end.y - start.y).powi(2)).sqrt();
        let offset_range = dist * 0.3;

        // 零距离守卫：start == end 时 offset_range 为 0，后续 gen_range(-0.0..0.0)
        // 会触发 rand 的 assert 而 panic。直接返回终点即可（无需移动）。
        if offset_range == 0.0 {
            path.push(end);
            return path;
        }

        // First control point (25% along the path with random offset)
        let p1 = Point {
            x: start.x + (end.x - start.x) * 0.25 + rng.gen_range(-offset_range..offset_range),
            y: start.y + (end.y - start.y) * 0.25 + rng.gen_range(-offset_range..offset_range),
        };

        // Second control point (75% along the path with random offset)
        // 20% chance of overshoot
        let mut p2 = Point {
            x: start.x + (end.x - start.x) * 0.75 + rng.gen_range(-offset_range..offset_range),
            y: start.y + (end.y - start.y) * 0.75 + rng.gen_range(-offset_range..offset_range),
        };

        if rng.gen_bool(0.20) {
            let overshoot_amt = dist * 0.05;
            p2.x += if end.x > start.x {
                overshoot_amt
            } else {
                -overshoot_amt
            };
            p2.y += if end.y > start.y {
                overshoot_amt
            } else {
                -overshoot_amt
            };
        }

        // Generate points along the Bezier curve
        for i in 0..=steps {
            let t = i as f64 / steps as f64;

            // Cubic Bezier formula
            let x = (1.0 - t).powi(3) * start.x
                + 3.0 * (1.0 - t).powi(2) * t * p1.x
                + 3.0 * (1.0 - t) * t.powi(2) * p2.x
                + t.powi(3) * end.x;

            let y = (1.0 - t).powi(3) * start.y
                + 3.0 * (1.0 - t).powi(2) * t * p1.y
                + 3.0 * (1.0 - t) * t.powi(2) * p2.y
                + t.powi(3) * end.y;

            path.push(Point { x, y });
        }

        path
    }
}

/// Parse the Chrome major version out of a User-Agent string.
/// Works for both `Chrome/131.0.0.0` and `HeadlessChrome/131.0.0.0`.
fn parse_chrome_major(ua: &str) -> Option<u32> {
    ua.split("Chrome/")
        .nth(1)
        .and_then(|s| s.split('.').next())
        .and_then(|v| v.parse().ok())
}
