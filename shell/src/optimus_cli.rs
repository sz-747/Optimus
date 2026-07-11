use std::io::{self, IsTerminal};
use std::process;

use optimus_shell::cli::{client, parser, runtime, stdin};

fn main() {
    process::exit(run());
}

fn run() -> i32 {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();

    let input = io::stdin();
    let stdin_payload = if input.is_terminal() {
        None
    } else {
        stdin::read_available(
            input,
            stdin::DEFAULT_FIRST_BYTE_TIMEOUT,
            stdin::DEFAULT_DRAIN_TIMEOUT,
        )
    };

    let get_env = |key: &str| std::env::var(key).ok();
    let invocation = match parser::parse(&arg_refs, &get_env, stdin_payload.as_deref()) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("{}", error.message);
            return error.exit_code;
        }
    };

    if let Some(stdout) = &invocation.stdout {
        println!("{stdout}");
    }

    match runtime::write_local_files(&invocation, &get_env) {
        Ok(paths) => {
            for path in paths {
                println!("{}", path.display());
            }
        }
        Err(error) => {
            eprintln!("optimus: could not write hook files: {error}");
            return 1;
        }
    }

    if invocation.frames.is_empty() {
        return 0;
    }

    let pipe = runtime::resolve_pipe(&invocation, &get_env, &client::can_connect);
    let responses = match client::send(&pipe, &invocation.frames, client::DEFAULT_CONNECT_TIMEOUT) {
        Ok(responses) => responses,
        Err(error) => {
            eprintln!("optimus: cannot reach the app on \"{pipe}\": {error}");
            return 1;
        }
    };

    let report = runtime::report_responses(&responses);
    for line in report.stdout {
        println!("{line}");
    }
    for line in report.stderr {
        eprintln!("{line}");
    }
    report.exit_code
}
