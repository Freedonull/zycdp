//! 离线 stealth 指纹回归测试（P0-3）
//!
//! 目的：在不依赖任何外部反爬站点的前提下，断言 bootstrap 注入后的指纹值
//! 与 ChaserProfile 配置一致。这样每次改 bootstrap / merge 上游后，CI 能自动
//! 验证 stealth 没回归（此前唯一的相关测试 rebrowser.rs 标了 #[ignore]，CI 零覆盖）。
//!
//! 策略：about:blank 空文档 + apply_profile 注入 bootstrap，再导航到 data: URL
//! 触发 init script，用 evaluate_stealth 读回各属性值断言。data: URL 不发网络请求，
//! 测试稳定可复现。
//!
//! 注意：仍需本机安装 Chrome/Chromium（CI 的 test-integration job 已装）。
//! 这是 stealth 库的本质要求——指纹 patch 必须在真实浏览器里验证，纯单测无意义。
//!
//! 生命周期：每个测试启动一个独立浏览器（唯一 user-data-dir），在闭包内做完断言后，
//! browser 在函数返回时自然 drop（kill_on_drop 杀掉 chrome 进程）。
//! 不用 tests/lib.rs 的 test_config：它用固定默认 user-data-dir，在开发机上若被
//! 日常 Chrome 占用会触发 LaunchExit(21) profile 锁错误。

use futures::StreamExt;
use serde_json::{Value, json};
use zycdp::{Browser, BrowserConfig, ChaserPage, ChaserProfile, Gpu};

/// 极简 HTML 测试页（含一个 canvas 供 WebGL 测试用）。用 data: URL 注入，
/// 避免任何网络依赖。
const TEST_PAGE: &str = "data:text/html,<!DOCTYPE html><html><body><canvas id='c' width='1' height='1'></canvas></body></html>";

/// 在隔离世界里执行脚本并取回 JSON 值。
async fn eval(chaser: &ChaserPage, script: &str) -> anyhow::Result<Value> {
    chaser
        .evaluate(script)
        .await?
        .ok_or_else(|| anyhow::anyhow!("evaluate 返回 None: {script}"))
}

/// 启动独立浏览器（唯一 user-data-dir），应用 profile，导航到 TEST_PAGE，
/// 然后执行传入的测试闭包。闭包返回后 browser 自然 drop，chrome 进程被回收。
///
/// `RUST_TEST_THREADS=1` 保证测试串行，避免多 chrome 并发抢资源。
async fn with_stealth_profile<F, Fut>(profile: &ChaserProfile, f: F) -> anyhow::Result<()>
where
    F: FnOnce(ChaserPage) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    // 每次调用生成唯一子目录，避免测试间 profile 锁冲突。
    let unique_dir = std::env::temp_dir().join(format!(
        "zycdp-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let (browser, mut handler) = Browser::launch(
        BrowserConfig::builder()
            .new_headless_mode()
            .user_data_dir(&unique_dir)
            .build()
            .map_err(|e| anyhow::anyhow!(e))?,
    )
    .await?;

    // handler 循环必须持续推进，否则 CDP 命令永远挂起。
    tokio::spawn(async move {
        while handler.next().await.is_some() {}
    });

    // 1. 先开 about:blank（建立页面）
    let page = browser.new_page("about:blank").await?;
    let chaser = ChaserPage::new(page);
    // 2. 应用 profile（bootstrap 经 AddScriptToEvaluateOnNewDocument 注入，
    //    会在下一个新文档加载时执行；about:blank 已加载完，所以 bootstrap
    //    此刻还没跑，必须再触发一次导航）。
    //    不能用 page.set_content()：它内部走 Runtime secondary execution context，
    //    而 zycdp 删除了 Runtime.enable（stealth 红线），拿不到该 context。
    //    用 data: URL 真实导航：不触网络、是真实新文档、init script 正常触发。
    chaser.apply_profile(profile).await?;
    chaser.goto(TEST_PAGE).await?;

    // 3. 执行测试逻辑
    let result = f(chaser).await;

    // 4. 清理：browser drop → kill_on_drop 杀 chrome；删临时目录。
    //    注意 browser 必须在这里 drop（不能 forget），否则 chrome 进程泄漏。
    drop(browser);
    let _ = std::fs::remove_dir_all(&unique_dir);

    result
}

