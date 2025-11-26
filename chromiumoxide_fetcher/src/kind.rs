use crate::error::Result;
use crate::{BrowserHost, BrowserVersion, BuildInfo, Platform, Revision};

/// The kind of browser to download.
#[derive(Clone, Copy, Debug)]
pub enum BrowserKind {
    Chromium,
    Chrome,
    ChromeHeadlessShell,
}

impl Default for BrowserKind {
    fn default() -> Self {
        Self::Chromium
    }
}

impl BrowserKind {
    #[doc(hidden)] // internal API
    pub fn download_url(
        &self,
        build_info: BuildInfo,
        platform: Platform,
        host: &BrowserHost,
    ) -> String {
        let folder = self.folder(platform);
        let archive = self.archive(platform, build_info.revision);
        match self {
            Self::Chromium => {
                format!(
                    "{host}/chromium-browser-snapshots/{folder}/{build_id}/chrome-{archive}.zip",
                    host = host.object,
                    folder = folder,
                    build_id = build_info.id,
                )
            }
            Self::Chrome => {
                format!(
                    "{host}/chrome-for-testing/{build_id}/{folder}/chrome-{archive}.zip",
                    host = host.object,
                    build_id = build_info.id,
                    folder = folder,
                    archive = archive,
                )
            }
            Self::ChromeHeadlessShell => {
                format!(
                    "{host}/chrome-for-testing/{build_id}/{folder}/chrome-headless-shell-{archive}.zip",
                    host = host.object,
                    build_id = build_info.id,
                    folder = folder,
                    archive = archive,
                )
            }
        }
    }

    fn archive(&self, platform: Platform, revision: Revision) -> &'static str {
        const CHROMIUM_REVISION_WIN32: Revision = Revision::new(591_479);
        match self {
            Self::Chromium => match platform {
                Platform::Linux => "linux",
                Platform::Mac | Platform::MacArm => "mac",
                Platform::Win32 | Platform::Win64 => {
                    if revision > CHROMIUM_REVISION_WIN32 {
                        "win"
                    } else {
                        "win32"
                    }
                }
            },
            Self::Chrome | Self::ChromeHeadlessShell => match platform {
                Platform::Linux => "linux64",
                Platform::Mac => "mac-x64",
                Platform::MacArm => "mac-arm64",
                Platform::Win32 => "win32",
                Platform::Win64 => "win64",
            },
        }
    }

    pub(crate) fn executable(&self, folder_path: &Path, revision: &Revision) -> PathBuf {
        let mut path = folder_path.to_path_buf();
        path.push(self.archive_name(revision));
        match self {
            Self::Linux => path.push("chrome"),
            Self::Mac | Self::MacArm => {
                path.push("Chromium.app");
                path.push("Contents");
                path.push("MacOS");
                path.push("Chromium")
            }
            Self::Win32 | Self::Win64 => path.push("chrome.exe"),
        }
        path
    }

    pub(crate) fn folder(&self, platform: Platform) -> &'static str {
        match self {
            Self::Chromium => match platform {
                Platform::Linux => "Linux_x64",
                Platform::Mac => "Mac",
                Platform::MacArm => "Mac_Arm",
                Platform::Win32 => "Win",
                Platform::Win64 => "Win_x64",
            },
            Self::Chrome | Self::ChromeHeadlessShell => match platform {
                Platform::Linux => "linux64",
                Platform::Mac => "mac-x64",
                Platform::MacArm => "mac-arm64",
                Platform::Win32 => "win32",
                Platform::Win64 => "win64",
            },
        }
    }
}
