# 01 - zycdp 架构与 Stealth 技术原理

> 本文档基于对源码的逐行分析整理。所有结论附代码位置（`file:line`），可直接溯源。

## 一、项目本质

zycdp 是 `chromiumoxide`（Rust CDP 客户端）的二次 fork，专门针对**反检测浏览器自动化（Stealth Browser Automation）**做了协议层、运行时层、行为层硬化。

**核心策略**：反爬检测的本质 = "区分自动化浏览器与真人浏览器"。检测方从多个维度采集信号，任一维度出现"自动化特征"或"内部不一致"即被判为 bot。zycdp 的应对：**在每一个检测维度上，要么消除自动化特征，要么保证所有特征内部自洽**。

**血缘关系**：
```
mattsse/chromiumoxide  ← CDP 客户端原始项目（上游）
    └── ccheshirecat/chaser-oxide  ← 加入 stealth 改造（原 fork）
        └── Freedonull/zycdp  ← 本项目（继续优化）
```

## 二、检测维度与对抗技术矩阵

| 检测层 | 检测方看什么 | zycdp 的对抗手段 | 代码位置 |
|---|---|---|---|
| **进程/启动参数** | Chrome 启动命令行中的自动化 flag | 从 `DEFAULT_ARGS` 删除暴露自动化的 flag | `src/browser/config.rs:469` |
| **协议层（CDP）** | `Runtime.enable` 被调用 | 用 `Page.createIsolatedWorld` 取代，**从不调用 Runtime domain** | `src/chaser.rs:435` |
| **HTTP 头层** | UA 与 `Sec-CH-UA-*` 是否一致 | `Emulation.setUserAgentOverride` 同时设 UA 头和 `UserAgentMetadata` | `src/chaser.rs:215` |
| **JS 环境层** | webdriver、platform、WebGL、chrome、plugins、codecs | `addScriptToEvaluateOnNewDocument` 注入 bootstrap patch prototype | `src/profiles.rs:289` |
| **行为层** | 鼠标轨迹、点击节奏、打字 | 贝塞尔曲线 + 抖动 + 过冲；变速打字 + 打错字 | `src/chaser.rs:485`、`735` |

## 三、工作空间结构

Cargo workspace，5 个 crate：

```
zycdp（主 crate，用户唯一接口）
├── chromiumoxide_cdp      ← 从 PDL 生成的 CDP 协议绑定（~60K 行，构建时生成）
├── chromiumoxide_pdl      ← PDL 解析器（协议定义语言 → Rust 代码）
├── chromiumoxide_types    ← 跨 crate 共享类型
└── chromiumoxide_fetcher  ← 可选：自动下载 Chrome 二进制
```

子 crate 名字保留 `chromiumoxide_*`（无商标冲突，且改名会牵连 path 依赖，徒增风险）。zycdp 改名只动主 crate。

## 四、三层 API 设计

### 第 1 层：`Page`（继承自 chromiumoxide，加了 stealth）

`Page::enable_stealth_mode()`（`src/page.rs:67`）—— 不改 OS/版本身份，只做自动化信号清除：
- `hide_webdriver()`：`navigator.webdriver = false`，挂在 `Navigator.prototype` 而非实例（绕过 `getOwnPropertyNames` 检测）
- `hide_plugins()`：构造真正的 `PluginArray.prototype` 后代（非 Array），绕过 `Array.isArray()` 检测
- `hide_webgl_vendor()`：patch `getParameter` 拦截 `37445`(vendor)/`37446`(renderer)
- `hide_codecs()`：让 `canPlayType('avc1')` 返回 `'probably'`（headless 默认缺失，Turnstile/DataDome 关键检测点）
- `hide_permissions()`：对齐 `permissions.query(notifications)` 与 `Notification.permission`
- UA 含 `HeadlessChrome` → 替换成 `Chrome`

### 第 2 层：`ChaserProfile`（builder 模式，`src/profiles.rs`）

