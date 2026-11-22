//! Authenticate Finder Sync requests while their Apple event is still current.
use super::{lifecycle, path_from_url};
use objc2::{msg_send, rc::Retained};
use objc2_foundation::{NSAppleEventDescriptor, NSURL, NSURLComponents};
use std::{
    ffi::{CStr, OsStr, c_char},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

pub(super) const INTERNET_EVENT: u32 = u32::from_be_bytes(*b"GURL");
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");
const SENDER_AUDIT_TOKEN: u32 = u32::from_be_bytes(*b"tokn");
const APPEX: &str = "Contents/PlugIns/VelocityFinderNewFile.appex";
const EXECUTABLE: &str = "Contents/MacOS/VelocityFinderNewFile";

pub(super) fn open_event(event: &NSAppleEventDescriptor) {
    // SAFETY: AEKeyword is a UInt32; this selector returns a nullable descriptor.
    let value: Option<Retained<NSAppleEventDescriptor>> =
        unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
    let url = value
        .and_then(|value| value.stringValue())
        .and_then(|value| NSURL::URLWithString(&value));
    match url {
        Some(url) if url.isFileURL() => {
            lifecycle::receive(path_from_url(&url).into_iter().collect())
        }
        Some(url) => receive(&url, Some(event)),
        None => eprintln!("velocity: ignoring invalid Apple URL event"),
    }
}

pub(super) fn receive(url: &NSURL, event: Option<&NSAppleEventDescriptor>) {
    let Some(directory) = directory_from_url(url) else {
        eprintln!("velocity: ignoring malformed or unsupported non-file URL");
        return;
    };
    let sender = event.and_then(sender_executable);
    let resolved = sender.as_ref().and_then(|path| path.canonicalize().ok());
    if !app_bundle()
        .is_some_and(|app| is_extension_sender(&app, sender.as_deref(), resolved.as_deref()))
    {
        eprintln!("velocity: ignoring Finder New Text File URL from an unverified sender");
        return;
    }
    lifecycle::receive_finder_new_text_file(directory);
}

fn directory_from_url(url: &NSURL) -> Option<PathBuf> {
    let url = NSURLComponents::componentsWithURL_resolvingAgainstBaseURL(url, false)?;
    if url.scheme()?.to_string() != "velocity"
        || url.host()?.to_string() != "new-text-file"
        || url.user().is_some()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || !url.percentEncodedPath()?.is_empty()
    {
        return None;
    }
    let params = url.queryItems()?;
    if params.len() != 1 {
        return None;
    }
    let param = params.objectAtIndex(0);
    if param.name().to_string() != "dir" {
        return None;
    }
    let value = param.value()?.to_string();
    let path = PathBuf::from(&value);
    (path.is_absolute() && !value.contains('\0')).then_some(path)
}

fn app_bundle() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?.canonicalize().ok()?;
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app")
        .then(|| app.to_owned())
}

pub(crate) fn extension_bundle() -> Option<PathBuf> {
    let appex = app_bundle()?.join(APPEX);
    appex.join(EXECUTABLE).is_file().then_some(appex)
}

fn is_extension_sender(app: &Path, reported: Option<&Path>, resolved: Option<&Path>) -> bool {
    let expected = app.join(APPEX).join(EXECUTABLE);
    // Require both the kernel's path and its resolved spelling. An exact executable
    // match excludes other PlugIns, prefix lookalikes and links escaping this bundle.
    app.is_absolute() && reported == Some(&expected) && resolved == Some(&expected)
}

