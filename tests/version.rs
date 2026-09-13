use std::fs;
use std::os::unix::net::UnixListener;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn version_flags_exit_before_config_or_socket_side_effects() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let test_dir =
        std::env::temp_dir().join(format!("cksk-version-{}-{nonce}", std::process::id()));
    let config_dir = test_dir.join("cksk");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("config.json"), b"not valid json").unwrap();

    let socket_path = test_dir.join("clck.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    listener.set_nonblocking(true).unwrap();

    for flag in ["--version", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_cksk"))
            .arg(flag)
            .env("XDG_CONFIG_HOME", &test_dir)
            .env("CLCK_SOCKET", &socket_path)
            .output()
            .unwrap();

        assert!(output.status.success(), "{flag} failed: {output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("cksk {}\n", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(output.stderr, b"");
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "{flag} unexpectedly connected to clck"
        );
    }

    fs::remove_dir_all(test_dir).unwrap();
}
