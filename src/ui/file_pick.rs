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
//!
//! A file may be large, as a backup of years of sessions is: a computer
//! reads it on a thread of its own, and a browser or a phone's webview a
//! slice at a time ([`SLICE_BYTES`]), rather than as one string, which may
//! be more than it hands over at once.

use dioxus::html::FileData;
use dioxus::prelude::*;
use serde::Deserialize;

use crate::page::use_page_messages;
use crate::state::AppCtx;
use crate::ui::widgets::{Icon, IconName};

/// Whether this build chooses files through the webview's own picker.
const WEBVIEW_PICKER: bool = cfg!(any(target_os = "android", target_os = "ios"));

/// How much of a file a browser or a phone's webview reads at a time:
/// one larger is read a slice of this at a time.
const SLICE_BYTES: u64 = 8 << 20;

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

/// A file chosen, handed over the moment it is chosen, unread: its name and
/// size, and the file, read as text by whatever takes it, a task that may
/// outlast the button ([`PickedFile::read`]). The phone's own picker reads
/// a file before it hands it over, so one it chose comes read.
#[derive(Clone)]
pub struct PickedFile {
    pub name: String,
    pub size: u64,
    /// Whether it is read by its byte order mark, as its button reads one.
    by_mark: bool,
    file: Picked,
}

/// A file chosen, read or not.
#[derive(Clone)]
enum Picked {
    /// Chosen by a file input, and still to be read.
    Unread(FileData),
    /// Read already, by the phone's own picker.
    Read(Option<String>),
}

impl PickedFile {
    /// What the file holds, read as text up to `max_bytes`, as the button
    /// reads a file it hands to `on_choose`.
    pub async fn read(self, max_bytes: u64) -> ChosenFile {
        match self.file {
            Picked::Unread(file) => read_chosen(file, max_bytes, self.by_mark).await,
            Picked::Read(text) => ChosenFile {
                name: self.name,
                size: self.size,
                text,
            },
        }
    }
}

/// A piece of a file the phone's own picker chose, as the picker hands it
/// over ([`picker_script`]): a slice of its text with more to come, or the
/// last piece, which ends it, with no text for a file that is no text.
#[derive(Deserialize)]
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios", test)),
    allow(dead_code)
)]
struct Piece {
    name: String,
    size: u64,
    text: Option<String>,
    #[serde(default)]
    more: bool,
    /// Which file chosen it is of, counted from the first.
    #[serde(default)]
    pick: u64,
}

/// The pieces of files the phone's picker hands over, put together: each
/// file whole once its last piece comes, then handed on. A file chosen
/// while another is still handed over takes its place: what there is of
/// the other goes, and a piece of it that comes after is left out.
#[derive(Default)]
#[cfg_attr(
    not(any(target_os = "android", target_os = "ios", test)),
    allow(dead_code)
)]
struct Pieces {
    /// Which file chosen the pieces are of.
    pick: u64,
    /// Its text so far.
    text: Option<String>,
}

#[cfg_attr(
    not(any(target_os = "android", target_os = "ios", test)),
    allow(dead_code)
)]
impl Pieces {
    /// Take `piece`, and give back the file it ends, if it ends one. A file
    /// whose last piece could not be read could not be read at all.
    fn take(&mut self, piece: Piece) -> Option<ChosenFile> {
        let Piece {
            name,
            size,
            text,
            more,
            pick,
        } = piece;
        if pick < self.pick {
            return None;
        }
        if pick > self.pick {
            self.pick = pick;
            self.text = None;
        }
        if more {
            if let Some(text) = text {
                self.text.get_or_insert_with(String::new).push_str(&text);
            }
            return None;
        }
        let text = match (self.text.take(), text) {
            (Some(mut whole), Some(last)) => {
                whole.push_str(&last);
                Some(whole)
            }
            (Some(_), None) => None,
            (None, text) => text,
        };
        Some(ChosenFile { name, size, text })
    }
}

