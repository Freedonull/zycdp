//! Stealth profile system for customizable browser fingerprints.
//!
//! This module provides an ergonomic builder pattern for creating consistent
//! browser "personalities" that bypass anti-bot detection.
//!
//! # Example
//!
//! ```rust
//! use zycdp::profiles::{ChaserProfile, Gpu};
//!
//! let profile = ChaserProfile::windows()
//!     .chrome_version(130)
//!     .gpu(Gpu::NvidiaRTX4080)
//!     .memory_gb(16)
//!     .cpu_cores(12)
//!     .build();
//! ```

use std::fmt;

/// GPU presets for WebGL spoofing
#[derive(Debug, Clone, Copy)]
pub enum Gpu {
    /// NVIDIA GeForce RTX 3080 (high-trust gaming GPU)
    NvidiaRTX3080,
    /// NVIDIA GeForce RTX 4080 (newer gaming GPU)
    NvidiaRTX4080,
    /// NVIDIA GeForce GTX 1660 (mid-range GPU)
    NvidiaGTX1660,
    /// Intel UHD Graphics 630 (common laptop GPU)
    IntelUHD630,
    /// Intel Iris Xe (modern laptop GPU)
    IntelIrisXe,
    /// Apple M1 Pro
    AppleM1Pro,
    /// Apple M2 Max
    AppleM2Max,
    /// Apple M4 Max
    AppleM4Max,
    /// AMD Radeon RX 6800
    AmdRadeonRX6800,
}

impl Gpu {
    /// Returns the WebGL vendor string
    pub fn vendor(&self) -> &'static str {
        match self {
            Gpu::NvidiaRTX3080 | Gpu::NvidiaRTX4080 | Gpu::NvidiaGTX1660 => "Google Inc. (NVIDIA)",
            Gpu::IntelUHD630 | Gpu::IntelIrisXe => "Google Inc. (Intel)",
            Gpu::AppleM1Pro | Gpu::AppleM2Max | Gpu::AppleM4Max => "Google Inc. (Apple)",
            Gpu::AmdRadeonRX6800 => "Google Inc. (AMD)",
        }
    }

    /// Returns the WebGL renderer string
    pub fn renderer(&self) -> &'static str {
        match self {
            Gpu::NvidiaRTX3080 => {
                "ANGLE (NVIDIA, NVIDIA GeForce RTX 3080 Direct3D11 vs_5_0 ps_5_0)"
            }
            Gpu::NvidiaRTX4080 => {
                "ANGLE (NVIDIA, NVIDIA GeForce RTX 4080 Direct3D11 vs_5_0 ps_5_0)"
            }
            Gpu::NvidiaGTX1660 => {
                "ANGLE (NVIDIA, NVIDIA GeForce GTX 1660 SUPER Direct3D11 vs_5_0 ps_5_0)"
            }
            Gpu::IntelUHD630 => "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0)",
            Gpu::IntelIrisXe => {
                "ANGLE (Intel, Intel(R) Iris(R) Xe Graphics Direct3D11 vs_5_0 ps_5_0)"
            }
            Gpu::AppleM1Pro => "ANGLE (Apple, Apple M1 Pro, OpenGL 4.1)",
            Gpu::AppleM2Max => "ANGLE (Apple, Apple M2 Max, OpenGL 4.1)",
            Gpu::AppleM4Max => {
                "ANGLE (Apple, ANGLE Metal Renderer: Apple M4 Max, Unspecified Version)"
            }
            Gpu::AmdRadeonRX6800 => "ANGLE (AMD, AMD Radeon RX 6800 XT Direct3D11 vs_5_0 ps_5_0)",
        }
    }
}

/// Operating system presets
#[derive(Debug, Clone, Copy)]
pub enum Os {
    /// Windows 10/11 64-bit
    Windows,
    /// macOS (Intel)
    MacOSIntel,
    /// macOS (Apple Silicon)
    MacOSArm,
    /// Linux x86_64
    Linux,
}

impl Os {
    /// Returns the navigator.platform value
    pub fn platform(&self) -> &'static str {
        match self {
            Os::Windows => "Win32",
            Os::MacOSIntel | Os::MacOSArm => "MacIntel",
            Os::Linux => "Linux x86_64",
        }
    }

    /// Returns the client hints platform
    pub fn hints_platform(&self) -> &'static str {
        match self {
            Os::Windows => "Windows",
            Os::MacOSIntel | Os::MacOSArm => "macOS",
            Os::Linux => "Linux",
        }
    }

    /// Returns the UA-CH platformVersion string
    pub fn platform_version(&self) -> &'static str {
        match self {
            Os::Windows => "15.0.0",                   // Windows 11
            Os::MacOSIntel | Os::MacOSArm => "15.3.1", // macOS Sequoia
            Os::Linux => "",
        }
    }

    /// Returns the UA-CH architecture string
    pub fn architecture(&self) -> &'static str {
        match self {
            Os::Windows | Os::MacOSIntel | Os::Linux => "x86",
            Os::MacOSArm => "arm",
        }
    }
}

