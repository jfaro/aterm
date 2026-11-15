use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::path::PathBuf;

/// Socket the server listens on. `ATERM_SOCKET` overrides the default so
/// multiple independent servers (e.g. tests) can coexist.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("ATERM_SOCKET") {
        Some(path) => PathBuf::from(path),
        None => default_runtime_dir().join("aterm.sock"),
    }
}

/// Path to the ATERM server logs.
pub fn log_path() -> PathBuf {
    socket_path().with_extension("log")
}

/// Creates `dir` (if missing) readable only by the current user.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// Kept under `/tmp` rather than `$TMPDIR` because macOS `$TMPDIR` paths can
/// exceed the 104-byte limit on Unix socket paths.
fn default_runtime_dir() -> PathBuf {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    PathBuf::from(format!("/tmp/aterm-{uid}"))
}
