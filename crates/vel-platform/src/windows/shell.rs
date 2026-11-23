//! STA Shell services and clipboard ownership.
use super::{WindowHandle, wide};
use std::{
    ffi::{OsString, c_void},
    mem::{size_of, transmute},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf, Prefix},
    ptr::{NonNull, null, null_mut},
    slice,
};
use windows::{
    Win32::{
        Foundation::HWND as ComHwnd,
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
        },
        UI::Shell::{
            FILEOPENDIALOGOPTIONS, FOS_ALLOWMULTISELECT, FOS_DONTADDTORECENT, FOS_FILEMUSTEXIST,
            FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST,
            FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog,
            IFileSaveDialog, IShellItem, ITaskbarList3, SHCreateItemFromParsingName,
            SIGDN_FILESYSPATH, TBPF_INDETERMINATE, TBPF_NOPROGRESS, TaskbarList,
        },
    },
    core::{PCWSTR, PWSTR},
};
use windows_sys::Win32::{
    Foundation::{ERROR_SUCCESS, FreeLibrary, HGLOBAL, HWND, LPARAM, WPARAM},
    System::{
        DataExchange::{
            COPYDATASTRUCT, CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard,
            RegisterClipboardFormatW, SetClipboardData,
        },
        LibraryLoader::{
            GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
        },
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    },
    UI::{
        Controls::{TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDF_ALLOW_DIALOG_CANCELLATION},
        Shell::{DragFinish, DragQueryFileW, HDROP, SHARD_PATHW, SHAddToRecentDocs, ShellExecuteW},
        WindowsAndMessaging::*,
    },
};
use windows_sys::w;

pub(super) struct Apartment;

impl Apartment {
    pub(super) fn new() -> Result<Self, String> {
        // SAFETY: This GUI thread owns every COM interface used here. Both S_OK
        // and S_FALSE add a reference balanced by the guard's destructor.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }
            .ok()
            .map_err(|error| format!("CoInitializeEx: {error}"))?;
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: Balanced with successful initialization on this same thread.
        unsafe { CoUninitialize() };
    }
}

impl WindowHandle {
    /// Opens the filesystem file chooser, allowing several files.
    pub fn open_files(&self) -> Vec<PathBuf> {
        open_dialog(self.parent(), false).unwrap_or_default()
    }

    /// Opens the filesystem folder chooser.
    pub fn open_directory(&self) -> Option<PathBuf> {
        open_dialog(self.parent(), true).ok()?.into_iter().next()
    }

    /// Asks for a path with an optional initial name.
    pub fn save_file(&self, suggested: &str) -> Option<PathBuf> {
        self.save_file_in(suggested, None, None)
    }

    /// Opens the Save dialog with localized captions and an optional starting directory.
    pub fn save_file_in(
        &self,
        suggested: &str,
        directory: Option<&Path>,
        captions: Option<(&str, &str)>,
    ) -> Option<PathBuf> {
        save_dialog(self.parent(), suggested, directory, captions).ok()
    }

    /// Returns a zero-based button index. Escape, closing the dialog, and
    /// errors return a non-choice rather than silently confirming an action.
    pub fn alert(&self, message: &str, detail: &str, buttons: &[&str]) -> usize {
        let buttons = if buttons.is_empty() {
            &["OK"][..]
        } else {
            buttons
        };
        let buttons = &buttons[..buttons.len().min(32)];
        let hwnd = self.parent();
        task_dialog(hwnd, message, detail, buttons)
            .unwrap_or_else(|| classic_dialog(hwnd, message, detail, buttons))
    }

    /// Copies a snapshot of CF_UNICODETEXT, normalizing CRLF to editor line feeds.
    pub fn clipboard_text(&self) -> Option<String> {
        let text = {
            let _clipboard = Clipboard::open(self.parent())?;
            // SAFETY: The clipboard is open; the data remains owned by the system.
            let data = unsafe { GetClipboardData(u32::from(CF_UNICODETEXT)) };
            let locked = LockedGlobal::new(data)?;
            // SAFETY: The locked memory is aligned as a HGLOBAL allocation and
            // GlobalSize bounds the readable data. A missing terminator is rejected.
            let units = unsafe {
                slice::from_raw_parts(locked.pointer.as_ptr().cast::<u16>(), GlobalSize(data) / 2)
            };
            units[..units.iter().position(|unit| *unit == 0)?].to_vec()
        };
        Some(String::from_utf16_lossy(&text).replace("\r\n", "\n"))
    }

