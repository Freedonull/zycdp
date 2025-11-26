use chromiumoxide_fetcher::{
    BrowserFetcherOptions, BrowserHost, BrowserKind, BrowserVersion, BuildInfo, Platform, Revision,
};
use reqwest::{IntoUrl, Response, StatusCode};

pub async fn head<T: IntoUrl>(url: T) -> reqwest::Result<Response> {
    reqwest::Client::builder().build()?.head(url).send().await
}

// Check if the chosen revision has a build available for all platforms.
// That not always the case, that is why we need to make sure of it.
#[tokio::test]
async fn verify_chromium_revision_available() {
    let host = BrowserHost::current(BrowserKind::Chromium);
    let BrowserVersion::Revision(revision) = BrowserVersion::current(BrowserKind::Chromium) else {
        panic!("Chromium revision is not available");
    };
    let build_info = BuildInfo::revision(revision);
    for platform in Platform::all() {
        let res = head(&BrowserKind::Chromium.download_url(*platform, &build_info, &host))
            .await
            .unwrap();

        if res.status() != StatusCode::OK {
            panic!("Revision {} is not available for {:?}", revision, platform);
        }
    }
}

#[ignore]
#[tokio::test]
async fn find_chromium_revision_available() {
    let min = 1520176; // Enter the minimum revision
    let max = 1520176; // Enter the maximum revision

    let host = BrowserHost::current(BrowserKind::Chromium);
    'outer: for revision in (min..max).rev() {
        println!("Checking revision {}", revision);

        let build_info = BuildInfo::revision(Revision::from(revision));
        for platform in Platform::all() {
            let res = head(&BrowserKind::Chromium.download_url(*platform, &build_info, &host))
                .await
                .unwrap();

            if res.status() != StatusCode::OK {
                println!("Revision {} is not available for {:?}", revision, platform);
                continue 'outer;
            }
        }

        println!("Found revision {}", revision);
        break;
    }
}

#[ignore]
#[tokio::test]
async fn download_chromium_revision() {
    let path = "./.cache";

    tokio::fs::create_dir(path).await.unwrap();

    for platform in Platform::all() {
        let revision = chromiumoxide_fetcher::BrowserFetcher::new(
            BrowserFetcherOptions::builder()
                .with_kind(BrowserKind::Chromium)
                .with_path(path)
                .with_platform(*platform)
                .build()
                .unwrap(),
        )
        .fetch()
        .await
        .unwrap();

        println!("Downloaded revision {} for {:?}", revision, platform);
    }
}