/// A builder for creating consistent browser fingerprint profiles.
///
/// # Example
///
/// ```rust
/// use zycdp::profiles::{ChaserProfile, Gpu, Os};
///
/// // Quick preset
/// let profile = ChaserProfile::windows().build();
///
/// // Customized
/// let profile = ChaserProfile::new(Os::Windows)
///     .chrome_version(130)
///     .gpu(Gpu::NvidiaRTX4080)
///     .memory_gb(32)
///     .cpu_cores(16)
///     .locale("de-DE")
///     .timezone("Europe/Berlin")
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct ChaserProfile {
    os: Os,
    chrome_version: u32,
    gpu: Gpu,
    memory_gb: u32,
    cpu_cores: u32,
    locale: String,
    timezone: String,
    screen_width: u32,
    screen_height: u32,
    /// When true, skip overriding navigator.userAgentData in JS and
    /// Emulation.setUserAgentOverride metadata in CDP. Use this for native
    /// profiles so HTTP Sec-CH-UA headers and JS navigator.userAgentData
    /// both reflect the real browser binary (no mismatch for Cloudflare to detect).
    pub native_ua_data: bool,
}

impl Default for ChaserProfile {
    fn default() -> Self {
        Self::native().build()
    }
}

impl ChaserProfile {
    /// Create a new profile builder with the specified OS
    #[allow(clippy::new_ret_no_self)]
    pub fn new(os: Os) -> ChaserProfileBuilder {
        ChaserProfileBuilder {
            os,
            chrome_version: 129,
            gpu: match os {
                Os::Windows => Gpu::NvidiaRTX3080,
                Os::MacOSIntel => Gpu::AppleM1Pro,
                Os::MacOSArm => Gpu::AppleM4Max,
                Os::Linux => Gpu::NvidiaGTX1660,
            },
            memory_gb: 8,
            cpu_cores: 8,
            locale: "en-US".to_string(),
            timezone: "America/New_York".to_string(),
            screen_width: 1920,
            screen_height: 1080,
            native_ua_data: false,
        }
    }

    /// Create a Windows profile with sensible defaults (RTX 3080, 8 cores)
    pub fn windows() -> ChaserProfileBuilder {
        Self::new(Os::Windows)
    }

    /// Create a macOS Intel profile
    pub fn macos_intel() -> ChaserProfileBuilder {
        Self::new(Os::MacOSIntel).gpu(Gpu::AppleM1Pro)
    }

    /// Create a macOS Apple Silicon profile
    pub fn macos_arm() -> ChaserProfileBuilder {
        Self::new(Os::MacOSArm).gpu(Gpu::AppleM4Max)
    }

    /// Create a Linux profile
    pub fn linux() -> ChaserProfileBuilder {
        Self::new(Os::Linux)
    }

    /// Create a profile matching the current host OS, with optional Chrome version auto-detection.
    /// Reads actual system RAM and tries to detect the installed Chrome version.
    /// Sets `native_ua_data = true` so HTTP Sec-CH-UA headers and JS navigator.userAgentData
    /// both reflect the real browser binary — no inconsistency for Cloudflare to detect.
    pub fn native() -> ChaserProfileBuilder {
        let os = detect_current_os();
        let chrome = detect_chrome_version().unwrap_or(131);
        let memory = detect_system_memory_gb();
        Self::new(os)
            .chrome_version(chrome)
            .memory_gb(memory)
            .native_ua_data(true)
    }

    // Getters
    pub fn os(&self) -> Os {
        self.os
    }
    pub fn chrome_version(&self) -> u32 {
        self.chrome_version
    }
    pub fn gpu(&self) -> Gpu {
        self.gpu
    }
    pub fn memory_gb(&self) -> u32 {
        self.memory_gb
    }
    pub fn cpu_cores(&self) -> u32 {
        self.cpu_cores
    }
    pub fn locale(&self) -> &str {
        &self.locale
    }
    pub fn timezone(&self) -> &str {
        &self.timezone
    }
    pub fn screen_width(&self) -> u32 {
        self.screen_width
    }
    pub fn screen_height(&self) -> u32 {
        self.screen_height
    }
    pub fn native_ua_data(&self) -> bool {
        self.native_ua_data
    }

    /// Returns the valid `navigator.deviceMemory` value (spec allows: 0.25, 0.5, 1, 2, 4, 8).
    fn device_memory_value(&self) -> f32 {
        match self.memory_gb {
            0 => 0.25,
            1 => 1.0,
            2 => 2.0,
            3 | 4 => 4.0,
            _ => 8.0,
        }
    }

    /// Generate the User-Agent string for this profile
    pub fn user_agent(&self) -> String {
        let os_part = match self.os {
            Os::Windows => "Windows NT 10.0; Win64; x64",
            Os::MacOSIntel | Os::MacOSArm => "Macintosh; Intel Mac OS X 10_15_7",
            Os::Linux => "X11; Linux x86_64",
        };
        format!(
            "Mozilla/5.0 ({}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
            os_part, self.chrome_version
        )
    }