    /// Publishes CF_UNICODETEXT using the Windows CRLF convention.
    pub fn set_clipboard_text(&self, text: &str) -> bool {
        self.put_clipboard(text, None)
    }

    pub(crate) fn write_clipboard_html(&self, html: &str, plain_text: &str) -> bool {
        self.put_clipboard(plain_text, Some(html))
    }

    fn put_clipboard(&self, text: &str, html: Option<&str>) -> bool {
        let owner = self.parent();
        if owner.is_null() {
            return false;
        }
        let units = wide(&text.replace("\r\n", "\n").replace('\n', "\r\n"));
        let bytes: Vec<_> = units.into_iter().flat_map(u16::to_le_bytes).collect();
        let Some(text) = ClipboardData::allocate(&bytes) else {
            return false;
        };
        let html = if let Some(html) = html {
            // SAFETY: A statically terminated format name; no retained pointer.
            let format = unsafe { RegisterClipboardFormatW(w!("HTML Format")) };
            if format == 0 {
                return false;
            }
            let Some(data) = ClipboardData::allocate(&clipboard_html(html)) else {
                return false;
            };
            Some((format, data))
        } else {
            None
        };
        // Allocate and encode everything before locking the system clipboard.
        let Some(_clipboard) = Clipboard::open(owner) else {
            return false;
        };
        // SAFETY: Clipboard::open succeeded using this non-null, live owner.
        (unsafe { EmptyClipboard() } != 0)
            && text.publish(u32::from(CF_UNICODETEXT))
            && html.is_none_or(|(format, data)| data.publish(format))
    }

    /// Uses the W1 adapter, which never silently deletes an unrecyclable item.
    pub fn trash_file(&self, path: &Path) -> Result<(), String> {
        vel_fileio::trash(path).map_err(|error| error.to_string())
    }

    /// Shows a path in Explorer, selecting the item rather than opening it.
    pub fn reveal_file(&self, path: &Path) -> Result<(), String> {
        use windows::Win32::UI::Shell::{
            Common::ITEMIDLIST, SHOpenFolderAndSelectItems, SHParseDisplayName,
        };
        let name = shell_path(path)?;
        let mut pidl: *mut ITEMIDLIST = null_mut();
        // SAFETY: The path is absolute and NUL-terminated. The STA remains
        // initialized, and the parsed PIDL is released by the allocation guard.
        unsafe { SHParseDisplayName(PCWSTR(name.as_ptr()), None, &mut pidl, 0, None) }
            .map_err(|error| error.to_string())?;
        let _pidl = ShellAllocation(pidl.cast());
        if pidl.is_null() {
            return Err("Shell returned no item for path".into());
        }
        // SAFETY: With no child items, the absolute PIDL is itself selected.
        unsafe { SHOpenFolderAndSelectItems(pidl, None, 0) }.map_err(|error| error.to_string())
    }

    /// Opens a directory in the system file manager.
    pub fn show_directory(&self, path: &Path) -> Result<(), String> {
        if !path.is_dir() {
            return Err("not a directory".into());
        }
        self.shell_open(&shell_path(path)?)
    }

    pub(crate) fn open_local_html(&self, path: &Path) -> bool {
        shell_path(path).is_ok_and(|path| self.shell_open(&path).is_ok())
    }

