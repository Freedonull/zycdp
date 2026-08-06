# 05 - 已知缺陷与待验证项 Baseline

> 本文档记录 zycdp（fork 时点）代码中**已确认的缺陷**和**未验证的声明**。
> 改动前对照此文档，避免重复劳动或回归。修复后在对应条目标 ✅ 并附 commit。
> baseline 基线 commit：`0752c99`（改名前）/ `c929526`（改名后）。

## 一、已确认缺陷（代码可证，必须修）

### D1：Windows 内存探测是假数据 🔴

- **位置**：`src/profiles.rs:802`，`_read_system_memory_gb` 的 Windows 分支
- **现状**：直接 `return 8`，硬编码
- **证据**：
  ```rust
  #[cfg(not(any(target_os = "macos", "target_os = "linux")))]
  fn _read_system_memory_gb() -> u32 {
      8  // ← 不是真实探测
  }
  ```
- **影响**：native 模式核心承诺是"用真实值"，Windows 上内存恒为 8GB，与系统实际 RAM 不符。`navigator.deviceMemory` 被设成假的离散值，破坏 native 一致性。
- **macOS/Linux 对比**：macOS 用 `sysctl hw.memsize`，Linux 读 `/proc/meminfo`，都是真实探测。只有 Windows 没做。
- **修复**：见 [改进路线 P0-2](./02-improvement-roadmap.md#p0-2修复-windows-内存探测假数据)
- **状态**：✅ 已修复（GlobalMemoryStatusEx via windows-sys 0.52，2026-08-06）

### D2：rebrowser parity 有缺口 🔴（已重新定性）

- **位置**：`src/chaser.rs`，`evaluate_stealth`
- **原描述（已证伪）**：旧版文档称"跳过了 rebrowser 主世界 binding 获取 context id 步骤"，并据此计划在 P0-1 里"补全"该步骤。
- **核实结果**：经对照 [rebrowser-patches 官方源码](https://github.com/rebrowser/rebrowser-patches) 的 patches/*.patch 文件，rebrowser 有三种模式：
  - `addBinding`（默认）：在主世界执行 JS
  - `alwaysIsolated`：在隔离世界执行 JS
  - `enableDisable`：瞬间 enable/disable Runtime 抓 context
  - zycdp 的 `evaluate_stealth` 走 `createIsolatedWorld`，**正好等价于 `alwaysIsolated` 模式**，是合法的 stealth 路线，不是"残缺的 addBinding"。
- **真实问题**：仅是注释夸大——`evaluate_stealth` 上方注释曾写 "100% stealth parity with Rebrowser"，这是营销式表述，与实现不符（默认模式不同）。
- **处理**：不"补全 binding 步骤"（那会把执行改到主世界，破坏对网站隐身性，参见 docs/01 第五节）。仅修正注释，准确描述为 alwaysIsolated 等价方案。
- **状态**：✅ 已修正（注释改为准确表述，commit 见下方路线图进度表）

### D3：find_element 无自动等待

- **位置**：`src/page.rs:544`
- **现状**：直接调 `DOM.querySelector`，找不到立刻返回 Error，无重试/超时
- **影响**：异步加载的网页上，用户被迫手写 sleep + 重试循环
- **修复**：见 [改进路线 P1-3](./02-improvement-roadmap.md#p1-3补自动化易用性playwright-风格-api-薄层)
- **状态**：✅ 已修复（ChaserPage::wait_for_selector + ZyLocator，2026-08-06）

### D4：代理认证不支持

- **位置**：`src/browser/mod.rs:481`，`create_incognito_context_with_proxy`
- **现状**：不支持 `user:pass@host:port`（Chrome 限制，非 zycdp bug），但 zycdp 也没有 `Fetch.continueWithAuth` 的封装
- **影响**：带认证的代理需要外部转发器才能用
- **修复**：见 [改进路线 P2-1](./02-improvement-roadmap.md#p2-1代理认证支持)
- **状态**：
  - **HTTP 代理**：✅ 已补 `ChaserPage::enable_proxy_auth`（Fetch.continueWithAuth 响应 407，2026-08-06）
  - **SOCKS5 代理**：✅ 已补 `Socks5Bridge`（feature `socks5-bridge`，本地 HTTP CONNECT 转发器代为完成 SOCKS5 认证握手，2026-08-06。本机真实代理验证通过）

> **为什么 SOCKS5 需要单独方案**：Chrome/Chromium 网络栈不支持 SOCKS5 用户名/密码认证（架构性缺失），`Fetch.authRequired` 和扩展 `webRequest.onAuthRequired` 都只覆盖 HTTP 代理。SOCKS5 认证在 TCP 握手层，CDP/扩展触达不到。唯一解法是浏览器进程外桥接——`Socks5Bridge` 在本地起 HTTP CONNECT 转发器，代为完成 RFC 1929 认证。详见 `src/socks5_bridge.rs` 文档。

### D5：类型名未统一改名

- **位置**：`src/chaser.rs`、`src/profiles.rs`、`src/lib.rs`
- **现状**：包名已改为 `zycdp`，但公开类型名 `ChaserPage`/`ChaserProfile` 仍带 Chaser
- **影响**：API 一致性、商标关联
- **修复**：见 [改进路线 P1-2](./02-improvement-roadmap.md#p1-2补全-api-类型名改名chaserpagechaserprofile--zycdp-命名)
- **状态**：⬜ 待修复

## 二、能力缺失（对照 Playwright 标配）

### M1：Dialog 处理缺失
- alert/confirm/prompt 无法处理，弹框一出页面卡死
- 修复：见 [改进路线 P2-2](./02-improvement-roadmap.md#p2-2dialog--文件上传--select-下拉补全)
- **状态**：✅ 已补（ChaserPage::on_dialog + auto_handle_dialogs，2026-08-06。本机验证 alert 自动接受、页面不卡死）

### M2：文件上传缺失
- 无 `set_input_files`，无法上传文件
- 修复：见 P2-2
- **状态**：✅ 已补（ChaserPage::set_input_files via DOM.setFileInputFiles，2026-08-06）

### M3：Select 下拉缺失
- 无 `<select>` 操作封装
- 修复：见 P2-2
- **状态**：✅ 已补（ChaserPage::select_option，按 value 选择并触发 change/input，2026-08-06）

### M4：Frame 操作句柄缺失
- `frames()`/`frame_url()` 只能只读查询，**没有 Frame 操作对象**，无法在 iframe 里点元素
- 待评估是否补

### M5：get_by_text/role/label 定位器缺失
- 只有 CSS selector + xpath，无法按文本/语义定位
- 部分由 P1-3 的 `find_by_text` 覆盖

## 三、未验证的声明（README/注释宣称但需核实）

### U1：README 营销文案夸大

- **README 原文**（fork 时点）："13,000+ V8 patches, no-JS protocol stealth, kernel-level egress, 300+ concurrent sessions"
- **核实结果**：开源代码中**没有任何 V8 patch、没有内核级网络代码**。这些是 chaser.sh 商业平台（HYPR PTE. LTD.）的特性，非开源版能力。
- **处理**：改名提交（`c929526`）已清理这些营销文案。
- **状态**：✅ 已处理（commit `c929526`）

### U2："passes cloudflare WAF, turnstile" 声明

- **git log**：`295c068 passes cloudflare WAF, turnstile etc. now`
- **核实结果**：仓库中 **没有任何自动化测试**证明这点（`tests/stealth/rebrowser.rs:18` 标 `#[ignore]`，注释 "flaky"）
- **状态**：⬜ 待验证 —— 需 P0-3 离线测试 + 实测反爬站点确认

### U3："Tested against Cloudflare Turnstile, Cloudflare WAF, bot.sannysoft.com, areyouheadless, deviceandbrowserinfo.com, CreepJS"

- **核实结果**：同 U2，无 CI 级证据，只是开发者手动测试记录
- **状态**：⬜ 待验证

## 四、反爬对抗有效性评估（基于代码 + 外部验证）

| 对抗项 | 有效性 | 依据 |
|---|---|---|
| `Runtime.enable` 对抗 | ✅ 真实有效 | git 历史可证原项目有此调用，zycdp 删除方向正确；rebrowser 官方文档确认该检测真实 |
| `DEFAULT_ARGS` 清洗 | ✅ 有效 | git 历史可证；属行业共识（patchright 同方向） |
| `HeadlessChrome` UA 修复 | ✅ 真实有效 | headless 默认含此字符串是已知特征 |
| screen 尺寸修复 | ✅ 真实有效 | `innerWidth > screen.width` 是已知 headless 信号 |
| 焦点仿真 | ✅ 真实有效 | headless 默认 hasFocus=false 是已知特征 |
| webdriver/plugins/WebGL/codecs/permissions patch | ✅ 有效 | rebrowser-bot-detector 明确测试 navigator.webdriver |
| native 零 JS 注入 | ⚠️ 部分有效 | 方向正确（规避 toString），但 rebrowser-bot-detector 未测 toString，CreepJS 才测 |
| 行为模拟 | ⚠️ 有效但非必需 | 对行为分析型反爬（DataDome/Akamai）有效，对 cf Turnstile 非决定性 |

## 五、Baseline 复现

若要验证当前代码的真实 stealth 水平，按以下步骤：

```bash
# 1. 启动 stealth_bot 示例（手动测试 bot.sannysoft.com）
cargo run --example stealth_bot

# 2. 启用 rebrowser 测试（标了 ignore，需手动跑）
cargo test --test chromiumoxide_tests stealth::rebrowser -- --ignored --nocapture

# 3. 对照 baseline 记录结果，作为后续优化的对比基准
```

## 六、文档更新规则

- 修复一个缺陷 → 对应 D 条目改 ✅，附 commit hash 和完成日期
- 发现新缺陷 → 新增 D 条目
- 验证一个 U 条目 → 改为 ✅（已验证有效）或 ❌（验证不通过），附证据
