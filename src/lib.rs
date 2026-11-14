#[cfg(target_os = "android")]
mod android;
pub mod collector;
pub mod discovery;
pub mod engine;
pub mod model;
pub mod storage;
pub mod ui;
#[cfg(not(target_os = "android"))]
pub mod updater;
#[cfg(target_os = "android")]
#[path = "updater_android.rs"]
pub mod updater;

#[cfg(any(target_os = "android", test))]
mod text_input;
