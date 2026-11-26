//! The platform commands of `ibc`: sign in, submit, scrim, read the ladder and matches, download replays and
//! the map pool. docs/CLI.md is the user-facing description; keep it in step.
//!
//! Every command takes `--json` (one JSON document on stdout, stable field names) and ends with exit code 0
//! (ok), 1 (server or user error), 2 (usage) or 3 (sign-in needed). Errors print the server's
//! `{error, message}`; under `--json` they are printed as `{error, message, status, retry_after}` on stdout.

mod auth;
mod client;
mod config;
mod league;
mod maps;
mod submit;

use clap::{Args, Subcommand};
use client::{CliError, CliResult, Client};
use serde_json::Value;
use std::path::PathBuf;

/// Options every remote command takes.
#[derive(Args, Clone, Debug, Default)]
pub struct Opts {
    /// Platform URL (default: IBC_SERVER, then the server you signed in to)
    #[arg(long, value_name = "URL")]
    server: Option<String>,
    /// Print one JSON document on stdout instead of text
    #[arg(long)]
    json: bool,
    /// When rate limited (429), sleep for the server's Retry-After and retry once
    #[arg(long)]
    wait: bool,
}

impl Opts {
    /// The client for the resolved server and token.
    fn client(&self) -> CliResult<Client> {
        let r = config::resolve_env(self.server.as_deref())?;
        Ok(Client::new(&r.server, r.token, self.wait))
    }

    /// Print `value` under `--json`, or run the text renderer.
    fn emit(&self, value: &Value, text: impl FnOnce(&Value)) {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(value).unwrap_or_default()
            );
        } else {
            text(value);
        }
    }
}

#[derive(Subcommand)]
pub enum RemoteCommand {
    /// Sign in to the platform: device code in the browser, or --token to paste an ibu…/ibt… token
    Login {
        /// Store this token instead of running the device flow
        #[arg(long)]
        token: Option<String>,
        #[command(flatten)]
        opts: Opts,
    },
    /// Forget the stored credentials (and end the session on the server)
    Logout {
        #[command(flatten)]
        opts: Opts,
    },
    /// Show the signed-in user and team
    Whoami {
        #[command(flatten)]
        opts: Opts,
    },
    /// Build a bot in release mode and upload it (a .wasm file is uploaded as is)
    Submit {
        bot: PathBuf,
        /// Make it the live submission (the one ranked and scrim matches use) once it validates
        #[arg(long)]
        live: bool,
        /// A note to remember this version by
        #[arg(long)]
        note: Option<String>,
        /// Language of a prebuilt .wasm (default: rust); a bot directory is detected
        #[arg(long)]
        language: Option<String>,
        #[command(flatten)]
        opts: Opts,
    },
    /// List your team's submissions, or choose which one is live with --live ID
    Submissions {
        /// Make this submission live without re-uploading
        #[arg(long, value_name = "ID")]
        live: Option<String>,
        #[command(flatten)]
        opts: Opts,
    },
    /// Show the ladder
    Ladder {
        /// Only the first N teams
        #[arg(long, value_name = "N")]
        top: Option<usize>,
        #[command(flatten)]
        opts: Opts,
    },
    /// List matches, most recent first
    Matches {
        /// Only your team's matches
        #[arg(long, conflicts_with = "team")]
        mine: bool,
        /// Only this team's matches (id or name)
        #[arg(long)]
        team: Option<String>,
        /// Only this series
        #[arg(long)]
        series: Option<String>,
        /// ranked, unranked or tournament
        #[arg(long, value_parser = ["ranked", "unranked", "tournament"])]
        kind: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// Page backwards: the `next` of the previous page
        #[arg(long)]
        before: Option<String>,
        #[command(flatten)]
        opts: Opts,
    },
    /// Request an unranked series against another team (counts against your daily quota)
    Scrim {
        /// The opponent team (id, or name where the server accepts names)
        opponent: String,
        #[arg(long, default_value_t = 1)]
        games: u32,
        /// Maps to play, comma separated (default: the server picks from the pool)
        #[arg(long, value_delimiter = ',')]
        maps: Vec<String>,
        /// Play this submission of yours instead of the live one
        #[arg(long, value_name = "SUBMISSION")]
        me: Option<String>,
        /// Play against the opponent's bot from this earlier series
        #[arg(long, value_name = "SERIES")]
        rematch: Option<String>,
        #[command(flatten)]
        opts: Opts,
    },
    /// The platform's map pool
    Maps {
        #[command(subcommand)]
        command: MapsCommand,
    },
}