    /// Opens an HTTP(S) URL in the default browser.
    pub fn open_url(&self, url: &str) -> Result<(), String> {
        let allowed = url.split_once(':').is_some_and(|(scheme, rest)| {
            (scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http"))
                && rest.starts_with("//")
        });
        if !allowed || url.chars().any(char::is_control) {
            return Err("unsupported URL".into());
        }
        self.shell_open(&wide(url))
    }

    fn shell_open(&self, name: &[u16]) -> Result<(), String> {
        // SAFETY: All strings are terminated and used synchronously. Pass the
        // file or URL as lpFile, not a command line or a shell expression.
        let result = unsafe {
            ShellExecuteW(
                self.parent(),
                w!("open"),
                name.as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            )
        };
        if result as isize > 32 {
            Ok(())
        } else {
            Err(format!("ShellExecuteW failed ({})", result as isize))
        }
    }

    /// Adds a document to Windows' automatic recent-documents / Jump List history.
    /// A clear request never clears other applications' system recents.
    pub fn system_recent_document(&self, path: Option<&Path>) {
        if let Some(path) = path.and_then(|path| shell_path(path).ok()) {
            // SAFETY: SHARD_PATHW takes a readable Unicode path during the call.
            unsafe { SHAddToRecentDocs(SHARD_PATHW as u32, path.as_ptr().cast()) };
        }
    }

    /// Shows an indeterminate indicator for a running editor build.
    pub fn set_taskbar_build_active(&self, active: bool) {
        let Some(hwnd) = self.hwnd() else { return };
        if self.0.taskbar_busy.get() != active && set_taskbar_progress(hwnd.as_ptr(), active) {
            self.0.taskbar_busy.set(active);
        }
    }

    fn parent(&self) -> HWND {
        self.hwnd().map_or(null_mut(), NonNull::as_ptr)
    }
}

fn set_taskbar_progress(hwnd: HWND, active: bool) -> bool {
    // SAFETY: The interface and HWND are used only in the initialized GUI STA.
    unsafe {
        let bar: windows::core::Result<ITaskbarList3> =
            CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER);
        bar.and_then(|bar| {
            bar.HrInit()?;
            let flag = if active {
                TBPF_INDETERMINATE
            } else {
                TBPF_NOPROGRESS
            };
            bar.SetProgressState(ComHwnd(hwnd), flag)
        })
        .is_ok()
    }
}

fn shell_path(path: &Path) -> Result<Vec<u16>, String> {
    let path = std::path::absolute(path).map_err(|error| error.to_string())?;
    let mut units: Vec<_> = path.as_os_str().encode_wide().collect();
    // Shell parsing names generally use ordinary DOS/UNC spelling even when
    // the IO layer canonicalized the path with a verbatim prefix.
    if units.starts_with(&[92, 92, 63, 92, 85, 78, 67, 92]) {
        units.splice(..8, [92, 92]);
    } else if units.starts_with(&[92, 92, 63, 92]) && units.get(5) == Some(&58) {
        units.drain(..4);
    }
    if units.contains(&0) {
        return Err("path contains NUL".into());
    }
    units.push(0);
    Ok(units)
}

fn open_dialog(hwnd: HWND, folders: bool) -> windows::core::Result<Vec<PathBuf>> {
    // SAFETY: COM was initialized in the GUI STA. All interfaces and transient
    // out parameters stay in this call stack and the parent HWND is live or null.
    unsafe {
        let dialog: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let flags = FOS_FORCEFILESYSTEM
            | FOS_NOCHANGEDIR
            | FOS_PATHMUSTEXIST
            | if folders {
                FOS_PICKFOLDERS
            } else {
                FOS_FILEMUSTEXIST | FOS_ALLOWMULTISELECT
            };
        configure(&dialog, flags, None, None)?;
        dialog.Show(NonNull::new(hwnd).map(|hwnd| ComHwnd(hwnd.as_ptr())))?;
        let items = dialog.GetResults()?;
        (0..items.GetCount()?)
            .map(|index| path_for_item(&items.GetItemAt(index)?))
            .collect()
    }
}

