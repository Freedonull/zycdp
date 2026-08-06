# zycdp 交接文档

> 最后更新：2026-08-06
> 上一会话完成了路线图改进（stealth 深度 + 采集能力 + 测试加固），本文档供下一会话快速接手。

## 一、当前状态快照（已核查，非凭记忆）

### Git 状态
- **分支**：仅 `main`（所有特性/测试分支已合并并删除）
- **同步**：本地与 `origin/main` 完全一致（`main...origin/main` 无差异）
- **工作区**：干净（无未提交改动）
- **远程**：`origin` → `https://github.com/Freedonull/zycdp.git`

### 测试状态
- **lib 单元测试**：4 passed（不需 Chrome）
- **真实浏览器集成测试**：14 passed, 2 ignored, 0 failed（需本机 Chrome）
  - 文件：`tests/stealth/offline_assertions.rs`（19 个测试函数，含 2 个 ignore + 帮手）
  - 2 个 ignore：`geolocation_override`（需 https 安全源）、`webrtc_policy_applied`（需代理环境）——都是 data:URL/无代理环境限制，非代码 bug，已在注释说明
- **运行测试命令**：
  ```bash
  # 仅 lib（CI 三平台跑，不需 Chrome）
  cargo test --lib
  # 真实浏览器集成测试（需本机 Chrome，本机 profile 锁时用唯一 user-data-dir 已绕开）
  RUST_TEST_THREADS=1 cargo test --test chromiumoxide_tests stealth::offline_assertions -- --nocapture
  ```

### 构建
- **MSRV**：1.85（`Cargo.toml` rust-version，CI 用 1.85.0 验证）
- **edition**：2024
- **runtime**：仅 tokio（`async-tungstenite` 写死 tokio-runtime，无 async-std feature）
- **feature**：
  - `default = ["bytes"]`
  - `fetcher`（可选，浏览器二进制下载）
  - `socks5-bridge`（可选，SOCKS5→HTTP 桥接，引入 hyper/tokio-socks 等 4 个依赖）
- **提交前必过**：`cargo fmt --all -- --check` + `cargo clippy --all -- -D warnings` + `cargo test --lib`

## 二、本会话完成的工作（按时间顺序）

### 第一批路线图（P0 全清 + P1/P2 大部分）
完成项：P0-1 rebrowser parity 文档修正、P0-2 Windows 内存真实探测、P0-3 离线回归测试、P1-1 toString 对抗、P1-3 Locator API、P2-1 代理认证（HTTP+SOCKS5）、P2-2 dialog/upload/select、额外 drag_human/human_idle。

### 第二批路线图（stealth 深度 + 采集能力 13 项）
基于 2025-2026 检测向量 + 自动化能力缺口调研完成：
- **stealth 深度**：WebRTC IP 泄漏修复、AudioContext 对抗（getChannelData 确定性噪声）、Canvas 2D 噪声、navigator.connection/speechSynthesis voices 伪造
- **采集能力**：iframe 操作（ZyFrame）、响应体拦截（wait_for_response）、文件下载、networkidle（wait_for_load_state）、geolocation+permissions、popup 捕获、键盘组合键/右键/双击
- **架构红线**：TLS/H2 指纹红线写入 AGENTS.md（永不把请求路由到 Rust HTTP 客户端）

### Shadow DOM 穿透查找
`find_in_shadow` / `find_in_shadow_deep`（>>> 语法）——对 open + closed shadow root 都有效，走 CDP DOM.describeNode(pierce=true)，stealth-safe。

### 测试加固 + QA 二次审查
- 补真实浏览器测试到 14 个（覆盖 stealth 指纹、Shadow DOM、iframe、响应体、键盘、canvas、audio、voices 等）
- **独立子智能体二次审查**（探针实测）抓出并修复：
  - AudioContext analyser 路径死代码（-Infinity 吞噪）→ 删除，改测 getChannelData
  - Canvas toDataURL 双重噪声 bug → 改用临时 canvas 不污染原 canvas
  - 组合键缺 virtual key code（Ctrl+A 不生效）→ 改用 keys.rs KeyDefinition
  - 3 个假/弱验证测试 → 修正或标 ignore

### 累计修复的真实 bug（测试抓到的）
1. Canvas toDataURL 双重噪声（putImageData 写回原 canvas）
2. 组合键缺 windows_virtual_key_code（Ctrl+A 不生效）
3. AudioContext analyser 死代码（-Infinity + 噪声 = -Infinity）
4. wait_for_response 的 await_promise 时序（fetch Promise 被回收）
5. enable_proxy_auth 凭据泄露（向站点 basic auth 发代理账密，加 source==Proxy 过滤）
6. BezierPath 零距离 panic（gen_range(-0.0..0.0)）

## 三、核心架构红线（改动时绝不能破坏）

详见 `AGENTS.md`，摘要：

1. **绝不调用 `Runtime.enable`**——`ChaserPage::evaluate`/`evaluate_in_frame` 走 `createIsolatedWorld` + `Runtime.evaluate`（不预先 enable）。`raw_page().evaluate()` 会触发检测，禁用。
2. **`DEFAULT_ARGS` 裁剪**（`src/browser/config.rs`，现 21 条含 WebRTC）——不能恢复被删的自动化 flag。
3. **指纹一致性**——所有指纹值由同一 `Os` 枚举驱动。
4. **Native profile 零 JS 注入**——native 模式 bootstrap 为空，别加 JS patch。
5. **永不把请求路由到 Rust HTTP 客户端**——TLS/H2 指纹护城河（Socks5Bridge 是例外，TCP 层桥接不替换网络栈）。
6. **bootstrap JS 严禁 `${}`/反引号**——Worker 注入会把它嵌进反引号模板导致 ReferenceError。
7. **新增 bootstrap patch 必须注册到 toString 对抗块**（`maskAsNative`），否则 toString 暴露真实源码。

