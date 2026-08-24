use std::io::IsTerminal;

use clap::Parser;

mod cmd;

use cmd::ProgramCommand;

fn main() {
    // Diagnostics go to stderr: with the stdio transport, stdout carries the
    // MCP protocol and must stay clean.
    let use_colors = std::io::stderr().is_terminal()
        && std::env::var("NO_COLOR") == Err(std::env::VarError::NotPresent);
    if use_colors {
        color_eyre::install().expect("init color_eyre");
    } else {
        color_eyre::config::HookBuilder::new()
            .theme(color_eyre::config::Theme::new())
            .install()
            .expect("init no color color_eyre");
    }
    let direnv_exists = std::env::var("DIRENV_DIR").is_ok();
    if !direnv_exists {
        if let Ok(dot_file) = std::env::var("DOT") {
            dotenvy::from_path_override(dot_file).expect("can not read dotenvy");
        } else {
            // Allows failure and do not override
            let _ = dotenvy::dotenv();
        }
    }
    let sub = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing::Level::INFO.into())
                .from_env()
                .expect("env contains non-utf8"),
        )
        .with_ansi(use_colors)
        .with_writer(std::io::stderr)
        .finish();
    tracing::subscriber::set_global_default(sub).expect("can not set default tracing");

    let cmd = ProgramCommand::parse();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("can not build tokio");

    if let Err(err) = runtime.block_on(cmd.run()) {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}
