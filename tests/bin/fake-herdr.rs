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
        .or_else(|| {
            env::var_os("HERDR_SOCKET_PATH")
                .map(PathBuf::from)
                .and_then(|path| path.parent().map(PathBuf::from))
        })
        .expect("FAKE_HERDR_DIR or HERDR_SOCKET_PATH");
    let call = env::args().skip(1).collect::<Vec<_>>().join("\t");
    writeln!(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("calls"))
            .expect("open fake Herdr calls"),
        "{call}"
    )
    .expect("record fake Herdr call");

    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments.windows(2).any(|window| window == ["pane", "split"]) {
        println!(r#"{{"result":{{"pane":{{"pane_id":"workspace-a:p4"}}}}}}"#);
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
