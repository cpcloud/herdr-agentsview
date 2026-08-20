// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::Duration;

fn main() {
    let root = env::var_os("FAKE_HERDR_DIR")
        .map(PathBuf::from)
        .expect("FAKE_HERDR_DIR");
    let args = env::args().skip(1).collect::<Vec<_>>();
    let call = args.join("\t");
    writeln!(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("calls"))
            .expect("open fake Herdr calls"),
        "{call}"
    )
    .expect("record fake Herdr call");

    if args.first().map(String::as_str) == Some("pane")
        && args.get(1).map(String::as_str) == Some("get")
    {
        if let Some(foreground_cwd) = env::var_os("FAKE_HERDR_FOREGROUND_CWD") {
            let foreground_cwd = foreground_cwd.to_string_lossy();
            let foreground_cwd = foreground_cwd.replace('\\', "\\\\").replace('"', "\\\"");
            println!(
                "{{\"result\":{{\"pane\":{{\"cwd\":\"/fallback/cwd\",\"foreground_cwd\":\"{foreground_cwd}\"}}}}}}"
            );
        } else {
            let response = r#"{"result":{"pane":{"cwd":"","foreground_cwd":null}}}"#;
            println!("{response}");
        }
        return;
    }

    match env::var("FAKE_HERDR_MODE").as_deref().unwrap_or("success") {
        "failure" => {
            eprintln!("fake Herdr failure");
            process::exit(17);
        }
        "hang" => {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake Herdr endpoint");
            fs::write(
                root.join("endpoint"),
                listener
                    .local_addr()
                    .expect("read fake Herdr endpoint")
                    .to_string(),
            )
            .expect("write fake Herdr endpoint");
            thread::sleep(Duration::from_secs(10));
        }
        "success" => {}
        mode => {
            eprintln!("unknown fake Herdr mode {mode:?}");
            process::exit(2);
        }
    }
}
