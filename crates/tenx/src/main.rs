use std::{fs, path::PathBuf};

use anyhow::{Context as AnyhowContext, Result};
use clap::{CommandFactory, Parser, Subcommand};
use colored::*;
use libtenx::{
    self,
    config::{self},
    dialect::DialectProvider,
    event_consumers::{self, output_logs, output_progress},
    model::ModelProvider,
    Session, Tenx,
};
use serde_json::to_string_pretty;
use tokio::sync::mpsc;
use tracing_subscriber::util::SubscriberInitExt;

mod edit;
mod pretty;

/// Gets the user's prompt from arguments or editor
fn get_prompt(
    prompt: &Option<String>,
    prompt_file: &Option<PathBuf>,
    session: &Session,
    retry: bool,
) -> Result<Option<String>> {
    if let Some(p) = prompt {
        Ok(Some(p.clone()))
    } else if let Some(file_path) = prompt_file {
        let prompt_content = fs::read_to_string(file_path).context("Failed to read prompt file")?;
        Ok(Some(prompt_content))
    } else {
        Ok(edit::edit_prompt(session, retry)?)
    }
}

#[derive(Parser)]
#[clap(name = "tenx")]
#[clap(author = "Aldo Cortesi")]
#[clap(version = "0.1.0")]
#[clap(max_term_width = 80)]
#[clap(about = "AI-powered coding assistant", long_about = None)]
struct Cli {
    /// Increase output verbosity
    #[clap(short, long, action = clap::ArgAction::Count,  default_value = "0")]
    verbose: u8,

    /// Decrease output verbosity
    #[clap(short, long)]
    quiet: bool,

    /// Show raw log output instead of progress indicators
    #[clap(long)]
    logs: bool,

    /// Model to use (overrides default_model in config)
    #[clap(long)]
    model: Option<String>,

    /// Session storage directory (~/.config/tenx/state by default)
    #[clap(long)]
    session_store_dir: Option<PathBuf>,

    /// Number of times to retry a prompt before failing
    #[clap(long)]
    retry_limit: Option<usize>,

    /// Skip preflight checks
    #[clap(long)]
    no_preflight: bool,

    /// Disable streaming model output
    #[clap(long)]
    no_stream: bool,

    /// Force colored output
    #[clap(long, conflicts_with = "no_color")]
    color: bool,

    /// Disable colored output
    #[clap(long)]
    no_color: bool,

    /// Smart mode for the Tags dialect
    #[clap(long)]
    tags_smart: Option<bool>,

    /// Replace mode for the Tags dialect
    #[clap(long)]
    tags_replace: Option<bool>,

    /// Udiff mode for the Tags dialect
    #[clap(long)]
    tags_udiff: Option<bool>,

    /// Rust Cargo Clippy validator
    #[clap(long)]
    rust_cargo_clippy: Option<bool>,

    /// Rust Cargo Check validator
    #[clap(long)]
    rust_cargo_check: Option<bool>,

    /// Rust Cargo Test validator
    #[clap(long)]
    rust_cargo_test: Option<bool>,

    /// Python Ruff Check validator
    #[clap(long)]
    python_ruff_check: Option<bool>,

    #[clap(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum DialectCommands {
    /// Print the current dialect and its settings
    Info,
    /// Print the complete system prompt
    System,
}

#[derive(Subcommand)]
enum Commands {
    /// List configured models (alias: ls)
    #[clap(alias = "ls")]
    Models {
        /// Show full configuration details
        #[clap(short, long)]
        full: bool,
    },
    /// Add editable files to a session
    Edit {
        /// Specifies files to edit
        #[clap(value_parser)]
        files: Vec<String>,
    },