    /// Generate the complete JavaScript bootstrap script for this profile
    pub fn bootstrap_script(&self) -> String {
        if self.native_ua_data {
            return Self::native_bootstrap_script();
        }
        let mut script = format!(
            r#"
            (function() {{
                // === zycdp HARDWARE HARMONY ===
                // Profile: {ua}

                // 0. CDP Marker Cleanup (run once at startup)
                for (const prop of Object.getOwnPropertyNames(window)) {{
                    if (/^cdc_|^\$cdc_|^__webdriver|^__selenium|^__driver|^\$chrome_/.test(prop)) {{
                        try {{ delete window[prop]; }} catch(e) {{}}
                    }}
                }}

                // Prevent CDP detection via Error.prepareStackTrace
                const OriginalError = Error;  
                const originalPrepareStackTrace = Error.prepareStackTrace;    
                let currentPrepareStackTrace = originalPrepareStackTrace;    
                Object.defineProperty(Error, 'prepareStackTrace', {{    
                    get() {{
                        return currentPrepareStackTrace;   
                    }},  
                    set(fn) {{ 
                        // do nothing to prevent detection of CDP
                    }},    
                    configurable: true,    
                    enumerable: false  
                }});

                // 1. Platform (on prototype to avoid getOwnPropertyNames detection)
                Object.defineProperty(Navigator.prototype, 'platform', {{
                    get: () => '{platform}',
                    configurable: true
                }});

                // 2. Hardware (on prototype)
                Object.defineProperty(Navigator.prototype, 'hardwareConcurrency', {{
                    get: () => {cores},
                    configurable: true
                }});
                Object.defineProperty(Navigator.prototype, 'deviceMemory', {{
                    get: () => {device_memory},
                    configurable: true
                }});
                Object.defineProperty(Navigator.prototype, 'maxTouchPoints', {{
                    get: () => 0,
                    configurable: true
                }});

                // 3. WebGL
                const spoofWebGL = (proto) => {{
                    const getParameter = proto.getParameter;
                    proto.getParameter = function(parameter) {{
                        if (parameter === 37445) return '{webgl_vendor}';
                        if (parameter === 37446) return '{webgl_renderer}';
                        return getParameter.apply(this, arguments);
                    }};
                }};
                spoofWebGL(WebGLRenderingContext.prototype);
                if (typeof WebGL2RenderingContext !== 'undefined') {{
                    spoofWebGL(WebGL2RenderingContext.prototype);
                }}

                {ua_data_block}

                // 5. Video Codecs
                const canPlayType = HTMLMediaElement.prototype.canPlayType;
                HTMLMediaElement.prototype.canPlayType = function(type) {{
                    if (type.includes('avc1')) return 'probably';
                    if (type.includes('mp4a.40')) return 'probably';
                    if (type === 'video/mp4') return 'probably';
                    return canPlayType.apply(this, arguments);
                }};

                // 6. WebDriver (set to false instead of delete - more realistic)
                Object.defineProperty(Object.getPrototypeOf(navigator), 'webdriver', {{
                    get: () => false,
                    configurable: true,
                    enumerable: true
                }});

                // 7. Chrome Object (enhanced with runtime APIs)
                if (!window.chrome) {{
                    window.chrome = {{}};
                }}
                if (!window.chrome.runtime) {{
                    window.chrome.runtime = {{}};
                }}
                
                // Chrome Runtime APIs (required by Turnstile)
                if (!window.chrome.runtime.connect) {{
                    window.chrome.runtime.connect = function() {{
                        return {{
                            name: '',
                            sender: undefined,
                            onDisconnect: {{ 
                                addListener: function() {{}}, 
                                removeListener: function() {{}},
                                hasListener: function() {{ return false; }},
                                hasListeners: function() {{ return false; }}
                            }},
                            onMessage: {{ 
                                addListener: function() {{}}, 
                                removeListener: function() {{}},
                                hasListener: function() {{ return false; }},
                                hasListeners: function() {{ return false; }}
                            }},
                            postMessage: function() {{}},
                            disconnect: function() {{}}
                        }};
                    }};
                }}
                if (!window.chrome.runtime.sendMessage) {{
                    window.chrome.runtime.sendMessage = function() {{ return; }};
                }}

                // Chrome CSI (Chrome Speed Index) - some sites check this
                if (!window.chrome.csi) {{
                    window.chrome.csi = function() {{
                        const now = Date.now();
                        return {{ 
                            startE: now, 
                            onloadT: now, 
                            pageT: now, 
                            tran: 15 
                        }};
                    }};
                }}

                // Chrome loadTimes (deprecated but still checked)
                if (!window.chrome.loadTimes) {{
                    window.chrome.loadTimes = function() {{
                        const now = Date.now() / 1000;
                        return {{
                            requestTime: now,
                            startLoadTime: now,
                            commitLoadTime: now,
                            finishDocumentLoadTime: now,
                            finishLoadTime: now,
                            firstPaintTime: now,
                            firstPaintAfterLoadTime: 0,
                            navigationType: "Other",
                            wasFetchedViaSpdy: false,
                            wasNpnNegotiated: false,
                            npnNegotiatedProtocol: "",
                            wasAlternateProtocolAvailable: false,
                            connectionInfo: "http/1.1"
                        }};
                    }};
                }}

                // Chrome app object
                if (!window.chrome.app) {{
                    window.chrome.app = {{
                        isInstalled: false,
                        InstallState: {{
                            DISABLED: 'disabled',
                            INSTALLED: 'installed',
                            NOT_INSTALLED: 'not_installed'
                        }},
                        RunningState: {{
                            CANNOT_RUN: 'cannot_run',
                            READY_TO_RUN: 'ready_to_run',
                            RUNNING: 'running'
                        }},
                        getDetails: function() {{ return null; }},
                        getIsInstalled: function() {{ return false; }}
                    }};
                }}

                // 8. navigator.languages — align with the configured locale
                // e.g. locale "en-US" → ["en-US", "en"]
                (function() {{
                    const loc = '{locale}';
                    const base = loc.split('-')[0];
                    const langs = loc === base ? [loc] : [loc, base];
                    Object.defineProperty(Navigator.prototype, 'language', {{
                        get: () => loc,
                        configurable: true
                    }});
                    Object.defineProperty(Navigator.prototype, 'languages', {{
                        get: () => Object.freeze(langs),
                        configurable: true
                    }});
                }})();

                // 9. navigator.permissions.query — return same state as Notification.permission
                // Inconsistency between the two is a known bot-detection signal.
                if (window.navigator.permissions) {{
                    const _origQuery = window.navigator.permissions.query.bind(window.navigator.permissions);
                    Object.defineProperty(window.navigator.permissions.__proto__, 'query', {{
                        value: function query(parameters) {{
                            if (parameters && parameters.name === 'notifications') {{
                                let state;
                                try {{ state = Notification.permission; }} catch (_) {{ state = 'default'; }}
                                return Promise.resolve({{ state: state || 'default', onchange: null }});
                            }}
                            return _origQuery(parameters);
                        }},
                        configurable: true,
                        writable: true
                    }});
                }}

                // 10. navigator.serviceWorker.register — no-op to prevent detection
                // (patchright patch: service worker registration is a fingerprinting vector)
                if (navigator.serviceWorker) {{
                    Object.defineProperty(navigator.serviceWorker, 'register', {{
                        value: async function() {{ return Promise.resolve(); }},
                        configurable: true,
                        writable: true
                    }});
                }}

                // 11. AudioContext 指纹对抗
                // 问题：headless/服务器环境无真实音频设备，AudioContext 用软件实现，
                // 产生的样本哈希与真实桌面不同（CreepJS/FingerprintJS/DataDome 检测点）。
                // 解法：对 AnalyserNode.getFloatFrequencyData 和 AudioBuffer.getChannelData
                // 注入确定性微小噪声。噪声基于固定种子，同一会话内稳定一致，避免 Castle
                // 噪声检测（多次采样比对随机性）识破。注意：噪声幅度极小（1e-7 级），
                // 不影响音频实际播放，只改变指纹哈希。
                // 严禁用 ES 模板字面量（反引号+美元花括号），下方 Worker 注入会把它
                // 嵌进反引号模板，里面的插值会被 Worker 侧求值报 ReferenceError。
                (function() {{
                    // 确定性种子：基于 UA 字符串哈希（Rust 侧预算），保证同一会话内所有
                    // 文档、跨导航噪声一致（避免 Castle 噪声检测：多次采样比对发现噪声
                    // 每次变化）。不同 profile（不同 UA）种子不同，避免全网 stealth 库
                    // 指纹相同。
                    var seed = {audio_seed};
                    function noise(index) {{
                        // 简单确定性 PRNG（mulberry32 变体），种子固定则输出固定
                        var t = (seed + index * 2654435761) | 0;
                        t = (t ^ (t >>> 15)) * (t | 1);
                        t ^= t + (t << 7) | (t >>> 9);
                        return ((t & 0xffffff) / 0x1000000 - 0.5) * 1e-7;
                    }}
                    if (typeof AnalyserNode !== 'undefined') {{
                        var origGetFloat = AnalyserNode.prototype.getFloatFrequencyData;
                        AnalyserNode.prototype.getFloatFrequencyData = function(array) {{
                            origGetFloat.apply(this, arguments);
                            for (var i = 0; i < array.length; i++) {{
                                array[i] += noise(i);
                            }}
                        }};
                    }}
                    // AudioBuffer.getChannelData：指纹脚本常用 OfflineAudioContext 渲染后读取。
                    // 加噪会改变返回的 Float32Array，但不影响离线渲染（离线 context 不实时播放）。
                    // 实时 context 极少调 getChannelData（用 AudioBufferSourceNode），风险低。
                    if (typeof AudioBuffer !== 'undefined' && AudioBuffer.prototype.getChannelData) {{
                        var origGetChannel = AudioBuffer.prototype.getChannelData;
                        AudioBuffer.prototype.getChannelData = function(channel) {{
                            var data = origGetChannel.apply(this, arguments);
                            var noisy = new Float32Array(data.length);
                            for (var i = 0; i < data.length; i++) {{
                                noisy[i] = data[i] + noise(i);
                            }}
                            return noisy;
                        }};
                    }}
                }})();

                // 12. Canvas 2D 指纹对抗（稳定噪声）
                // 问题：headless 用软件渲染（SwiftShader/llvmpipe），2D canvas 哈希与真实
                // GPU 不同。反爬用 toDataURL/getImageData 读哈希比对。
                // 解法：对 toDataURL / getImageData 注入确定性噪声（同 AudioContext 的
                // mulberry32 种子思路），噪声极小（单像素 ±1）不影响视觉但改变哈希。
                (function() {{
                    var seed = {audio_seed};
                    function noise(i) {{
                        var t = (seed + i * 2654435761) | 0;
                        t = (t ^ (t >>> 15)) * (t | 1);
                        t ^= t + (t << 7) | (t >>> 9);
                        return (t & 0x3) - 1; // -1..1
                    }}
                    if (typeof HTMLCanvasElement !== 'undefined') {{
                        var origToDataURL = HTMLCanvasElement.prototype.toDataURL;
                        HTMLCanvasElement.prototype.toDataURL = function() {{
                            // 用临时 canvas 加噪，绝不修改原 canvas（否则多次调用会叠加噪声，
                            // 导致 toDataURL() !== toDataURL()，被反爬噪声检测识破）。
                            // CORS 污染的 canvas（getImageData 抛 SecurityError）回退到原方法。
                            try {{
                                var w = this.width, h = this.height;
                                var tmp = document.createElement('canvas');
                                tmp.width = w; tmp.height = h;
                                var tctx = tmp.getContext('2d');
                                tctx.drawImage(this, 0, 0);
                                var img = tctx.getImageData(0, 0, w, h);
                                for (var i = 0; i < img.data.length; i += 4) {{
                                    img.data[i] = (img.data[i] + noise(i)) & 0xff;
                                }}
                                tctx.putImageData(img, 0, 0);
                                return origToDataURL.apply(tmp, arguments);
                            }} catch (_) {{
                                return origToDataURL.apply(this, arguments);
                            }}
                        }};
                    }}
                }})();

                // 13. navigator.connection 伪造
                // 问题：headless/服务器环境的 NetworkInformation 值异常（downlink 高、
                // rtt 接近 0 且从不变化），FingerprintJS 用于一致性校验。
                // 解法：注入合理的、会微小波动的 connection 对象。
                if (navigator.connection === undefined) {{
                    var conn = {{
                        effectiveType: '4g',
                        rtt: 50 + (Math.random() * 30 | 0),
                        downlink: 5 + Math.random() * 3,
                        saveData: false,
                        addEventListener: function() {{}},
                        removeEventListener: function() {{}}
                    }};
                    try {{
                        Object.defineProperty(navigator, 'connection', {{
                            get: function() {{ return conn; }},
                            configurable: true
                        }});
                    }} catch (_) {{}}
                }}

                // 14. speechSynthesis voices 伪造
                // 问题：headless Chrome（尤其 Linux 服务器）的 speechSynthesis.getVoices()
                // 返回空数组，是经典 headless 信号。
                // 解法：伪造一个与 Windows UA 匹配的 voices 列表。
                if (window.speechSynthesis) {{
                    // 始终返回伪造的稳定 voices 列表（缓存引用，getVoices() === getVoices() 为 true）。
                    // headless 环境真实 voices 可能延迟加载且为空，统一返回伪造列表更可靠，
                    // 避免不同调用返回不同数组（空 vs fake vs real）被指纹识别。
                    var fakeVoices = [
                        {{name: 'Microsoft David Desktop - English (United States)', lang: 'en-US', localService: true, default: true, voiceURI: 'Microsoft David Desktop - English (United States)'}},
                        {{name: 'Microsoft Zira Desktop - English (United States)', lang: 'en-US', localService: true, default: false, voiceURI: 'Microsoft Zira Desktop - English (United States)'}},
                        {{name: 'Google US English', lang: 'en-US', localService: false, default: false, voiceURI: 'Google US English'}}
                    ];
                    window.speechSynthesis.getVoices = function() {{
                        return fakeVoices;
                    }};
                }}
            }})();
        "#,
            ua = self.user_agent(),
            platform = self.os.platform(),
            cores = self.cpu_cores,
            device_memory = self.device_memory_value(),
            webgl_vendor = self.gpu.vendor(),
            webgl_renderer = self.gpu.renderer(),
            // AudioContext/Canvas 噪声的确定性种子：基于 UA 的简单哈希（i32），
            // 保证同 profile 跨文档/跨导航噪声一致（抗 Castle 噪声检测）。
            audio_seed = self.user_agent().bytes().fold(0i32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as i32)),
            locale = self.locale,
            ua_data_block = if self.native_ua_data {
                String::new()
            } else {
                format!(
                    r#"// 4. Client Hints (only for non-native profiles — native profiles use the
                // browser's real Sec-CH-UA so HTTP headers and JS stay consistent)
                Object.defineProperty(Navigator.prototype, 'userAgentData', {{
                    get: () => ({{
                        brands: [
                            {{ brand: "Google Chrome", version: "{cv}" }},
                            {{ brand: "Chromium", version: "{cv}" }},
                            {{ brand: "Not=A?Brand", version: "24" }}
                        ],
                        mobile: false,
                        platform: "{hp}"
                    }}),
                    configurable: true
                }});

                Object.defineProperty(Navigator.prototype.userAgentData.__proto__, 'getHighEntropyValues', {{
                    value: async function(hints) {{
                        const values = {{}};
                        for (const hint of hints) {{
                            if (hint === 'platform') values.platform = "{hp}";
                            else if (hint === 'platformVersion') values.platformVersion = "{pv}";
                            else if (hint === 'architecture') values.architecture = "{arch}";
                            else if (hint === 'model') values.model = "";
                            else if (hint === 'bitness') values.bitness = "64";
                            else if (hint === 'uaFullVersion') values.uaFullVersion = "{cv}.0.0.0";
                        }}
                        return values;
                    }},
                    configurable: true
                }});"#,
                    cv = self.chrome_version,
                    hp = self.os.hints_platform(),
                    pv = self.os.platform_version(),
                    arch = self.os.architecture(),
                )
            },
        );

