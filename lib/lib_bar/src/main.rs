//! desicompass-bar: see the library's documentation (`src/lib.rs`).

#[cfg(target_os = "linux")]
fn main() {
    use clap::Parser;
    use desicompass_bar::{gui, ipc::Ipc};
    use desicompass_bar_protocol::ENV_IPC_FD;

    /// The bar of the desicompass session. desicompass starts it and hands it
    /// its sockets; run it by hand only with --standalone.
    #[derive(Parser, Debug)]
    #[command(version, about)]
    struct Args {
        /// The bar-protocol socket inherited from desicompass.
        #[arg(long, env = ENV_IPC_FD)]
        ipc_fd: Option<i32>,

        /// Run without a compositor, in a window of its own. For development.
        #[arg(long)]
        standalone: bool,
    }

    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }
    let args = Args::parse();

    let ipc = match (args.standalone, args.ipc_fd) {
        (true, _) => None,
        (false, Some(fd)) => match Ipc::from_fd(fd) {
            Ok(ipc) => Some(ipc),
            Err(e) => {
                eprintln!("desicompass-bar: fd {fd} is not a usable channel: {e}");
                std::process::exit(1);
            }
        },
        (false, None) => {
            eprintln!(
                "desicompass-bar is started by desicompass. \
                 To try it on its own, pass --standalone."
            );
            std::process::exit(2);
        }
    };

    if let Err(e) = gui::run(gui::Options {
        ipc,
        standalone: args.standalone,
    }) {
        eprintln!("desicompass-bar: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("desicompass-bar is Linux-only");
    std::process::exit(1);
}
