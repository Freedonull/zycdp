//! SOCKS5→HTTP 桥接器（feature `socks5-bridge`）。
//!
//! Chrome 不支持 SOCKS5 代理的用户名/密码认证（Chromium 网络栈的架构性缺失，
//! 非扩展/CDP 能修复）。本模块在本地起一个 HTTP CONNECT 转发器，代为完成到上游
//! SOCKS5（带认证）的握手，让 Chrome 经无认证的本地 HTTP 代理间接使用带认证的 SOCKS5。
//!
//! # 工作原理
//!
//! ```text
//! Chrome ──CONNECT host:port──▶ 本地 HTTP 转发器（127.0.0.1:自动端口）
//!                                   │
//!                                   │ 1. SOCKS5 握手 + 用户名/密码认证（RFC 1929）
//!                                   │ 2. SOCKS5 CONNECT 目标
//!                                   ▼
//!                              上游 SOCKS5（host:port, user:pass）
//!                                   │
//!                              3. 回 Chrome "200 Connection Established"
//!                              4. 双向透明转发 TCP 字节流
//! ```
//!
//! # 用法
//!
//! 转发器必须在 [`crate::browser::Browser::launch`] **之前**启动，因为
//! `--proxy-server` 是 Chrome 启动参数，浏览器进程级配置，启动后无法修改。
//!
//! ```rust,ignore
//! use zycdp::Socks5Bridge;
//!
//! // 1. 启动桥接器（自动分配本地端口）
//! let bridge = Socks5Bridge::start("114.80.42.29", 18081, "user", "pass").await?;
//!
//! // 2. 把本地代理传给 Chrome
//! let cfg = zycdp::BrowserConfig::builder()
//!     .arg(bridge.proxy_arg())
//!     // ... 其他配置
//!     .build()?;
//! let (browser, mut handler) = zycdp::Browser::launch(cfg).await?;
//! // handler 循环 ...
//!
//! // 3. 正常使用，所有流量自动经 SOCKS5 出去
//! let page = browser.new_page("https://example.com").await?;
//! ```

use std::convert::Infallible;
use std::net::Ipv4Addr;
use std::sync::Arc;

use http_body_util::Empty;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

/// 上游 SOCKS5 代理的连接配置。
#[derive(Debug, Clone)]
struct UpstreamConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
}

/// SOCKS5→HTTP 桥接器。Drop 时自动停止本地转发器。
///
/// 构造方式见 [`Socks5Bridge::start`]。
pub struct Socks5Bridge {
    local_port: u16,
    /// 持有 shutdown 信号；drop 时触发转发器退出。
    shutdown: Arc<tokio::sync::Notify>,
}

impl std::fmt::Debug for Socks5Bridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Socks5Bridge")
            .field("local_port", &self.local_port)
            .finish()
    }
}

