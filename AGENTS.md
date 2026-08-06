# AGENTS.md

> 本文件给未来在本仓库工作的 ZCode agent 提供**项目特定**要点。
> 详细原理请读 `docs/`，AI 交互习惯见用户级 `~/.zcode/AGENTS.md`。

## 项目本质（先读这个再动手）

zycdp 是 [chromiumoxide](https://github.com/mattsse/chromiumoxide) 的**二次 fork**，专注**反检测浏览器自动化（stealth）**。血缘：

```
mattsse/chromiumoxide      ← 上游（CDP 客户端原项目）
  └─ ccheshirecat/chaser-oxide  ← 原 fork（加 stealth 改造）
      └─ Freedonull/zycdp       ← 本仓库（继续优化）
```

**改名边界**：只有主 crate 改名为 `zycdp`。4 个子 crate 仍保留 `chromiumoxide_*` 前缀（`chromiumoxide_cdp` / `chromiumoxide_pdl` / `chromiumoxide_types` / `chromiumoxide_fetcher`）—— 故意不改，避免牵连 path 依赖。git remote `upstream` 指向 `chaser-oxide`，不是 `chromiumoxide`。

## 动手前必读文档

| 何时 | 读什么 |
|---|---|
| 想理解 stealth 原理 / "为什么这样改" | `docs/01-architecture.md`（检测维度对抗矩阵，附 `file:line`） |
| 改动前对照已知缺陷 | `docs/05-defects-baseline.md` |
| 规划开发周期 | `docs/02-improvement-roadmap.md` |
| merge 上游前 | `docs/03-upstream-sync.md`（高冲突文件清单见下） |
| 集成到业务 | `docs/04-usage-guide.md` |

注意：`CLAUDE.md` 也存在，但其中 **MSRV 与 feature 说明已过时**——以本文件和 `Cargo.toml`/CI 为准。

## 构建 / 测试命令（以实际配置为准）

```bash
cargo build
cargo test --lib                                              # 单元测试（CI 三平台都跑）
RUST_TEST_THREADS=1 cargo test --test '*'                     # 集成测试，需本机装 Chrome/Chromium
cargo test --lib <test_name>                                  # 跑单个测试
cargo fmt                                                     # 格式化
cargo fmt --all -- --check                                    # CI 格式检查（必须无 diff）
cargo clippy --all -- -D warnings                             # CI 强制零警告
cargo check --examples --features fetcher,zip8,rustls         # CI 验证 examples 能编译
cargo run --example stealth_bot                               # 跑某个 example
cargo run --example profile_demo
```

- **MSRV = 1.85**（`Cargo.toml` 的 `rust-version`，CI 用 `1.85.0` 跑 `cargo check --lib`）。
- **edition = 2024**（`Cargo.toml`），所以工具链必须 ≥ 1.85。
- **runtime 只有 tokio**：`async-tungstenite` 写死 `features = ["tokio-runtime"]`。当前 Cargo.toml **没有** `async-std-runtime` feature，别按上游文档套用。
- feature 默认开 `bytes`；`fetcher` 可选（依赖 `chromiumoxide_fetcher`）。
- `autotests = false`；集成测试入口在 `tests/lib.rs`（test target 名 `chromiumoxide_tests`），`tests/stealth/` 子目录含 `incolumitas.rs`、`rebrowser.rs`。

## 不可触碰的 Stealth 红线（改错会让整个项目失去价值）

这些是 zycdp 区别于上游的核心改动，**任何"恢复上游行为"的修改都必须拒绝**：

1. **绝不调用 `Runtime.enable`**。`ChaserPage.evaluate()` 走 `Page.createIsolatedWorld` + `Runtime.evaluate`（不预先 enable Runtime domain），见 `src/chaser.rs`。反爬靠探测 Runtime domain 被启用判定 CDP 自动化。
   - **使用红线**：`raw_page().evaluate()` 会触发 `Runtime.enable` → 触发检测。对外只能暴露 `chaser.evaluate()`。引导用户用 `ChaserPage`，不要用裸 `Page.evaluate()`。
2. **`DEFAULT_ARGS` 裁剪必须保留**（`src/browser/config.rs`，从上游 24 条裁到 19 条）。删除的是暴露自动化的 flag（`--metrics-recording-only`、`--enable-blink-features=IdleDetection` 等）。
3. **`src/handler/frame.rs` 中删除 `enable_runtime` 调用的地方**，不能被上游版本覆盖回去。
4. **指纹一致性约束**：所有指纹值（UA、`navigator.platform`、WebGL vendor/renderer、`userAgentData`、硬件并发、deviceMemory）必须由同一 `Os` 枚举驱动，内部自洽。Windows UA 配 MacIntel platform 会被秒判。改 `ChaserProfile` 时保持这套一致性。
5. **Native profile 零 JS 注入策略**（`src/profiles.rs`）：native 模式 bootstrap 为空字符串，UA/平台全用浏览器真实值，仅替换 `HeadlessChrome` → `Chrome`。真实属性比 JS 伪造更难检测，别无端往 native 模式加 JS patch。

> 代码里带 `// ZYCDP-STEALTH` / `// chaser-oxide Stealth` / "THE REBROWSER METHOD" 注释的区段是核心，动它们前先读 `docs/01` 对应章节。

## 同步上游时的高冲突文件

合并 `upstream/main`（chaser-oxide）时，冲突优先保留 zycdp 的 stealth 改动；`chromiumoxide_cdp/` 的 CDP 协议生成代码更新直接接受上游。完整流程见 `docs/03-upstream-sync.md`。合并后必须 `cargo check --lib --examples --tests` + stealth 回归测试全绿才能 push。

## 约定

- **语言**：代码注释、文档、commit message 默认用简体中文（遵循用户默认 `~/.zcode/AGENTS.md`）。
- **提交前必过**：`cargo fmt --check` + `cargo clippy --all -- -D warnings` + `cargo test --lib`。CI 任一不过会阻断 PR。
- **改动同步更新文档**：新增/修改 stealth 对抗项时，同步更新 `docs/01-architecture.md` 对应表格；修复 `docs/05-defects-baseline.md` 中的缺陷条目并附 commit hash。
- **三层 API 层次**：`Page`（底层 + `enable_stealth_mode`）→ `ChaserProfile`（builder，指纹）→ `ChaserPage`（高层封装，推荐入口，组合 Page + 指纹 + 人类行为 + 请求拦截）。对外公开类型集中在 `src/lib.rs` 的 re-export。

## Stealth 安全的查询/执行路径（改 ChaserPage/Page 时必读）

并非所有 CDP 命令都触发 `Runtime.enable` 检测。区分清楚才能既加功能又不破坏 stealth：

| 操作 | 走的域 | 是否 stealth-safe |
|---|---|---|
| `find_element` / `find_xpath` | DOM（QuerySelector / PerformSearch） | ✅ 安全，不碰 Runtime |
| `Element::click` / `call_js_fn` / `focus` | `Runtime.callFunctionOn`（**不**需 Runtime.enable） | ✅ 安全，且在**主世界**执行 |
| `ChaserPage::evaluate` / `evaluate_stealth` | `Page.createIsolatedWorld` + `Runtime.evaluate` | ✅ 安全（隔离世界，从不 enable Runtime） |
| `Page::evaluate` / `set_content` | 走 secondary execution context，**依赖 Runtime.enable 填充 context** | ❌ 禁用，会触发检测 |
| `DOM.setFileInputFiles` / `Emulation.*` / `Fetch.*` / `Input.*` | 各自域 | ✅ 安全 |

**关键结论**：需要"在页面执行 JS"时，用 `ChaserPage::evaluate`（隔离世界），**禁止** `raw_page().evaluate()` 或 `raw_page().set_content()`。需要 DOM 交互时，`find_element` / `Element` 方法都安全。

### ⚠️ 隔离世界 vs 主世界的可见性差异（踩过的坑）

`ChaserPage::evaluate`（隔离世界）**读不到** bootstrap 注入的某些值，因为：

- **prototype 上的 getter**（`navigator.hardwareConcurrency` / `deviceMemory` / `platform` 等）：bootstrap 用 `Object.defineProperty(Navigator.prototype, ...)` 在主世界定义了 getter。隔离世界的 `Navigator.prototype` 虽 `hasOwnProperty` 返回 true，但 descriptor **没有 get 函数**（getter 不跨 realm 传播），读属性会 fallback 到原生值（真实核数等）。
- **window 实例赋值**（`window.chrome = {...}`）：只在主世界生效，隔离世界 `window.chrome` 是 undefined。

**所以**：读指纹属性做断言/验证时，必须用 **主世界**（`Element::call_js_fn("function(){return navigator.xxx;}", false)`），这才是反爬站点真正看到的视角（站点 JS 跑在主世界）。`tests/stealth/offline_assertions.rs` 的 `fingerprint_consistency_windows_profile` 和 `chrome_object_present` 就是这么做的。只有读取 bootstrap 本身的结构（如 `Object.getOwnPropertyDescriptor(...).get.toString()`）才能用隔离世界。

## 在 Windows 开发机跑集成测试的坑

`tests/lib.rs` 的 `test_config` 帮手用固定默认 user-data-dir（`%TEMP%/chromiumoxide-runner`），若本机日常 Chrome 或前次测试残留进程占用该目录，`Browser::launch` 会 `LaunchExit(21)`（profile 锁冲突）。解法：用唯一临时 user-data-dir（见 `offline_assertions.rs` 的 `with_stealth_profile`）。另外 chrome 命令行传 URL 参数（如 `about:blank`）在 `headless=new` 模式会报 "Multiple targets are not supported in headless mode" 退 21——zycdp 的 `Browser::launch` 本身不传 URL，但自写启动逻辑时注意。

## ChaserPage 能力清单（当前已封装）

| 方法 | 用途 | stealth 路径 |
|---|---|---|
| `apply_profile` / `apply_native_profile` | 注入指纹（UA + bootstrap JS + screen + focus） | Emulation + Page |
| `evaluate(script)` | 隔离世界执行 JS（不触发 Runtime.enable） | createIsolatedWorld |
| `wait_for_selector(sel, timeout)` | 轮询等待元素出现（解决 D3 无自动等待） | DOM |
| `find_by_text` / `click_by_text` | XPath 文本定位 | DOM PerformSearch |
| `locator(sel) -> ZyLocator` | 抗 stale 句柄（每次操作重新查询） | DOM |
| `select_option(sel, val)` | `<select>` 选值（触发 change/input） | DOM + 隔离世界 |
| `set_input_files(sel, paths)` | 文件上传 | DOM.setFileInputFiles |
| `drag_human(x, y)` | 贝塞尔拖拽（mousedown→move→mouseup） | Input |
| `human_idle(min, max)` / `idle()` | 随机停顿，抗行为分析 | 纯等待 |
| `move_mouse_human` / `click_human` / `scroll_human` / `type_text[_with_typos]` | 人类式鼠标/键盘/滚动 | Input |
| `enable_request_interception` / `fulfill_request_html` / `continue_request` | 请求拦截 | Fetch |
| `on_dialog(handler)` / `auto_handle_dialogs(accept)` | 自动处理 alert/confirm/prompt/beforeunload，不注册则弹框阻塞页面 | Page 事件 + handleJavaScriptDialog |
| `enable_proxy_auth(user, pass)` | 代理 HTTP 认证（响应 407），让 `user:pass@host:port` 代理可用 | Fetch.continueWithAuth |

## Bootstrap JS 的 toString 对抗约束

非 native 模式的 bootstrap patch 了大量函数（getParameter/canPlayType/connect/query 等）。
patch 后这些函数 `toString()` 不返回 `[native code]`，会被 CreepJS 级检测识破。
bootstrap 末尾的 `zycdp toString 深度对抗` 块用 WeakMap 记录被 patch 函数 → 重写
`Function.prototype.toString` 返回伪造的 native 字符串。

**新增 bootstrap patch 时必须**：在 toString 对抗块的注册列表里加上新 patch 的函数
（`maskAsNative(fn, name)`），否则新 patch 立即成为检测向量。**约束**：该块严禁用
ES 模板字面量（反引号 + `${}`），因为下方 Worker 注入会把整段 script 嵌进反引号模板，
`${}` 会被 Worker 侧当成插值报 ReferenceError。用字符串拼接。

## 验证 stealth 改动

- **离线回归测试**：`tests/stealth/offline_assertions.rs`（4 个测试，断言指纹一致性/
  chrome 对象/CDP 标记清理/toString 返回 native code）。CI 的 `test-integration` job
  跑（ubuntu + chromium）。本机跑：`RUST_TEST_THREADS=1 cargo test --test chromiumoxide_tests stealth::offline_assertions`。
  注意：需本机 Chrome，且 `BrowserConfig` 默认 user-data-dir 可能与本机日常 Chrome
  profile 锁冲突（LaunchExit 21），测试帮手已用唯一临时目录绕开。
- **外部站点测试**（手动，标 `#[ignore]`）：`tests/stealth/rebrowser.rs`（bot-detector.rebrowser.net）、
  `tests/stealth/incolumitas.rs`（bot.incolumitas.com）。