    /// Add items to context (alias: ctx)
    #[clap(alias = "ctx")]
    Context {
        /// Add items as ruskel documentation modules
        #[clap(long, group = "type")]
        ruskel: bool,

        /// Add items as files
        #[clap(long, group = "type")]
        file: bool,

        /// Add items as URLs
        #[clap(long, group = "type")]
        url: bool,

        /// Items to add to context
        items: Vec<String>,
    },
    /// Clear the current session without resetting changes
    Clear,
    /// Print the current configuration
    Conf {
        /// Output as JSON instead of TOML
        #[clap(long)]
        json: bool,
        /// Output full configuration
        #[clap(long)]
        full: bool,
        /// Output default configuration
        #[clap(long)]
        defaults: bool,
    },
    /// Dialect commands (alias: dia)
    #[clap(alias = "dia")]
    Dialect {
        #[clap(subcommand)]
        command: DialectCommands,
    },
    /// Make an AI-assisted change using the current session
    Ask {
        /// Specifies files to edit
        #[clap(value_parser)]
        files: Option<Vec<String>>,

        /// User prompt for the edit operation
        #[clap(long)]
        prompt: Option<String>,

        /// Path to a file containing the prompt
        #[clap(long)]
        prompt_file: Option<PathBuf>,

        /// Add ruskel documentation as context
        #[clap(long)]
        ruskel: Vec<String>,

        /// Add files as context
        #[clap(long)]
        ctx: Vec<String>,

        /// Add URLs as context
        #[clap(long)]
        url: Vec<String>,
    },
    /// List files included in the project
    Files {
        /// Optional glob pattern to filter files
        pattern: Option<String>,
    },
    /// Start a new session and attempt to fix any preflight failures
    Fix {
        /// Specifies files to edit
        #[clap(value_parser)]
        files: Vec<String>,

        /// Add ruskel documentation as context
        #[clap(long)]
        ruskel: Vec<String>,

        /// Add files as context
        #[clap(long)]
        ctx: Vec<String>,

        /// Add URLs as context
        #[clap(long)]
        url: Vec<String>,

        /// Clear the current session, and use it to fix
        #[clap(long)]
        clear: bool,

        /// User prompt for the fix operation
        #[clap(long)]
        prompt: Option<String>,

        /// Path to a file containing the prompt
        #[clap(long)]
        prompt_file: Option<PathBuf>,

        /// Edit the prompt before fixing
        #[clap(long)]
        edit: bool,
    },
    /// Run formatters on the current session
    Format,
    /// List all formatters and their status
    Formatters,
    /// Create a new session
    New {
        /// Specifies files to add as context
        #[clap(value_parser)]
        files: Vec<String>,

        /// Add ruskel documentation
        #[clap(long)]
        ruskel: Vec<String>,

        /// Add URLs as context
        #[clap(long)]
        url: Vec<String>,
    },
    /// Start a new session, edit the prompt, and run it
    Quick {
        /// Specifies files to edit
        #[clap(value_parser)]
        files: Vec<String>,

        /// Add ruskel documentation as context
        #[clap(long)]
        ruskel: Vec<String>,

        /// Add files as context
        #[clap(long)]
        ctx: Vec<String>,

        /// Add URLs as context
        #[clap(long)]
        url: Vec<String>,

        /// User prompt for the edit operation
        #[clap(long)]
        prompt: Option<String>,

        /// Path to a file containing the prompt
        #[clap(long)]
        prompt_file: Option<PathBuf>,
    },
    /// Run preflight validation suite on the current session
    Preflight,
    /// Print information about the current project
    Project,
    /// Refresh all contexts in the current session
    Refresh,
    /// Reset the session to a specific step, undoing changes
    Reset {
        /// The step offset to reset to
        step_offset: usize,
    },
    /// Retry a prompt
    Retry {
        /// The step offset to retry from
        step_offset: Option<usize>,
        /// Edit the prompt before retrying
        #[clap(long)]
        edit: bool,
        /// Add files as context
        #[clap(long)]
        ctx: Vec<String>,
        /// Add ruskel documentation as context
        #[clap(long)]
        ruskel: Vec<String>,
        /// Add URLs as context
        #[clap(long)]
        url: Vec<String>,
        /// User prompt for the edit operation
        #[clap(long)]
        prompt: Option<String>,
        /// Path to a file containing the prompt
        #[clap(long)]
        prompt_file: Option<PathBuf>,
    },
    /// Show the current session (alias: sess)
    #[clap(alias = "sess")]
    Session {
        /// Print the entire session object verbosely
        #[clap(long)]
        raw: bool,

        /// Output the rendered session
        #[clap(long)]
        render: bool,

        /// Show full details
        #[clap(long)]
        full: bool,
    },
    /// List all validators and their status
    Validators,
}

