use crate::BrowserKind;

/// Host for downloading browsers and metadata.
pub struct BrowserHost {
    pub(crate) object: String,
    pub(crate) metadata: String,
}

impl BrowserHost {
    pub fn new(object: String, metadata: String) -> Self {
        Self { object, metadata }
    }

    pub(crate) fn current(kind: BrowserKind) -> Self {
        match kind {
            BrowserKind::Chromium => Self {
                object: "https://storage.googleapis.com".to_string(),
                metadata: "https://storage.googleapis.com".to_string(),
            },
            BrowserKind::Chrome | BrowserKind::ChromeHeadlessShell => Self {
                object: "https://storage.googleapis.com".to_string(),
                metadata: "https://googlechromelabs.github.io/".to_string(),
            },
        }
    }
}
