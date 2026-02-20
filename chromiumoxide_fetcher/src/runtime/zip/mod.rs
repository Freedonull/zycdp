#[cfg(feature = "zip-0_2")]
mod zip_0_2;
#[cfg(feature = "zip-0_2")]
pub use zip_0_2::ZipArchive;

#[cfg(feature = "zip-8")]
mod zip_8;
#[cfg(feature = "zip-8")]
pub use zip_8::ZipArchive;