        // 11. toString() 深度对抗（针对 CreepJS 级检测）
        // 问题：上面 patch 的函数（getParameter / canPlayType / connect / query /
        // register 等）被替换成 JS 函数后，其 toString() 不再返回
        // `function name() { [native code] }`，反爬用 Function.prototype.toString
        // 直接检查即可识别。
        // 解法：维护一个 WeakMap，记录"被 patch 的函数 → 应伪装的原生 toString 字符串"，
        // 重写 Function.prototype.toString，命中时返回伪造串，未命中走原生 toString。
        // 同时 patch `toString` 本身的 toString，避免它自己暴露。
        // 注意：本块严禁使用 ES 模板字面量（反引号 + ${}），因为下方 Worker 注入
        // 会把整段 script 嵌进反引号模板 `${script}`，里面的 ${} 会被 Worker 侧
        // 当成插值求值而报 ReferenceError，从而破坏所有 Worker。
        script.push_str(
            r#"
            // === zycdp toString 深度对抗 ===
            (function() {
                var nativeToStringFn = Function.prototype.toString;
                var NATIVE_BODY = '{ [native code] }';
                // 存储被 patch 函数 -> 应返回的伪造 toString
                var patched = new WeakMap();
                // 把 fn 注册为"看起来是原生函数"。name 为应伪装的方法名。
                function maskAsNative(fn, name) {
                    if (typeof fn === 'function') {
                        patched.set(fn, 'function ' + name + '() ' + NATIVE_BODY);
                    }
                    return fn;
                }

                // === 注册所有上面被 patch 的函数 ===
                // Navigator.prototype getters（platform/hardwareConcurrency/deviceMemory/
                // maxTouchPoints/language/languages/userAgentData/webdriver）
                try {
                    var navProto = Navigator.prototype;
                    var navProps = ['platform','hardwareConcurrency','deviceMemory',
                        'maxTouchPoints','language','languages','userAgentData','webdriver'];
                    for (var i = 0; i < navProps.length; i++) {
                        var d = Object.getOwnPropertyDescriptor(navProto, navProps[i]);
                        if (d && typeof d.get === 'function') maskAsNative(d.get, 'get ' + navProps[i]);
                    }
                } catch (_) {}

                // WebGL getParameter（WebGL / WebGL2 两套 prototype）
                try {
                    var Ctors = [];
                    if (typeof WebGLRenderingContext !== 'undefined') Ctors.push(WebGLRenderingContext);
                    if (typeof WebGL2RenderingContext !== 'undefined') Ctors.push(WebGL2RenderingContext);
                    for (var i = 0; i < Ctors.length; i++) {
                        maskAsNative(Ctors[i].prototype.getParameter, 'getParameter');
                    }
                } catch (_) {}

                try { maskAsNative(HTMLMediaElement.prototype.canPlayType, 'canPlayType'); } catch (_) {}

                try {
                    if (window.navigator.permissions) {
                        maskAsNative(window.navigator.permissions.__proto__.query, 'query');
                    }
                } catch (_) {}

                try {
                    if (navigator.serviceWorker) maskAsNative(navigator.serviceWorker.register, 'register');
                } catch (_) {}

                // AudioContext 对抗 patch 的函数
                try {
                    if (typeof AnalyserNode !== 'undefined') {
                        maskAsNative(AnalyserNode.prototype.getFloatFrequencyData, 'getFloatFrequencyData');
                    }
                    if (typeof AudioBuffer !== 'undefined' && AudioBuffer.prototype.getChannelData) {
                        maskAsNative(AudioBuffer.prototype.getChannelData, 'getChannelData');
                    }
                } catch (_) {}

                // Canvas / speechSynthesis 对抗 patch 的函数
                try {
                    if (typeof HTMLCanvasElement !== 'undefined' && HTMLCanvasElement.prototype.toDataURL) {
                        maskAsNative(HTMLCanvasElement.prototype.toDataURL, 'toDataURL');
                    }
                    if (window.speechSynthesis && window.speechSynthesis.getVoices) {
                        maskAsNative(window.speechSynthesis.getVoices, 'getVoices');
                    }
                } catch (_) {}

                try {
                    if (window.chrome) {
                        var rt = window.chrome.runtime;
                        if (rt) {
                            maskAsNative(rt.connect, 'connect');
                            maskAsNative(rt.sendMessage, 'sendMessage');
                        }
                        if (window.chrome.csi) maskAsNative(window.chrome.csi, 'csi');
                        if (window.chrome.loadTimes) maskAsNative(window.chrome.loadTimes, 'loadTimes');
                    }
                } catch (_) {}

                try {
                    var uad = Navigator.prototype.userAgentData;
                    if (uad && uad.__proto__ && typeof uad.__proto__.getHighEntropyValues === 'function') {
                        maskAsNative(uad.__proto__.getHighEntropyValues, 'getHighEntropyValues');
                    }
                } catch (_) {}

                // Error.prepareStackTrace 的 get/set（上面 bootstrap 定义在 Error 上）
                try {
                    var esd = Object.getOwnPropertyDescriptor(Error, 'prepareStackTrace');
                    if (esd) {
                        if (typeof esd.get === 'function') maskAsNative(esd.get, 'get prepareStackTrace');
                        if (typeof esd.set === 'function') maskAsNative(esd.set, 'set prepareStackTrace');
                    }
                } catch (_) {}

                // === 重写 Function.prototype.toString ===
                var toStringProxy = function toString() {
                    if (patched.has(this)) {
                        return patched.get(this);
                    }
                    return nativeToStringFn.call(this);
                };
                // 让 toString 本身看起来也像原生（防 toString.toString() 检测）
                patched.set(toStringProxy, 'function toString() ' + NATIVE_BODY);
                Function.prototype.toString = toStringProxy;
            })();
"#,
        );

