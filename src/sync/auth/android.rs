//! Signing in on Android: Google Play services' authorization, which asks
//! the user once, in a sheet of its own over the app, and gives a token
//! each time after with no one asked. Play services knows the app by its
//! package and its signing key, which Google's console ties to the project.
//!
//! The asking is the activity's (android/MainActivity.kt): it starts each
//! request, and answers it once Play services, or the user, has. The app
//! asks it over JNI and looks for the answer every so often, as nothing
//! here can be woken from Java: `dustDriveAuthorize` gives the request's
//! number, and `dustDriveAnswer` its answer once there is one, as a line of
//! text: `token <token>`, `needs-user`, `cancelled` or `error <why>`.

use dioxus::mobile::wry::prelude::{JObject, jni};
use jni::objects::{JString, JValue};

use super::{Auth, AuthError};
use crate::persist::record::Pending;

/// How often the answer is looked for.
const LOOK_MS: u32 = 150;

/// The sign-in, through the activity.
pub struct PlayAuth;

/// Do `call` with the VM attached to this thread and the activity: what it
/// gives, or why not.
fn with_activity<T>(
    call: impl FnOnce(&mut jni::JNIEnv, &JObject) -> jni::errors::Result<T>,
) -> Result<T, String> {
    let android = ndk_context::android_context();
    // Safety: tao hands ndk-context the VM, and a global reference to the
    // activity, before the app's `main` runs, and the activity lives as
    // long as the app does (src/persist/android.rs).
    let vm =
        unsafe { jni::JavaVM::from_raw(android.vm().cast()) }.map_err(|err| err.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|err| err.to_string())?;
    // Safety: the same global reference, used as the object it refers to.
    let activity = unsafe { JObject::from_raw(android.context().cast()) };
    let done = env.with_local_frame(4, |env| call(env, &activity));
    // A Java exception left pending would fail every JNI call after it.
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    done.map_err(|err| err.to_string())
}

/// Ask the activity for a token, `asking` the user where Play services
/// needs them to: the request's number.
fn start(asking: bool) -> Result<i64, String> {
    with_activity(|env, activity| {
        env.call_method(
            activity,
            "dustDriveAuthorize",
            "(Z)J",
            &[JValue::Bool(asking.into())],
        )?
        .j()
    })
}

/// The answer to the request `id`, once there is one.
fn answer(id: i64) -> Result<Option<String>, String> {
    with_activity(|env, activity| {
        let answer = env
            .call_method(
                activity,
                "dustDriveAnswer",
                "(J)Ljava/lang/String;",
                &[JValue::Long(id)],
            )?
            .l()?;
        if answer.is_null() {
            return Ok(None);
        }
        let answer = JString::from(answer);
        Ok(Some(env.get_string(&answer)?.into()))
    })
}

/// A token from Play services, `asking` the user where it needs them to.
fn authorize(asking: bool) -> Pending<Result<String, AuthError>> {
    Box::pin(async move {
        let id = start(asking).map_err(AuthError::Failed)?;
        loop {
            crate::time::sleep_ms(LOOK_MS).await;
            let Some(answer) = answer(id).map_err(AuthError::Failed)? else {
                continue;
            };
            return match answer.split_once(' ').unwrap_or((answer.as_str(), "")) {
                ("token", token) if !token.is_empty() => Ok(token.to_string()),
                ("needs-user", _) => Err(AuthError::NeedsUser),
                ("cancelled", _) => Err(AuthError::Cancelled),
                (_, why) => Err(AuthError::Failed(if why.is_empty() {
                    "Google Play services gave no token".into()
                } else {
                    format!("Google Play services said {why}")
                })),
            };
        }
    })
}

impl Auth for PlayAuth {
    /// Play services keeps the token and its renewal: each asked for is
    /// good, fresh or not.
    fn token(&self, _fresh: bool) -> Pending<Result<String, AuthError>> {
        authorize(false)
    }

    fn sign_in(&self) -> Pending<Result<(), AuthError>> {
        let asked = authorize(true);
        Box::pin(async move { asked.await.map(|_| ()) })
    }

    /// Play services keeps no token of the app's to forget: the access
    /// itself is the user's to take back, in their Google account.
    fn forget(&self) {}
}
