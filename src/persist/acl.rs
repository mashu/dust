//! A file for the user alone on Windows, as the mode 0600 makes one on
//! Unix: its access list gives the account the app runs as every right to
//! it and no one else any, and inherits nothing from its folder. What sync
//! keeps, the key to the synced copy and the sign-in, is kept so
//! ([`super::desktop`]).
//!
//! The list is written out ([`for_the_user_alone`]) and tested on every
//! platform; the calls that give a file the list are Windows's alone, and
//! are compiled by CI's Windows lint, never run by a test there.

#[cfg(windows)]
use std::{ffi::c_void, fs, io, os::windows::io::AsRawHandle, ptr};

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_SUCCESS, GENERIC_WRITE, HANDLE, LocalFree},
    Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
    },
    Security::{
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
        PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{FILE_SHARE_DELETE, WRITE_DAC},
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

/// The access list of a file that the account whose security identifier
/// is `sid` alone may open, in the words Windows reads one in (SDDL): one
/// entry, which allows that account every right to the file (`FA`), and
/// none inherited from its folder (`P`, protected). None for a `sid` that
/// is no identifier written out as Windows writes one (`S-1-…`): no other
/// word may stand in for the account there, nor more entries be written
/// into the list.
pub(super) fn for_the_user_alone(sid: &str) -> Option<String> {
    let parts = sid.strip_prefix("S-1-")?;
    parts
        .split('-')
        .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        .then(|| format!("D:P(A;;FA;;;{sid})"))
}

/// What a file must be opened for to be given its list: writing, as every
/// file the store writes is opened, and its access list (`WRITE_DAC`).
#[cfg(windows)]
pub(super) const OPENED_TO_SET_ACCESS: u32 = GENERIC_WRITE | WRITE_DAC;

/// How a file for the user alone is shared while it is open to be made and
/// written: with no one, but to be deleted. No one else may open it to read
/// or write it while it is open, as in the moment before its list is set
/// ([`keep_to_the_user`]), whatever its folder lets them do.
#[cfg(windows)]
pub(super) const SHARED_TO_DELETE_ALONE: u32 = FILE_SHARE_DELETE;

/// Make `file`, opened to be given its list ([`OPENED_TO_SET_ACCESS`]),
/// the user's alone: the account this app runs as alone may open it
/// ([`for_the_user_alone`]).
#[cfg(windows)]
pub(super) fn keep_to_the_user(file: &fs::File) -> io::Result<()> {
    let sid = user_sid()?;
    let list = for_the_user_alone(&sid)
        .ok_or_else(|| io::Error::other(format!("{sid} is no account's identifier")))?;
    let descriptor = descriptor(&list)?;
    let (mut present, mut defaulted) = (0, 0);
    let mut dacl = ptr::null_mut();
    // Safety: the descriptor is one Windows made, held until this returns,
    // and each answer goes to a local.
    if unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    // No list at all, given to a file, would let everyone in.
    if present == 0 || dacl.is_null() {
        return Err(io::Error::other(format!("{list} holds no access list")));
    }
    // Safety: the handle is `file`'s, open while it is borrowed; the list
    // lies in `descriptor`, held past the call; no owner, group or system
    // list is given, so none is changed.
    let set = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null(),
        )
    };
    match set {
        ERROR_SUCCESS => Ok(()),
        code => Err(io::Error::from_raw_os_error(code as i32)),
    }
}