- OS 预设：`windows()` / `linux()` / `macos_intel()` / `macos_arm()` / `native()`
- 可配置：Chrome 版本、GPU（9 种预设）、内存、CPU、locale、timezone、屏幕分辨率
- 一致性约束：所有指纹由同一 `Os` 枚举驱动，从源头保证一致（Windows UA + MacIntel platform 会被秒判）

### 第 3 层：`ChaserPage`（高层封装，`src/chaser.rs`）

组合 `Page` + 指纹应用 + 人类行为模拟 + 请求拦截。推荐入口。

## 五、关键技术深挖

### 1. Rebrowser 式隐身执行（最核心创新，`src/chaser.rs:435`）

**问题**：常规 `Page.evaluate()` 内部调用 `Runtime.enable`，网站 JS 可探测到 Runtime domain 被启用 → 判定 CDP 自动化。

**zycdp 解法**（注释称 "THE REBROWSER METHOD"）：
```
1. Page.createIsolatedWorld(frameId, worldName="chaser") → 响应返回 executionContextId
2. Runtime.evaluate(contextId=上面拿到的 id, expression=脚本)
   ↑ 用 evaluate 命令本身，但未事先 Runtime.enable，整个 Runtime domain 处于未启用状态
```

隔离世界（isolated world）双重价值：
- **对网站隐身**：网站 JS 看不到注入的变量（独立 JS 上下文）
- **对反爬隐身**：不触发 `Runtime.enable` 副作用

> ⚠️ **已知差距**：README 声称 "100% rebrowser parity"，但实测 `evaluate_stealth` 跳过了 rebrowser 官方补丁的"主世界 binding 获取 context id"步骤。严格检测下可能暴露。详见 [05-defects-baseline.md](./05-defects-baseline.md)。

⚠️ **使用红线**：`raw_page().evaluate()` 会触发检测，必须用 `chaser.evaluate()`。

### 2. Native Profile 的"零 JS 注入"策略（最新演进）

近期 commit（`47450f8`）把 native 模式 bootstrap 改成空字符串（`src/profiles.rs:611`）：
- Native 模式**完全不注入 bootstrap JS**
- UA/Sec-CH-UA/platform 全用浏览器真实值（`native_ua_data = true`）
- 只做：① `Emulation.setUserAgentOverride` 把 `HeadlessChrome` → `Chrome`；② `SetDeviceMetricsOverride` 修 screen 尺寸

**原理**：真实浏览器原生属性永远比 JS 伪造难检测。JS 越少 = 攻击面越小（规避 `toString()` 检测）。

### 3. screen 尺寸修复（`src/chaser.rs:184`）

**问题**：headless 默认 `screen.width=800, height=600`，但 `--window-size=1920,1080` 让 `innerWidth=1920`。`innerWidth > screen.width` 物理不可能，是已知 headless 检测信号。

**解法**：`Emulation.setDeviceMetricsOverride` 的 `screenWidth/screenHeight` 在浏览器内核层覆盖（非 JS wrapper）。

### 4. 焦点仿真（`src/chaser.rs:202`）

`SetFocusEmulationEnabled(true)` 让 `hasFocus()=true`、`visibilityState='visible'`。不加则 headless 报 false/hidden。

### 5. Bootstrap JS 注入的对抗清单（`src/profiles.rs:293`）

| # | 项 | 对抗的检测 |
|---|---|---|
| 0 | 清 `cdc_`/`$cdc_`/`__webdriver` 等 window 属性 | ChromeDriver/Selenium 痕迹 |
| 0b | 劫持 `Error.prepareStackTrace` setter（no-op） | CDP 改此属性会被检测 |
| 1-2 | `Navigator.prototype` platform/hardwareConcurrency/deviceMemory | prototype 层绕 `getOwnPropertyNames` |
| 3 | WebGL `getParameter` patch | GPU 指纹 |
| 4 | `userAgentData` + `getHighEntropyValues()` | UA-CH 一致性 |
| 5 | `canPlayType` patch | H.264/AAC 编解码 |
| 6 | `navigator.webdriver = false` | 基础 bot 信号 |
| 7 | 完整 `window.chrome`（runtime.connect/sendMessage、csi()、loadTimes()、app） | Turnstile 必查项 |
| 8 | `navigator.language/languages` 对齐 locale | locale 一致性 |
| 9 | `permissions.query` 对齐 `Notification.permission` | 权限一致性 |
| 10 | `serviceWorker.register` 变 no-op | 注册 SW 是指纹向量 |
| 末 | `Worker` 构造器包装注入 bootstrap | 防 Worker 暴露真实指纹 |

