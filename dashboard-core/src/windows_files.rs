//! RepoWiki 的只读 Windows 边界：逐段句柄打开，持有祖先，禁止重解析与硬链接。
use std::os::windows::{
    ffi::OsStrExt,
    io::{AsRawHandle, FromRawHandle},
};
use std::{
    collections::HashSet,
    ffi::OsStr,
    fs::File,
    io::{self, Read},
    mem,
    path::Path,
    ptr,
};
use windows_sys::{
    Wdk::{Foundation::OBJECT_ATTRIBUTES, Storage::FileSystem as nt},
    Win32::{Foundation::*, Storage::FileSystem::*, System::IO::IO_STATUS_BLOCK},
};

fn unsafe_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "RepoWiki 路径或文件不符合 Windows 安全边界",
    )
}
fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

pub(crate) fn validate_lexical(path: &Path) -> io::Result<()> {
    let value = path.to_str().ok_or_else(unsafe_path)?;
    let bytes = value.as_bytes();
    if value.encode_utf16().count() >= 260
        || bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'/' | b'\\')
    {
        return Err(unsafe_path());
    }
    for part in value[3..].split(['/', '\\']) {
        if part.is_empty() {
            continue;
        }
        if part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part.chars().any(|c| c < ' ' || "<>:\"|?*".contains(c))
        {
            return Err(unsafe_path());
        }
        let name = part.split('.').next().unwrap().to_uppercase();
        if matches!(
            name.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || ["COM", "LPT"].iter().any(|prefix| {
            name.strip_prefix(prefix).is_some_and(|n| {
                matches!(
                    n,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        }) {
            return Err(unsafe_path());
        }
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct Identity {
    volume: u32,
    index: u64,
    size: u64,
    write: u64,
    change: i64,
    links: u32,
    attrs: u32,
}
fn identity(file: &File) -> io::Result<Identity> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
    let mut basic: FILE_BASIC_INFO = unsafe { mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
        || unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                mem::size_of_val(&basic) as u32,
            )
        } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(unsafe_path());
    }
    Ok(Identity {
        volume: info.dwVolumeSerialNumber,
        index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
        size: ((info.nFileSizeHigh as u64) << 32) | info.nFileSizeLow as u64,
        write: ((info.ftLastWriteTime.dwHighDateTime as u64) << 32)
            | info.ftLastWriteTime.dwLowDateTime as u64,
        change: basic.ChangeTime,
        links: info.nNumberOfLinks,
        attrs: info.dwFileAttributes,
    })
}

fn names_unique(parent: &File) -> io::Result<()> {
    // 枚举不访问目标；逐段打开仍相对同一目录句柄。
    let mut buffer = vec![0_u64; 8192];
    let mut names = HashSet::new();
    let mut restart = true;
    loop {
        let mut status: IO_STATUS_BLOCK = unsafe { mem::zeroed() };
        let code = unsafe {
            nt::NtQueryDirectoryFile(
                parent.as_raw_handle(),
                ptr::null_mut(),
                None,
                ptr::null(),
                &mut status,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                nt::FileNamesInformation,
                false,
                ptr::null(),
                restart,
            )
        };
        if code == STATUS_NO_MORE_FILES {
            break;
        }
        if code < 0 {
            return Err(unsafe_path());
        }
        restart = false;
        let data =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), status.Information) };
        let mut offset = 0;
        loop {
            if offset + 12 > data.len() {
                return Err(unsafe_path());
            }
            let next = u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            let length =
                u32::from_ne_bytes(data[offset + 8..offset + 12].try_into().unwrap()) as usize;
            if length % 2 != 0 || offset + 12 + length > data.len() {
                return Err(unsafe_path());
            }
            let units: Vec<u16> = data[offset + 12..offset + 12 + length]
                .chunks_exact(2)
                .map(|s| u16::from_ne_bytes([s[0], s[1]]))
                .collect();
            let name = String::from_utf16(&units)
                .map_err(|_| unsafe_path())?
                .to_lowercase();
            if !names.insert(name) {
                return Err(unsafe_path());
            }
            if next == 0 {
                break;
            }
            if next < 12 {
                return Err(unsafe_path());
            }
            offset += next;
        }
    }
    Ok(())
}

