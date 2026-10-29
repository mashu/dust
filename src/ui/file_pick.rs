//! A button that chooses a file and reads it as text: a word list, a
//! backup to restore.
//!
//! On the web and the desktop it is a file input under a label dressed as
//! a button. The app builds for phones cannot use one: there the webview
//! renderer asks its native side for the file a file input chose, and on
//! Android and iOS the native side gives none back, so a file input on a
//! phone never yields a file. So on a phone the button opens the webview's
//! own picker instead, through a file input made outside the app's page,
//! where the renderer does not reach, and the file is read there and handed
//! back.
//!
//! On Linux the desktop build's file input opens the desktop's file chooser
//! portal, or zenity without one. With neither, nothing opens, and the
//! renderer hands the input no file, just as for a dialog cancelled: so
//! when no file comes back and neither is there, the button says why.

use dioxus::html::FileData;
use dioxus::prelude::*;
use serde::Deserialize;

use crate::state::AppCtx;
use crate::ui::widgets::{Icon, IconName};

/// Whether this build chooses files through the webview's own picker.
const WEBVIEW_PICKER: bool = cfg!(any(target_os = "android", target_os = "ios"));

/// What the button says when no file came back and no file chooser could
/// have opened.
const NO_FILE_CHOOSER: &str = "No file chooser could open. Dust asks the desktop's file chooser portal for one, or zenity: install xdg-desktop-portal with a backend for your desktop (-gtk, -gnome or -kde), or zenity.";

/// Whether a file chooser can open here. Only the Linux desktop build can
/// be without one; asked once, the first time no file comes back.
fn file_chooser_here() -> bool {
    #[cfg(all(feature = "desktop", target_os = "linux"))]
    {
        static HERE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *HERE.get_or_init(|| {
            let env = |name| std::env::var_os(name).unwrap_or_default();
            std::path::Path::new("/.flatpak-info").exists()
                || on_path("zenity", &env("PATH"))
                || portal_chooses_files(&data_dirs(&env("XDG_DATA_HOME"), &env("XDG_DATA_DIRS")))
        })
    }
    #[cfg(not(all(feature = "desktop", target_os = "linux")))]
    {
        true
    }
}

/// Whether `program` is in a folder of `path`.
#[cfg(any(all(feature = "desktop", target_os = "linux"), test))]
fn on_path(program: &str, path: &std::ffi::OsStr) -> bool {
    std::env::split_paths(path).any(|dir| dir.join(program).is_file())
}

/// The folders the desktop's data is looked for in, as the XDG base
/// directories have it: the user's own first, then the system's.
#[cfg(any(all(feature = "desktop", target_os = "linux"), test))]
fn data_dirs(home: &std::ffi::OsStr, system: &std::ffi::OsStr) -> Vec<std::path::PathBuf> {
    let home = if home.is_empty() {
        dirs::home_dir().map(|home| home.join(".local/share"))
    } else {
        Some(home.into())
    };
    let system = if system.is_empty() {
        std::ffi::OsStr::new("/usr/local/share:/usr/share")
    } else {
        system
    };
    home.into_iter()
        .chain(std::env::split_paths(system))
        .collect()
}

/// Whether the file chooser portal is installed in one of `dirs`, with a
/// backend that chooses files: its D-Bus service, and a `.portal` file
/// that names the FileChooser interface.
#[cfg(any(all(feature = "desktop", target_os = "linux"), test))]
fn portal_chooses_files(dirs: &[std::path::PathBuf]) -> bool {
    let frontend = dirs.iter().any(|dir| {
        dir.join("dbus-1/services/org.freedesktop.portal.Desktop.service")
            .is_file()
    });
    let backend = dirs.iter().any(|dir| {
        std::fs::read_dir(dir.join("xdg-desktop-portal/portals"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "portal"))
            .any(|entry| {
                std::fs::read_to_string(entry.path())
                    .is_ok_and(|text| text.contains("org.freedesktop.impl.portal.FileChooser"))
            })
    });
    frontend && backend
}

/// A file chosen, as the button hands it over: its name and size, and its
/// text, unless it was larger than the button reads or could not be read as
/// text.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ChosenFile {
    pub name: String,
    pub size: u64,
    pub text: Option<String>,
}

/// Read a file a file input chose, up to `max_bytes`.
async fn read_chosen(file: FileData, max_bytes: u64) -> ChosenFile {
    let (name, size) = (file.name(), file.size());
    let text = if size > max_bytes {
        None
    } else {
        file.read_string().await.ok()
    };
    ChosenFile { name, size, text }
}

/// The script that has the element `id` open the webview's own file picker
/// for `accept`, and sends back each file chosen as a [`ChosenFile`]. The
/// file input it opens is the document body's own, outside the page the
/// renderer draws, so the renderer leaves its clicks alone. A picker made
/// before for the same id, by a button drawn before, gives way to this one.
#[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
fn picker_script(id: &str, accept: &str, max_bytes: u64) -> String {
    format!(
        r##"const id = {id:?};
const pickers = window.__dustPickers || (window.__dustPickers = {{}});
if (pickers[id]) {{
    pickers[id]();
}}
const input = document.createElement("input");
input.type = "file";
input.accept = {accept:?};
input.style.display = "none";
document.body.appendChild(input);
const open = (event) => {{
    if (!(event.target instanceof Element) || !event.target.closest("#" + id)) {{
        return;
    }}
    event.preventDefault();
    event.stopPropagation();
    input.value = "";
    input.click();
}};
document.addEventListener("click", open, true);
input.addEventListener("change", () => {{
    const file = input.files && input.files[0];
    if (!file) {{
        return;
    }}
    const chosen = (text) => dioxus.send({{ name: file.name, size: file.size, text }});
    if (file.size > {max_bytes}) {{
        chosen(null);
        return;
    }}
    const reader = new FileReader();
    reader.onload = () => chosen(String(reader.result));
    reader.onerror = () => chosen(null);
    reader.readAsText(file);
}});
pickers[id] = () => {{
    document.removeEventListener("click", open, true);
    input.remove();
}};"##
    )
}