fn save_dialog(
    hwnd: HWND,
    suggested: &str,
    directory: Option<&Path>,
    captions: Option<(&str, &str)>,
) -> windows::core::Result<PathBuf> {
    // SAFETY: COM is initialized. Strings are terminated and borrowed only for
    // calls that copy them; the modal dialog owns any object refs it keeps.
    unsafe {
        let dialog: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        configure(
            &dialog,
            FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR | FOS_PATHMUSTEXIST | FOS_OVERWRITEPROMPT,
            directory,
            captions,
        )?;
        if !suggested.is_empty() {
            let suggested = wide(suggested);
            dialog.SetFileName(PCWSTR(suggested.as_ptr()))?;
        }
        dialog.Show(NonNull::new(hwnd).map(|hwnd| ComHwnd(hwnd.as_ptr())))?;
        path_for_item(&dialog.GetResult()?)
    }
}

fn configure(
    dialog: &IFileDialog,
    flags: FILEOPENDIALOGOPTIONS,
    directory: Option<&Path>,
    captions: Option<(&str, &str)>,
) -> windows::core::Result<()> {
    // SAFETY: Live interface on the STA. Every input string is NUL-terminated.
    unsafe {
        dialog.SetOptions(dialog.GetOptions()? | flags | FOS_DONTADDTORECENT)?;
        if let Some((title, button)) = captions {
            let title = wide(title);
            let button = wide(button);
            dialog.SetTitle(PCWSTR(title.as_ptr()))?;
            dialog.SetOkButtonLabel(PCWSTR(button.as_ptr()))?;
        }
        if let Some(path) = directory.and_then(|path| shell_path(path).ok())
            && let Ok(folder) =
                SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(path.as_ptr()), None)
        {
            let _ = dialog.SetFolder(&folder);
        }
    }
    Ok(())
}

struct ShellAllocation(*mut c_void);

impl Drop for ShellAllocation {
    fn drop(&mut self) {
        // SAFETY: Every pointer guarded here came from a Shell COM task allocation.
        unsafe { CoTaskMemFree(Some(self.0)) };
    }
}

fn path_for_item(item: &IShellItem) -> windows::core::Result<PathBuf> {
    // SAFETY: The returned PWSTR is COM-task allocated and NUL-terminated;
    // copy all its UTF-16 units before freeing it, without a lossy UTF-8 hop.
    unsafe {
        let display: PWSTR = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let _allocation = ShellAllocation(display.0.cast());
        Ok(PathBuf::from(OsString::from_wide(display.as_wide())))
    }
}

struct Clipboard;

impl Clipboard {
    fn open(owner: HWND) -> Option<Self> {
        // SAFETY: owner is a live HWND, or null for a clipboard read.
        (unsafe { OpenClipboard(owner) } != 0).then(|| Self)
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: Only constructed on a successful OpenClipboard.
        unsafe { CloseClipboard() };
    }
}

struct LockedGlobal {
    handle: HGLOBAL,
    pointer: NonNull<c_void>,
}

impl LockedGlobal {
    fn new(handle: HGLOBAL) -> Option<Self> {
        if handle.is_null() {
            return None;
        }
        // SAFETY: This is a clipboard or owned global-memory handle.
        let pointer = NonNull::new(unsafe { GlobalLock(handle) })?;
        Some(Self { handle, pointer })
    }
}

impl Drop for LockedGlobal {
    fn drop(&mut self) {
        // SAFETY: This guard owns the matching lock.
        unsafe { GlobalUnlock(self.handle) };
    }
}

struct ClipboardData(HGLOBAL);

impl ClipboardData {
    fn allocate(bytes: &[u8]) -> Option<Self> {
        // SAFETY: Create a movable buffer of the exact requested size.
        let block = NonNull::new(unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) })?;
        let data = Self(block.as_ptr());
        {
            let locked = LockedGlobal::new(data.0)?;
            // SAFETY: The allocation has bytes.len() writable bytes, and
            // cannot alias the caller's slice. Unlock before publishing.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    locked.pointer.as_ptr().cast(),
                    bytes.len(),
                )
            };
        }
        Some(data)
    }

    fn publish(self, format: u32) -> bool {
        // SAFETY: The clipboard is open and the moved clipboard allocation is initialized.
        if unsafe { SetClipboardData(format, self.0) }.is_null() {
            false
        } else {
            // Windows owns the allocation now; the sender must not free it.
            std::mem::forget(self);
            true
        }
    }
}

