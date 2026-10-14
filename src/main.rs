mod agent;
mod api;
mod app;
mod auth;
mod backend;
mod battery;
mod cli;
mod commands;
mod extensions;
mod forums;
mod frontend;
mod install;
mod io;
mod limits;
mod mcp;
mod mode;
mod paths;
mod permission;
mod release;
mod session;
mod settings;
mod skills;
mod text;
mod tools;
mod trust;
mod tui;
mod update;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    cli::run().await
}
