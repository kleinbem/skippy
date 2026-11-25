"""Native loader and redistribution policy."""

WINDOWS_HOST = {
    "advapi32.dll",
    "bcrypt.dll",
    "cfgmgr32.dll",
    "comdlg32.dll",
    "crypt32.dll",
    "d3d12.dll",
    "dbghelp.dll",
    "dxgi.dll",
    "gdi32.dll",
    "kernel32.dll",
    "msvcp140.dll",
    "msvcrt.dll",
    "nvcuda.dll",
    "ntdll.dll",
    "ole32.dll",
    "oleaut32.dll",
    "psapi.dll",
    "rpcrt4.dll",
    "secur32.dll",
    "setupapi.dll",
    "shell32.dll",
    "shlwapi.dll",
    "ucrtbase.dll",
    "user32.dll",
    "version.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
    "vulkan-1.dll",
    "winmm.dll",
    "ws2_32.dll",
}

ELF_HOST = {
    "ld-linux.so.2",
    "ld-linux-x86-64.so.2",
    "ld-linux-aarch64.so.1",
    "libc.so.6",
    "libcrypt.so.1",
    "libdl.so.2",
    "libgcc_s.so.1",
    "libatomic.so.1",
    "libgomp.so.1",
    "libm.so.6",
    "libnsl.so.1",
    "libnuma.so.1",
    "libpthread.so.0",
    "libresolv.so.2",
    "librt.so.1",
    "libstdc++.so.6",
    "libutil.so.1",
    "libz.so.1",
    "libcuda.so",
    "libcuda.so.1",
}

NATIVE_TARGETS = {
    "aarch64-apple-darwin": ("macos", "aarch64"),
    "x86_64-apple-darwin": ("macos", "x86_64"),
    "aarch64-unknown-linux-gnu": ("linux", "aarch64"),
    "aarch64-linux-android": ("linux", "aarch64"),
    "x86_64-unknown-linux-gnu": ("linux", "x86_64"),
    "x86_64-linux-android": ("linux", "x86_64"),
    "armv7-unknown-linux-gnueabihf": ("linux", "arm"),
    "armv7-linux-androideabi": ("linux", "arm"),
    "x86_64-pc-windows-msvc": ("windows", "x86_64"),
}

HOST_TARGETS = {
    "darwin-aarch64": ("macho", "aarch64"),
    "linux-x86_64": ("elf", "x86_64"),
    "linux-aarch64": ("elf", "aarch64"),
    "windows-x86_64": ("pe", "x86_64"),
}


def basename(name):
    return name.replace("\\", "/").rsplit("/", 1)[-1]


def forbidden(name):
    lower = name.lower()
    return "metal.framework" in lower or basename(lower).startswith((
        "libcuda", "libcublas", "libnccl", "libamdhip", "libhip", "libhsa-", "nvcuda",
        "cudart", "cublas", "amdhip", "hipblas", "rocblas", "libvulkan", "vulkan-",
        "ggml", "libggml", "llama", "libllama",
    ))


def host_owned(name, kind):
    if kind == "macho":
        return name.startswith(("/usr/lib/", "/System/Library/"))
    name = name.lower()
    if kind == "pe":
        return name in WINDOWS_HOST or name.startswith(("api-ms-win-", "ext-ms-win-"))
    return name in ELF_HOST or name.startswith(("libnvidia-", "linux-vdso"))


def admit_cuda(name, major):
    if major not in {"12", "13"}:
        raise ValueError("unsupported CUDA toolkit major")
    if not any(name == stem + major or name.startswith(stem + major + ".")
               for stem in ("libcudart.so.", "libcublas.so.", "libcublasLt.so.", "libnvJitLink.so.")):
        raise ValueError(f"unreviewed CUDA redistributable: {name}")