        // Prevent CDP detection via worker threads
        let worker_script = format!(
            r#"
                const OriginalWorker = Worker;
                window.Worker = function (url, options) {{
                
                    const injectedCode = `{script}`
                    const workerPromise = fetch(url)
                        .then((res) => res.text())
                        .then((code) => {{
                            const blob = new Blob([injectedCode + code], {{
                                type: "application/javascript",
                            }});
                            return new OriginalWorker(URL.createObjectURL(blob), options);
                        }});

                    
                        let realWorker = null;
                        const pendingMessages = [];
                        workerPromise.then((w) => {{
                            realWorker = w;
                            pendingMessages.forEach((msg) => w.postMessage(msg));
                        }});
                        return {{
                            postMessage(msg) {{
                            if (realWorker) {{
                                realWorker.postMessage(msg);
                            }} else {{
                                pendingMessages.push(msg);
                            }}
                        }},
                            set onmessage(fn) {{
                                workerPromise.then((w) => (w.onmessage = fn));
                            }},
                            terminate() {{
                                workerPromise.then((w) => w.terminate());
                            }},
                        }};
                }};
            "#,
            script = script
        );

        script.push_str(&worker_script);
        script
    }

    /// Minimal bootstrap for native profiles (`native_ua_data = true`).
    ///
    /// Native profiles use the browser's real UA, Sec-CH-UA, platform, and navigator
    /// properties — overriding them with JS would make them non-native (detectable via
    /// `toString()`). This script only does two safe things:
    ///   1. Deletes any CDP automation markers left by ChromeDriver/Selenium.
    ///   2. Stubs `chrome.runtime` APIs that Chromium on Linux may not expose but that
    ///      some sites require (e.g. `chrome.runtime.connect`).
    fn native_bootstrap_script() -> String {
        // For native profiles, return empty — the browser's own properties are
        // already correct and any JS injection risks non-native toString() detection.
        String::new()
    }
}

