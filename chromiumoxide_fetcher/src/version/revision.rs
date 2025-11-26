use std::{fmt, str::FromStr};

use super::metadata::KnownGoodVersions;
use super::{Result, VersionError};
use crate::{BrowserHost, BrowserKind, BuildInfo, Runtime};

/// A [`Revision`] represents a version of chromium.
///
/// The revision must be compatible with the Chrome DevTools Protocol (CDP)
/// shipped with chromiumoxide otherwise it will fail to communicate with
/// the browser.
#[derive(Clone, Copy, Debug, PartialOrd, Ord, PartialEq, Eq)]
pub struct Revision(u32);

impl Revision {
    pub const fn new(revision: u32) -> Self {
        Self(revision)
    }

    pub(crate) async fn resolve(&self, kind: BrowserKind, host: &BrowserHost) -> Result<BuildInfo> {
        match kind {
            BrowserKind::Chromium => Ok(BuildInfo::revision(*self)),
            BrowserKind::Chrome | BrowserKind::ChromeHeadlessShell => {
                // We do our best to try to match the revision to a known good version.
                let url = format!(
                    "{host}/chrome-for-testing/known-good-versions.json",
                    host = host.metadata
                );
                let revision_str = self.to_string();
                let known_good_versions = Runtime::download_json::<KnownGoodVersions>(&url)
                    .await
                    .map_err(VersionError::ResolveFailed)?;
                let Some(version) = known_good_versions
                    .versions
                    .iter()
                    .find(|version| version.revision == revision_str)
                else {
                    return Err(VersionError::InvalidRevision(self.to_string()));
                };
                Ok(BuildInfo::both(
                    version.version.clone(),
                    version.revision.parse()?,
                ))
            }
        }
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Revision {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u32>()
            .map(Self)
            .map_err(|_| VersionError::InvalidRevision(s.to_string()))
    }
}

impl TryFrom<String> for Revision {
    type Error = VersionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<u32> for Revision {
    fn from(value: u32) -> Self {
        Self(value)
    }
}