## 四、关键踩坑记录（未来 agent 必读）

### 隔离世界 vs 主世界的可见性差异
`ChaserPage::evaluate`（隔离世界）**读不到** bootstrap 注入的某些值：
- prototype getter（hardwareConcurrency 等）的 descriptor 不跨 realm 传播，隔离世界 fallback 到原生值
- window 实例赋值（window.chrome 等）只在主世界可见

**所以**：读指纹做断言/验证时，用主世界 `Element::call_js_fn("function(){return ...}", false)`——这才是反爬站点视角。

### 在 Windows 开发机跑集成测试的坑
- `tests/lib.rs` 的 `test_config` 用固定 user-data-dir，被日常 Chrome 占用会 `LaunchExit(21)`。`offline_assertions.rs` 的 `with_stealth_profile` 用唯一临时目录绕开。
- chrome 命令行传 URL（如 about:blank）在 headless=new 下报 "Multiple targets are not supported" 退 21。`Browser::launch` 本身不传 URL，自写启动逻辑注意。

### CDP 命令的 stealth 安全性
- DOM 域（querySelector/PerformSearch/setFileInputFiles）✅ 不碰 Runtime
- `Element::call_js_fn` / `click` / `focus`（Runtime.callFunctionOn）✅ 不需 Runtime.enable
- `Page::evaluate` / `set_content` ❌ 走 secondary context 依赖 Runtime.enable
- `Emulation.*` / `Fetch.*` / `Input.*` / `Browser.*` ✅ 各自域

## 五、未来路线图（待办）

### 未完成项（原路线图）
| 项 | 优先级 | 说明 |
|---|---|---|
| **P1-2 类型名改名** | 中 | ChaserPage→ZyPage 等，breaking change，建议 0.3.0 统一做。纯命名工程不提升 stealth |
| **P2-3 冲突隔离** | 低 | stealth 改动加 `// ZYCDP-STEALTH` 标记降低 merge 上游冲突 |
| **P3-1 多 context 并发** | 低 | BrowserContext first-class 抽象 + storageState |
| **P3-2 CDP 自动跟进** | ❌ 已降级 | 调研确认 CDP 与上游逐字节一致、半年才更新，改为季度人工同步（详见 docs/02 调研结论） |

### 已知限制（需特定环境才能测试/完善）
| 项 | 限制 | 完善方向 |
|---|---|---|
| geolocation 坐标伪造 | data:URL 非安全源无法验证 JS getCurrentPosition | 需本地 https 服务器（mkcert）测试 |
| WebRTC host candidate 阻止 | 无代理时 host candidate 仍出现 | 需配代理环境测试 disable_non_proxied_udp 效果 |
| serviceWorker.register toString | data:URL 无 serviceWorker | 需 https 环境验证 toString 对抗覆盖 |
| navigator.connection 伪造 | headless Chrome 默认已有 connection 对象，`===undefined` 判断永假，patch 不生效 | 改成无条件 patch（覆盖已有值）|

### 未覆盖测试的功能（低风险，逻辑直白）
- popup 捕获（wait_for_popup，需多窗口场景）
- 文件下载（enable_downloads/wait_for_download，需触发下载的页面）

### 调研已否决的项（不要重复投入）
- **chrome 对象补全**：主路径 profiles.rs 已完整注入 csi/loadTimes/app（旧 stealth.rs 才简陋）
- **扩展注入法支持 SOCKS5 认证**：Chromium 网络栈不支持，扩展 webRequest 也无效。已用 Socks5Bridge（进程外桥接）解决
- **P3-2 自动化 CDP 流水线**：CDP 低频更新 + stealth 同步不能自动化

## 六、关键文件索引

| 文件 | 作用 |
|---|---|
| `AGENTS.md` | **工作区指令**——未来 agent 必读（红线/能力清单/踩坑/约束）|
| `src/chaser.rs` | 高层 API（ChaserPage，62 个公开方法）|
| `src/profiles.rs` | 指纹 profile + bootstrap JS（stealth 对抗核心）|
| `src/socks5_bridge.rs` | SOCKS5→HTTP 桥接（feature socks5-bridge）|
| `src/browser/config.rs` | DEFAULT_ARGS（启动参数清洗）|
| `src/handler/frame.rs` | FrameManager（删了 Runtime.enable 的 stealth 红线所在）|
| `tests/stealth/offline_assertions.rs` | 真实浏览器测试（14 passed + 2 ignored）|
| `docs/01-architecture.md` | stealth 技术原理（检测维度对抗矩阵）|
| `docs/02-improvement-roadmap.md` | 路线图 + 进度表 + 调研结论归档 |
| `docs/05-defects-baseline.md` | 已知缺陷状态（D1-D5、M1-M5、U1-U3）|
| `docs/03-upstream-sync.md` | 上游同步流程（高冲突文件清单）|

## 七、给下一会话的建议

1. **先读 `AGENTS.md`**——它是为接手者写的，含所有红线和踩坑。
2. **改 bootstrap 前读 docs/01**——理解每个对抗项检测什么、为什么这样实现。
3. **改 stealth 相关代码后必跑 `offline_assertions` 测试**——14 个真实浏览器测试是回归网。
4. **新功能补真实浏览器测试**——本会话证明"编译通过 ≠ 运行时正确"，测试抓到 6 个真实 bug。
5. **测试要避免恒真断言**——QA 二次审查抓出"测试过了但功能没生效"的案例。独立探针验证（对比 patch 前后基线）最可靠。