#[tokio::test]
async fn fingerprint_consistency_windows_profile() -> anyhow::Result<()> {
    let profile = ChaserProfile::windows()
        .chrome_version(131)
        .gpu(Gpu::NvidiaRTX3080)
        .memory_gb(16)
        .cpu_cores(8)
        .build();

    with_stealth_profile(&profile, async |chaser| {
        // 指纹属性大部分是 prototype 上的 getter 或实例赋值，隔离世界
        // （evaluate_stealth）读不到（descriptor 不跨 realm 传播 getter）。
        // 所以这里全部用 Element::call_js_fn 在主世界读——这才是反爬站点
        // 真正看到的视角（站点 JS 跑在主世界）。
        let body = chaser.raw_page().find_element("body").await.map_err(|e| anyhow::anyhow!(e))?;

        // 1. navigator.webdriver 必须为 false
        let v = body
            .call_js_fn("function() { return navigator.webdriver; }", false)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("webdriver 返回 None"))?;
        assert_eq!(v, json!(false), "navigator.webdriver 应为 false");

        // 2. navigator.platform 与 profile 一致
        let v = body
            .call_js_fn("function() { return navigator.platform; }", false)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("platform 返回 None"))?;
        assert_eq!(v, json!("Win32"), "platform 应为 Win32, 实际: {v}");

        // 3. hardwareConcurrency
        let v = body
            .call_js_fn("function() { return navigator.hardwareConcurrency; }", false)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("hardwareConcurrency 返回 None"))?;
        assert_eq!(v, json!(8), "hardwareConcurrency 应为 8, 实际: {v}");

        // 4. deviceMemory —— profile 设了 16GB，规范上限离散值为 8
        //    （JSON 里 8 和 8.0 等价，用数值比较）
        let v = body
            .call_js_fn("function() { return navigator.deviceMemory; }", false)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("deviceMemory 返回 None"))?;
        assert_eq!(
            v.as_f64(),
            Some(8.0),
            "deviceMemory 应为 8.0（16GB 离散化）, 实际: {v}"
        );

        // 5. WebGL vendor/renderer（需要先 getContext）
        let v = body
            .call_js_fn(
                "function(){var c=document.getElementById('c');if(!c)return null;\
                 var g=c.getContext('webgl2')||c.getContext('webgl');if(!g)return null;\
                 return g.getParameter(37445);}",
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("webgl vendor 返回 None"))?;
        assert_eq!(
            v,
            json!("Google Inc. (NVIDIA)"),
            "WebGL vendor 应为 Google Inc. (NVIDIA), 实际: {v}"
        );

        let v = body
            .call_js_fn(
                "function(){var c=document.getElementById('c');if(!c)return null;\
                 var g=c.getContext('webgl2')||c.getContext('webgl');if(!g)return null;\
                 return g.getParameter(37446);}",
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("webgl renderer 返回 None"))?;
        let renderer = v.as_str().unwrap_or("");
        assert!(
            renderer.contains("RTX 3080"),
            "WebGL renderer 应含 'RTX 3080', 实际: {v}"
        );

        // 6. video codec —— headless 默认缺失，patch 后应返回 'probably'
        let v = body
            .call_js_fn(
                "function(){return document.createElement('video').canPlayType('video/mp4; codecs=\"avc1.42E01E\"');}",
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("canPlayType 返回 None"))?;
        assert_eq!(v, json!("probably"), "avc1 canPlayType 应为 probably, 实际: {v}");

        // 7. userAgentData brands —— 非 native 模式应被 patch
        let v = body
            .call_js_fn(
                "function(){return (navigator.userAgentData && navigator.userAgentData.brands) ? \
                 navigator.userAgentData.brands.map(function(b){return b.brand}).join(',') : null;}",
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("brands 返回 None"))?;
        let brands = v.as_str().unwrap_or("");
        assert!(
            brands.contains("Google Chrome") && brands.contains("Chromium"),
            "userAgentData.brands 应含 Google Chrome 和 Chromium, 实际: {v}"
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn chrome_object_present() -> anyhow::Result<()> {
    // window.chrome 的 runtime/csi/loadTimes 是 Turnstile 等必查项。
    //
    // 注意：bootstrap 里 `window.chrome = {...}` 是 window 实例级赋值，只在主世界生效；
    // 隔离世界（evaluate_stealth）看不到它。所以这里用 Element::call_js_fn 在
    // 主世界执行（它走 Runtime.callFunctionOn 带 objectId，在主世界跑，且不需要
    // Runtime.enable）。navigator 的 prototype 级 patch 才在隔离世界可见。
    let profile = ChaserProfile::linux().build();

    with_stealth_profile(&profile, async |chaser| {
        // 拿 body 元素作为主世界执行载体（Element::call_js_fn 在主世界跑，
        // 不需要 Runtime.enable）。bootstrap 里 window.chrome 是实例赋值，
        // 只在主世界可见，所以必须用主世界读，不能用隔离世界的 evaluate。
        let body = chaser.raw_page().find_element("body").await.map_err(|e| anyhow::anyhow!(e))?;

        let v = body
            .call_js_fn("function() { return typeof window.chrome; }", false)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("window.chrome typeof 返回 None"))?;
        assert_eq!(v, json!("object"), "window.chrome 应为 object");

        let v = body
            .call_js_fn(
                "function() { return typeof (window.chrome.runtime && window.chrome.runtime.connect); }",
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("connect typeof 返回 None"))?;
        assert_eq!(v, json!("function"), "chrome.runtime.connect 应为 function");

        let v = body
            .call_js_fn("function() { return typeof window.chrome.csi; }", false)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("csi typeof 返回 None"))?;
        assert_eq!(v, json!("function"), "chrome.csi 应为 function");

        let v = body
            .call_js_fn("function() { return typeof window.chrome.loadTimes; }", false)
            .await
            .map_err(|e| anyhow::anyhow!(e))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("loadTimes typeof 返回 None"))?;
        assert_eq!(v, json!("function"), "chrome.loadTimes 应为 function");

        Ok(())
    })
    .await
}

