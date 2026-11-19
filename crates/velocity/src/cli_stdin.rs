//! Classification and bounded reading of the client's standard input.
use crate::{cli::Options, instance::Stdin};
use std::{io::Read, path::Path};

fn wants_input(explicit: bool, has_files: bool, launch_services: bool, file_type: u32) -> bool {
    explicit
        || (!has_files
            && !launch_services
            && [u32::from(libc::S_IFIFO), u32::from(libc::S_IFREG)].contains(&file_type))
}

pub fn read(
    options: &Options,
    launch_services: bool,
    gui_child: bool,
) -> Result<Option<Stdin>, String> {
    let explicit = options.files.iter().any(|f| f.path == Path::new("-"));
    if gui_child && !explicit {
        return Ok(None);
    }
    // SAFETY: fstat initializes the provided stat when it returns success.
    let kind = unsafe {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if libc::fstat(libc::STDIN_FILENO, stat.as_mut_ptr()) == 0 {
            u32::from(stat.assume_init().st_mode & libc::S_IFMT)
        } else {
            0
        }
    };
    if !wants_input(explicit, !options.files.is_empty(), launch_services, kind) {
        return Ok(None);
    }
    // A serde byte array is at most four wire bytes per input byte. Leave room
    // for envelope/paths under the 64 MiB protocol limit.
    const MAX_INPUT: u64 = 15 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_INPUT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_INPUT {
        return Err("stdin exceeds the 15 MiB input limit".into());
    }
    Ok(Some(Stdin::Bytes(bytes)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn implicit_stdin_requires_no_files_and_a_redirected_stream() {
        let pipe = u32::from(libc::S_IFIFO);
        let regular = u32::from(libc::S_IFREG);
        let device = u32::from(libc::S_IFCHR);
        for (explicit, files, native, kind, expected) in [
            (true, true, false, device, true),
            (false, false, false, pipe, true),
            (false, false, false, regular, true),
            (false, false, false, device, false),
            (false, false, true, pipe, false),
            (false, true, false, pipe, false),
        ] {
            assert_eq!(wants_input(explicit, files, native, kind), expected);
        }
    }
}
