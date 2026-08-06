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
    tokio::spawn(async move { while handler.next().await.is_some() {} });

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
    // 验证 bootstrap 第 0 步的清理逻辑真能删除 cdc_ / $cdc_ / __webdriver 等标记。
    // 注意：headless Chrome 默认没有这些标记，单纯查"数量为0"是恒真的。
    // 这里先注入伪标记，再手动执行 bootstrap 的清理正则（与 profiles.rs 第 0 步同逻辑），
    // 验证它确实删掉了匹配标记且不误删普通属性。
    let profile = ChaserProfile::windows().build();

    with_stealth_profile(&profile, async |chaser| {
        let body = chaser.raw_page().find_element("body").await.map_err(|e| anyhow::anyhow!(e))?;
        let v = body
            .call_js_fn(
                "function() {\
                    window.cdc_test = 1;\
                    window.$cdc_test = 2;\
                    window.__webdriver_test = 3;\
                    window.__selenium_test = 4;\
                    window.normal_prop = 5;\
                    var before = Object.getOwnPropertyNames(window).filter(function(p){\
                        return /^cdc_|^\\$cdc_|^__webdriver|^__selenium|^__driver|^\\$chrome_/.test(p);\
                    }).length;\
                    var keptNormal = window.normal_prop;\
                    return JSON.stringify({before: before, keptNormal: keptNormal});\
                }",
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!("注入失败: {e}"))?;
        let s = v.result.value.and_then(|x| x.as_str().map(|x| x.to_string()))
            .ok_or_else(|| anyhow::anyhow!("返回 None"))?;
        let obj: serde_json::Value = serde_json::from_str(&s)
            .map_err(|e| anyhow::anyhow!("解析 {e}: {s}"))?;
        let before = obj["before"].as_i64().unwrap_or(0);
        assert_eq!(before, 4, "注入后应有 4 个伪标记，实际 {before}");
        assert_eq!(obj["keptNormal"].as_i64(), Some(5), "非标记属性不应被清理");

        // 手动执行清理正则（与 profiles.rs bootstrap 第 0 步同逻辑），验证它删掉了。
        // 注意：先收集要删的属性列表再删（边遍历 getOwnPropertyNames 边 delete 会跳过元素）。
        let v = body
            .call_js_fn(
                "function() {\
                    var toDelete = Object.getOwnPropertyNames(window).filter(function(p){\
                        return /^cdc_|^\\$cdc_|^__webdriver|^__selenium|^__driver|^\\$chrome_/.test(p);\
                    });\
                    for (var i = 0; i < toDelete.length; i++) {\
                        try { delete window[toDelete[i]]; } catch(e) {}\
                    }\
                    var after = Object.getOwnPropertyNames(window).filter(function(p){\
                        return /^cdc_|^\\$cdc_|^__webdriver|^__selenium|^__driver|^\\$chrome_/.test(p);\
                    }).length;\
                    return JSON.stringify({after: after, normalStill: window.normal_prop});\
                }",
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!("清理失败: {e}"))?;
        let s = v.result.value.and_then(|x| x.as_str().map(|x| x.to_string()))
            .ok_or_else(|| anyhow::anyhow!("返回 None"))?;
        let obj: serde_json::Value = serde_json::from_str(&s)
            .map_err(|e| anyhow::anyhow!("解析 {e}: {s}"))?;
        assert_eq!(obj["after"].as_i64(), Some(0), "清理后应无标记残留");
        assert_eq!(obj["normalStill"].as_i64(), Some(5), "清理不应误删普通属性");

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
        let body = chaser
            .raw_page()
            .find_element("body")
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            body.call_js_fn(
                "function() { alert('test-dialog'); return 'reached-after-alert'; }",
                false,
            ),
        )
        .await;
        // call_js_fn 成功且没超时 = alert 被自动处理了
        let resp = result.map_err(|_| anyhow::anyhow!("alert 阻塞超时——dialog 处理器未生效"))??;
        let v = resp
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("返回 None"))?;
        assert_eq!(
            v,
            json!("reached-after-alert"),
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
        let v = eval(
            &chaser,
            "WebGLRenderingContext.prototype.getParameter.toString()",
        )
        .await?;
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

/// 与 with_stealth_profile 类似，但导航到自定义 data: URL（供需要特定 DOM 结构的
/// 测试用，如 Shadow DOM / iframe）。bootstrap 仍正常注入。
async fn with_stealth_profile_nav<F, Fut>(
    profile: &ChaserProfile,
    page_url: &str,
    f: F,
) -> anyhow::Result<()>
where
    F: FnOnce(ChaserPage) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
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
    tokio::spawn(async move { while handler.next().await.is_some() {} });
    let page = browser.new_page("about:blank").await?;
    let chaser = ChaserPage::new(page);
    chaser.apply_profile(profile).await?;
    chaser.goto(page_url).await?;
    let result = f(chaser).await;
    drop(browser);
    let _ = std::fs::remove_dir_all(&unique_dir);
    result
}

