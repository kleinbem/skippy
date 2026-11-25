//! What the fixture children inherit after `env_clear()`.
//!
//! Hosts without `/bin/bash` or `/usr/bin` (NixOS, Nix build sandboxes, some
//! containers) keep bash and the coreutils the scripts call elsewhere on
//! `PATH`, and the CA bundle may only be reachable through `SSL_CERT_FILE`, so
//! the children get the test process's `PATH` and CA variables instead of
//! fixed system paths. Children that open sockets or temporary files also keep
//! Windows' `SystemRoot`, `TEMP` and `TMP`. Everything else stays cleared.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The test process's `PATH`, or the usual system directories without one.
pub fn path() -> OsString {
    std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin"))
}

/// `dir` in front of [`path`].
pub fn path_with(dir: &Path) -> OsString {
    let mut dirs = vec![dir.to_path_buf()];
    dirs.extend(std::env::split_paths(&path()));
    std::env::join_paths(dirs).expect("PATH entries are joinable")
}

/// The bash the fixtures run: `SKIPPY_TEST_BASH` when set, so a bash outside
/// the usual places can be chosen on purpose. Relative overrides are resolved
/// against the test process's current directory. Otherwise use Homebrew's bash
/// on macOS (the system one is 3.2), or the first `bash` on [`path`].
pub fn bash() -> PathBuf {
    if let Some(chosen) = std::env::var_os("SKIPPY_TEST_BASH").map(PathBuf::from) {
        let chosen = if chosen.is_absolute() {
            chosen
        } else {
            std::env::current_dir()
                .expect("resolve SKIPPY_TEST_BASH against the current directory")
                .join(chosen)
        };
        assert!(
            chosen.is_file(),
            "SKIPPY_TEST_BASH={} is not a file",
            chosen.display()
        );
        return chosen;
    }
    let homebrew = Path::new("/opt/homebrew/bin/bash");
    if homebrew.exists() {
        return homebrew.to_path_buf();
    }
    std::env::split_paths(&path())
        .map(|dir| dir.join("bash"))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from("/bin/bash"))
}

/// `script` with its `#!/bin/bash` line pointing at [`bash`].
pub fn bash_script(script: impl AsRef<[u8]>) -> Vec<u8> {
    let script = script.as_ref();
    let body = script.strip_prefix(b"#!/bin/bash").unwrap_or(script);
    let mut out = format!("#!{}", bash().display()).into_bytes();
    out.extend_from_slice(body);
    out
}

/// The Windows variables that are set, for children that open sockets or
/// temporary files: without `SystemRoot`, Winsock cannot initialise and every
/// connection fails with `WSAEPROVIDERFAILEDINIT` (os error 10106); without
/// `TEMP` and `TMP`, the temporary directory falls back to the Windows
/// directory, which a user cannot write. Unset, and so empty, elsewhere.
pub fn windows_system() -> Vec<(&'static str, OsString)> {
    ["SystemRoot", "TEMP", "TMP"]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
        .collect()
}

/// The CA variables that are set, for children that build HTTPS clients.
pub fn ca_certificates() -> Vec<(&'static str, OsString)> {
    ["SSL_CERT_FILE", "SSL_CERT_DIR", "NIX_SSL_CERT_FILE"]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
        .collect()
}
