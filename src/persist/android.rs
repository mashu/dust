//! Saving a backup file on Android, where the Files app and every other app
//! can reach it: the shared Downloads folder. And where the app keeps its
//! own files ([`files_dir`]).
//!
//! From Android 10 an app adds a file to Downloads through the media store,
//! without asking for any permission. Before that, writing there takes a
//! permission the app does not hold, so the file goes to the app's own
//! folder on the shared storage, which file managers of those versions show.
//!
//! A backup is written on the history's own thread, a session at a time,
//! and never held whole: into the app's cache first, and from there copied
//! into Downloads by Android itself, from 10 on; straight into the app's
//! own folder before that.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use dioxus::mobile::wry::prelude::{JNIEnv, JObject, jni};
use jni::objects::{JString, JValue};

/// The folder Android gives the app for its own files, as the user it runs
/// as: `Context.getFilesDir()`. That is `/data/user/<user>/<package>/files`,
/// `/data/data/<package>/files` for the phone's owner only; a work profile,
/// another user, Private Space and a cloned app each have one of their own.
/// None when Android cannot be asked.
///
/// The store opens as the app starts, on the app's own thread, and needs
/// the answer there and then; so this asks the activity tao keeps for the
/// app directly, with this thread attached to the VM for the call, rather
/// than wait on the activity's thread.
pub fn files_dir() -> Option<PathBuf> {
    let android = ndk_context::android_context();
    // Safety: tao hands ndk-context the VM, and a global reference to the
    // activity, before the app's `main` runs, and the activity lives as
    // long as the app does.
    let vm = unsafe { jni::JavaVM::from_raw(android.vm().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    // Safety: the same global reference, used as the object it refers to.
    let activity = unsafe { JObject::from_raw(android.context().cast()) };
    // Every reference made on the way is let go of with the frame: a thread
    // that stays attached keeps what it made otherwise.
    let path = env.with_local_frame(4, |env| -> jni::errors::Result<String> {
        let folder = env
            .call_method(&activity, "getFilesDir", "()Ljava/io/File;", &[])?
            .l()?;
        if folder.is_null() {
            return Err(jni::errors::Error::NullPtr("the app has no files folder"));
        }
        let path: JString = env
            .call_method(&folder, "getAbsolutePath", "()Ljava/lang/String;", &[])?
            .l()?
            .into();
        Ok(env.get_string(&path)?.into())
    });
    // A Java exception left pending would fail every JNI call after it.
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    path.ok().map(PathBuf::from)
}

/// Whether a backup file goes to the app's own folder on the shared
/// storage, which Android deletes with the app: before Android 10, which
/// adds a file to Downloads only through the media store. As
/// [`files_dir`], it asks Android from the app's own thread; Android that
/// cannot be asked is taken for a later one.
pub fn backups_inside_app() -> bool {
    let android = ndk_context::android_context();
    // Safety: as in `files_dir`.
    let Ok(vm) = (unsafe { jni::JavaVM::from_raw(android.vm().cast()) }) else {
        return false;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return false;
    };
    let version = env
        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
        .and_then(|value| value.i());
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    version.is_ok_and(|version| version < ANDROID_10)
}

use super::backup::BackedUp;
use super::desktop::save_streamed;

/// `Build.VERSION_CODES.Q`, Android 10: the first that adds to Downloads
/// through the media store.
const ANDROID_10: i32 = 29;
/// `Environment.DIRECTORY_DOWNLOADS`.
const DOWNLOADS: &str = "Download";

/// Save a backup called `name` where the user can reach it, written a piece
/// at a time by `write`, and say where, and what `write` said of it.
///
/// Done on the thread that asks, the history's own, attached to Java for
/// it as [`files_dir`] attaches the app's: the media store's work is slow,
/// and none of it waits on the activity's thread, nor holds the backup in
/// a Java array, which a long history would not fit.
pub fn save_backup(
    name: &str,
    write: impl FnOnce(&mut dyn Write) -> io::Result<BackedUp>,
) -> Result<(String, BackedUp), String> {
    let android = ndk_context::android_context();
    // Safety: as in `files_dir`.
    let vm =
        unsafe { jni::JavaVM::from_raw(android.vm().cast()) }.map_err(|err| err.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|err| err.to_string())?;
    // Safety: as in `files_dir`.
    let activity = unsafe { JObject::from_raw(android.context().cast()) };
    let saved = save(&mut env, &activity, name, write);
    // A Java exception left pending would fail every JNI call after it.
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
    saved
}

fn save(
    env: &mut JNIEnv,
    activity: &JObject,
    name: &str,
    write: impl FnOnce(&mut dyn Write) -> io::Result<BackedUp>,
) -> Result<(String, BackedUp), String> {
    let version = env
        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
        .and_then(|value| value.i())
        .map_err(|err| err.to_string())?;
    if version >= ANDROID_10 {
        let cache = cache_dir(env, activity).map_err(|err| err.to_string())?;
        let (kept, backed) = save_streamed(&cache, name, write).map_err(|err| err.to_string())?;
        let written = cache.join(kept);
        let saved = into_downloads(env, activity, name, &written).map_err(|err| err.to_string());
        // Copied or not, the copy in the cache goes.
        let _ = fs::remove_file(&written);
        return saved.map(|saved| (format!("{saved}, in Downloads"), backed));
    }
    let folder = app_downloads(env, activity).map_err(|err| err.to_string())?;
    let (name, backed) = save_streamed(&folder, name, write).map_err(|err| err.to_string())?;
    Ok((folder.join(name).display().to_string(), backed))
}

/// Add `written`, a file in the app's cache, to Downloads through the media
/// store as `name`, and say what it is called there. One already called
/// `name` is kept; the media store names the new one apart, `name (1)`.
fn into_downloads(
    env: &mut JNIEnv,
    activity: &JObject,
    name: &str,
    written: &Path,
) -> jni::errors::Result<String> {
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
    let copied = copy_into(env, &resolver, &file, written);
    if copied.is_err() {
        // Nothing half written is left in Downloads. A Java exception the
        // copy left pending is cleared first: every call after it would
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
    copied?;
    // The name the file was given is the one to tell of; the one asked
    // for, should the media store not say.
    let saved = display_name(env, &resolver, &file);
    let _ = env.exception_clear();
    Ok(saved.unwrap_or_else(|_| name.to_string()))
}

/// The name the media store gave `file`: the one asked for, or that name
/// told apart from a file already there.
fn display_name(
    env: &mut JNIEnv,
    resolver: &JObject,
    file: &JObject,
) -> jni::errors::Result<String> {
    let column = env.new_string("_display_name")?;
    let columns = env.new_object_array(1, "java/lang/String", &column)?;
    let cursor = env
        .call_method(
            resolver,
            "query",
            "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
            &[
                JValue::Object(file),
                JValue::Object(&columns),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
                JValue::Object(&JObject::null()),
            ],
        )?
        .l()?;
    if cursor.is_null() {
        return Err(jni::errors::Error::NullPtr(
            "the media store has no such file",
        ));
    }
    let name = first_string(env, &cursor);
    // Closed, read or not, an exception the read left pending cleared
    // first.
    let _ = env.exception_clear();
    let _ = env.call_method(&cursor, "close", "()V", &[]);
    name
}

/// The first column of `cursor`'s first row, as a string.
fn first_string(env: &mut JNIEnv, cursor: &JObject) -> jni::errors::Result<String> {
    if !env.call_method(cursor, "moveToFirst", "()Z", &[])?.z()? {
        return Err(jni::errors::Error::NullPtr(
            "the media store has no such file",
        ));
    }
    let name: JString = env
        .call_method(
            cursor,
            "getString",
            "(I)Ljava/lang/String;",
            &[JValue::Int(0)],
        )?
        .l()?
        .into();
    if name.is_null() {
        return Err(jni::errors::Error::NullPtr("the file has no name"));
    }
    Ok(env.get_string(&name)?.into())
}

/// Copy `written`, a file of the app's, into `file`, a file the media store
/// made, by `android.os.FileUtils.copy`, a piece at a time; and close both,
/// copied or not.
fn copy_into(
    env: &mut JNIEnv,
    resolver: &JObject,
    file: &JObject,
    written: &Path,
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
    let path = env.new_string(written.to_string_lossy());
    let input = path.and_then(|path| {
        env.new_object(
            "java/io/FileInputStream",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&path)],
        )
    });
    let copied = match &input {
        Ok(input) => env
            .call_static_method(
                "android/os/FileUtils",
                "copy",
                "(Ljava/io/InputStream;Ljava/io/OutputStream;)J",
                &[JValue::Object(input), JValue::Object(&stream)],
            )
            .map(drop),
        Err(_) => Err(jni::errors::Error::NullPtr("the backup could not be read")),
    };
    if copied.is_err() {
        let _ = env.exception_clear();
    }
    if let Ok(input) = &input {
        let _ = env.call_method(input, "close", "()V", &[]);
        let _ = env.exception_clear();
    }
    let closed = env.call_method(&stream, "close", "()V", &[]).map(drop);
    copied.and(closed)
}

/// The app's own cache folder: `Context.getCacheDir()`, where Android may
/// clear what is left, and from which nothing is backed up.
fn cache_dir(env: &mut JNIEnv, activity: &JObject) -> jni::errors::Result<PathBuf> {
    let folder = env
        .call_method(activity, "getCacheDir", "()Ljava/io/File;", &[])?
        .l()?;
    if folder.is_null() {
        return Err(jni::errors::Error::NullPtr("the app has no cache folder"));
    }
    let path: JString = env
        .call_method(&folder, "getAbsolutePath", "()Ljava/lang/String;", &[])?
        .l()?
        .into();
    let path: String = env.get_string(&path)?.into();
    Ok(PathBuf::from(path))
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