fn sender_executable(event: &NSAppleEventDescriptor) -> Option<PathBuf> {
    // SAFETY: the Apple event class and ID are both UInt32 four-character codes.
    let class: u32 = unsafe { msg_send![event, eventClass] };
    // SAFETY: same descriptor ABI as eventClass.
    let id: u32 = unsafe { msg_send![event, eventID] };
    if class != INTERNET_EVENT || id != INTERNET_EVENT {
        return None;
    }
    // SAFETY: AEKeyword is a UInt32; this selector returns a nullable descriptor.
    let attr: Option<Retained<NSAppleEventDescriptor>> =
        unsafe { msg_send![event, attributeDescriptorForKeyword: SENDER_AUDIT_TOKEN] };
    let attr = attr?;
    // SAFETY: DescType is a UInt32 four-character code.
    let kind: u32 = unsafe { msg_send![&*attr, descriptorType] };
    if kind != SENDER_AUDIT_TOKEN {
        return None;
    }
    let data = attr.data().to_vec();
    let mut token = AuditToken([0; 8]);
    if data.len() != std::mem::size_of::<AuditToken>() {
        return None;
    }
    for (value, bytes) in token.0.iter_mut().zip(data.as_chunks::<4>().0) {
        *value = u32::from_ne_bytes(*bytes);
    }
    let mut path = [0 as c_char; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: token contains the audit_token_t bytes from the event; path is a writable
    // buffer of the supplied length. This form of proc_pidpath also checks PID reuse.
    let result =
        unsafe { proc_pidpath_audittoken(&mut token, path.as_mut_ptr(), path.len() as u32) };
    if result <= 0 {
        return None;
    }
    // SAFETY: successful proc_pidpath writes a NUL-terminated filesystem path.
    let bytes = unsafe { CStr::from_ptr(path.as_ptr()) }.to_bytes();
    Some(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[repr(C)]
struct AuditToken([u32; 8]);

unsafe extern "C" {
    fn proc_pidpath_audittoken(token: *mut AuditToken, buffer: *mut c_char, size: u32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSString;

    fn parse(url: &str) -> Option<PathBuf> {
        let url = NSURL::URLWithString(&NSString::from_str(url))?;
        directory_from_url(&url)
    }

    #[test]
    fn finder_url_uses_one_absolute_decoded_directory() {
        assert_eq!(
            parse("velocity://new-text-file?dir=%2Ftmp%2F%E6%96%87%20A+%23%26%3D%25%3F"),
            Some("/tmp/文 A+#&=%?".into())
        );
        assert_eq!(parse("velocity://new-text-file?dir=/"), Some("/".into()));
        for invalid in [
            "https://new-text-file?dir=/tmp",
            "velocity://other?dir=/tmp",
            "velocity://new-text-file/extra?dir=/tmp",
            "velocity://user@new-text-file?dir=/tmp",
            "velocity://new-text-file:99?dir=/tmp",
            "velocity://new-text-file?dir=/tmp#fragment",
            "velocity://new-text-file?dir=relative",
            "velocity://new-text-file?dir=/tmp%00",
            "velocity://new-text-file?dir",
            "velocity://new-text-file?dir=",
            "velocity://new-text-file",
            "velocity://new-text-file?path=/tmp",
            "velocity://new-text-file?dir=/tmp&dir=/other",
            "velocity://new-text-file?dir=/tmp&extra=1",
            "velocity://new-text-file?dir=/tmp&",
        ] {
            assert!(parse(invalid).is_none(), "{invalid}");
        }
    }

    #[test]
    fn sender_must_be_this_copys_exact_extension_without_symlink_escapes() {
        let app = Path::new("/Applications/Velocity.app");
        let expected = app.join(APPEX).join(EXECUTABLE);
        assert!(is_extension_sender(app, Some(&expected), Some(&expected)));
        for wrong in [
            "/Applications/Velocity.app/Contents/MacOS/velocity",
            "/Applications/Velocity.app/Contents/PlugIns/not-our-extension",
            "/Applications/Velocity.app/Contents/PlugInsOther/x",
            "/Applications/Safari.app/Contents/MacOS/Safari",
            "/Applications/Other/Velocity.app/Contents/PlugIns/VelocityFinderNewFile.appex/Contents/MacOS/VelocityFinderNewFile",
        ] {
            let wrong = Path::new(wrong);
            assert!(!is_extension_sender(app, Some(wrong), Some(wrong)));
            assert!(!is_extension_sender(app, Some(wrong), Some(&expected)));
            assert!(!is_extension_sender(app, Some(&expected), Some(wrong)));
        }
        assert!(!is_extension_sender(app, None, None));
        assert!(!is_extension_sender(app, Some(&expected), None));
    }
}
