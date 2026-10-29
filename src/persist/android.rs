//! Saving a backup file on Android, where the Files app and every other app
//! can reach it: the shared Downloads folder.
//!
//! From Android 10 an app adds a file to Downloads through the media store,
//! without asking for any permission. Before that, writing there takes a
//! permission the app does not hold, so the file goes to the app's own
//! folder on the shared storage, which file managers of those versions show.
//! Either way the work is Java's, on the activity's thread, so the result
//! comes back later.

use std::path::PathBuf;

use dioxus::mobile::wry::prelude::{JNIEnv, JObject, dispatch, jni};
use jni::objects::{JString, JValue};

use super::BackupSaving;
use super::desktop::{free_name, write_whole};

/// `Build.VERSION_CODES.Q`, Android 10: the first that adds to Downloads
/// through the media store.
const ANDROID_10: i32 = 29;
/// `Environment.DIRECTORY_DOWNLOADS`.
const DOWNLOADS: &str = "Download";

/// Save `contents` as `name` where the user can reach it, and say where.
pub fn save_backup(name: String, contents: String) -> BackupSaving {
    let (sent, saved) = tokio::sync::oneshot::channel();
    dispatch(move |env, activity, _webview| {
        let saved = save(env, activity, &name, contents.as_bytes());
        // A Java exception left pending would fail every JNI call after it,
        // the webview's own among them.
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        let _ = sent.send(saved);
    });
    Box::pin(async move {
        saved
            .await
            .unwrap_or_else(|_| Err("the app closed before it was saved".into()))
    })
}

fn save(env: &mut JNIEnv, activity: &JObject, name: &str, bytes: &[u8]) -> Result<String, String> {
    let version = env
        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
        .and_then(|value| value.i())
        .map_err(|err| err.to_string())?;
    if version >= ANDROID_10 {
        into_downloads(env, activity, name, bytes).map_err(|err| err.to_string())?;
        return Ok(format!("{name}, in Downloads"));
    }
    let folder = app_downloads(env, activity).map_err(|err| err.to_string())?;
    let name = free_name(&folder, name);
    write_whole(&folder, &name, bytes).map_err(|err| err.to_string())?;
    Ok(folder.join(name).display().to_string())
}

/// Add a file to Downloads through the media store. One already called
/// `name` is kept; the media store names the new one apart.
fn into_downloads(
    env: &mut JNIEnv,
    activity: &JObject,
    name: &str,
    bytes: &[u8],
) -> jni::errors::Result<()> {
    let resolver = env
        .call_method(
            activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()?;
    let values = env.new_object("android/content/ContentValues", "()V", &[])?;
    // `MediaStore.MediaColumns`' DISPLAY_NAME, MIME_TYPE and RELATIVE_PATH.
    for (column, value) in [
        ("_display_name", name),
        ("mime_type", "application/json"),
        ("relative_path", DOWNLOADS),
    ] {
        let column = env.new_string(column)?;
        let value = env.new_string(value)?;
        env.call_method(
            &values,
            "put",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&column), JValue::Object(&value)],
        )?;
    }
    let downloads = env
        .get_static_field(
            "android/provider/MediaStore$Downloads",
            "EXTERNAL_CONTENT_URI",
            "Landroid/net/Uri;",
        )?
        .l()?;
    let file = env
        .call_method(
            &resolver,
            "insert",
            "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;",
            &[JValue::Object(&downloads), JValue::Object(&values)],
        )?
        .l()?;
    if file.is_null() {
        return Err(jni::errors::Error::NullPtr("the media store made no file"));
    }
    let written = write_into(env, &resolver, &file, bytes);
    if written.is_err() {
        // Nothing half written is left in Downloads. A Java exception the
        // write left pending is cleared first: every call after it would
        // fail too.
        let _ = env.exception_clear();
        let _ = env.call_method(
            &resolver,
            "delete",
            "(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I",
            &[
                JValue::Object(&file),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
            ],
        );
        let _ = env.exception_clear();
    }
    written
}

/// Write `bytes` into `file`, a file the media store made, and close it,
/// written or not.
fn write_into(
    env: &mut JNIEnv,
    resolver: &JObject,
    file: &JObject,
    bytes: &[u8],
) -> jni::errors::Result<()> {
    let stream = env
        .call_method(
            resolver,
            "openOutputStream",
            "(Landroid/net/Uri;)Ljava/io/OutputStream;",
            &[JValue::Object(file)],
        )?
        .l()?;
    if stream.is_null() {
        return Err(jni::errors::Error::NullPtr("the file could not be opened"));
    }
    let written = match env.byte_array_from_slice(bytes) {
        Ok(array) => env
            .call_method(&stream, "write", "([B)V", &[JValue::Object(&array)])
            .map(drop),
        Err(err) => Err(err),
    };
    if written.is_err() {
        let _ = env.exception_clear();
    }
    let closed = env.call_method(&stream, "close", "()V", &[]).map(drop);
    written.and(closed)
}

/// The app's own Downloads folder on the shared storage:
/// `Android/data/<package>/files/Download`.
fn app_downloads(env: &mut JNIEnv, activity: &JObject) -> jni::errors::Result<PathBuf> {
    let kind = env.new_string(DOWNLOADS)?;
    let folder = env
        .call_method(
            activity,
            "getExternalFilesDir",
            "(Ljava/lang/String;)Ljava/io/File;",
            &[JValue::Object(&kind)],
        )?
        .l()?;
    if folder.is_null() {
        return Err(jni::errors::Error::NullPtr(
            "the shared storage is not there",
        ));
    }
    let path: JString = env
        .call_method(&folder, "getAbsolutePath", "()Ljava/lang/String;", &[])?
        .l()?
        .into();
    let path: String = env.get_string(&path)?.into();
    Ok(PathBuf::from(path))
}
