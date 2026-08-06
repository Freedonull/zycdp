//! 示例：经带认证的 SOCKS5 代理访问外网（用 Socks5Bridge 桥接）。
//!
//! 需开启 feature：`cargo run --example socks5_bridge --features socks5-bridge`
//!
//! 演示如何让不支持 SOCKS5 认证的 Chrome，通过本地 HTTP 转发器间接使用
//! 带用户名/密码的 SOCKS5 代理。

use futures::StreamExt;
use zycdp::{Browser, BrowserConfig, Socks5Bridge};

// 上游 SOCKS5 代理（带认证）——替换成你自己的
const SOCKS5_HOST: &str = "114.80.42.29";
const SOCKS5_PORT: u16 = 18081;
const SOCKS5_USER: &str = "240832d123A";
const SOCKS5_PASS: &str = "190030";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 启动本地桥接器（必须在 Browser::launch 之前）
    let bridge = Socks5Bridge::start(SOCKS5_HOST, SOCKS5_PORT, SOCKS5_USER, SOCKS5_PASS).await?;
    println!(
        "[+] 本地桥接器监听 127.0.0.1:{}，上游 SOCKS5 {}:{}",
        bridge.local_port(),
        SOCKS5_HOST,
        SOCKS5_PORT
    );

    // 2. 用本地代理端口启动 Chrome
    let dir = std::env::temp_dir().join(format!(
        "zycdp-socks5-example-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let cfg = BrowserConfig::builder()
        .new_headless_mode()
        .user_data_dir(&dir)
        .arg(bridge.proxy_arg())
        .build()
        .map_err(|e| anyhow::anyhow!(e))?;

    let (browser, mut handler) = Browser::launch(cfg).await?;
    tokio::spawn(async move { while handler.next().await.is_some() {} });

    println!("\n[+] 经 SOCKS5 桥接访问 http://ip-api.com/json ...");
    let page = browser.new_page("about:blank").await?;

    // 带超时导航（代理不通时不至于永久挂起）
    let nav = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        page.goto("http://ip-api.com/json"),
    )
    .await;

    match nav {
        Ok(Ok(_)) => {
            let html = page.content().await?;
            println!("[+] 页面响应:\n{html}");
            println!("\n[✓] 桥接成功 —— Chrome 经带认证的 SOCKS5 代理访问到了外网");
        }
        Ok(Err(e)) => println!("[!] 导航失败: {e}"),
        Err(_) => println!("[!] 导航超时（代理可能不通或目标站不可达）"),
    }

    drop(browser);
    drop(bridge); // 停止本地转发器
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
