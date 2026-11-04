//! Which Google Cloud project's OAuth clients this build signs in with,
//! given as the build is made: none in a build made without them, which then
//! offers no sync at all (docs/sync.md, Setting up).
//!
//! A client ID is no secret: it names the app to Google's sign-in, in every
//! request a browser or a device makes, and a desktop client's secret is
//! no secret either, as Google says of every app installed on a computer.
//! They are given to the build rather than written here only so that a
//! fork signs in as itself, under a project of its own, or not at all.

/// The scope sync asks for: Drive's app data folder, a folder of Dust's own
/// that neither the user's files nor any other app's can be reached from,
/// and that no other app can reach. Google counts it non-sensitive.
#[cfg_attr(
    not(any(feature = "web", feature = "desktop", target_os = "android", test)),
    allow(dead_code)
)]
pub const SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";

/// `value`, unless it is empty, as a variable set but left blank is.
const fn given(value: Option<&'static str>) -> Option<&'static str> {
    match value {
        Some(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

/// The web client's ID, for the browser's sign-in.
pub const WEB_CLIENT_ID: Option<&str> = given(option_env!("DUST_GOOGLE_WEB_CLIENT_ID"));
/// The desktop client's ID and secret, for a computer's sign-in.
pub const DESKTOP_CLIENT_ID: Option<&str> = given(option_env!("DUST_GOOGLE_DESKTOP_CLIENT_ID"));
pub const DESKTOP_CLIENT_SECRET: Option<&str> =
    given(option_env!("DUST_GOOGLE_DESKTOP_CLIENT_SECRET"));
/// Android signs in through Google Play services, which knows the app by
/// its package and signing key, with no ID in the app: this says only that
/// the project has an Android client for them.
pub const ANDROID_CLIENT: bool = given(option_env!("DUST_GOOGLE_ANDROID_SYNC")).is_some();

/// Whether this build can sync at all: built with a client for where it
/// runs. iOS has none yet.
pub const fn available() -> bool {
    if cfg!(feature = "web") {
        WEB_CLIENT_ID.is_some()
    } else if cfg!(target_os = "android") {
        ANDROID_CLIENT
    } else if cfg!(target_os = "ios") {
        false
    } else {
        cfg!(feature = "desktop") && DESKTOP_CLIENT_ID.is_some() && DESKTOP_CLIENT_SECRET.is_some()
    }
}