impl Socks5Bridge {
    /// 启动本地 HTTP CONNECT 转发器，桥接到指定上游 SOCKS5（带用户名/密码认证）。
    ///
    /// 自动分配本地空闲端口（监听 127.0.0.1）。返回后用 [`proxy_arg`] 拿到
    /// `--proxy-server=...` 字符串传给 [`crate::browser::BrowserConfig`]。
    ///
    /// `host` 是上游 SOCKS5 的地址（域名或 IP），`port` 是端口，
    /// `username`/`password` 是 RFC 1929 认证凭据。
    pub async fn start(
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> std::io::Result<Self> {
        let upstream = Arc::new(UpstreamConfig {
            host: host.into(),
            port,
            username: username.into(),
            password: password.into(),
        });

        // 绑定 127.0.0.1:0 让 OS 分配空闲端口
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let local_port = listener.local_addr()?.port();

        let shutdown = Arc::new(tokio::sync::Notify::new());
        let shutdown_clone = Arc::clone(&shutdown);

        tokio::spawn(async move {
            serve_loop(listener, upstream, shutdown_clone).await;
        });

        Ok(Self {
            local_port,
            shutdown,
        })
    }

    /// 本地转发器监听端口。
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// 返回传给 Chrome 的 `--proxy-server` 参数字符串。
    ///
    /// 形如 `--proxy-server=http://127.0.0.1:12345`，直接喂给
    /// [`crate::browser::BrowserConfig::arg`]。
    pub fn proxy_arg(&self) -> String {
        format!("--proxy-server=http://127.0.0.1:{}", self.local_port)
    }
}

impl Drop for Socks5Bridge {
    fn drop(&mut self) {
        // 通知转发器主循环退出。已 accept 的连接会随各自 task 结束自然关闭。
        self.shutdown.notify_waiters();
    }
}

/// 转发器主循环：接受连接，每个 spawn 一个 CONNECT 处理器。
async fn serve_loop(
    listener: TcpListener,
    upstream: Arc<UpstreamConfig>,
    shutdown: Arc<tokio::sync::Notify>,
) {
    loop {
        tokio::select! {
            // shutdown 信号到达即退出
            _ = shutdown.notified() => return,
            accept = listener.accept() => {
                let (tcp, _peer) = match accept {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::debug!("socks5-bridge accept 错误: {e}");
                        continue;
                    }
                };
                let upstream = Arc::clone(&upstream);
                let io = TokioIo::new(tcp);
                tokio::spawn(async move {
                    let svc = service_fn(move |req: Request<Incoming>| {
                        let upstream = Arc::clone(&upstream);
                        async move {
                            let resp: Result<Response<Empty<Bytes>>, Infallible> =
                                if req.method() == hyper::Method::CONNECT {
                                    Ok(handle_connect(req, &upstream).await)
                                } else {
                                    // 转发器只做 CONNECT 隧道，拒绝普通 HTTP 代理请求
                                    Ok(Response::builder()
                                        .status(StatusCode::METHOD_NOT_ALLOWED)
                                        .body(Empty::new())
                                        .unwrap())
                                };
                            resp
                        }
                    });
                    // with_upgrades 必须调，否则 CONNECT 拿不到升级后的隧道连接
                    if let Err(e) = http1::Builder::new()
                        .serve_connection(io, svc)
                        .with_upgrades()
                        .await
                    {
                        tracing::debug!("socks5-bridge 连接处理错误: {e}");
                    }
                });
            }
        }
    }
}

/// 处理一个 CONNECT 请求：连上游 SOCKS5（带认证）→ 回 200 → 双向转发。
///
/// 无论上游连接成功还是失败都返回一个 Response（失败回 502），便于上层统一处理。
async fn handle_connect(req: Request<Incoming>, upstream: &UpstreamConfig) -> Response<Empty<Bytes>> {
    // CONNECT 的 authority 形如 "example.com:443"
    let host_port = match req.uri().authority().map(|a| a.as_str().to_string()) {
        Some(h) => h,
        None => {
            return Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Empty::new())
                .unwrap();
        }
    };

    // 1. 连上游 SOCKS5 + 完成认证 + CONNECT 目标。
    //    关键：先握手成功再回 200，否则先回 200 后握手失败会让客户端无感知地挂死。
    let upstream_stream =
        tokio_socks::tcp::Socks5Stream::connect_with_password(
            (upstream.host.as_str(), upstream.port),
            host_port.as_str(),
            upstream.username.as_str(),
            upstream.password.as_str(),
        )
        .await;

    let upstream_tcp = match upstream_stream {
        Ok(s) => s.into_inner(),
        Err(e) => {
            tracing::debug!("socks5-bridge SOCKS5 连接 {host_port} 失败: {e}");
            return Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Empty::new())
                .unwrap();
        }
    };

    // 2. 回 200 Connection Established（空 body）
    let resp = Response::builder()
        .status(StatusCode::OK)
        .body(Empty::new())
        .unwrap();

    // 3. 等升级，双向桥接
    tokio::spawn(async move {
        let upgraded = match hyper::upgrade::on(req).await {
            Ok(u) => u,
            Err(e) => {
                tracing::debug!("socks5-bridge upgrade 失败: {e}");
                return;
            }
        };
        // Upgraded 是 hyper 的 IO，包 TokioIo 适配到 tokio AsyncRead/Write。
        // upstream_tcp 是 tokio TcpStream，直接实现 AsyncRead/Write。
        let mut client = TokioIo::new(upgraded);
        let mut server = upstream_tcp;
        if let Err(e) = tokio::io::copy_bidirectional(&mut client, &mut server).await {
            tracing::debug!("socks5-bridge {host_port} 转发错误: {e}");
        }
    });

    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bridge_starts_and_allocates_port() {
        // 仅验证能启动并分配到非零端口；真实代理连通性见 example。
        let bridge = Socks5Bridge::start("127.0.0.1", 1080, "u", "p")
            .await
            .expect("启动桥接器");
        assert!(bridge.local_port() > 0);
        let arg = bridge.proxy_arg();
        assert!(arg.starts_with("--proxy-server=http://127.0.0.1:"));
        // drop 触发 shutdown，不应卡住
        drop(bridge);
    }
}