impl fmt::Display for ChaserProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ChaserProfile({:?}, Chrome {}, {:?})",
            self.os, self.chrome_version, self.gpu
        )
    }
}

/// Builder for constructing `ChaserProfile` instances
#[derive(Debug, Clone)]
pub struct ChaserProfileBuilder {
    os: Os,
    chrome_version: u32,
    gpu: Gpu,
    memory_gb: u32,
    cpu_cores: u32,
    locale: String,
    timezone: String,
    screen_width: u32,
    screen_height: u32,
    native_ua_data: bool,
}

impl ChaserProfileBuilder {
    /// Set the Chrome version (default: 129)
    pub fn chrome_version(mut self, version: u32) -> Self {
        self.chrome_version = version;
        self
    }

    /// Set the GPU for WebGL spoofing
    pub fn gpu(mut self, gpu: Gpu) -> Self {
        self.gpu = gpu;
        self
    }

    /// Set device memory in GB (default: 8)
    pub fn memory_gb(mut self, gb: u32) -> Self {
        self.memory_gb = gb;
        self
    }

    /// Set CPU core count (default: 8)
    pub fn cpu_cores(mut self, cores: u32) -> Self {
        self.cpu_cores = cores;
        self
    }

    /// Set the locale (e.g., "en-US", "de-DE")
    pub fn locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = locale.into();
        self
    }

    /// Set the timezone (e.g., "America/New_York", "Europe/Berlin")
    pub fn timezone(mut self, tz: impl Into<String>) -> Self {
        self.timezone = tz.into();
        self
    }

    /// Set screen resolution
    pub fn screen(mut self, width: u32, height: u32) -> Self {
        self.screen_width = width;
        self.screen_height = height;
        self
    }

    /// Build the final profile
    /// Skip overriding navigator.userAgentData in JS and Sec-CH-UA metadata in
    /// CDP. Use for native profiles where the browser's own values are correct.
    pub fn native_ua_data(mut self, native: bool) -> Self {
        self.native_ua_data = native;
        self
    }

    pub fn build(self) -> ChaserProfile {
        ChaserProfile {
            os: self.os,
            chrome_version: self.chrome_version,
            gpu: self.gpu,
            memory_gb: self.memory_gb,
            cpu_cores: self.cpu_cores,
            locale: self.locale,
            timezone: self.timezone,
            screen_width: self.screen_width,
            screen_height: self.screen_height,
            native_ua_data: self.native_ua_data,
        }
    }
}

