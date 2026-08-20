// SPDX-FileCopyrightText: 2026 Phillip Cloud
//
// SPDX-License-Identifier: Apache-2.0

use std::env;
use std::process;

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let expected_cwd = env::var("FAKE_GIT_CWD").expect("FAKE_GIT_CWD");
    if args.first().map(String::as_str) != Some("-C")
        || args.get(1).map(String::as_str) != Some(expected_cwd.as_str())
    {
        eprintln!("unexpected fake Git working directory arguments");
        process::exit(2);
    }

    let command = args[2..].iter().map(String::as_str).collect::<Vec<_>>();
    match command.as_slice() {
        ["symbolic-ref", "--quiet", "--short", "HEAD"] => {
            println!("feature/source-scope");
        }
        ["remote"] => {
            println!("origin");
        }
        ["remote", "get-url", "origin"] => {
            println!("ssh://git@Example.Invalid:22/acme%2Ftmp%2F..%2Fproject-alpha.git");
        }
        _ => {
            eprintln!("unexpected fake Git command");
            process::exit(2);
        }
    }
}
