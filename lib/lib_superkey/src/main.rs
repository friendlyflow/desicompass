//! desicompass-superkey: see the library's documentation (`src/lib.rs`).

#[cfg(target_os = "linux")]
fn main() {
    use clap::Parser;
    use desicompass_superkey::{gui, ipc::Ipc, power};
    use desicompass_superkey_protocol::ENV_IPC_FD;

    /// The superkey of the desicompass session. desicompass starts it and
    /// hands it its sockets; run it by hand only with --standalone.
    #[derive(Parser, Debug)]
    #[command(version, about)]
    struct Args {
        /// The superkey-protocol socket inherited from desicompass.
        #[arg(long, env = ENV_IPC_FD)]
        ipc_fd: Option<i32>,

        /// Run without a compositor: start on screen, and quit on Escape.
        /// For development.
        #[arg(long)]
        standalone: bool,

        #[arg(long, default_value = "")]
        suspend_command: String,
        #[arg(long, default_value = "")]
        reboot_command: String,
        #[arg(long, default_value = "")]
        poweroff_command: String,

        /// Also look for desktop entries here, before the standard
        /// directories. May be given more than once.
        #[arg(long = "applications-dir")]
        applications_dirs: Vec<std::path::PathBuf>,
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
                eprintln!("desicompass-superkey: fd {fd} is not a usable channel: {e}");
                std::process::exit(1);
            }
        },
        (false, None) => {
            eprintln!(
                "desicompass-superkey is started by desicompass. \
                 To try it on its own, pass --standalone."
            );
            std::process::exit(2);
        }
    };

    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let user = std::env::var("USER").ok();
    let app_dirs = desicompass_superkey::apps::search_dirs(
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("XDG_DATA_DIRS").ok().as_deref(),
        home.as_deref(),
        user.as_deref(),
        &args.applications_dirs,
    );

    let opts = gui::Options {
        ipc,
        power: power::Commands::from_args(
            &args.suspend_command,
            &args.reboot_command,
            &args.poweroff_command,
        ),
        app_dirs,
        standalone: args.standalone,
    };
    if let Err(e) = gui::run(opts) {
        eprintln!("desicompass-superkey: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("desicompass-superkey is Linux-only");
    std::process::exit(1);
}