/// The identifier of the account this app runs as, written out (`S-1-…`).
#[cfg(windows)]
fn user_sid() -> io::Result<String> {
    let mut token = ptr::null_mut();
    // Safety: this process's own pseudo-handle, which needs no closing; the
    // token's handle goes to a local, closed as `Handle` goes.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = Handle(token);
    let mut needed = 0;
    // Safety: asked with no room, Windows only says how much it needs.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut needed) };
    // Of u64s, so that the TOKEN_USER Windows writes at its start is
    // aligned as the pointer in it must be.
    let mut room = vec![0u64; (needed as usize).div_ceil(8)];
    // Safety: `room` is as large as Windows said it needs, and aligned for
    // what it writes there.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            room.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // Safety: `room` begins with a TOKEN_USER now, whose identifier lies
    // in `room` after it.
    let sid = unsafe { (*room.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut text = ptr::null_mut();
    // Safety: `sid` is the identifier Windows wrote, in `room`, held past
    // the call; the text goes to a local, freed as `Local` goes.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let text = Local(text.cast());
    // Safety: Windows wrote the text there, ending in a NUL.
    Ok(unsafe { wide_text(text.0.cast()) })
}

/// The security descriptor `sddl` writes out.
#[cfg(windows)]
fn descriptor(sddl: &str) -> io::Result<Local> {
    let wide: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
    let mut made = ptr::null_mut();
    // Safety: `wide` ends in a NUL; the descriptor goes to a local, freed
    // as `Local` goes.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide.as_ptr(),
            SDDL_REVISION_1,
            &mut made,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Local(made))
}

/// The text of the UTF-16 at `wide`, up to the NUL it ends in.
///
/// # Safety
///
/// `wide` points at UTF-16 that ends in a NUL, held while this reads it.
#[cfg(windows)]
unsafe fn wide_text(wide: *const u16) -> String {
    let mut len = 0;
    // Safety: every unit up to the NUL is the text's.
    while unsafe { *wide.add(len) } != 0 {
        len += 1;
    }
    // Safety: the `len` units before the NUL.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(wide, len) })
}

/// What Windows made for this app to free with `LocalFree`, freed as it
/// goes.
#[cfg(windows)]
struct Local(*mut c_void);

#[cfg(windows)]
impl Drop for Local {
    fn drop(&mut self) {
        // Safety: Windows made it for this app to free, and only this frees
        // it.
        unsafe { LocalFree(self.0) };
    }
}

/// A handle this app opened, closed as it goes.
#[cfg(windows)]
struct Handle(HANDLE);

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        // Safety: the handle is this app's, and only this closes it.
        unsafe { CloseHandle(self.0) };
    }
}

/// The access list `file` has, opened to be read (as `File::open` opens
/// it), in the words Windows writes one in (SDDL), with what Windows
/// notes of how the list was made dropped: whether it took up what its
/// folder gives (`AI`).
#[cfg(all(windows, test))]
pub(super) fn access_list(file: &fs::File) -> io::Result<String> {
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetSecurityInfo,
    };
    let mut made = ptr::null_mut();
    // Safety: the handle is `file`'s, open while it is borrowed; only the
    // whole descriptor is asked for, freed as `Local` goes.
    let got = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut made,
        )
    };
    if got != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(got as i32));
    }
    let descriptor = Local(made);
    let mut text = ptr::null_mut();
    // Safety: a descriptor Windows made, held past the call; the text goes
    // to a local, freed as `Local` goes.
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor.0,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let text = Local(text.cast());
    // Safety: Windows wrote the text there, ending in a NUL.
    let list = unsafe { wide_text(text.0.cast()) };
    Ok(list
        .replacen("D:PAI(", "D:P(", 1)
        .replacen("D:AI(", "D:(", 1))
}

/// The list a file for the account this app runs as alone is given.
#[cfg(all(windows, test))]
pub(super) fn the_users_alone() -> io::Result<String> {
    let sid = user_sid()?;
    for_the_user_alone(&sid).ok_or_else(|| io::Error::other(sid))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The list a file for the user alone is given holds one entry, which
    /// allows the user every right to it, and inherits none from its
    /// folder: no one else, not the folder's other users nor its
    /// administrators, may open it. Only an identifier written out as
    /// Windows writes one (`S-1-…`) is taken, so that nothing else can
    /// stand in for the user there, an alias for everyone or more entries
    /// written into the list.
    #[test]
    fn the_access_list_of_a_file_for_the_user_alone_names_the_user_alone() {
        let user = "S-1-5-21-1004336348-1177238915-682003330-1001";
        assert_eq!(
            for_the_user_alone(user).as_deref(),
            Some("D:P(A;;FA;;;S-1-5-21-1004336348-1177238915-682003330-1001)"),
            "one entry, the user's, with every right, and none inherited"
        );
        assert_eq!(
            for_the_user_alone("S-1-12-1-3570982434-1240345611-1867716228-2617614006").as_deref(),
            Some("D:P(A;;FA;;;S-1-12-1-3570982434-1240345611-1867716228-2617614006)"),
            "an account of an organisation's directory"
        );
        for not_a_user in [
            "",
            "WD",
            "BA",
            "S-1-",
            "S-1-5--1001",
            "S-1-5-21-1)(A;;FA;;;WD",
            "S-1-5-21-1;WD",
            "s-1-5-21-1",
        ] {
            assert_eq!(for_the_user_alone(not_a_user), None, "{not_a_user:?}");
        }
    }
}