/// Read a file a file input chose, up to `max_bytes`, by its byte order
/// mark where `by_mark` says.
async fn read_chosen(file: FileData, max_bytes: u64, by_mark: bool) -> ChosenFile {
    let (name, size) = (file.name(), file.size());
    let text = if size > max_bytes {
        None
    } else {
        read_text(&file, by_mark).await
    };
    ChosenFile { name, size, text }
}

/// What `file` holds, as text: none where it cannot be read as text, or
/// cannot be read at all. A computer reads it by its byte order mark where
/// `by_mark` says ([`text_in`]), on a thread of its own where one starts,
/// and a browser a slice at a time where it is large, every file by its
/// mark where it is read whole ([`text_of`]).
async fn read_text(file: &FileData, by_mark: bool) -> Option<String> {
    #[cfg(test)]
    crate::testing::reading_a_file(&file.name()).await;
    #[cfg(not(any(feature = "desktop", test)))]
    let _ = by_mark;
    #[cfg(all(feature = "desktop", not(test)))]
    if let Some(text) = read_apart(file.path(), by_mark).await {
        return text;
    }
    #[cfg(all(feature = "web", not(test)))]
    if let Some(file) = file.inner().downcast_ref::<web_sys::File>()
        && reads_by_promise(file)
    {
        if file.size() > SLICE_BYTES as f64 {
            return read_in_slices(file).await;
        }
        // Through the browser's own promise, which fails with a file it
        // cannot read, as one moved since it was chosen: the file input's
        // reader hears only a file read, and waits for good on any other.
        // What it reads is decoded as that reader decodes it.
        let read = wasm_bindgen_futures::JsFuture::from(file.array_buffer())
            .await
            .ok()?;
        return Some(text_of(&js_sys::Uint8Array::new(&read).to_vec()));
    }
    // A computer's file that could not be read on a thread of its own is
    // read here as one is there, by its mark where `by_mark` says; and so
    // is a test's.
    #[cfg(any(feature = "desktop", test))]
    {
        text_in(file.read_bytes().await.ok()?.to_vec(), by_mark)
    }
    #[cfg(not(any(feature = "desktop", test)))]
    {
        file.read_string().await.ok()
    }
}

/// Whether this browser reads a file through a promise of its own
/// (`Blob.arrayBuffer`), as every one has since Safari 14: called where
/// there is none, it would throw through the app. Where there is none, the
/// file input's reader reads the file.
#[cfg(all(feature = "web", not(test)))]
fn reads_by_promise(file: &web_sys::File) -> bool {
    js_sys::Reflect::get(file, &wasm_bindgen::JsValue::from_str("arrayBuffer"))
        .is_ok_and(|read| read.is_function())
}