impl Drop for ClipboardData {
    fn drop(&mut self) {
        // SAFETY: Only unpublished allocations remain owned by this object.
        unsafe { windows_sys::Win32::Foundation::GlobalFree(self.0) };
    }
}

fn clipboard_html(fragment: &str) -> Vec<u8> {
    const BEFORE: &str = "<html><body><!--StartFragment-->";
    const AFTER: &str = "<!--EndFragment--></body></html>";
    let header = |html, end, start_fragment, end_fragment| {
        format!(
            "Version:1.0\r\nStartHTML:{html:010}\r\nEndHTML:{end:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
        )
    };
    let start = header(0, 0, 0, 0).len();
    let start_fragment = start + BEFORE.len();
    let end_fragment = start_fragment + fragment.len();
    let end = end_fragment + AFTER.len();
    let mut bytes = header(start, end, start_fragment, end_fragment).into_bytes();
    bytes.extend_from_slice(BEFORE.as_bytes());
    bytes.extend_from_slice(fragment.as_bytes());
    bytes.extend_from_slice(AFTER.as_bytes());
    bytes.push(0);
    bytes
}

pub(super) fn dropped_paths(wparam: WPARAM) -> Vec<PathBuf> {
    let drop = wparam as HDROP;
    // SAFETY: WM_DROPFILES transfers ownership of the HDROP. Query the length
    // before allocating each output and always end the drop with DragFinish.
    unsafe {
        let count = DragQueryFileW(drop, u32::MAX, null_mut(), 0);
        let paths = (0..count)
            .filter_map(|index| {
                let length = DragQueryFileW(drop, index, null_mut(), 0);
                if length == 0 {
                    return None;
                }
                let mut buffer = vec![0_u16; (length + 1) as usize];
                let copied = DragQueryFileW(drop, index, buffer.as_mut_ptr(), length + 1);
                (copied == length)
                    .then(|| PathBuf::from(OsString::from_wide(&buffer[..length as usize])))
            })
            .collect();
        DragFinish(drop);
        paths
    }
}

const COPYDATA_OPEN_PATHS: usize = 0x5645_4c01;
const COPYDATA_MAX: usize = 64 * 1024;

pub(super) fn copydata_paths(lparam: LPARAM) -> Option<Vec<PathBuf>> {
    if lparam == 0 {
        return None;
    }
    // SAFETY: WM_COPYDATA provides a system-marshaled COPYDATASTRUCT and a
    // readable cbData-byte buffer for this call. No pointer is stored.
    let packet = unsafe { &*(lparam as *const COPYDATASTRUCT) };
    let len = packet.cbData as usize;
    if packet.dwData != COPYDATA_OPEN_PATHS
        || packet.lpData.is_null()
        || !(4..=COPYDATA_MAX).contains(&len)
        || !len.is_multiple_of(2)
    {
        return None;
    }
    // SAFETY: Checked the size above; u8 has no alignment requirement.
    decode_paths(unsafe { slice::from_raw_parts(packet.lpData.cast(), len) })
}

fn decode_paths(bytes: &[u8]) -> Option<Vec<PathBuf>> {
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let units: Vec<_> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| u16::from_le_bytes(b))
        .collect();
    let (0, rest) = units.split_last()? else {
        return None;
    };
    rest.split(|unit| *unit == 0)
        .map(|path| {
            let path = PathBuf::from(OsString::from_wide(path));
            let filesystem = matches!(
                path.components().next(),
                Some(Component::Prefix(prefix)) if matches!(
                    prefix.kind(),
                    Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _)
                )
            );
            (filesystem && path.is_absolute()).then_some(path)
        })
        .collect()
}

type TaskDialog =
    unsafe extern "system" fn(*const TASKDIALOGCONFIG, *mut i32, *mut i32, *mut i32) -> i32;
const FIRST_BUTTON: u16 = 100;