/// Detect the host OS at runtime (distinguishes ARM vs Intel on macOS).
pub fn detect_current_os() -> Os {
    #[cfg(target_os = "macos")]
    {
        let is_arm = std::process::Command::new("uname")
            .arg("-m")
            .output()
            .map(|o| {
                let arch = String::from_utf8_lossy(&o.stdout);
                arch.contains("arm64") || arch.contains("aarch64")
            })
            .unwrap_or(false);
        if is_arm { Os::MacOSArm } else { Os::MacOSIntel }
    }
    #[cfg(target_os = "windows")]
    {
        Os::Windows
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Os::Linux
    }
}

/// Try to detect the installed Chrome major version from the system binary.
/// Returns `None` if Chrome cannot be found or its version parsed.
pub fn detect_chrome_version() -> Option<u32> {
    let path = which::which("google-chrome")
        .or_else(|_| which::which("chromium-browser"))
        .or_else(|_| which::which("chromium"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned());

    #[cfg(target_os = "macos")]
    let path = path.or_else(|| {
        [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|s| s.to_string())
    });

    let path = path?;
    std::process::Command::new(&path)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            s.split_whitespace()
                .find(|part| part.starts_with(|c: char| c.is_ascii_digit()))
                .and_then(|v| v.split('.').next())
                .and_then(|major| major.parse().ok())
        })
}