/// What `bytes`, a whole file, hold as text, decoded by their byte order
/// mark as a browser's file reader decodes them: UTF-16, little-endian or
/// big, where they start with its mark, as Windows' Notepad and Excel save
/// "Unicode" text; else UTF-8, its own mark left out. A byte, or a pair,
/// that is no character is read as a mark for one, as the reader reads it.
#[cfg(any(feature = "web", feature = "desktop", test))]
fn text_of(bytes: &[u8]) -> String {
    let utf16 = |bytes: &[u8], unit: fn([u8; 2]) -> u16| {
        let (pairs, cut) = bytes.as_chunks::<2>();
        let mut text: String = char::decode_utf16(pairs.iter().map(|pair| unit(*pair)))
            .map(|read| read.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
        if !cut.is_empty() {
            text.push(char::REPLACEMENT_CHARACTER);
        }
        text
    };
    match bytes {
        [0xff, 0xfe, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xfe, 0xff, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// What `bytes`, a whole file, hold as text, as a computer reads them:
/// UTF-8, or no text. Read `by_mark`, as a word list is, they may be UTF-16
/// as well, little-endian or big, where they start with its byte order
/// mark, as Windows' Notepad and Excel save "Unicode" text, and are decoded
/// then as a browser decodes them ([`text_of`]). A backup is UTF-8 or
/// nothing.
#[cfg(any(feature = "desktop", test))]
fn text_in(bytes: Vec<u8>, by_mark: bool) -> Option<String> {
    match bytes.as_slice() {
        [0xff, 0xfe, ..] | [0xfe, 0xff, ..] if by_mark => Some(text_of(&bytes)),
        _ => String::from_utf8(bytes).ok(),
    }
}

/// What the file at `path` holds, as text, read on a thread of its own, by
/// its byte order mark where `by_mark` says ([`text_in`]): none where no
/// thread would start, for it to be read otherwise.
#[cfg(all(feature = "desktop", not(test)))]
async fn read_apart(path: std::path::PathBuf, by_mark: bool) -> Option<Option<String>> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let (answer, answered) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("dust-file-read".into())
        .spawn(move || {
            let text = std::fs::read(&path)
                .ok()
                .and_then(|bytes| text_in(bytes, by_mark));
            let _ = answer.send(text);
        })
        .ok()?;
    Some(answered.await.ok().flatten())
}

/// What `file` holds, as text, read a slice at a time: as one string, a
/// browser may not read a file of hundreds of megabytes at all. A file that
/// is no UTF-8 is no text, as a computer reads it: none, found at the slice
/// that shows it, the first of a photo or a film, rather than once the whole
/// of it is held, and held again as text; room for the whole of a file is
/// made only once its first slice reads as text, as the page keeps for good
/// the memory it grows to. So is a file there is no room for: a browser's
/// memory has an end, and running out of it would end the app.
#[cfg(all(feature = "web", not(test)))]
async fn read_in_slices(file: &web_sys::File) -> Option<String> {
    let size = file.size();
    let mut bytes = Vec::new();
    let (mut at, mut text_up_to) = (0.0, 0);
    while at < size {
        let end = (at + SLICE_BYTES as f64).min(size);
        let slice = file.slice_with_f64_and_f64(at, end).ok()?;
        let read = wasm_bindgen_futures::JsFuture::from(slice.array_buffer())
            .await
            .ok()?;
        let read = js_sys::Uint8Array::new(&read);
        let (from, length) = (bytes.len(), read.length() as usize);
        bytes.try_reserve(length).ok()?;
        bytes.resize(from + length, 0);
        read.copy_to(&mut bytes[from..]);
        text_up_to = utf8_up_to(&bytes, text_up_to)?;
        if from == 0 {
            bytes
                .try_reserve_exact((size as usize).saturating_sub(length))
                .ok()?;
        }
        at = end;
    }
    String::from_utf8(bytes).ok()
}

/// How far `bytes`, read a slice at a time, read as UTF-8, looked at from
/// `from`, as far as they did before: to their end, or to a character the
/// end of the slice cut in two, which the next slice finishes. None where
/// they are no UTF-8, and so no text.
#[cfg(any(feature = "web", test))]
fn utf8_up_to(bytes: &[u8], from: usize) -> Option<usize> {
    match std::str::from_utf8(&bytes[from..]) {
        Ok(_) => Some(bytes.len()),
        Err(cut) if cut.error_len().is_none() => Some(from + cut.valid_up_to()),
        Err(_) => None,
    }
}

/// The script that has the element `id` open the webview's own file picker
/// for `accept`, and sends back each file chosen as [`Piece`]s, its text a
/// slice of [`SLICE_BYTES`] at a time, as the webview may not hand over
/// hundreds of megabytes at once: text while it reads as UTF-8, and from
/// the slice that does not, none, the rest left unread. Read `by_mark`, as
/// a word list is, a file that starts with UTF-16's byte order mark is
/// UTF-16, decoded as a computer decodes it ([`text_in`]). The file input
/// it opens is the document body's own, outside the page the renderer
/// draws, so the renderer leaves its clicks alone. A picker made before for
/// the same id, by a button drawn before, gives way to this one.
#[cfg_attr(not(any(target_os = "android", target_os = "ios")), allow(dead_code))]
fn picker_script(id: &str, accept: &str, max_bytes: u64, by_mark: bool) -> String {
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
// Each file chosen is counted, and its pieces say which they are of: one
// chosen while another is still read takes its place, the other read no
// further and nothing more of it sent.
let picks = 0;
input.addEventListener("change", () => {{
    const file = input.files && input.files[0];
    if (!file) {{
        return;
    }}
    picks += 1;
    const pick = picks;
    const send = (text, more) => {{
        if (pick === picks) {{
            dioxus.send({{ name: file.name, size: file.size, text, more, pick }});
        }}
    }};
    if (file.size > {max_bytes}) {{
        send(null, false);
        return;
    }}
    // A slice at a time, each decoded as it comes, as UTF-8 or not at all,
    // as a computer reads a file: a character cut in two by a slice's end
    // is decoded whole with the next, and a slice that is no UTF-8, the
    // first of a photo or a film, ends the file there, unread. A file read
    // by its byte order mark whose first slice starts with UTF-16's is
    // UTF-16, a pair that is no character read as a mark for one.
    const byMark = {by_mark};
    let decoder = null;
    const decode = (bytes) => {{
        if (decoder === null) {{
            const utf16 = byMark && bytes && bytes.length >= 2
                && ((bytes[0] === 0xff && bytes[1] === 0xfe && "utf-16le")
                    || (bytes[0] === 0xfe && bytes[1] === 0xff && "utf-16be"));
            decoder = utf16 ? new TextDecoder(utf16) : new TextDecoder("utf-8", {{ fatal: true }});
        }}
        try {{
            return bytes ? decoder.decode(bytes, {{ stream: true }}) : decoder.decode();
        }} catch (notText) {{
            return null;
        }}
    }};
    let at = 0;
    const next = () => {{
        if (pick !== picks) {{
            return;
        }}
        if (at >= file.size) {{
            send(decode(), false);
            return;
        }}
        const reader = new FileReader();
        reader.onload = () => {{
            const text = decode(new Uint8Array(reader.result));
            if (text === null) {{
                send(null, false);
                return;
            }}
            send(text, true);
            next();
        }};
        reader.onerror = () => send(null, false);
        reader.readAsArrayBuffer(file.slice(at, at + {SLICE_BYTES}));
        at += {SLICE_BYTES};
    }};
    next();
}});
pickers[id] = () => {{
    document.removeEventListener("click", open, true);
    input.remove();
}};"##
    )
}

/// A button that chooses a file of the kinds `accept` names, reads it as
/// text up to `max_bytes`, and hands it to `on_choose`, unless another is
/// chosen while it is read, which takes its place; or, given `on_pick`,
/// hands it to that the moment it is chosen, unread, to be read by a task
/// that may outlast the button, which goes with its screen.
#[component]
pub fn FileButton(
    /// The button's id, and its file input's.
    id: String,
    /// The kinds of file offered, as an `accept` attribute.
    accept: &'static str,
    max_bytes: u64,
    icon: IconName,
    label: &'static str,
    #[props(default)] on_choose: EventHandler<ChosenFile>,
    #[props(default)] on_pick: Option<EventHandler<PickedFile>>,
    /// Whether a computer and a phone read a file by its byte order mark,
    /// UTF-16 as well as UTF-8, as a browser reads every file it reads
    /// whole: a word list, which Windows' Notepad and Excel save as UTF-16
    /// for "Unicode" text. Else they read it as UTF-8 or not at all, as a
    /// backup.
    #[props(default)]
    by_mark: bool,
) -> Element {
    // Moved on by every file chosen, to draw a fresh file input: the same
    // file chosen again in the old one would change nothing, and not load.
    let mut picked = use_signal(|| 0u64);
    if WEBVIEW_PICKER {
        let script = picker_script(&id, accept, max_bytes, by_mark);
        let mut pieces = Pieces::default();
        use_page_messages(
            move || script,
            move |piece: Piece| {
                let Some(file) = pieces.take(piece) else {
                    return;
                };
                match on_pick {
                    Some(on_pick) => on_pick.call(PickedFile {
                        name: file.name,
                        size: file.size,
                        by_mark,
                        file: Picked::Read(file.text),
                    }),
                    None => on_choose.call(file),
                }
            },
        );
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
                            if let Some(on_pick) = on_pick {
                                on_pick.call(PickedFile {
                                    name: file.name(),
                                    size: file.size(),
                                    by_mark,
                                    file: Picked::Unread(file),
                                });
                                return;
                            }
                            // Handed on unless another file is chosen
                            // before it is read: that one takes its place,
                            // read sooner or later.
                            let pick = *picked.peek();
                            spawn(async move {
                                let file = read_chosen(file, max_bytes, by_mark).await;
                                if *picked.peek() == pick {
                                    on_choose.call(file);
                                }
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
    use super::{
        ChosenFile, Piece, Pieces, SLICE_BYTES, data_dirs, on_path, picker_script,
        portal_chooses_files, text_in, text_of, utf8_up_to,
    };
    use crate::testing::{PageStep, on_fake_page};
    use serde_json::Value;
    use std::ffi::OsStr;
    use std::fs;
    use std::path::PathBuf;

    /// What the phone's own picker sends the app, reading files by their
    /// byte order mark where `by_mark` says ([`picker_script`]), run in Node
    /// on a page made up for it (`src/fake_page.mjs`), as `choose` has it
    /// choose files there (`picker.choose(file)`), with what `choose` sends
    /// itself among it. The webview's file input and reader are made up
    /// beside it: the reader counts the reads it is asked for in `read`, and
    /// does each once `await settled()` lets the reads waiting be done, and
    /// those they ask for, or `await settled(count)` only the first `count`.
    /// None on a machine with no Node.
    fn picked_on_page(by_mark: bool, choose: &str) -> Option<Vec<Value>> {
        let page = r#"globalThis.read = 0;
const waiting = [];
globalThis.FileReader = class {
  readAsArrayBuffer(blob) {
    this.take(blob.arrayBuffer());
  }
  readAsText(blob) {
    this.take(blob.text());
  }
  take(reading) {
    globalThis.read += 1;
    waiting.push(async () => {
      this.result = await reading;
      this.onload();
    });
  }
};
globalThis.settled = async (count = Infinity) => {
  for (let done = 0; done < count && waiting.length > 0; done += 1) {
    await waiting.shift()();
  }
};
document.createElement = () => {
  const heard = {};
  return {
    style: {},
    addEventListener: (type, listener) => {
      heard[type] = listener;
    },
    click: () => {},
    remove: () => {},
    choose(file) {
      this.files = [file];
      heard.change();
    },
  };
};
document.body.appendChild = (input) => {
  globalThis.picker = input;
};"#;
        let heard = on_fake_page(&[
            PageStep::run(page),
            PageStep::run(picker_script("file-backup", "", 1 << 30, by_mark)),
            PageStep::wait(choose),
        ])?;
        Some(heard.into_iter().last()?.sent)
    }

    /// The files the pieces in `sent` put together, in turn, as the button
    /// puts them together.
    fn files_in(sent: &[Value]) -> Vec<ChosenFile> {
        let mut pieces = Pieces::default();
        sent.iter()
            .filter(|said| said.get("name").is_some())
            .filter_map(|piece| pieces.take(serde_json::from_value(piece.clone()).ok()?))
            .collect()
    }

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
        let script = picker_script("file-backup", ".json", 500, false);
        assert!(script.contains(r#"const id = "file-backup";"#), "{script}");
        assert!(script.contains(r#"input.accept = ".json";"#), "{script}");
        assert!(script.contains("document.body.appendChild(input);"));
        assert!(script.contains(r##"event.target.closest("#" + id)"##));
        assert!(script.contains("input.click();"));
        assert!(script.contains("if (file.size > 500)"));
        assert!(script.contains("dioxus.send("));
        // A slice at a time, as UTF-8 or not at all.
        assert!(script.contains(&format!(
            "reader.readAsArrayBuffer(file.slice(at, at + {SLICE_BYTES}));"
        )));
        assert!(script.contains(r#"new TextDecoder("utf-8", { fatal: true })"#));
        // Drawn again, it takes over from the picker before.
        let takeover = script.find("pickers[id]();").expect("takes over");
        let made = script.find("document.createElement").expect("makes one");
        assert!(takeover < made, "{script}");
    }

    /// A file read a slice at a time in a browser is text as far as it reads
    /// as UTF-8: a character cut in two by a slice's end waits for the next
    /// slice, and a byte that is no UTF-8 makes it no text at the slice that
    /// holds it, the first of a photo or a film. Such a file read as text
    /// was held whole, then again with a mark for every byte not read, three
    /// times its size or more, which a browser could not find room for.
    #[test]
    fn a_large_file_is_text_only_while_it_reads_as_utf8() {
        let text = "dust é".as_bytes();
        let cut = text.len() - 1;
        assert_eq!(utf8_up_to(&text[..cut], 0), Some(cut - 1), "é cut in two");
        assert_eq!(utf8_up_to(text, cut - 1), Some(text.len()), "and whole");
        assert_eq!(utf8_up_to(text, 0), Some(text.len()));
        let mut film = b"\x00\x00\x00 ftypisom".to_vec();
        film.extend([0x93, 0xff, 0x00]);
        assert_eq!(utf8_up_to(&film, 0), None);
        assert_eq!(utf8_up_to(&film, 4), None, "past the slices before");
    }

    /// A file a browser reads whole is decoded by its byte order mark, as
    /// the file input's reader decodes one: UTF-16, little-endian or big,
    /// as Windows' Notepad and Excel save "Unicode" text, and else UTF-8,
    /// its own mark left out; a byte, or a pair, that is no character is
    /// read as a mark for one, as the reader reads it. Read through the
    /// browser's promise as UTF-8 whatever its mark, a word list saved as
    /// UTF-16 came back as garbage, its mark read as two marks and a NUL
    /// after every letter.
    #[test]
    fn a_file_read_whole_is_decoded_by_its_byte_order_mark() {
        let words = "CQ\r\nDE é\r\n";
        let utf16 = |mark: [u8; 2], unit: fn(u16) -> [u8; 2]| -> Vec<u8> {
            mark.into_iter()
                .chain(words.encode_utf16().flat_map(unit))
                .collect()
        };
        assert_eq!(text_of(&utf16([0xff, 0xfe], u16::to_le_bytes)), words);
        assert_eq!(text_of(&utf16([0xfe, 0xff], u16::to_be_bytes)), words);
        let marked: Vec<u8> = [0xef, 0xbb, 0xbf]
            .into_iter()
            .chain(words.bytes())
            .collect();
        assert_eq!(text_of(&marked), words);
        assert_eq!(text_of(words.as_bytes()), words);
        assert_eq!(text_of(&[b'C', 0xff, b'Q']), "C\u{fffd}Q");
        assert_eq!(text_of(&[0xff, 0xfe, b'C', 0, b'Q']), "C\u{fffd}");
    }

    /// A computer reads a word list by its byte order mark, UTF-16 as well
    /// as UTF-8, as a browser reads one; a backup, read otherwise, is UTF-8
    /// or nothing, whatever its mark. Read as UTF-8 whatever its mark, a
    /// word list saved as UTF-16 could not be read as text.
    #[test]
    fn a_computer_reads_a_word_list_by_its_byte_order_mark_and_a_backup_as_utf8() {
        let words = "CQ\r\nDE é\r\n";
        let utf16 = |mark: [u8; 2], unit: fn(u16) -> [u8; 2]| -> Vec<u8> {
            mark.into_iter()
                .chain(words.encode_utf16().flat_map(unit))
                .collect()
        };
        let little = utf16([0xff, 0xfe], u16::to_le_bytes);
        let big = utf16([0xfe, 0xff], u16::to_be_bytes);
        assert_eq!(text_in(little.clone(), true).as_deref(), Some(words));
        assert_eq!(text_in(big.clone(), true).as_deref(), Some(words));
        assert_eq!(text_in(little, false), None);
        assert_eq!(text_in(big, false), None);
        for by_mark in [true, false] {
            assert_eq!(text_in(words.into(), by_mark).as_deref(), Some(words));
            assert_eq!(text_in(vec![b'C', 0xff, b'Q'], by_mark), None);
        }
    }

    /// The phone's picker reads a file as UTF-8 or not at all, as a computer
    /// reads one: its text whole, a character cut in two by the end of a
    /// slice decoded with the next; a file that is no text handed back
    /// unread from its first slice, the rest of it never read, as a photo or
    /// a film among every file a phone offers; and one cut off in the middle
    /// of a character unread as well. Each was read as text whatever it
    /// held, a mark for every byte that was not, and a film of a gigabyte
    /// put together whole in the app, three times over.
    #[test]
    fn the_phone_picker_reads_a_file_as_utf8_or_not_at_all() {
        let slice = SLICE_BYTES;
        let Some(sent) = picked_on_page(
            false,
            &format!(
                r#"const big = new Uint8Array({slice} + 1).fill(0x61);
big.set([0xc3, 0xa9], {slice} - 1);
picker.choose(new File([big], "big.json"));
await settled();
dioxus.send({{ read }});
picker.choose(new File(['{{"dustBackup":1}} é'], "b.json"));
await settled();
const film = new Uint8Array({slice} + 1).fill(0x61);
film[0] = 0xff;
picker.choose(new File([film], "film.mp4"));
await settled();
dioxus.send({{ read }});
picker.choose(new File([new Uint8Array([0x61, 0xc3])], "cut.txt"));
await settled();"#
            ),
        ) else {
            return;
        };
        let files = files_in(&sent);
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, ["big.json", "b.json", "film.mp4", "cut.txt"]);
        let big = files[0].text.as_deref().expect("text");
        assert_eq!(big.len(), slice as usize + 1);
        assert!(big.ends_with("aé"), "cut in two by the slice's end");
        assert_eq!(files[1].text.as_deref(), Some(r#"{"dustBackup":1} é"#));
        assert_eq!(files[2].text, None, "a film is no text");
        assert_eq!(files[3].text, None, "cut off mid-character");
        let read: Vec<u64> = sent
            .iter()
            .filter_map(|said| said.get("read")?.as_u64())
            .collect();
        assert_eq!(read, [2, 4], "the film's first slice alone");
    }

    /// A file handed over a slice at a time is put together whole, and
    /// handed on once, as its last slice comes; one whose last slice could
    /// not be read could not be read at all, and a file whole is handed on
    /// as it comes.
    #[test]
    fn a_large_file_is_put_together_from_its_slices() {
        let piece = |text: Option<&str>, more| Piece {
            name: "b.json".into(),
            size: 9,
            text: text.map(str::to_string),
            more,
            pick: 1,
        };
        let chosen = |text: Option<&str>| ChosenFile {
            name: "b.json".into(),
            size: 9,
            text: text.map(str::to_string),
        };
        let mut pieces = Pieces::default();
        assert_eq!(pieces.take(piece(Some("{\"dust"), true)), None);
        assert_eq!(pieces.take(piece(Some("Backup\":"), true)), None);
        assert_eq!(
            pieces.take(piece(Some("1}"), false)),
            Some(chosen(Some("{\"dustBackup\":1}")))
        );
        assert_eq!(
            pieces.take(piece(Some("[]"), false)),
            Some(chosen(Some("[]")))
        );
        assert_eq!(pieces.take(piece(Some("[1,"), true)), None);
        assert_eq!(pieces.take(piece(None, false)), Some(chosen(None)));
        assert_eq!(pieces.take(piece(None, false)), Some(chosen(None)));
    }

    /// A file chosen while another is still handed over takes its place:
    /// what was put together of the other goes, and a piece of it that
    /// comes after is left out. The two were put together as one, under
    /// the second's name.
    #[test]
    fn a_file_chosen_while_another_is_handed_over_takes_its_place() {
        let piece = |pick, name: &str, text: &str, more| Piece {
            name: name.into(),
            size: 9,
            text: Some(text.into()),
            more,
            pick,
        };
        let mut pieces = Pieces::default();
        assert_eq!(pieces.take(piece(1, "a.json", "{\"a", true)), None);
        assert_eq!(pieces.take(piece(2, "b.json", "[", true)), None);
        assert_eq!(pieces.take(piece(1, "a.json", "\":1}", false)), None);
        assert_eq!(
            pieces.take(piece(2, "b.json", "]", false)),
            Some(ChosenFile {
                name: "b.json".into(),
                size: 9,
                text: Some("[]".into()),
            })
        );
    }

    /// On the page, a file chosen while another is still read takes its
    /// place: the other is read no further, and nothing more of it is sent,
    /// so the app is handed the newer alone, whole. The two were read side
    /// by side, and their slices spliced into one file.
    #[test]
    fn the_phone_picker_reads_a_file_chosen_over_another_alone() {
        let slice = SLICE_BYTES;
        let Some(sent) = picked_on_page(
            false,
            &format!(
                r#"picker.choose(new File([new Uint8Array(3 * {slice}).fill(0x61)], "big.json"));
await settled(1);
picker.choose(new File(["[]"], "b.json"));
await settled();
dioxus.send({{ read }});"#
            ),
        ) else {
            return;
        };
        let files = files_in(&sent);
        assert_eq!(
            files,
            [ChosenFile {
                name: "b.json".into(),
                size: 2,
                text: Some("[]".into()),
            }]
        );
        let read: Vec<u64> = sent
            .iter()
            .filter_map(|said| said.get("read")?.as_u64())
            .collect();
        assert_eq!(read, [3], "two slices of the first, none after");
    }

    /// The phone's picker reads a word list by its byte order mark: UTF-16,
    /// little-endian or big, as well as UTF-8, its mark left out, as a
    /// computer and a browser read one. A backup is UTF-8 or nothing still.
    /// The picker read every file as UTF-8, and a word list saved as
    /// UTF-16, as Windows' Notepad and Excel save "Unicode" text, as no
    /// text at all.
    #[test]
    fn the_phone_picker_reads_a_word_list_by_its_byte_order_mark() {
        let choose = r#"for (const [name, bytes] of [
    ["little.txt", [0xff, 0xfe, 0x43, 0, 0x51, 0, 0xe9, 0]],
    ["big.txt", [0xfe, 0xff, 0, 0x43, 0, 0x51, 0, 0xe9]],
    ["marked.txt", [0xef, 0xbb, 0xbf, 0x43, 0x51, 0xc3, 0xa9]],
]) {
    picker.choose(new File([new Uint8Array(bytes)], name));
    await settled();
}"#;
        let texts = |by_mark| -> Option<Vec<Option<String>>> {
            let sent = picked_on_page(by_mark, choose)?;
            Some(files_in(&sent).into_iter().map(|file| file.text).collect())
        };
        let Some(word_lists) = texts(true) else {
            return;
        };
        let read = Some("CQé".to_string());
        assert_eq!(word_lists, [read.clone(), read.clone(), read.clone()]);
        let backups = texts(false).expect("Node, as before");
        assert_eq!(backups, [None, None, read], "UTF-8 or nothing");
    }
}