#[tokio::test]
async fn cdp_markers_removed() -> anyhow::Result<()> {
    // bootstrap 第 0 步会删除 cdc_ / $cdc_ / __webdriver 等 ChromeDriver 痕迹。
    let profile = ChaserProfile::windows().build();

    with_stealth_profile(&profile, async |chaser| {
        let v = eval(
            &chaser,
            "Object.getOwnPropertyNames(window).filter(function(p){\
             return /^cdc_|^\\$cdc_|^__webdriver|^__selenium|^__driver|^\\$chrome_/.test(p);}).length",
        )
        .await?;
        assert_eq!(v, json!(0), "不应残留任何 CDP 自动化标记属性");

        Ok(())
    })
    .await
}

#[tokio::test]
async fn dialog_auto_handled() -> anyhow::Result<()> {
    // 验证 auto_handle_dialogs 能自动接受 alert，页面不卡死。
    // 不注册处理器时 alert 会阻塞页面 JS 执行，后续操作全部挂起。
    let profile = ChaserProfile::windows().build();

    with_stealth_profile(&profile, async |chaser| {
        // 1. 注册自动接受所有 dialog
        chaser.auto_handle_dialogs(true).await?;

        // 2. 在主世界执行 alert —— 不注册处理器时这一步会永久阻塞。
        //    用 call_js_fn（主世界），alert 弹出后由 spawn 的处理器接受，
        //    函数才返回。
        let body = chaser.raw_page().find_element("body").await.map_err(|e| anyhow::anyhow!(e))?;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            body.call_js_fn("function() { alert('test-dialog'); return 'reached-after-alert'; }", false),
        )
        .await;
        // call_js_fn 成功且没超时 = alert 被自动处理了
        let resp = result.map_err(|_| anyhow::anyhow!("alert 阻塞超时——dialog 处理器未生效"))??;
        let v = resp.result.value.ok_or_else(|| anyhow::anyhow!("返回 None"))?;
        assert_eq!(
            v, json!("reached-after-alert"),
            "alert 应被处理、函数应继续执行到 return"
        );

        Ok(())
    })
    .await
}

#[tokio::test]
async fn to_string_returns_native_code() -> anyhow::Result<()> {
    // P1-1 验证：被 patch 的函数 toString() 必须返回 [native code] 风格。
    let profile = ChaserProfile::windows().build();

    with_stealth_profile(&profile, async |chaser| {
        // navigator.webdriver 的 getter（被 patch）的 toString
        let v = eval(
            &chaser,
            "Object.getOwnPropertyDescriptor(Navigator.prototype,'webdriver').get.toString()",
        )
        .await?;
        let s = v.as_str().unwrap_or("");
        assert!(
            s.contains("[native code]"),
            "webdriver getter.toString 应含 [native code], 实际: {s}"
        );

        // WebGL getParameter
        let v = eval(&chaser, "WebGLRenderingContext.prototype.getParameter.toString()").await?;
        let s = v.as_str().unwrap_or("");
        assert!(
            s.contains("[native code]"),
            "getParameter.toString 应含 [native code], 实际: {s}"
        );

        // canPlayType
        let v = eval(&chaser, "HTMLMediaElement.prototype.canPlayType.toString()").await?;
        let s = v.as_str().unwrap_or("");
        assert!(
            s.contains("[native code]"),
            "canPlayType.toString 应含 [native code], 实际: {s}"
        );

        // Function.prototype.toString 本身也要看起来像原生
        let v = eval(&chaser, "Function.prototype.toString.toString()").await?;
        let s = v.as_str().unwrap_or("");
        assert!(
            s.contains("[native code]"),
            "toString.toString 应含 [native code], 实际: {s}"
        );

        // 关键：未 patch 的普通函数应返回真实源码（不能把所有函数都伪装成 native）
        let v = eval(&chaser, "(function foo(){}).toString()").await?;
        let s = v.as_str().unwrap_or("");
        assert!(
            s.contains("foo") && !s.contains("[native code]"),
            "普通函数应返回真实源码而非 native code, 实际: {s}"
        );

        Ok(())
    })
    .await
}