/// Creates a Config from disk and CLI arguments
fn load_config(cli: &Cli) -> Result<config::Config> {
    let mut config = config::Config::default();

    // Load from home config file
    let home_config_path = config::home_config_dir().join(config::HOME_CONFIG_FILE);
    if home_config_path.exists() {
        let home_config_str =
            fs::read_to_string(&home_config_path).context("Failed to read home config file")?;
        let home_config = config::Config::from_toml(&home_config_str)
            .context("Failed to parse home config file")?;
        config.merge(&home_config);
    }

    // Load from local config file
    let project_root = config.project_root();
    let local_config_path = project_root.join(config::LOCAL_CONFIG_FILE);
    if local_config_path.exists() {
        let local_config_str =
            fs::read_to_string(&local_config_path).context("Failed to read local config file")?;
        let local_config = config::Config::from_toml(&local_config_str)
            .context("Failed to parse local config file")?;
        config.merge(&local_config);
    }

    macro_rules! set_config {
        ($config:expr, $($field:ident).+, $value:expr) => {
            if let Some(val) = $value {
                $config.$($field).+ = val;
            }
        };
    }

    // Apply CLI arguments
    config = config.load_env();
    set_config!(config, session_store_dir, cli.session_store_dir.clone());
    set_config!(config, retry_limit, cli.retry_limit);
    set_config!(config, tags.smart, cli.tags_smart);
    set_config!(config, tags.replace, cli.tags_replace);
    set_config!(config, tags.udiff, cli.tags_udiff);
    if let Some(model) = &cli.model {
        config.default_model = Some(model.clone());
    }
    config.no_preflight = cli.no_preflight;
    config.no_stream = cli.no_stream;

    // Override validator configurations
    if let Some(value) = cli.rust_cargo_clippy {
        config.validators.rust_cargo_clippy = value;
    }
    if let Some(value) = cli.rust_cargo_check {
        config.validators.rust_cargo_check = value;
    }
    if let Some(value) = cli.rust_cargo_test {
        config.validators.rust_cargo_test = value;
    }
    if let Some(value) = cli.python_ruff_check {
        config.validators.python_ruff_check = value;
    }

    Ok(config)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let verbosity = if cli.quiet { 0 } else { cli.verbose };
    let config = load_config(&cli)?;
    let tx = Tenx::new(config.clone());

    // Removed add_context function

    if cli.color {
        colored::control::set_override(true);
    } else if cli.no_color {
        colored::control::set_override(false);
    }

    let (sender, receiver) = mpsc::channel(100);
    let (event_kill_tx, event_kill_rx) = mpsc::channel(1);
    let subscriber = event_consumers::create_tracing_subscriber(verbosity, sender.clone());
    subscriber.init();
    let event_task = if cli.logs {
        tokio::spawn(output_logs(receiver, event_kill_rx))
    } else {
        tokio::spawn(output_progress(receiver, event_kill_rx, verbosity))
    };

    let result = match &cli.command {
        Some(cmd) => match cmd {
            Commands::Models { full } => {
                for model in &config.models {
                    match model {
                        libtenx::config::ModelConfig::Claude(_)
                        | libtenx::config::ModelConfig::OpenAi(_) => {
                            println!("{}", model.name().blue().bold());
                            println!("    kind: {}", model.kind());
                            for line in model.text_config(*full).lines() {
                                println!("    {}", line);
                            }
                            println!();
                        }
                    }
                }
                Ok(())
            }
            Commands::Conf {
                json,
                full,
                defaults,
            } => {
                let mut conf = if *defaults {
                    config::Config::default()
                } else {
                    config.clone()
                };
                if *full || *defaults {
                    conf = conf.with_full(true);
                }
                if *json {
                    println!("{}", to_string_pretty(&conf)?);
                } else {
                    println!("{}", conf.to_toml()?);
                }
                Ok(()) as anyhow::Result<()>
            }
            Commands::Project => {
                let project_root = config.project_root();
                println!(
                    "{} {}",
                    "project root:".blue().bold(),
                    project_root.display()
                );
                println!("{} {}", "include strategy:".blue().bold(), config.include);
                Ok(())
            }
            Commands::Files { pattern } => {
                let files = if let Some(p) = pattern {
                    config.match_files_with_glob(p)?
                } else {
                    config.included_files()?
                };
                for file in files {
                    println!("{}", file.display());
                }
                Ok(())
            }
            Commands::Validators => {
                for validator in libtenx::all_validators() {
                    let name = validator.name();
                    let configured = validator.is_configured(&config);
                    let runnable = validator.runnable();

                    let status = if !configured {
                        format!("{} {}", "✗".yellow(), " (disabled)".yellow())
                    } else {
                        match runnable {
                            Ok(libtenx::Runnable::Ok) => "✓".green().to_string(),
                            Ok(libtenx::Runnable::Error(msg)) => {
                                format!("{}  ({})", "✗".red(), msg.red())
                            }
                            Err(_) => "✗".red().to_string(),
                        }
                    };

                    println!("{:<30} {}", name, status);
                }
                Ok(())
            }
            Commands::Formatters => {
                for formatter in libtenx::formatters::all_formatters() {
                    let name = formatter.name();
                    let configured = formatter.is_configured(&config);
                    let runnable = formatter.runnable();

                    let status = if !configured {
                        format!("{} {}", "✗".yellow(), " (disabled)".yellow())
                    } else {
                        match runnable {
                            Ok(libtenx::Runnable::Ok) => "✓".green().to_string(),
                            Ok(libtenx::Runnable::Error(msg)) => {
                                format!("{}  ({})", "✗".red(), msg.red())
                            }
                            Err(_) => "✗".red().to_string(),
                        }
                    };

                    println!("{:<30} {}", name, status);
                }
                Ok(())
            }
            Commands::Dialect { command } => {
                let dialect = config.dialect()?;
                match command {
                    DialectCommands::Info => {
                        println!("Current dialect: {}", dialect.name());
                        println!("\nSettings:\n{:#?}", config.tags);
                    }
                    DialectCommands::System => {
                        println!("Current dialect: {}", dialect.name());
                        println!("\nSystem prompt:\n{}", dialect.system());
                    }
                }
                Ok(())
            }
            Commands::Quick {
                files,
                ruskel,
                ctx,
                url,
                prompt,
                prompt_file,
            } => {
                let mut session = tx.new_session_from_cwd(&Some(sender.clone())).await?;
                tx.add_contexts(&mut session, ctx, ruskel, url, false, &Some(sender.clone()))
                    .await?;
                for file in files {
                    session.add_editable(&config, file)?;
                }

                let user_prompt = match get_prompt(prompt, prompt_file, &session, false)? {
                    Some(p) => p,
                    None => return Ok(()),
                };
                tx.ask(&mut session, user_prompt, Some(sender.clone()))
                    .await?;
                Ok(())
            }
            Commands::Ask {
                files,
                prompt,
                prompt_file,
                ruskel,
                ctx,
                url,
            } => {
                let mut session = tx.load_session()?;

                for f in files.clone().unwrap_or_default() {
                    session.add_editable(&config, &f)?;
                }
                tx.add_contexts(&mut session, ctx, ruskel, url, false, &Some(sender.clone()))
                    .await?;

                let user_prompt = match get_prompt(prompt, prompt_file, &session, false)? {
                    Some(p) => p,
                    None => return Ok(()),
                };
                tx.ask(&mut session, user_prompt, Some(sender)).await?;
                Ok(())
            }
            Commands::Session { raw, render, full } => {
                let model = config.model()?;
                let session = tx.load_session()?;
                if *raw {
                    println!("{:#?}", session);
                } else if *render {
                    println!("{}", model.render(&config, &session)?);
                } else {
                    println!("{}", pretty::session(&config, &session, *full)?);
                }
                Ok(())
            }
            Commands::Edit { files } => {
                let mut session = tx.load_session()?;
                let mut total = 0;

                for file in files {
                    let added = session.add_editable(&config, file)?;
                    if added == 0 {
                        return Err(anyhow::anyhow!("glob did not match any files"));
                    }
                    total += added;
                }

                println!("{} files added for editing", total);
                tx.save_session(&session)?;
                Ok(())
            }
            Commands::Context {
                ruskel,
                file,
                url,
                items,
            } => {
                let mut session = tx.load_session()?;
                let added = tx
                    .add_contexts(
                        &mut session,
                        if *file { items } else { &[] },
                        if *ruskel { items } else { &[] },
                        if *url { items } else { &[] },
                        false,
                        &Some(sender.clone()),
                    )
                    .await?;
                println!("{} context items added", added);
                tx.save_session(&session)?;
                Ok(())
            }
            Commands::Reset { step_offset } => {
                let mut session = tx.load_session()?;
                tx.reset(&mut session, *step_offset)?;
                println!("Session reset to step {}", step_offset);
                Ok(())
            }
            Commands::Retry {
                step_offset,
                edit,
                ctx,
                ruskel,
                url,
                prompt,
                prompt_file,
            } => {
                let mut session = tx.load_session()?;

                let offset = step_offset.unwrap_or(session.steps().len() - 1);
                tx.reset(&mut session, offset)?;

                tx.add_contexts(&mut session, ctx, ruskel, url, false, &Some(sender.clone()))
                    .await?;

                let prompt = if *edit || prompt.is_some() || prompt_file.is_some() {
                    get_prompt(prompt, prompt_file, &session, true)?
                } else {
                    None
                };

                tx.retry(&mut session, prompt, Some(sender.clone())).await?;
                Ok(())
            }
            Commands::New { files, ruskel, url } => {
                let mut session = tx.new_session_from_cwd(&Some(sender.clone())).await?;
                tx.add_contexts(
                    &mut session,
                    files,
                    ruskel,
                    url,
                    false,
                    &Some(sender.clone()),
                )
                .await?;
                tx.save_session(&session)?;
                println!("new session: {}", config.project_root().display());
                Ok(())
            }
            Commands::Fix {
                files,
                ruskel,
                ctx,
                url,
                clear,
                prompt,
                prompt_file,
                edit,
            } => {
                let mut session = if *clear {
                    let mut current_session = tx.load_session()?;
                    current_session.clear();
                    current_session
                } else {
                    tx.new_session_from_cwd(&Some(sender.clone())).await?
                };

                for file in files {
                    session.add_editable(&config, file)?;
                }
                tx.add_contexts(&mut session, ctx, ruskel, url, false, &Some(sender.clone()))
                    .await?;

                let prompt = if prompt.is_some() || prompt_file.is_some() || *edit {
                    get_prompt(prompt, prompt_file, &session, false)?
                } else {
                    None
                };
                tx.fix(&mut session, Some(sender.clone()), prompt).await?;
                Ok(())
            }
            Commands::Clear => {
                let mut session = tx.load_session()?;
                session.clear();
                tx.save_session(&session)?;
                println!("Session cleared");
                Ok(())
            }
            Commands::Format => {
                let mut session = tx.load_session()?;
                tx.run_formatters(&mut session, &Some(sender.clone()))?;
                tx.save_session(&session)?;
                Ok(())
            }
            Commands::Preflight => {
                let mut session = tx.load_session()?;
                tx.run_preflight_validators(&mut session, &Some(sender.clone()))?;
                Ok(())
            }
            Commands::Refresh => {
                let mut session = tx.load_session()?;
                tx.refresh_context(&mut session, &Some(sender.clone()))
                    .await?;
                tx.save_session(&session)?;
                Ok(())
            }
        },
        None => {
            // This incredibly clunky way of doing things is because Clap is just broken when it
            // comes to specifying required subcommands in combination with Optional flags. The
            // clues to this awful situation are cunningly hidden among half a dozen issues and PRs
            // on the clap repo, e.g.
            //
            // https://github.com/clap-rs/clap/issues/5358
            //
            // In future, we can try making subcommands non-optional and removing this catchall and
            // see if this has been fixed.
            let help = Cli::command().render_help();
            println!("{help}");
            // Print help and exit
            Ok(())
        }
    };

    // Wait for the event task to finish
    let _ = event_kill_tx.send(()).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), event_task).await;

    result?;

    Ok(())
}