/// A button that chooses a file of the kinds `accept` names, reads it as
/// text up to `max_bytes`, and hands it to `on_choose`.
#[component]
pub fn FileButton(
    /// The button's id, and its file input's.
    id: String,
    /// The kinds of file offered, as an `accept` attribute.
    accept: &'static str,
    max_bytes: u64,
    icon: IconName,
    label: &'static str,
    on_choose: EventHandler<ChosenFile>,
) -> Element {
    // Moved on by every file chosen, to draw a fresh file input: the same
    // file chosen again in the old one would change nothing, and not load.
    let mut picked = use_signal(|| 0u64);
    if WEBVIEW_PICKER {
        let script = picker_script(&id, accept, max_bytes);
        use_hook(move || {
            spawn(async move {
                let mut picker = document::eval(&script);
                while let Ok(file) = picker.recv::<ChosenFile>().await {
                    on_choose.call(file);
                }
            })
        });
        return rsx! {
            button { id, class: "btn btn-secondary btn-sm", r#type: "button",
                Icon { name: icon }
                "{label}"
            }
        };
    }
    rsx! {
        label { class: "btn btn-secondary btn-sm file-button",
            for fresh in [picked()] {
                input {
                    key: "{fresh}",
                    id: id.clone(),
                    class: "file-input",
                    r#type: "file",
                    accept,
                    onchange: move |e| {
                        if let Some(file) = e.files().into_iter().next() {
                            picked += 1;
                            spawn(async move {
                                on_choose.call(read_chosen(file, max_bytes).await);
                            });
                        } else if !file_chooser_here()
                            && let Some(ctx) = try_consume_context::<AppCtx>()
                        {
                            ctx.toasts.show(NO_FILE_CHOOSER);
                        }
                    },
                }
            }
            Icon { name: icon }
            "{label}"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{data_dirs, on_path, picker_script, portal_chooses_files};
    use std::ffi::OsStr;
    use std::fs;
    use std::path::PathBuf;

    /// A scratch folder, gone again with the test.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("dust-file-pick-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn put(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A file chooser is had from the portal only with a backend that
    /// chooses files, from any of the data folders; or from zenity, found
    /// on the path.
    #[test]
    fn a_file_chooser_is_looked_for_where_the_desktop_keeps_one() {
        let user = Scratch::new("user");
        let system = Scratch::new("system");
        let dirs = [user.0.clone(), system.0.clone()];
        assert!(!portal_chooses_files(&dirs));
        system.put(
            "dbus-1/services/org.freedesktop.portal.Desktop.service",
            "[D-BUS Service]\n",
        );
        assert!(!portal_chooses_files(&dirs), "a portal with no backend");
        user.put(
            "xdg-desktop-portal/portals/wlr.portal",
            "[portal]\nInterfaces=org.freedesktop.impl.portal.Screenshot;\n",
        );
        assert!(
            !portal_chooses_files(&dirs),
            "a backend that chooses no files"
        );
        user.put(
            "xdg-desktop-portal/portals/gtk.portal",
            "[portal]\nInterfaces=org.freedesktop.impl.portal.FileChooser;\n",
        );
        assert!(portal_chooses_files(&dirs));

        let bin = Scratch::new("bin");
        let path = std::env::join_paths([user.0.clone(), bin.0.clone()]).unwrap();
        assert!(!on_path("zenity", &path));
        bin.put("zenity", "");
        assert!(on_path("zenity", &path));
    }

    /// The XDG base directories: the user's own data folder first, then
    /// the system's, each with its default when unset.
    #[test]
    fn the_data_folders_are_the_xdg_ones() {
        let dirs = data_dirs(
            OsStr::new("/home/me/.data"),
            OsStr::new("/opt/share:/usr/share"),
        );
        assert_eq!(
            dirs,
            [
                PathBuf::from("/home/me/.data"),
                PathBuf::from("/opt/share"),
                PathBuf::from("/usr/share")
            ]
        );
        let defaults = data_dirs(OsStr::new(""), OsStr::new(""));
        assert!(defaults.ends_with(&[
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share")
        ]));
    }

    /// The picker is the body's own, opened from the button with the click
    /// that pressed it, and a file too large is handed back unread.
    #[test]
    fn the_phone_picker_opens_from_its_button_and_reads_the_file() {
        let script = picker_script("file-backup", ".json", 500);
        assert!(script.contains(r#"const id = "file-backup";"#), "{script}");
        assert!(script.contains(r#"input.accept = ".json";"#), "{script}");
        assert!(script.contains("document.body.appendChild(input);"));
        assert!(script.contains(r##"event.target.closest("#" + id)"##));
        assert!(script.contains("input.click();"));
        assert!(script.contains("if (file.size > 500)"));
        assert!(script.contains("reader.readAsText(file);"));
        assert!(script.contains("dioxus.send("));
        // Drawn again, it takes over from the picker before.
        let takeover = script.find("pickers[id]();").expect("takes over");
        let made = script.find("document.createElement").expect("makes one");
        assert!(takeover < made, "{script}");
    }
}