/// Read total system RAM in GB (capped at 8 to match `navigator.deviceMemory` spec max).
pub fn detect_system_memory_gb() -> u32 {
    let gb = _read_system_memory_gb();
    gb.min(8)
}

#[cfg(target_os = "macos")]
fn _read_system_memory_gb() -> u32 {
    std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|bytes| (bytes / (1024 * 1024 * 1024)) as u32)
        .unwrap_or(8)
}

#[cfg(target_os = "linux")]
fn _read_system_memory_gb() -> u32 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        })
        .map(|kb| (kb / (1024 * 1024)) as u32)
        .unwrap_or(8)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn _read_system_memory_gb() -> u32 {
    // 通过 GlobalMemoryStatusEx 读取真实物理内存（此前硬编码返回 8，破坏
    // native 模式一致性，见 docs/05-defects-baseline.md D1）。
    use windows_sys::Win32::Foundation::BOOL;
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    // dwLength 必须在调用前设为结构体大小，否则 GlobalMemoryStatusEx 会失败。
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        dwMemoryLoad: 0,
        ullTotalPhys: 0,
        ullAvailPhys: 0,
        ullTotalPageFile: 0,
        ullAvailPageFile: 0,
        ullTotalVirtual: 0,
        ullAvailVirtual: 0,
        ullAvailExtendedVirtual: 0,
    };

    // SAFETY: 传入本机结构体指针，dwLength 已正确设置。函数只写入 status，
    // 不持有指针，无并发风险。
    let ok: BOOL = unsafe { GlobalMemoryStatusEx(&mut status) };
    if ok == 0 {
        return 8; // 探测失败兜底（理论上几乎不会发生）
    }
    (status.ullTotalPhys / (1024 * 1024 * 1024)) as u32
}

// Re-export the old trait-based system for backwards compatibility
pub use crate::stealth::{LinuxProfile, MacOSProfile, StealthProfile, WindowsNvidiaProfile};
