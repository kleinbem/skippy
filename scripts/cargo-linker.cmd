@echo off
setlocal EnableExtensions EnableDelayedExpansion

set "SKIPPY_LLD="
for /f "usebackq delims=" %%S in (`rustc --print sysroot 2^>nul`) do set "SKIPPY_RUST_SYSROOT=%%S"
if defined SKIPPY_RUST_SYSROOT (
  for %%T in (x86_64-pc-windows-msvc aarch64-pc-windows-msvc) do (
    if not defined SKIPPY_LLD if exist "!SKIPPY_RUST_SYSROOT!\lib\rustlib\%%T\bin\rust-lld.exe" set "SKIPPY_LLD=!SKIPPY_RUST_SYSROOT!\lib\rustlib\%%T\bin\rust-lld.exe"
  )
)
if not defined SKIPPY_LLD for %%L in (rust-lld.exe lld-link.exe) do (
  if not defined SKIPPY_LLD for /f "delims=" %%P in ('where %%L 2^>nul') do if not defined SKIPPY_LLD set "SKIPPY_LLD=%%P"
)
if not defined SKIPPY_LLD (
  >&2 echo Error: LLVM lld-link is required for Windows Rust links.
  >&2 echo Run "rustup component add llvm-tools-preview" or install LLVM.LLVM with winget.
  exit /b 1
)

set "SKIPPY_LLD_FLAVOR="
for %%F in ("!SKIPPY_LLD!") do if /I "%%~nxF"=="rust-lld.exe" set "SKIPPY_LLD_FLAVOR=-flavor link"

if "%~1"=="--skippy-probe" (
  echo %SKIPPY_LLD%
  exit /b 0
)

setlocal DisableDelayedExpansion
"%SKIPPY_LLD%" %SKIPPY_LLD_FLAVOR% %*
exit /b %ERRORLEVEL%