#[derive(Subcommand)]
pub enum MapsCommand {
    /// Download the pool into a directory as .ibcmap files (other files there are left alone)
    Pull {
        #[arg(long, default_value = "maps")]
        dir: PathBuf,
        /// Also maps retired from the pool
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        opts: Opts,
    },
}

/// `ibc replay download`
#[derive(Args)]
pub struct ReplayDownload {
    match_id: String,
    /// Where to write it (default: out/replays/<MATCH_ID>.ibcr)
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// public, or team (your own team's memory and logs kept)
    #[arg(long, value_parser = ["public", "team"])]
    view: Option<String>,
    #[command(flatten)]
    opts: Opts,
}

/// Run a remote command. Failures are printed here and end the process with their exit code.
/// The server `ibc login` stored, if any (`ibc update` and the daily version check fall back to it).
pub fn stored_server() -> Option<String> {
    let path = config::credentials_path().ok()?;
    config::load(&path).ok().flatten().map(|c| c.server)
}

pub fn run(command: RemoteCommand) -> anyhow::Result<()> {
    let opts = match &command {
        RemoteCommand::Login { opts, .. }
        | RemoteCommand::Logout { opts }
        | RemoteCommand::Whoami { opts }
        | RemoteCommand::Submit { opts, .. }
        | RemoteCommand::Submissions { opts, .. }
        | RemoteCommand::Ladder { opts, .. }
        | RemoteCommand::Matches { opts, .. }
        | RemoteCommand::Scrim { opts, .. }
        | RemoteCommand::Maps {
            command: MapsCommand::Pull { opts, .. },
        } => opts.clone(),
    };
    finish(&opts, dispatch(command))
}

pub fn replay_download(args: ReplayDownload) -> anyhow::Result<()> {
    let opts = args.opts.clone();
    finish(
        &opts,
        league::replay_download(&args.opts, &args.match_id, args.out, args.view),
    )
}

fn dispatch(command: RemoteCommand) -> CliResult<()> {
    match command {
        RemoteCommand::Login { token, opts } => auth::login(&opts, token),
        RemoteCommand::Logout { opts } => auth::logout(&opts),
        RemoteCommand::Whoami { opts } => auth::whoami(&opts),
        RemoteCommand::Submit {
            bot,
            live,
            note,
            language,
            opts,
        } => submit::submit(&opts, &bot, live, note, language),
        RemoteCommand::Submissions { live, opts } => submit::submissions(&opts, live),
        RemoteCommand::Ladder { top, opts } => league::ladder(&opts, top),
        RemoteCommand::Matches {
            mine,
            team,
            series,
            kind,
            limit,
            before,
            opts,
        } => {
            let filter = league::MatchArgs {
                mine,
                team,
                series,
                kind,
                limit,
                before,
            };
            league::matches(&opts, filter)
        }
        RemoteCommand::Scrim {
            opponent,
            games,
            maps,
            me,
            rematch,
            opts,
        } => league::scrim(
            &opts,
            league::ScrimArgs {
                opponent,
                games,
                maps,
                me,
                rematch,
            },
        ),
        RemoteCommand::Maps {
            command: MapsCommand::Pull { dir, all, opts },
        } => maps::pull(&opts, &dir, all),
    }
}

fn finish(opts: &Opts, result: CliResult<()>) -> anyhow::Result<()> {
    if let Err(e) = result {
        if opts.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&e.to_json()).unwrap_or_default()
            );
        } else {
            eprintln!("error: {e}");
        }
        std::process::exit(e.exit);
    }
    Ok(())
}

/// A server-chosen name that is safe as a file name: `[A-Za-z0-9_.-]`, at most 64 bytes, not starting with
/// a dot (so no separators, no `..`, nothing Windows refuses).
fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// A string field, or "-".
fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("-")
}

/// Any scalar field as text, or "-".
fn t(v: &Value, key: &str) -> String {
    match v.get(key) {
        None | Some(Value::Null) => "-".to_string(),
        Some(Value::String(x)) => x.clone(),
        Some(other) => other.to_string(),
    }
}

/// An error for an answer that lacks a field the command needs.
fn missing(what: &str) -> CliError {
    CliError::new(
        client::EXIT_ERROR,
        "bad_response",
        format!("the server's answer has no {what}"),
    )
}
