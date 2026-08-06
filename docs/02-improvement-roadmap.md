# 02 - 改进路线图

> 按"投入产出比"和"优先级"分级。每项附代码位置、问题、方案、验收标准。
> **改动前必读 [05-defects-baseline.md](./05-defects-baseline.md)，避免回归。**

## 总体方向

zycdp 的核心价值在 **stealth 内核**（`Runtime.enable` 对抗、指纹一致性、行为模拟），不在通用自动化 API。改进资源应优先投入 stealth 深度，其次补自动化易用性。

## P0 - 最高优先级（直接影响 stealth 效果，必须做）

### P0-1：补全 rebrowser parity 的缺失步骤

- **问题**：`evaluate_stealth`（`src/chaser.rs:435`）直接用 `createIsolatedWorld` 返回的 context id，跳过了 rebrowser 官方补丁的"主世界 binding 获取 context"步骤。README 声称 "100% parity" 但实测有差距。
- **为什么重要**：cf 高级检测针对 isolated world 探测，这一步差距在严格检测下会暴露。
- **方案**：参考 [rebrowser-patches 官方实现](https://github.com/rebrowser/rebrowser-patches)：
  1. 在主世界创建一次性 binding
  2. 调用 binding 拿到主世界 context id
  3. `createIsolatedWorld` 拿到隔离 context id
  4. 后续 evaluate 走隔离 context
- **验收**：通过 `tests/stealth/rebrowser.rs` 的 `mainWorldExecution` 测试项（当前可能 fail）。
- **涉及文件**：`src/chaser.rs`、可能新增 `src/handler/` 辅助逻辑。

### P0-2：修复 Windows 内存探测假数据

- **问题**：`src/profiles.rs:802` 的 `_read_system_memory_gb` 在 Windows 分支直接 `return 8`，不是真实探测。
- **为什么重要**：native 模式核心承诺是"用真实值"。Windows 内存是假值 → native 一致性承诺被破坏 → cf 可能据此判异常。
- **方案**：用 `windows` crate（非 `windows-registry`）的 `GlobalMemoryStatusEx` 读真实物理内存。
- **验收**：Windows 上运行 `ChaserProfile::native().build()`，内存值与系统实际 RAM 一致（≤8GB，受 deviceMemory 规范上限约束）。
- **涉及文件**：`src/profiles.rs`、`Cargo.toml`（条件依赖 `windows` feature）。

### P0-3：建立离线 stealth 回归测试

- **问题**：`tests/stealth/rebrowser.rs:18` 标 `#[ignore]`（注释 "flaky"）。没有任何 CI 可跑的测试证明 stealth 有效。
- **为什么重要**：长期 fork，每次 merge 上游后必须验证 stealth 没回归。靠手动跑反爬站点不可持续。
- **方案**：写**断言指纹一致性**的离线测试（不依赖反爬站点）：
  ```rust
  // 注入 bootstrap 后，用 evaluate_stealth 读回值，断言符合预期
  assert_eq!(chaser.evaluate("navigator.webdriver").await?, json!(false));
  assert_eq!(chaser.evaluate("navigator.platform").await?, json!("Win32"));
  assert!(chaser.evaluate("WebGLRenderingContext.prototype.getParameter(37445)").await?.as_str().contains("Google Inc."));
  ```
- **验收**：CI 能跑、不 flaky、覆盖 bootstrap 的所有对抗项。
- **涉及文件**：新增 `tests/stealth/offline_assertions.rs`。

## P1 - 高优先级（提升 stealth 深度 + 修复已知缺陷）

### P1-1：toString() 深度对抗（针对 CreepJS 级检测）

- **问题**：非 native 模式的 bootstrap patch 了函数，被 patch 的函数 `toString()` 不返回 `[native code]`，是真实检测向量。native 模式用"零 JS"绕过，但非 native 模式仍暴露。
- **为什么重要**：cf 深度指纹层、CreepJS 会查这个。
- **方案**：patch `Function.prototype.toString`，让被改函数返回 `[native code]`。参考 [svebaa 的 CDP fingerprinting 分析](https://svebaa.github.io/personal/blog/cdp-fingerprinting/)。
- **验收**：`chaser.evaluate("navigator.webdriver.toString()").await?` 返回 `function() { [native code] }` 风格。
- **涉及文件**：`src/profiles.rs` bootstrap。

### P1-2：补全 API 类型名改名（ChaserPage/ChaserProfile → ZyCdp 命名）

- **问题**：包名已改为 zycdp，但公开类型名 `ChaserPage`/`ChaserProfile`/`ChaserPage` 仍带 Chaser。
- **为什么重要**：① API 一致性；② 彻底切断与 chaser 商标关联。
- **方案**：`ChaserPage → ZyPage`、`ChaserProfile → ZyProfile`、`src/chaser.rs → src/page_stealth.rs`。需同步改所有引用 + examples + README。
- **注意**：这是 **breaking change**，建议在 0.3.0 版本统一做，保留 `pub use 旧名` 做 deprecation 过渡。
- **涉及文件**：`src/chaser.rs`、`src/profiles.rs`、`src/lib.rs`、`examples/*`、`README.md`。

### P1-3：补自动化易用性（Playwright 风格 API 薄层）

- **问题**：`find_element`（`src/page.rs:544`）直接调 `DOM.querySelector`，**无自动等待**，元素未加载就 fail。这是和 Playwright 最根本的差距。
- **为什么重要**：现代网页异步加载，用户被迫手写 sleep + 重试循环。
- **方案**：加一个最小 `Locator` 层，只补**实际会用到的**几个核心能力：
  1. `wait_for_selector(sel, timeout)` —— 自动等待（解决 80% 痛点）
  2. `ZyLocator` —— 每次操作前重新查询，抗 stale
  3. `find_by_text(text)` / `click_by_text` —— 按文本定位（爬虫常用）
- **关键约束**：查询必须用 isolated world 执行（保持 stealth，不触发 `Runtime.enable`）。
- **验收**：`ZyPage::locator("#btn").click().await?` 一行完成"等元素可见+点击"，无需 sleep。
- **涉及文件**：新增 `src/locator.rs` 或扩展 `src/chaser.rs`。

## P2 - 中优先级（稳定性与工程化）

### P2-1：代理认证支持

- **问题**：`create_incognito_context_with_proxy`（`src/browser/mod.rs:481`）不支持 `user:pass@host:port`（Chrome 限制）。采集场景的代理大多需要认证。
- **方案**：用 `Fetch.continueWithAuth` 封装代理认证响应（407 challenge）。
- **验收**：带认证的 SOCKS5/HTTP 代理可直接用，无需本地转发器。
- **涉及文件**：`src/browser/mod.rs` 或新增 `src/proxy.rs`。

### P2-2：Dialog / 文件上传 / Select 下拉补全

- **问题**：对照 Playwright 标配，这三项完全缺失。
  - Dialog（alert/confirm/prompt）处理：无
  - 文件上传 `set_input_files`：无
  - Select 下拉选择：无
- **方案**：封装对应 CDP 命令（`Page.javaScriptDialogOpening` 事件 + `handleJavaScriptDialog`、`DOM.setFileInputFiles`、`Input.dispatchKeyEvent` 组合）。
- **涉及文件**：`src/page.rs`、`src/element.rs`。

### P2-3：stealth 改动冲突隔离

- **问题**：stealth 改动深度侵入上游活跃文件（`handler/frame.rs`、`page.rs`、`config.rs`），每次 merge 上游易冲突。
- **方案**：把侵入式改动用醒目标记包裹（`// ZYCDP-STEALTH-START ... END`），stealth 专属参数抽到独立 `static`，与上游物理分离。
- **涉及文件**：`src/handler/frame.rs`、`src/browser/config.rs`。

## P3 - 低优先级（长期方向）

### P3-1：评估多 context 多代理并发模型

针对"单浏览器进程跑多代理会话"场景，设计 first-class 的 `BrowserContext` 抽象。

### P3-2：CDP 版本自动跟进

监控上游 chromiumoxide 的 PDL 更新，建立自动化 merge + 测试流水线。

## 进度跟踪

每完成一项，在此处更新：

| 项 | 状态 | commit | 完成日期 | 备注 |
|---|---|---|---|---|
| P0-1 rebrowser parity | ⬜ 待开始 | - | - | - |
| P0-2 Windows 内存探测 | ⬜ 待开始 | - | - | - |
| P0-3 离线回归测试 | ⬜ 待开始 | - | - | - |
| P1-1 toString 对抗 | ⬜ 待开始 | - | - | - |
| P1-2 类型名改名 | ⬜ 待开始 | - | - | - |
| P1-3 Locator API | ⬜ 待开始 | - | - | - |
| P2-1 代理认证 | ⬜ 待开始 | - | - | - |
| P2-2 dialog/upload/select | ⬜ 待开始 | - | - | - |
| P2-3 冲突隔离 | ⬜ 待开始 | - | - | - |