### 6. Chrome 启动参数清洗（`src/browser/config.rs:458`）

对照 Puppeteer 默认参数，主动删除暴露自动化的 flag（git 历史可证 `DEFAULT_ARGS` 从 24 条裁到 19 条）：
- `--metrics-recording-only`：patchright 明确移除，测试/自动化信号
- `--enable-features=NetworkService,NetworkServiceInProcess`：Chrome 80 起已默认，显式设置是旧自动化指纹
- `--enable-blink-features=IdleDetection`：非默认 API，`typeof IdleDetector` 可检测

### 7. Client Hints 全链路一致性（`src/chaser.rs:215`）

非 native 模式，`Emulation.setUserAgentOverride` 传完整 `UserAgentMetadata`（brands/fullVersionList/platform/platformVersion/architecture/bitness/wow64）。一个 CDP 调用同步 HTTP UA 头 + `Sec-CH-UA-*` 头 + JS `navigator.userAgentData`，从源头消除不一致——过 Cloudflare 的关键。

## 六、行为模拟（行为层对抗，`src/chaser.rs`）

| 方法 | 要点 |
|---|---|
| `move_mouse_human` | 三次贝塞尔曲线（25 步），控制点随机化 ±30%；目标 ±2px 抖动；20% 过冲 5%；步间 5-15ms |
| `click_human` | 移动后 50-150ms 停顿再点，点后 30-80ms 停顿 |
| `type_text` | 按键间 50-150ms；5% 概率 200-400ms 思考停顿 |
| `type_text_with_typos` | 3% 概率打错字（qwerty 邻近键）→ 停顿 → Backspace → 正确字 |
| `scroll_human` | 3-15 步，首尾 ease-in/out，每步 ±10px 抖动，16-50ms 帧间隔 |

## 七、Native Profile 环境探测（`src/profiles.rs:712`）

`ChaserProfile::native()` 真实读取宿主环境：
- **OS**：编译期 `cfg` + macOS `uname -m` 区分 arm64/x86
- **Chrome 版本**：`which` 找 chrome → `--version` 解析
- **内存**：macOS `sysctl hw.memsize`，Linux `/proc/meminfo`，**Windows 兜底 8GB（缺陷，见 baseline）**

`apply_native_profile`（`src/chaser.rs:295`）优先用 `page.user_agent()` 从活着的浏览器拿版本，覆盖 fetcher 下载二进制版本与系统 Chrome 不一致的情况。

## 八、版本演进脉络（git log 可证）

项目哲学从"显式伪装"转向"native 真实优先"：
1. 早期 `enable_stealth_mode` 硬编码 Windows NVIDIA（`ae42d3c` 移除硬编码）
2. 中期加 `apply_profile` + `apply_native_profile`，引入全链路 `UserAgentMetadata`（`2ab8de8`、`a814bee`）
3. 近期 native 逐步减少 JS 注入，最终"零 JS"（`47450f8`）—— 规避 `toString()` 检测

## 九、外部参考

- [rebrowser-patches 官方原理](https://github.com/rebrowser/rebrowser-patches) —— `Runtime.enable` 对抗的权威实现
- [rebrowser-bot-detector](https://github.com/rebrowser/rebrowser-bot-detector) —— 检测项对照
- [Castle.io: CDP bot detection signal 演化](https://blog.castle.io/why-a-classic-cdp-bot-detection-signal-suddenly-stopped-working-and-nobody-noticed/)
- [svebaa: V8 指纹与 toString 检测](https://svebaa.github.io/personal/blog/cdp-fingerprinting/) —— 深度检测向量分析
