//! Windows 创建时收容的内部进程执行器。读端独占，不等待 EOF 或 drain 线程。
use std::os::windows::{ffi::OsStrExt, process::ExitStatusExt};
use std::{
    ffi::{OsStr, OsString},
    mem,
    path::{Path, PathBuf},
    process::{ExitStatus, Output},
    ptr,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::*,
    System::{JobObjects::*, Pipes::*, Threading::*},
};

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub timeout: Duration,
    pub stdout: Option<usize>,
    pub stderr: Option<usize>,
}

#[derive(Debug)]
pub(crate) enum Error {
    Spawn(std::io::Error),
    Containment,
    Timeout,
    OutputTooLarge,
    Io,
    Cleanup { cause: Option<Box<Error>> },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(f, "启动进程失败：{error}"),
            Self::Containment => write!(f, "创建时进程收容失败"),
            Self::Timeout => write!(f, "执行超时"),
            Self::OutputTooLarge => write!(f, "输出超过预算"),
            Self::Io => write!(f, "进程或管道操作失败"),
            Self::Cleanup { cause: Some(cause) } => write!(f, "进程树清理失败；原始原因：{cause}"),
            Self::Cleanup { cause: None } => write!(f, "进程树清理失败"),
        }
    }
}

// 可注入的失败边界只在私有执行器内使用，生产路径均调用真实 Win32 API。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    JobCreate,
    JobConfigure,
    PipeCreate,
    PipeInheritance,
    Input,
    AttributesSize,
    AttributesInit,
    HandleList,
    JobList,
    Spawn,
    Peek,
    Read,
    Wait,
    ExitCode,
    Terminate,
    Query,
}
pub(crate) trait WinOps {
    fn before(&self, operation: Operation) -> Result<(), Error>;
}
struct Native;
impl WinOps for Native {
    fn before(&self, _: Operation) -> Result<(), Error> {
        Ok(())
    }
}

struct Handle(HANDLE);
impl Handle {
    fn new(raw: HANDLE) -> Result<Self, Error> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            Err(Error::Spawn(std::io::Error::last_os_error()))
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct Attributes {
    storage: Vec<usize>,
}
impl Attributes {
    fn new(ops: &dyn WinOps) -> Result<Self, Error> {
        ops.before(Operation::AttributesSize)?;
        let mut size = 0;
        unsafe {
            InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut size);
        }
        if size == 0 {
            return Err(Error::Containment);
        }
        ops.before(Operation::AttributesInit)?;
        let mut result = Self {
            storage: vec![0; size.div_ceil(mem::size_of::<usize>())],
        };
        if unsafe { InitializeProcThreadAttributeList(result.as_ptr(), 2, 0, &mut size) } == 0 {
            // 初始化失败的列表不能交给 DeleteProcThreadAttributeList。
            result.storage.clear();
            return Err(Error::Containment);
        }
        Ok(result)
    }
    fn as_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
    fn set(&mut self, kind: u32, handles: &[HANDLE]) -> Result<(), Error> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.as_ptr(),
                0,
                kind as usize,
                handles.as_ptr().cast(),
                mem::size_of_val(handles),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(Error::Containment);
        }
        Ok(())
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        if !self.storage.is_empty() {
            unsafe {
                DeleteProcThreadAttributeList(self.as_ptr());
            }
        }
    }
}

fn wide(value: &OsStr) -> Result<Vec<u16>, Error> {
    let mut value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(Error::Spawn(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "参数含NUL",
        )));
    }
    value.push(0);
    Ok(value)
}