fn task_dialog(hwnd: HWND, message: &str, detail: &str, buttons: &[&str]) -> Option<usize> {
    // ComCtl32 v6 may not be selected until the executable has a manifest.
    // Resolve at runtime so this never imposes a missing import on the binary.
    // SAFETY: Load a fixed OS DLL from the system directory, not PATH or cwd.
    let module =
        unsafe { LoadLibraryExW(w!("comctl32.dll"), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
    let module = NonNull::new(module)?;
    // SAFETY: The library is loaded. If exported, the symbol has this documented signature.
    let procedure = unsafe {
        GetProcAddress(module.as_ptr(), c"TaskDialogIndirect".as_ptr().cast())
            .map(|proc| transmute::<_, TaskDialog>(proc))
    };
    let result = procedure.and_then(|call| {
        let message = wide(message);
        let detail = wide(detail);
        let captions: Vec<_> = buttons.iter().map(|button| wide(button)).collect();
        let native: Vec<_> = captions
            .iter()
            .enumerate()
            .map(|(index, caption)| TASKDIALOG_BUTTON {
                nButtonID: i32::from(FIRST_BUTTON) + index as i32,
                pszButtonText: caption.as_ptr(),
            })
            .collect();
        let config = TASKDIALOGCONFIG {
            cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
            hwndParent: hwnd,
            pszWindowTitle: w!("Velocity"),
            pszMainInstruction: message.as_ptr(),
            pszContent: detail.as_ptr(),
            dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
            cButtons: native.len() as u32,
            pButtons: native.as_ptr(),
            nDefaultButton: i32::from(FIRST_BUTTON),
            ..Default::default()
        };
        let mut pressed = 0;
        // SAFETY: Config and caption storage remain live through the modal call.
        let status = unsafe { call(&config, &mut pressed, null_mut(), null_mut()) };
        (status >= ERROR_SUCCESS as i32).then(|| answer(pressed as isize, buttons.len()))
    });
    // SAFETY: The call returned before releasing its DLL.
    unsafe { FreeLibrary(module.as_ptr()) };
    result
}

fn answer(id: isize, buttons: usize) -> usize {
    id.checked_sub(FIRST_BUTTON as isize)
        .and_then(|index| usize::try_from(index).ok())
        .filter(|index| *index < buttons)
        .unwrap_or(usize::MAX)
}

fn classic_dialog(hwnd: HWND, message: &str, detail: &str, buttons: &[&str]) -> usize {
    let template = alert_template(message, detail, buttons);
    // SAFETY: The DWORD-aligned template and all embedded strings are live for
    // the whole modal call. The dialog callback carries no Rust pointers.
    let pressed = unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr().cast(),
            hwnd,
            Some(dialog_proc),
            0,
        )
    };
    answer(pressed, buttons.len())
}

