#[cfg(feature = "zip-0_6")]
mod zip_0_6;
#[cfg(feature = "zip-0_6")]
pub use zip_0_2::ZipArchive;

#[cfg(feature = "zip-8")]
mod zip_8;
#[cfg(feature = "zip-8")]
pub use zip_8::ZipArchive;
