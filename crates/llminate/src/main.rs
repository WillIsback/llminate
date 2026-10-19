//! `llminate` — CLI entry point.

mod cli;
mod commands;
mod output;

use clap::Parser;
use cli::{Cli, Command};
use llminate_core::config::{self, Overrides};
use llminate_core::Error;
use output::Style;

#[tokio::main]
async fn main() {
    std::process::exit(run().await);
}

async fn run() -> i32 {
    let cli = Cli::parse();
    let style = Style::new(cli.no_color);
    match dispatch(cli, style).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("llminate: {e}");
            e.exit_code()
        }
    }
}

async fn dispatch(cli: Cli, style: Style) -> Result<(), Error> {
    match &cli.command {
        Command::Completions { shell } => {
            commands::completions(*shell);
            return Ok(());
        }
        Command::Targets => return commands::targets(&style),
        Command::Use { name } => return commands::use_target(name),
        _ => {}
    }

    let cfg = config::load()?;
    let ov = Overrides {
        target: cli.target.clone(),
        url: cli.url.clone(),
        api_key: cli.api_key.clone(),
        model: cli.model.clone(),
        timeout_secs: cli.timeout,
    };
    let target = config::resolve(&cfg, &ov)?;

    match cli.command {
        Command::Models { ids, raw } => commands::models(&target, &style, ids, raw).await,
        Command::Chat {
            message,
            model,
            system,
            max_tokens,
            temperature,
            no_stream,
            raw,
        } => {
            commands::chat(
                &target,
                message,
                model,
                system,
                max_tokens,
                temperature,
                no_stream,
                raw,
            )
            .await
        }
        Command::Complete {
            prompt,
            model,
            max_tokens,
            temperature,
            raw,
        } => commands::complete(&target, prompt, model, max_tokens, temperature, raw).await,
        Command::Embed { text, model, raw } => commands::embed(&target, text, model, raw).await,
        Command::Tokenize { text, model, raw } => {
            commands::tokenize(&target, text, model, raw).await
        }
        Command::Health { raw, time } => commands::health(&target, raw, time).await,
        Command::Info { raw } => commands::info(&target, &style, raw).await,
        Command::Metrics { raw, filter } => commands::metrics(&target, &style, raw, filter).await,
        Command::Smoke { prompt, model } => commands::smoke(&target, prompt, model).await,
        Command::Bench {
            n,
            concurrency,
            prompt,
            max_tokens,
            model,
        } => commands::bench(&target, n, concurrency, prompt, max_tokens, model).await,
        Command::Config => commands::config(&target, &cfg, &style),
        Command::Completions { .. } | Command::Targets | Command::Use { .. } => unreachable!(),
    }
}
