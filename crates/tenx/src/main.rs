use std::{fs, io, path::PathBuf};

use anyhow::{Context as AnyhowContext, Result};
use clap::{Parser, Subcommand};
use colored::*;
use tokio::sync::mpsc;
use tracing::{info, Subscriber};
use tracing_subscriber::fmt::format::{FmtSpan, Writer};
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

use libtenx::{
    self, dialect::Dialect, model::Claude, model::Model, Config, PromptInput, Session,
    SessionStore, Tenx,
};

mod edit;

struct NoTime;

impl FormatTime for NoTime {
    fn format_time(&self, _: &mut Writer<'_>) -> std::fmt::Result {
        Ok(())
    }
}

/// Creates a subscriber that writes to stdout without timestamps.
fn create_subscriber(verbosity: u8) -> impl Subscriber {
    let filter = match verbosity {
        0 => EnvFilter::new("warn"),
        1 => EnvFilter::new("info"),
        2 => EnvFilter::new("debug"),
        _ => EnvFilter::new("trace"),
    };

    fmt::Subscriber::builder()
        .with_env_filter(filter)
        .with_timer(NoTime)
        .with_writer(io::stdout)
        .with_span_events(FmtSpan::NONE)
        .finish()
}

#[derive(Parser)]
#[clap(name = "tenx")]
#[clap(author = "Aldo Cortesi")]
#[clap(version = "0.1.0")]
#[clap(about = "AI-powered command-line assistant for Rust", long_about = None)]
struct Cli {
    /// Increase output verbosity
    #[clap(short, long, action = clap::ArgAction::Count, global = true, default_value = "1")]
    verbose: u8,

    /// Decrease output verbosity
    #[clap(short, long, global = true)]
    quiet: bool,

    /// Anthropic API key
    #[clap(long, env = "ANTHROPIC_API_KEY", hide_env_values = true, global = true)]
    anthropic_key: Option<String>,

    /// State directory
    #[clap(long, global = true)]
    state_dir: Option<PathBuf>,

    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Resume an existing conversation
    Edit {
        /// Specifies files to edit
        #[clap(value_parser)]
        files: Option<Vec<PathBuf>>,

        /// User prompt for the edit operation
        #[clap(long)]
        prompt: Option<String>,

        /// Path to a file containing the prompt
        #[clap(long)]
        prompt_file: Option<PathBuf>,
    },
    /// Create a new session
    New {
        /// Specifies files to add as context
        #[clap(value_parser)]
        files: Vec<PathBuf>,

        /// Add ruskel documentation
        #[clap(long)]
        ruskel: Vec<String>,
    },
    /// Add context to an existing session
    Add {
        /// Specifies files to add as context
        #[clap(value_parser)]
        files: Vec<PathBuf>,

        /// Add ruskel documentation
        #[clap(long)]
        ruskel: Vec<String>,
    },
    /// Show the current state
    Show,
}

/// Creates a Config from CLI arguments
fn load_config(cli: &Cli) -> Result<Config> {
    let mut config =
        Config::default().with_anthropic_key(cli.anthropic_key.clone().unwrap_or_default());
    if let Some(state_dir) = cli.state_dir.clone() {
        config = config.with_state_dir(state_dir);
    }
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let verbosity = if cli.quiet { 0 } else { cli.verbose };
    let subscriber = create_subscriber(verbosity);
    subscriber.init();

    match &cli.command {
        Commands::Add { files, ruskel } => {
            let config = load_config(&cli)?;
            let tx = Tenx::new(config);
            let mut state = tx.load_session::<PathBuf>(None)?;

            for file in files {
                state.add_path(file)?;
            }

            for ruskel_doc in ruskel {
                state.add_ruskel(ruskel_doc.clone())?;
            }

            tx.save_session(state)?;
            info!("Context added to session successfully");
            Ok(())
        }
        Commands::New { files, ruskel } => {
            let config = load_config(&cli)?;
            let tx = Tenx::new(config);
            let mut state = Session::new(
                None,
                Dialect::Tags(libtenx::dialect::Tags::default()),
                Model::Claude(Claude::default()),
            );

            for file in files {
                state.add_path(file)?;
            }

            for ruskel_doc in ruskel {
                state.add_ruskel(ruskel_doc.clone())?;
            }

            tx.save_session(state)?;
            info!("New session created successfully");
            Ok(())
        }
        Commands::Edit {
            files,
            prompt,
            prompt_file,
        } => {
            let config = load_config(&cli)?;
            let tx = Tenx::new(config);

            let (sender, mut receiver) = mpsc::channel(100);
            let print_task = tokio::spawn(async move {
                while let Some(chunk) = receiver.recv().await {
                    print!("{}", chunk);
                }
            });

            let user_prompt = if let Some(p) = prompt {
                PromptInput {
                    edit_paths: files.clone().unwrap_or_default(),
                    user_prompt: p.clone(),
                }
            } else if let Some(file_path) = prompt_file {
                let prompt_content =
                    fs::read_to_string(file_path).context("Failed to read prompt file")?;
                PromptInput {
                    edit_paths: files.clone().unwrap_or_default(),
                    user_prompt: prompt_content,
                }
            } else {
                let f = files.clone().unwrap_or_default();
                match edit::edit_prompt(&f)? {
                    Some(p) => p,
                    None => return Ok(()),
                }
            };

            tx.resume(user_prompt, Some(sender)).await?;

            print_task.await?;
            info!("\n\n{}", "Changes applied successfully".green().bold());
            Ok(())
        }
        Commands::Show => {
            let config = load_config(&cli)?;
            let session_store = SessionStore::open(config.state_dir.as_ref())?;
            let state = session_store.load(&std::env::current_dir()?)?;
            println!("{}", state.pretty_print());
            Ok(())
        }
    }
}

