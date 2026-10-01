//! Starting on every Linux, NixOS included. Releases carry no PT_INTERP (`linux/package.sh`
//! clears it), so the kernel starts them at `sb_entry` as it would a static executable;
//! stage 1 then re-executes the file under the dynamic loader `/bin/sh` uses. Started by a
//! dynamic loader, `sb_entry` hands straight to glibc's `_start`.

use std::{
    ffi::{CStr, CString, OsStr, c_void},
    io,
    mem::MaybeUninit,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

// A dynamic loader passes its finalizer to `_start` in x0 or rdx; the kernel passes zero.
#[cfg(target_arch = "aarch64")]
std::arch::global_asm!(
    ".globl sb_entry",
    ".type sb_entry, %function",
    "sb_entry:",
    "cbz x0, 1f",
    "b _start",
    "1: mov x0, sp",
    "b {stage1}",
    stage1 = sym stage1,
);
#[cfg(target_arch = "x86_64")]
std::arch::global_asm!(
    ".globl sb_entry",
    ".type sb_entry, @function",
    "sb_entry:",
    "test rdx, rdx",
    "jnz _start",
    "mov rdi, rsp",
    "and rsp, -16",
    "call {stage1}",
    stage1 = sym stage1,
);

#[cfg(target_arch = "aarch64")]
mod call {
    pub const OPENAT: usize = 56;
    pub const PREAD: usize = 67;
    pub const READLINKAT: usize = 78;
    pub const EXECVE: usize = 221;
    pub const EXIT: usize = 93;
    pub const WRITE: usize = 64;
    pub const MMAP: usize = 222;

    pub unsafe fn syscall(number: usize, a: usize, b: usize, c: usize, d: usize) -> isize {
        let result;
        unsafe {
            std::arch::asm!("svc 0", in("x8") number, inlateout("x0") a => result, in("x1") b,
                in("x2") c, in("x3") d, in("x4") usize::MAX, in("x5") 0, options(nostack))
        };
        result
    }
}
#[cfg(target_arch = "x86_64")]
mod call {
    pub const OPENAT: usize = 257;
    pub const PREAD: usize = 17;
    pub const READLINKAT: usize = 267;
    pub const EXECVE: usize = 59;
    pub const EXIT: usize = 60;
    pub const WRITE: usize = 1;
    pub const MMAP: usize = 9;

    pub unsafe fn syscall(number: usize, a: usize, b: usize, c: usize, d: usize) -> isize {
        let result;
        unsafe {
            std::arch::asm!("syscall", inlateout("rax") number => result, in("rdi") a,
                in("rsi") b, in("rdx") c, in("r10") d, in("r8") usize::MAX, in("r9") 0,
                lateout("rcx") _, lateout("r11") _, options(nostack))
        };
        result
    }
}

const CWD: usize = -100isize as usize;
const O_CLOEXEC: usize = 0o2000000;
static PROC_SELF_EXE: [u8; 15] = *b"/proc/self/exe\0";
static SHELLS: [[u8; 13]; 2] = [*b"/bin/sh\0\0\0\0\0\0", *b"/usr/bin/env\0"];
static NO_LOADER: [u8; 62] = *b"snowbound: no dynamic loader; Snowbound runs on glibc systems\n";

/// The interpreter of the ELF at `path` into `buffer`, NUL-terminated; false where it has
/// none.
unsafe fn interpreter(path: *const u8, buffer: *mut u8, capacity: usize) -> bool {
    use call::*;
    unsafe {
        let file = syscall(OPENAT, CWD, path as usize, O_CLOEXEC, 0);
        if file < 0 {
            return false;
        }
        let file = file as usize;
        let mut header = MaybeUninit::<[u64; 8]>::uninit();
        let header = header.as_mut_ptr() as *mut u8;
        if syscall(PREAD, file, header as usize, 64, 0) != 64
            || (header as *const u32).read_unaligned() != u32::from_le_bytes(*b"\x7fELF")
        {
            return false;
        }
        let table = (header.add(32) as *const u64).read_unaligned() as usize;
        let size = (header.add(54) as *const u16).read_unaligned() as usize;
        let count = (header.add(56) as *const u16).read_unaligned() as usize;
        let mut entry = MaybeUninit::<[u64; 7]>::uninit();
        let entry = entry.as_mut_ptr() as *mut u8;
        let mut index = 0;
        while index < count {
            if syscall(PREAD, file, entry as usize, 56, table + index * size) == 56
                && (entry as *const u32).read_unaligned() == 3
            {
                let offset = (entry.add(8) as *const u64).read_unaligned() as usize;
                let length = (entry.add(32) as *const u64).read_unaligned() as usize;
                return length < capacity
                    && syscall(PREAD, file, buffer as usize, length, offset) == length as isize
                    && {
                        buffer.add(length).write_volatile(0);
                        true
                    };
            }
            index += 1;
        }
        false
    }
}

/// Runs before relocation: only system calls, the stack and position-relative statics.
unsafe extern "C" fn stage1(stack: *const usize) -> ! {
    use call::*;
    unsafe {
        let argc = *stack;
        let argv = stack.add(1);
        let environment = argv.add(argc + 1);
        let mut exe = MaybeUninit::<[u8; 4096]>::uninit();
        let exe = exe.as_mut_ptr() as *mut u8;
        let mut loader = MaybeUninit::<[u8; 4096]>::uninit();
        let loader = loader.as_mut_ptr() as *mut u8;
        let length = syscall(
            READLINKAT,
            CWD,
            PROC_SELF_EXE.as_ptr() as usize,
            exe as usize,
            4095,
        );
        let found = length > 0 && {
            exe.add(length as usize).write_volatile(0);
            let mut shell = 0;
            while shell < SHELLS.len() && !interpreter(SHELLS[shell].as_ptr(), loader, 4096) {
                shell += 1;
            }
            shell < SHELLS.len()
        };
        // The loader runs the program its `argv[1]` names, which the program gets as `argv[0]`.
        let bytes = (argc + 2) * size_of::<usize>();
        let arguments = syscall(MMAP, 0, bytes, 3, 0x22) as *mut usize;
        if found && (arguments as isize) > 0 {
            arguments.write_volatile(loader as usize);
            arguments.add(1).write_volatile(exe as usize);
            let mut index = 1;
            while index < argc {
                arguments.add(index + 1).write_volatile(*argv.add(index));
                index += 1;
            }
            arguments.add(argc + 1).write_volatile(0);
            syscall(
                EXECVE,
                loader as usize,
                arguments as usize,
                environment as usize,
                0,
            );
        }
        syscall(WRITE, 2, NO_LOADER.as_ptr() as usize, NO_LOADER.len(), 0);
        loop {
            syscall(EXIT, 127, 0, 0, 0);
        }
    }
}

/// This executable's path. `current_exe` names the dynamic loader where stage 1 or a person
/// started the file through it; the loader passes the file's path on as `argv[0]`.
pub fn executable() -> io::Result<PathBuf> {
    let running = std::env::current_exe()?;
    let name = running.file_name().map_or(&b""[..], |name| name.as_bytes());
    if !name.starts_with(b"ld-") {
        return Ok(running);
    }
    std::fs::canonicalize(std::env::args_os().next().ok_or(io::ErrorKind::NotFound)?)
}

/// Whether the library `name` loads, as the crates that open it would find it.
pub fn loads(name: &CStr) -> bool {
    !unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY) }.is_null()
}