fn alert_template(message: &str, detail: &str, buttons: &[&str]) -> Vec<u32> {
    // Standard DLGTEMPLATE: fixed header, then menu/class/title/font, then
    // DWORD-aligned DLGITEMTEMPLATEs with predefined button/static class atoms.
    let mut words = Vec::<u16>::new();
    let widths: Vec<_> = buttons
        .iter()
        .map(|s| (s.encode_utf16().count() * 6 + 12).clamp(56, 220) as u16)
        .collect();
    let button_rows = buttons.len().div_ceil(3) as u16;
    let width = 350_u16.max(
        widths
            .chunks(3)
            .map(|row| row.iter().sum::<u16>() + 40)
            .max()
            .unwrap_or(0),
    );
    let line_width = usize::from(width.saturating_sub(18)) / 5;
    let lines = |text: &str| {
        text.lines()
            .map(|line| line.encode_utf16().count().div_ceil(line_width).max(1))
            .sum::<usize>()
    };
    let message_height = (lines(message).clamp(1, 6) * 13) as u16;
    let detail_height = (lines(detail).clamp(1, 12) * 12) as u16;
    let button_top = 21 + message_height + detail_height;
    let height = button_top + 12 + button_rows * 18;
    dword(
        &mut words,
        WS_POPUP | WS_CAPTION | WS_SYSMENU | (DS_CENTER | DS_MODALFRAME | DS_SETFONT) as u32,
    );
    dword(&mut words, 0);
    words.extend([2 + buttons.len() as u16, 0, 0, width, height, 0, 0]);
    words.extend(wide("Velocity"));
    words.push(9);
    words.extend(wide("Segoe UI"));

    let mut item = |style, [x, y, w, h]: [u16; 4], id, class, text: &str| {
        if !words.len().is_multiple_of(2) {
            words.push(0);
        }
        dword(&mut words, style | WS_CHILD | WS_VISIBLE);
        dword(&mut words, 0);
        words.extend([x, y, w, h, id, u16::MAX, class]);
        words.extend(wide(text));
        words.push(0); // no creation data
    };
    let no_prefix = 0x80; // SS_NOPREFIX for literal ampersands
    item(
        no_prefix,
        [9, 9, width - 18, message_height],
        0xffff,
        0x0082,
        message,
    );
    item(
        no_prefix,
        [9, 14 + message_height, width - 18, detail_height],
        0xffff,
        0x0082,
        detail,
    );
    for (row, items) in buttons.chunks(3).enumerate() {
        let offset = row * 3;
        let widths = &widths[offset..offset + items.len()];
        let mut left = width - 9 - widths.iter().sum::<u16>() - 6 * (items.len() - 1) as u16;
        for (column, (text, w)) in items.iter().zip(widths.iter().copied()).enumerate() {
            let index = offset + column;
            let style = WS_TABSTOP
                | if index == 0 {
                    BS_DEFPUSHBUTTON as u32
                } else {
                    0
                };
            item(
                style,
                [left, button_top + row as u16 * 18, w, 14],
                FIRST_BUTTON + index as u16,
                0x0080,
                &text.replace('&', "&&"),
            );
            left += w + 6;
        }
    }
    if !words.len().is_multiple_of(2) {
        words.push(0);
    }
    words
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u32::from(pair[0]) | u32::from(pair[1]) << 16)
        .collect()
}

fn dword(words: &mut Vec<u16>, value: u32) {
    words.extend([value as u16, (value >> 16) as u16]);
}

unsafe extern "system" fn dialog_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    _: LPARAM,
) -> isize {
    let pressed = wparam as u16;
    let selected = if message == WM_COMMAND && (FIRST_BUTTON..FIRST_BUTTON + 32).contains(&pressed)
    {
        pressed as isize
    } else if message == WM_CLOSE || (message == WM_COMMAND && pressed == IDCANCEL as u16) {
        isize::from(IDCANCEL as i16)
    } else if message == WM_INITDIALOG {
        return 1;
    } else {
        return 0;
    };
    // SAFETY: Use the HWND from the modal dialog manager's active callback.
    unsafe { EndDialog(hwnd, selected) };
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copydata_only_accepts_framed_absolute_paths() {
        let bytes = |s: &str| {
            s.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        };
        let paths = decode_paths(&bytes("C:\\hello.txt\0C:\\work\0")).unwrap();
        assert_eq!(
            paths,
            [PathBuf::from("C:\\hello.txt"), PathBuf::from("C:\\work")]
        );
        assert!(decode_paths(&bytes("C:\\hello.txt")).is_none());
        assert!(decode_paths(&bytes("relative.txt\0")).is_none());
        assert!(decode_paths(&bytes("\\\\.\\PIPE\\Velocity\0")).is_none());
        assert!(decode_paths(&[0]).is_none());
    }

    #[test]
    fn clipboard_html_byte_offsets_cover_the_original_fragment() {
        let fragment = "<b>\u{1f980}</b>";
        let data = clipboard_html(fragment);
        let body = std::str::from_utf8(&data).unwrap();
        let offset = |key| {
            body.lines()
                .find_map(|line| line.strip_prefix(key))
                .unwrap()
                .parse::<usize>()
                .unwrap()
        };
        assert_eq!(
            &body[offset("StartFragment:")..offset("EndFragment:")],
            fragment
        );
        assert_eq!(&body[offset("EndHTML:")..], "\0");
    }

    #[test]
    fn cancelled_dialog_never_returns_a_real_button() {
        assert_eq!(answer(100, 3), 0);
        assert_eq!(answer(102, 3), 2);
        for id in [-1, IDCANCEL as isize, 103] {
            assert_eq!(answer(id, 3), usize::MAX);
        }
    }
}