fn child(parent: &File, name: &OsStr, directory: bool) -> io::Result<File> {
    names_unique(parent)?;
    let mut units: Vec<u16> = name.encode_wide().collect();
    let mut text = UNICODE_STRING {
        Length: (units.len() * 2) as u16,
        MaximumLength: (units.len() * 2) as u16,
        Buffer: units.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut text,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: ptr::null_mut(),
        SecurityQualityOfService: ptr::null_mut(),
    };
    let mut raw = ptr::null_mut();
    let mut status: IO_STATUS_BLOCK = unsafe { mem::zeroed() };
    let share = FILE_SHARE_READ | if directory { FILE_SHARE_WRITE } else { 0 };
    let code = unsafe {
        nt::NtCreateFile(
            &mut raw,
            FILE_GENERIC_READ,
            &attributes,
            &mut status,
            ptr::null(),
            0,
            share,
            nt::FILE_OPEN,
            nt::FILE_OPEN_REPARSE_POINT
                | nt::FILE_SYNCHRONOUS_IO_NONALERT
                | if directory {
                    nt::FILE_DIRECTORY_FILE
                } else {
                    nt::FILE_NON_DIRECTORY_FILE
                },
            ptr::null(),
            0,
        )
    };
    if code < 0 {
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(code) } as i32,
        ));
    }
    let file = unsafe { File::from_raw_handle(raw) };
    let metadata = identity(&file)?;
    if (metadata.attrs & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        || (!directory && metadata.links != 1)
    {
        return Err(unsafe_path());
    }
    Ok(file)
}

pub(crate) struct Pinned {
    file: File,
    _ancestors: Vec<File>,
}
fn open(path: &Path, directory: bool) -> io::Result<Pinned> {
    validate_lexical(path)?;
    let value = path.to_str().unwrap();
    let root = wide(OsStr::new(&value[..3]));
    // 映射网络盘也可能报告 NTFS；盘符形式本身不能证明是本地卷。
    if !matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 2 | 3 | 6) {
        return Err(unsafe_path());
    }
    let raw = unsafe {
        CreateFileW(
            root.as_ptr(),
            FILE_GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let root = unsafe { File::from_raw_handle(raw) };
    identity(&root)?;
    let mut filesystem = [0_u16; 32];
    if unsafe {
        GetVolumeInformationByHandleW(
            root.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    } == 0
        || &filesystem[..5] != [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0]
    {
        return Err(unsafe_path());
    }
    let parts: Vec<_> = value[3..]
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();
    let mut chain = vec![root];
    for (index, part) in parts.iter().enumerate() {
        let file = child(
            chain.last().unwrap(),
            OsStr::new(part),
            directory || index + 1 < parts.len(),
        )?;
        chain.push(file);
    }
    if parts.is_empty() && !directory {
        return Err(unsafe_path());
    }
    Ok(Pinned {
        file: chain.pop().unwrap(),
        _ancestors: chain,
    })
}
pub(crate) fn pin_file(path: &Path) -> io::Result<Pinned> {
    open(path, false)
}
pub(crate) fn pin_directory(path: &Path) -> io::Result<Pinned> {
    open(path, true)
}
pub(crate) fn validate_existing_parent(path: &Path) -> io::Result<()> {
    pin_existing_parent(path).map(|_| ())
}
pub(crate) fn pin_existing_parent(path: &Path) -> io::Result<Pinned> {
    validate_lexical(path)?;
    let mut candidate = path;
    loop {
        match pin_directory(candidate) {
            Ok(pin) => return Ok(pin),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                candidate = candidate.parent().ok_or_else(unsafe_path)?
            }
            Err(e) => return Err(e),
        }
    }
}
pub(crate) fn read(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let mut pinned = pin_file(path)?;
    let before = identity(&pinned.file)?;
    if before.size > max || before.size > usize::MAX as u64 {
        return Err(unsafe_path());
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(before.size as usize)
        .map_err(|_| io::Error::other("内存不足"))?;
    (&mut pinned.file)
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != before.size || identity(&pinned.file)? != before {
        return Err(unsafe_path());
    }
    Ok(bytes)
}

/// 核验大证据文件时流式计算摘要，避免把整份 inventory/coverage 留在内存。
pub(crate) fn sha256(
    path: &Path,
    max: u64,
    remaining: &mut u64,
    deadline: std::time::Instant,
) -> io::Result<String> {
    use sha2::{Digest, Sha256};
    let check_time = || {
        if std::time::Instant::now() >= deadline {
            Err(io::Error::from(io::ErrorKind::TimedOut))
        } else {
            Ok(())
        }
    };
    check_time()?;
    let mut pinned = pin_file(path)?;
    let before = identity(&pinned.file)?;
    if before.size > max {
        return Err(unsafe_path());
    }
    if before.size > *remaining {
        return Err(io::Error::from(io::ErrorKind::FileTooLarge));
    }
    *remaining -= before.size;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        check_time()?;
        let count = pinned.file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.checked_add(count as u64).ok_or_else(unsafe_path)?;
        if total > max {
            return Err(unsafe_path());
        }
        digest.update(&buffer[..count]);
    }
    if total != before.size || identity(&pinned.file)? != before {
        return Err(unsafe_path());
    }
    check_time()?;
    Ok(format!("{:x}", digest.finalize()))
}