/// The libraries Snowbound and its crates open by name, Wayland's and X11's first.
const LIBRARIES: [&CStr; 14] = [
    c"libwayland-client.so.0",
    c"libwayland-egl.so.1",
    c"libxkbcommon.so.0",
    c"libxkbcommon-x11.so.0",
    c"libX11.so.6",
    c"libX11-xcb.so.1",
    c"libXcursor.so.1",
    c"libXi.so.6",
    c"libxcb.so.1",
    c"libvulkan.so.1",
    c"libEGL.so.1",
    c"libfontconfig.so.1",
    c"libenchant-2.so.2",
    c"libgstreamer-1.0.so.0",
];

/// On NixOS, where libraries sit only in the store, loads by path each library the system's
/// and the user's profiles have that no search finds; opening one by name then finds it
/// loaded.
pub fn preload() {
    let system = Path::new("/run/current-system");
    if !system.exists() {
        return;
    }
    let mut missing: Vec<&CStr> = LIBRARIES.into_iter().filter(|name| !loads(name)).collect();
    if missing.is_empty() {
        return;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let user = std::env::var_os("USER").map(|user| Path::new("/etc/profiles/per-user").join(user));
    let profiles = [
        Some(system.to_owned()),
        user,
        home.map(|home| home.join(".nix-profile")),
    ];
    let Ok(output) = std::process::Command::new(system.join("sw/bin/nix-store"))
        .arg("--query")
        .arg("--requisites")
        .args(
            profiles
                .into_iter()
                .flatten()
                .filter(|profile| profile.exists()),
        )
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return;
    };
    for path in output.stdout.split(|&byte| byte == b'\n') {
        let folder = Path::new(OsStr::from_bytes(path)).join("lib");
        if missing.is_empty() {
            break;
        }
        if !folder.is_dir() {
            continue;
        }
        missing.retain(|name| {
            let file = folder.join(OsStr::from_bytes(name.to_bytes()));
            let Ok(file) = CString::new(file.as_os_str().as_bytes()) else {
                return true;
            };
            let library: *mut c_void = unsafe { libc::dlopen(file.as_ptr(), libc::RTLD_LAZY) };
            library.is_null()
        });
    }
}