fn quote(value: &OsStr, output: &mut Vec<u16>) -> Result<(), Error> {
    output.push(b'"' as u16);
    let mut slashes = 0;
    for unit in value.encode_wide() {
        if unit == 0 {
            return Err(Error::Spawn(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "参数含NUL",
            )));
        }
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        output.extend(std::iter::repeat_n(
            b'\\' as u16,
            if unit == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        slashes = 0;
        output.push(unit);
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

pub(crate) fn find_executable(name: &OsStr) -> Result<PathBuf, Error> {
    let path = Path::new(name);
    if path.is_absolute() {
        return Ok(path.to_owned());
    }
    if path.components().count() != 1 {
        return Err(Error::Spawn(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "程序必须为绝对路径或PATH名称",
        )));
    }
    let file = if path.extension().is_none() {
        path.with_extension("exe")
    } else {
        path.to_owned()
    };
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        if !dir.is_absolute() {
            continue;
        }
        let candidate = dir.join(&file);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(Error::Spawn(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "找不到可执行程序",
    )))
}

fn pipe(sa: &SECURITY_ATTRIBUTES, ops: &dyn WinOps) -> Result<(Handle, Handle), Error> {
    let (mut read, mut write) = (ptr::null_mut(), ptr::null_mut());
    ops.before(Operation::PipeCreate)?;
    if unsafe { CreatePipe(&mut read, &mut write, sa, 0) } == 0 {
        return Err(Error::Spawn(std::io::Error::last_os_error()));
    }
    let (read, write) = (Handle::new(read)?, Handle::new(write)?);
    ops.before(Operation::PipeInheritance)?;
    if unsafe { SetHandleInformation(read.0, HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(Error::Containment);
    }
    Ok((read, write))
}

// 每个HANDLE只有本线程读取；每轮有限工作量，连续有输出也必须检查期限。
fn pump(
    handle: &Handle,
    output: &mut Vec<u8>,
    limit: Option<usize>,
    deadline: Instant,
    ops: &dyn WinOps,
) -> Result<bool, Error> {
    let mut progress = false;
    for _ in 0..8 {
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        let mut available = 0;
        ops.before(Operation::Peek)?;
        if unsafe {
            PeekNamedPipe(
                handle.0,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut available,
                ptr::null_mut(),
            )
        } == 0
        {
            if unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
                return Ok(progress);
            }
            return Err(Error::Io);
        }
        if available == 0 {
            return Ok(progress);
        }
        let mut buffer = [0_u8; 8192];
        let mut read = 0;
        ops.before(Operation::Read)?;
        if unsafe {
            ReadFile(
                handle.0,
                buffer.as_mut_ptr(),
                available.min(buffer.len() as u32),
                &mut read,
                ptr::null_mut(),
            )
        } == 0
        {
            if unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
                return Ok(progress);
            }
            return Err(Error::Io);
        }
        if read == 0 {
            return Ok(progress);
        }
        if limit.is_some_and(|n| output.len().saturating_add(read as usize) > n) {
            return Err(Error::OutputTooLarge);
        }
        output.extend_from_slice(&buffer[..read as usize]);
        progress = true;
    }
    Ok(progress)
}

pub(crate) fn run(
    exe: &Path,
    args: &[OsString],
    cwd: Option<&Path>,
    limits: Limits,
) -> Result<Output, Error> {
    run_with_ops(exe, args, cwd, limits, &Native)
}