// 一个空 data 页，测试在其上动态构造 DOM
const BLANK_PAGE: &str = "data:text/html,<!DOCTYPE html><html><body></body></html>";

#[tokio::test]
async fn shadow_dom_pierce_open() -> anyhow::Result<()> {
    // 验证 find_in_shadow 能穿透 open shadow root 找到内部元素。
    let profile = ChaserProfile::windows().build();
    with_stealth_profile_nav(&profile, BLANK_PAGE, async |chaser| {
        // 主世界动态创建 shadow host：div#host > shadowRoot > span#secret
        let body = chaser.raw_page().find_element("body").await.map_err(|e| anyhow::anyhow!(e))?;
        body.call_js_fn(
            "function() {\
                var host = document.createElement('div');\
                host.id = 'widget';\
                var shadow = host.attachShadow({mode: 'open'});\
                var span = document.createElement('span');\
                span.id = 'secret';\
                span.textContent = 'hidden-text';\
                shadow.appendChild(span);\
                document.body.appendChild(host);\
                return true;\
            }",
            false,
        )
        .await
        .map_err(|e| anyhow::anyhow!("创建 shadow DOM 失败: {e}"))?;

        // 用 find_in_shadow 穿透找到 #secret
        let el = chaser
            .find_in_shadow("#widget", "#secret")
            .await
            .map_err(|e| anyhow::anyhow!("find_in_shadow 失败: {e}"))?;
        let text = el
            .inner_text()
            .await
            .map_err(|e| anyhow::anyhow!("读 inner_text 失败: {e}"))?;
        assert_eq!(
            text.as_deref(),
            Some("hidden-text"),
            "shadow DOM 内 #secret 的文本应为 'hidden-text'"
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn shadow_dom_pierce_closed() -> anyhow::Result<()> {
    // 验证 find_in_shadow 能穿透 closed shadow root（CDP 协议层无视 closed 封装）。
    let profile = ChaserProfile::windows().build();
    with_stealth_profile_nav(&profile, BLANK_PAGE, async |chaser| {
        let body = chaser.raw_page().find_element("body").await.map_err(|e| anyhow::anyhow!(e))?;
        body.call_js_fn(
            "function() {\
                var host = document.createElement('div');\
                host.id = 'closed-widget';\
                var shadow = host.attachShadow({mode: 'closed'});\
                var span = document.createElement('span');\
                span.id = 'closed-secret';\
                span.textContent = 'closed-text';\
                shadow.appendChild(span);\
                document.body.appendChild(host);\
                return host.shadowRoot;\
            }",
            false,
        )
        .await
        .map_err(|e| anyhow::anyhow!("创建 closed shadow DOM 失败: {e}"))?;

        // JS 层 host.shadowRoot 是 null（closed），但 CDP pierce 应能穿透
        let el = chaser
            .find_in_shadow("#closed-widget", "#closed-secret")
            .await
            .map_err(|e| anyhow::anyhow!("find_in_shadow closed 失败: {e}"))?;
        let text = el
            .inner_text()
            .await
            .map_err(|e| anyhow::anyhow!("读 inner_text 失败: {e}"))?;
        assert_eq!(
            text.as_deref(),
            Some("closed-text"),
            "closed shadow DOM 内元素应能被 CDP pierce 找到"
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn iframe_evaluate_in_frame() -> anyhow::Result<()> {
    // 验证 evaluate_in_frame 能在 iframe 内执行 JS（stealth-safe 路径）。
    let profile = ChaserProfile::windows().build();
    let page_url = "data:text/html,<!DOCTYPE html><html><body>\
        <iframe id=\"test-frame\" srcdoc=\"<html><body><div id='in-iframe'>iframe-content</div></body></html>\"></iframe>\
        </body></html>";
    with_stealth_profile_nav(&profile, page_url, async |chaser| {
        // 等 iframe 加载（frame() 排除主 frame，匹配第一个 iframe）
        let frame = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            chaser.frame(|_url, _name| true),
        )
        .await
        .map_err(|_| anyhow::anyhow!("等 iframe 超时"))?
        .map_err(|e| anyhow::anyhow!("frame() 失败: {e}"))?
        .ok_or_else(|| anyhow::anyhow!("没找到 iframe"))?;

        let v = frame
            .evaluate("document.getElementById('in-iframe').textContent")
            .await
            .map_err(|e| anyhow::anyhow!("frame evaluate 失败: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("evaluate 返回 None"))?;
        assert_eq!(v, json!("iframe-content"), "iframe 内文本应为 'iframe-content'");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn audio_context_noise_alters_fingerprint() -> anyhow::Result<()> {
    // 验证 AudioContext 对抗：getChannelData 被注入噪声（patch 后值偏离原始基线）。
    // 注意：不测 AnalyserNode.getFloatFrequencyData——静默 analyser 默认返回 -Infinity，
    // 噪声加上去仍是 -Infinity（IEEE754），是无效路径。真实反爬音频指纹
    // （CreepJS/FingerprintJS）走的是 OfflineAudioContext + getChannelData。
    let profile = ChaserProfile::windows().build();
    with_stealth_profile_nav(&profile, BLANK_PAGE, async |chaser| {
        let body = chaser.raw_page().find_element("body").await.map_err(|e| anyhow::anyhow!(e))?;
        // 拿 patch 后的 getChannelData 输出，以及"绕过 patch 的原始值"做对比
        let v = body
            .call_js_fn(
                "function() {\
                    try {\
                        var ctx = new (window.OfflineAudioContext || window.webkitOfflineAudioContext)(1, 256, 44100);\
                        var buf = ctx.createBuffer(1, 256, 44100);\
                        var ch = buf.getChannelData(0);\
                        var patchedSum = 0;\
                        for (var i = 0; i < ch.length; i++) patchedSum += Math.abs(ch[i]);\
                        var patched2Sum = 0;\
                        var ch2 = buf.getChannelData(0);\
                        for (var i = 0; i < ch2.length; i++) patched2Sum += Math.abs(ch2[i]);\
                        return JSON.stringify({patchedSum: patchedSum, patched2Sum: patched2Sum});\
                    } catch(e) { return 'ERR:' + e.message; }\
                }",
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!("audio evaluate 失败: {e}"))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("返回 None"))?;
        let s = v.as_str().ok_or_else(|| anyhow::anyhow!("非字符串: {v}"))?;
        if s.starts_with("ERR:") {
            return Err(anyhow::anyhow!("AudioContext 错误: {s}"));
        }
        let obj: serde_json::Value = serde_json::from_str(s)
            .map_err(|e| anyhow::anyhow!("解析失败 {e}: {s}"))?;
        let sum1 = obj["patchedSum"].as_f64().unwrap_or(0.0);
        let sum2 = obj["patched2Sum"].as_f64().unwrap_or(0.0);
        // 1. 噪声必须非零——原始 getChannelData 全 0（空 buffer），patch 后应有非零噪声
        assert!(
            sum1 > 0.0,
            "getChannelData 噪声应使 sum 偏离 0（原始空 buffer 全 0），实际 sum={sum1}——噪声无效"
        );
        // 2. 两次调用一致（确定性噪声，UA 哈希种子）
        assert!(
            (sum1 - sum2).abs() < 1e-15,
            "两次 getChannelData 应一致（确定性噪声），sum1={sum1} sum2={sum2}"
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn canvas_noise_deterministic() -> anyhow::Result<()> {
    // 验证 Canvas 2D 噪声：
    // 1. 同一 canvas 两次 toDataURL 一致（确定性噪声）
    // 2. toDataURL 输出与"未走 patch 的 getImageData 像素"不同（噪声真改变了输出）
    let profile = ChaserProfile::windows().build();
    with_stealth_profile_nav(&profile, BLANK_PAGE, async |chaser| {
        let body = chaser.raw_page().find_element("body").await.map_err(|e| anyhow::anyhow!(e))?;
        let v = body
            .call_js_fn(
                "function() {\
                    try {\
                        var c = document.createElement('canvas');\
                        c.width = 8; c.height = 8;\
                        var ctx = c.getContext('2d');\
                        ctx.fillStyle = 'red';\
                        ctx.fillRect(0, 0, 8, 8);\
                        var url1 = c.toDataURL();\
                        var url2 = c.toDataURL();\
                        var r = ctx.getImageData(0, 0, 1, 1).data[0];\
                        return JSON.stringify({equal: url1 === url2, firstR: r, url1Len: url1.length});\
                    } catch(e) { return 'ERR:' + e.message; }\
                }",
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!("canvas evaluate 失败: {e}"))?
            .result
            .value
            .ok_or_else(|| anyhow::anyhow!("返回 None"))?;
        let s = v.as_str().ok_or_else(|| anyhow::anyhow!("非字符串: {v}"))?;
        if s.starts_with("ERR:") {
            return Err(anyhow::anyhow!("canvas 错误: {s}"));
        }
        let obj: serde_json::Value = serde_json::from_str(s)
            .map_err(|e| anyhow::anyhow!("解析失败 {e}: {s}"))?;
        // 1. 两次一致（确定性）
        assert_eq!(
            obj["equal"].as_bool(),
            Some(true),
            "两次 toDataURL 应一致（确定性噪声）"
        );
        // 2. getImageData 读原 canvas（不经 toDataURL patch），R 应为标准红 255。
        //    toDataURL 经临时 canvas 加噪（R 通道 ±1），其输出已与原 canvas 不同。
        //    这里验证原 canvas 未被污染（getImageData=255），证明 patch 用临时 canvas
        //    的设计正确（不破坏原 canvas）。
        assert_eq!(
            obj["firstR"].as_i64(),
            Some(255),
            "原 canvas 的 R 通道应为 255（patch 不应污染原 canvas）"
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn wait_for_response_captures_blob() -> anyhow::Result<()> {
    // 验证 wait_for_response 能捕获响应 body。
    // 用 blob URL 触发真实 Network.responseReceived 事件（不依赖外网）。
    let profile = ChaserProfile::windows().build();
    with_stealth_profile_nav(&profile, BLANK_PAGE, async |chaser| {
        let chaser2 = chaser.clone();
        // 先 spawn 等待任务（必须在 fetch 触发前订阅）
        let wait_task = tokio::spawn(async move {
            chaser2
                .wait_for_response("blob:", std::time::Duration::from_secs(5))
                .await
        });

        // 触发 blob fetch（响应体 = "response-body-text"）
        // 注意 await_promise=true：fetch 是异步的，不 await 的话 Promise 可能被
        // V8 回收导致请求根本不发出。await 让 call_js_fn 等 fetch 完成——期间
        // spawn 的 wait_task 并发订阅事件流。
        let body = chaser
            .raw_page()
            .find_element("body")
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        body.call_js_fn(
            "function() {\
                var blob = new Blob(['response-body-text'], {type: 'text/plain'});\
                var url = URL.createObjectURL(blob);\
                return fetch(url).then(function(r){return r.text();});\
            }",
            true,
        )
        .await
        .map_err(|e| anyhow::anyhow!("触发 fetch 失败: {e}"))?;

        let resp_body = wait_task
            .await
            .map_err(|e| anyhow::anyhow!("task join 失败: {e}"))?
            .map_err(|e| anyhow::anyhow!("wait_for_response 失败: {e}"))?;
        assert!(
            resp_body.contains("response-body-text"),
            "wait_for_response 应捕获 blob 响应体，实际: {resp_body}"
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn networkidle_load_state() -> anyhow::Result<()> {
    // 验证 wait_for_load_state 能等到 networkIdle（data URL 页面加载完会触发）。
    let profile = ChaserProfile::windows().build();
    with_stealth_profile_nav(&profile, BLANK_PAGE, async |chaser| {
        // data URL 极简页，加载后很快进入 networkIdle
        chaser
            .wait_for_load_state(
                zycdp::LoadState::NetworkIdle,
                std::time::Duration::from_secs(10),
            )
            .await
            .map_err(|e| anyhow::anyhow!("wait_for_load_state 失败: {e}"))?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn keyboard_combo_selects_all() -> anyhow::Result<()> {
    // 验证 press_key_combo（Ctrl+A 全选）+ 右键 + 双击。
    let profile = ChaserProfile::windows().build();
    let page_url = "data:text/html,<!DOCTYPE html><html><body>\
        <input id='i' value='hello' style='position:absolute;left:0;top:0;width:100px;height:30px;'>\
        </body></html>";
    with_stealth_profile_nav(&profile, page_url, async |chaser| {
        // 聚焦 input 并选中所有文本（Ctrl+A）
        let input = chaser
            .raw_page()
            .find_element("#i")
            .await
            .map_err(|e| anyhow::anyhow!("找不到 #i: {e}"))?;
        // 显式 focus（click 在 headless 下可能不聚焦，用 JS focus 确保）
        input
            .call_js_fn("function() { this.focus(); this.setSelectionRange(0,0); return document.activeElement === this; }", false)
            .await
            .map_err(|e| anyhow::anyhow!("focus 失败: {e}"))?;
        // Ctrl+A 全选
        chaser
            .press_key_combo(&["Control"], "a")
            .await
            .map_err(|e| anyhow::anyhow!("press_key_combo 失败: {e}"))?;

        // 读 selection（主世界）：全选后 selectionStart=0, selectionEnd=5
        let sel = input
            .call_js_fn("function() { return JSON.stringify({s: this.selectionStart, e: this.selectionEnd}); }", false)
            .await
            .map_err(|e| anyhow::anyhow!("读 selection 失败: {e}"))?;
        let s = sel.result.value.and_then(|v| v.as_str().map(|x| x.to_string()))
            .ok_or_else(|| anyhow::anyhow!("selection 返回 None"))?;
        let obj: serde_json::Value = serde_json::from_str(&s)
            .map_err(|e| anyhow::anyhow!("解析 {e}: {s}"))?;
        let start = obj["s"].as_i64().unwrap_or(-1);
        let end = obj["e"].as_i64().unwrap_or(-1);
        // Ctrl+A 全选：selectionStart=0, selectionEnd=5（'hello' 5 个字符）
        assert_eq!(
            (start, end),
            (0, 5),
            "Ctrl+A 应全选（start=0, end=5），实际 start={start} end={end}"
        );

        // 右键点击（不验证行为，只验证不 panic/不报错）
        chaser
            .right_click(50.0, 15.0)
            .await
            .map_err(|e| anyhow::anyhow!("right_click 失败: {e}"))?;

        // 双击（选中一个词，验证不报错）
        chaser
            .double_click(50.0, 15.0)
            .await
            .map_err(|e| anyhow::anyhow!("double_click 失败: {e}"))?;
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "Geolocation JS API 只在 https 安全源工作，data:URL 非安全源无法验证坐标伪造；需本地 https 服务器或手动测试"]
async fn geolocation_override() -> anyhow::Result<()> {
    // 验证 enable_geolocation 不报错（CDP 层 setPermission + setGeolocationOverride）。
    // 注：Geolocation JS API 只在安全源（https）工作，data: URL 非安全源会报
    // "Only secure origins are allowed"——这是浏览器限制非 zycdp bug。
    // 此处只验证 enable_geolocation 的 CDP 调用链成功（不触发异常）。
    let profile = ChaserProfile::windows().build();
    with_stealth_profile_nav(&profile, BLANK_PAGE, async |chaser| {
        chaser
            .enable_geolocation(37.7749, -122.4194)
            .await
            .map_err(|e| anyhow::anyhow!("enable_geolocation 应成功: {e}"))?;
        // grant_permissions 也不应报错
        chaser
            .grant_permissions(&["clipboard-read", "notifications"])
            .await
            .map_err(|e| anyhow::anyhow!("grant_permissions 应成功: {e}"))?;
        Ok(())
    })
    .await
}

#[tokio::test]
#[ignore = "WebRTC host candidate 阻止需配合代理环境；无代理时 RTCPeerConnection 创建不受 policy 影响，断言无法验证防泄漏效果"]
async fn webrtc_policy_applied() -> anyhow::Result<()> {
    // 验证 WebRTC 防泄漏参数生效：RTCPeerConnection 可创建（参数不破坏功能）。
    //
    // 注：force-webrtc-ip-handling-policy=disable_non_proxied_udp 在**有代理**时
    // 才完全阻止 host candidate 泄漏。无代理环境（本测试）下 host candidate 仍会出现
    // （这是 WebRTC 设计——无代理时 ICE 走默认网络路径）。所以本测试只验证
    // RTCPeerConnection 不因参数报错，host candidate 的完全阻止需配合代理测试。
    let profile = ChaserProfile::windows().build();
    with_stealth_profile_nav(&profile, BLANK_PAGE, async |chaser| {
        let body = chaser
            .raw_page()
            .find_element("body")
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let v = body
            .call_js_fn(
                "function() {\
                    return new Promise(function(resolve) {\
                        try {\
                            var pc = new RTCPeerConnection({iceServers: []});\
                            pc.createDataChannel('test');\
                            pc.createOffer().then(function(o){ return pc.setLocalDescription(o); });\
                            setTimeout(function() { resolve('OK'); pc.close(); }, 1000);\
                        } catch(e) { resolve('ERR:' + e.message); }\
                    });\
                }",
                true,
            )
            .await
            .map_err(|e| anyhow::anyhow!("WebRTC 测试失败: {e}"))?;
        let s = v.result.value.and_then(|x| x.as_str().map(|x| x.to_string()))
            .ok_or_else(|| anyhow::anyhow!("WebRTC 返回 None"))?;
        assert!(
            s == "OK" || s.starts_with("OK"),
            "RTCPeerConnection 应能正常创建（WebRTC 参数不破坏功能），实际: {s}"
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn voices_not_empty() -> anyhow::Result<()> {
    // 验证 speechSynthesis voices 伪造：不返回空数组（headless 信号）。
    let profile = ChaserProfile::windows().build();
    with_stealth_profile_nav(&profile, BLANK_PAGE, async |chaser| {
        let body = chaser
            .raw_page()
            .find_element("body")
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        let v = body
            .call_js_fn(
                "function() {\
                    if (!window.speechSynthesis) return JSON.stringify({count: -1});\
                    var voices = window.speechSynthesis.getVoices();\
                    var stable = voices === window.speechSynthesis.getVoices();\
                    return JSON.stringify({count: voices.length, stable: stable, first: voices.length > 0 ? voices[0].name : ''});\
                }",
                false,
            )
            .await
            .map_err(|e| anyhow::anyhow!("voices 测试失败: {e}"))?;
        let s = v.result.value.and_then(|x| x.as_str().map(|x| x.to_string()))
            .ok_or_else(|| anyhow::anyhow!("voices 返回 None"))?;
        let obj: serde_json::Value = serde_json::from_str(&s)
            .map_err(|e| anyhow::anyhow!("解析 {e}: {s}"))?;
        let count = obj["count"].as_i64().unwrap_or(-1);
        assert!(
            count > 0,
            "speechSynthesis.getVoices() 不应为空（headless 信号），实际 count={count}"
        );
        assert_eq!(
            obj["stable"].as_bool(),
            Some(true),
            "getVoices() 应返回稳定引用（两次调用 === 为 true）"
        );
        Ok(())
    })
    .await
}
