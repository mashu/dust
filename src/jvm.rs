//! Android's Java, asked from any of the app's own threads: the activity
//! tao keeps for the app, asked directly, with the thread attached to the
//! VM for the call, rather than waited on through the activity's thread.
//! What asks it: where the app keeps its files and its backups
//! (src/persist/android.rs), and the sign-in to Google
//! (src/sync/auth/android.rs).

use dioxus::mobile::wry::prelude::{JNIEnv, JObject, jni};

/// Do `call` with the VM attached to this thread, and the activity: what
/// it gives, or why the VM could not be reached. A Java exception the call
/// leaves pending is cleared after it: one left pending would fail every
/// JNI call after it.
pub fn attached<T>(call: impl FnOnce(&mut JNIEnv, &JObject) -> T) -> jni::errors::Result<T> {
    let android = ndk_context::android_context();
    // Safety: tao hands ndk-context the VM, and a global reference to the
    // activity, before the app's `main` runs, and the activity lives as
    // long as the app does.
    let vm = unsafe { jni::JavaVM::from_raw(android.vm().cast()) }?;
    let mut env = vm.attach_current_thread()?;
    // Safety: the same global reference, used as the object it refers to.
    let activity = unsafe { JObject::from_raw(android.context().cast()) };
    let done = call(&mut env, &activity);
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    Ok(done)
}

/// [`attached`], with every reference the call makes on the way let go of
/// as it returns, with a local frame of four: a thread that stays attached
/// keeps what it made otherwise. What the call gives, or why not.
pub fn with_activity<T>(
    call: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<T>,
) -> jni::errors::Result<T> {
    attached(|env, activity| env.with_local_frame(4, |env| call(env, activity)))?
}