pub(crate) fn run_with_ops(
    exe: &Path,
    args: &[OsString],
    cwd: Option<&Path>,
    limits: Limits,
    ops: &dyn WinOps,
) -> Result<Output, Error> {
    let deadline = Instant::now()
        .checked_add(limits.timeout)
        .ok_or(Error::Timeout)?;
    if !exe.is_absolute()
        || !exe
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
    {
        return Err(Error::Spawn(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "要求绝对exe路径",
        )));
    }
    let application = wide(exe.as_os_str())?;
    let directory = cwd.map(|p| wide(p.as_os_str())).transpose()?;
    let mut command = Vec::new();
    quote(exe.as_os_str(), &mut command)?;
    for arg in args {
        command.push(b' ' as u16);
        quote(arg, &mut command)?;
    }
    command.push(0);
    if command.len() > 32767 {
        return Err(Error::Spawn(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "命令行过长",
        )));
    }
    ops.before(Operation::JobCreate)?;
    let job = Handle::new(unsafe { CreateJobObjectW(ptr::null(), ptr::null()) })
        .map_err(|_| Error::Containment)?;
    let mut job_limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
    job_limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    ops.before(Operation::JobConfigure)?;
    if unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&job_limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            mem::size_of_val(&job_limits) as u32,
        )
    } == 0
    {
        return Err(Error::Containment);
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: 1,
    };
    let (out_read, out_write) = pipe(&sa, ops)?;
    let (err_read, err_write) = pipe(&sa, ops)?;
    let null_name = wide(OsStr::new("NUL"))?;
    ops.before(Operation::Input)?;
    let input = Handle::new(unsafe {
        CreateFileW(
            null_name.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &sa,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    })?;
    let handles = [input.0, out_write.0, err_write.0];
    let jobs = [job.0];
    let mut attrs = Attributes::new(ops)?;
    ops.before(Operation::HandleList)?;
    attrs.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &handles)?;
    ops.before(Operation::JobList)?;
    attrs.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
    let mut startup: STARTUPINFOEXW = unsafe { mem::zeroed() };
    startup.StartupInfo.cb = mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = input.0;
    startup.StartupInfo.hStdOutput = out_write.0;
    startup.StartupInfo.hStdError = err_write.0;
    startup.lpAttributeList = attrs.as_ptr();
    let mut info: PROCESS_INFORMATION = unsafe { mem::zeroed() };
    ops.before(Operation::Spawn)?;
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            ptr::null(),
            directory.as_ref().map_or(ptr::null(), |d| d.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    } == 0
    {
        return Err(Error::Spawn(std::io::Error::last_os_error()));
    }
    let process = Handle::new(info.hProcess)?;
    drop(Handle::new(info.hThread)?);
    drop(input);
    drop(out_write);
    drop(err_write);
    drop(attrs);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let outcome = loop {
        let progressed =
            match pump(&out_read, &mut stdout, limits.stdout, deadline, ops).and_then(|out| {
                pump(&err_read, &mut stderr, limits.stderr, deadline, ops).map(|err| out || err)
            }) {
                Ok(progressed) => progressed,
                Err(error) => break Err(error),
            };
        if let Err(error) = ops.before(Operation::Wait) {
            break Err(error);
        }
        match unsafe { WaitForSingleObject(process.0, 0) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if let Err(error) = ops.before(Operation::ExitCode) {
                    break Err(error);
                }
                if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
                    break Err(Error::Io);
                }
                break Ok(code);
            }
            WAIT_TIMEOUT => {}
            _ => break Err(Error::Io),
        }
        // 有输出时继续公平轮转，避免 Windows 粗粒度 Sleep 限制管道吞吐。
        if !progressed {
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    // 根进程正常结束仍终止其后代；异常时保留原始原因，但不能把清理失败报成功。
    let cleanup_deadline = Instant::now() + Duration::from_secs(1);
    let cleanup_error = || Error::Cleanup {
        cause: outcome.as_ref().err().map(|error| {
            Box::new(match error {
                Error::Timeout => Error::Timeout,
                Error::OutputTooLarge => Error::OutputTooLarge,
                Error::Containment => Error::Containment,
                _ => Error::Io,
            })
        }),
    };
    if ops.before(Operation::Terminate).is_err() || unsafe { TerminateJobObject(job.0, 1) } == 0 {
        return Err(cleanup_error());
    }
    loop {
        let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { mem::zeroed() };
        if ops.before(Operation::Query).is_err() {
            return Err(cleanup_error());
        }
        if unsafe {
            QueryInformationJobObject(
                job.0,
                JobObjectBasicAccountingInformation,
                (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                mem::size_of_val(&accounting) as u32,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(cleanup_error());
        }
        if accounting.ActiveProcesses == 0
            && unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0
        {
            break;
        }
        if Instant::now() >= cleanup_deadline {
            return Err(cleanup_error());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let code = outcome?;
    let drain_error = |error| match error {
        Error::Timeout => Error::Cleanup { cause: None },
        other => other,
    };
    loop {
        let out = pump(&out_read, &mut stdout, limits.stdout, cleanup_deadline, ops)
            .map_err(drain_error)?;
        let err = pump(&err_read, &mut stderr, limits.stderr, cleanup_deadline, ops)
            .map_err(drain_error)?;
        if !out && !err {
            break;
        }
    }
    Ok(Output {
        status: ExitStatus::from_raw(code),
        stdout,
        stderr,
    })
}
