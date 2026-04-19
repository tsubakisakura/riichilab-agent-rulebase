mod rules;

use anyhow::{Context, Result, anyhow, bail};
use argh::FromArgs;
use chrono::Utc;
use dotenvy::dotenv;
use futures_util::{SinkExt, StreamExt};
use riichienv_core::observation::Observation;
use riichilab_agent_protocol::IncomingMessage;
use rustls::crypto::ring::default_provider;
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::{MissedTickBehavior, interval};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;

const VALIDATE_ENDPOINT: &str = "wss://game.riichi.dev/ws/validate";
const RANKED_ENDPOINT: &str = "wss://game.riichi.dev/ws/ranked";
const TOKEN_ENV: &str = "RIICHILAB_BOT_TOKEN";

#[derive(FromArgs)]
#[argh(description = "RiichiLab rule-based agent CLI")]
struct Args {
    #[argh(subcommand)]
    command: Command,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum Command {
    Validate(ValidateCommand),
    Ranked(RankedCommand),
}

#[derive(FromArgs)]
#[argh(subcommand, name = "validate", description = "connect to the validation queue")]
struct ValidateCommand {
    #[argh(option, description = "bot token; falls back to RIICHILAB_BOT_TOKEN")]
    token: Option<String>,
    #[argh(option, description = "override the websocket endpoint")]
    url: Option<String>,
    #[argh(option, description = "directory for per-game jsonl logs")]
    log_dir: Option<PathBuf>,
    #[argh(option, default = "1", description = "number of sessions to run")]
    games: u32,
}

#[derive(FromArgs)]
#[argh(subcommand, name = "ranked", description = "connect to the ranked queue")]
struct RankedCommand {
    #[argh(option, description = "bot token; falls back to RIICHILAB_BOT_TOKEN")]
    token: Option<String>,
    #[argh(option, description = "override the websocket endpoint")]
    url: Option<String>,
    #[argh(option, description = "directory for per-game jsonl logs")]
    log_dir: Option<PathBuf>,
    #[argh(option, default = "1", description = "number of sessions to run")]
    games: u32,
}

struct ConnectCommand {
    token: Option<String>,
    url: Option<String>,
    log_dir: Option<PathBuf>,
    games: u32,
}

impl From<ValidateCommand> for ConnectCommand {
    fn from(command: ValidateCommand) -> Self {
        Self {
            token: command.token,
            url: command.url,
            log_dir: command.log_dir,
            games: command.games,
        }
    }
}

impl From<RankedCommand> for ConnectCommand {
    fn from(command: RankedCommand) -> Self {
        Self {
            token: command.token,
            url: command.url,
            log_dir: command.log_dir,
            games: command.games,
        }
    }
}

#[derive(Clone, Copy)]
enum QueueKind {
    Validate,
    Ranked,
}

impl QueueKind {
    fn default_url(self) -> &'static str {
        match self {
            Self::Validate => VALIDATE_ENDPOINT,
            Self::Ranked => RANKED_ENDPOINT,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::Ranked => "ranked",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    default_provider().install_default().map_err(|_| anyhow!("failed to install rustls crypto provider"))?;
    let _ = dotenv();
    let args: Args = argh::from_env();

    match args.command {
        Command::Validate(command) => run(command.into(), QueueKind::Validate).await,
        Command::Ranked(command) => run(command.into(), QueueKind::Ranked).await,
    }
}

async fn run(command: ConnectCommand, queue: QueueKind) -> Result<()> {
    let token = load_token(command.token)?;
    let url = command.url.unwrap_or_else(|| queue.default_url().to_owned());
    let mut logger = GameLogger::new(command.log_dir.unwrap_or_else(|| PathBuf::from("logs")), queue);
    let games = command.games;

    for game_index in 0..games {
        let outcome = run_single_game(&token, &url, queue, &mut logger).await?;
        eprintln!("completed session {}/{} ({})", game_index + 1, games, outcome.label());
    }

    Ok(())
}

async fn run_single_game(token: &str, url: &str, queue: QueueKind, logger: &mut GameLogger) -> Result<SessionOutcome> {
    let mut request = url.to_owned().into_client_request().context("failed to build websocket request")?;
    request.headers_mut().insert("Authorization", format!("Bearer {token}").parse().context("failed to encode authorization header")?);

    let (mut socket, _) = connect_async(request).await.with_context(|| format!("failed to connect to {url}"))?;
    let mut heartbeat = interval(Duration::from_secs(5));
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Skip);
    heartbeat.tick().await;

    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                eprintln!("waiting for {} queue activity...", queue.label());
            }
            maybe_frame = socket.next() => {
                let Some(frame_result) = maybe_frame else {
                    logger.close()?;
                    bail!("websocket stream ended before session completion");
                };

                match frame_result.context("failed to receive websocket frame")? {
                    Message::Text(text) => {
                        let raw_value: Value = serde_json::from_str(&text).with_context(|| format!("invalid json frame: {text}"))?;
                        let message: IncomingMessage = serde_json::from_str(&text).with_context(|| format!("invalid transport message: {text}"))?;
                        eprintln!("in  {text}");
                        logger.log_incoming(&message, raw_value)?;

                        if let Some(outbound) = handle_message(&message)? {
                            eprintln!("out {outbound}");
                            logger.log_outgoing(&message, &outbound)?;
                            socket.send(Message::Text(outbound.into())).await.context("failed to send action")?;
                        }

                        if let Some(outcome) = session_outcome(queue, &message) {
                            logger.close()?;
                            return Ok(outcome);
                        }
                    }
                    Message::Close(_) => {
                        logger.close()?;
                        bail!("websocket closed before session completion");
                    }
                    Message::Binary(_) => {}
                    Message::Ping(payload) => {
                        socket.send(Message::Pong(payload)).await.context("failed to respond to ping")?;
                    }
                    Message::Pong(_) => {}
                    Message::Frame(_) => {}
                }
            }
        }
    }
}

fn session_outcome(queue: QueueKind, message: &IncomingMessage) -> Option<SessionOutcome> {
    match queue {
        QueueKind::Validate if message.is_validation_result() => Some(SessionOutcome::ValidationResult),
        QueueKind::Ranked if message.is_end_game() => Some(SessionOutcome::EndGame),
        _ => None,
    }
}

fn handle_message(message: &IncomingMessage) -> Result<Option<String>> {
    let Some(request) = message.request_action()? else {
        return Ok(None);
    };

    let observation = decode_observation(request.observation)?;
    let action = rules::choose_action(&observation).ok_or_else(|| anyhow!("request_action contained no legal actions"))?;

    let action_mjai = action.to_mjai();
    if !request.possible_actions.iter().any(|candidate| candidate == &serde_json::from_str::<Value>(&action_mjai).expect("action mjai must be valid json")) {
        bail!("selected action not present in request_action.possible_actions: {action_mjai}");
    }

    Ok(Some(action_mjai))
}

fn decode_observation(encoded: &str) -> Result<Observation> {
    Observation::deserialize_from_base64(encoded).context("failed to decode 4-player observation")
}

fn load_token(token_arg: Option<String>) -> Result<String> {
    if let Some(token) = token_arg {
        return Ok(token);
    }

    match std::env::var(TOKEN_ENV) {
        Ok(token) if !token.is_empty() => Ok(token),
        Ok(_) => bail!("{TOKEN_ENV} is empty"),
        Err(_) => bail!("missing bot token: pass --token or set {TOKEN_ENV}"),
    }
}

struct GameLogger {
    log_dir: PathBuf,
    queue: QueueKind,
    current: Option<BufWriter<File>>,
}

enum SessionOutcome {
    ValidationResult,
    EndGame,
}

impl SessionOutcome {
    fn label(&self) -> &'static str {
        match self {
            Self::ValidationResult => "validation_result",
            Self::EndGame => "end_game",
        }
    }
}

impl GameLogger {
    fn new(log_dir: PathBuf, queue: QueueKind) -> Self {
        Self { log_dir, queue, current: None }
    }

    fn log_incoming(&mut self, message: &IncomingMessage, raw_value: Value) -> Result<()> {
        if message.is_start_game() {
            self.open_new_file()?;
        }

        if self.current.is_some() {
            self.write_record("in", &raw_value)?;
        }

        if message.is_end_game() {
            self.close()?;
        }

        Ok(())
    }

    fn log_outgoing(&mut self, cause_message: &IncomingMessage, outbound: &str) -> Result<()> {
        if cause_message.request_action()?.is_none() {
            return Ok(());
        }

        if let Some(writer) = self.current.as_mut() {
            let raw_value = serde_json::from_str(outbound).context("failed to convert outbound action to json value")?;
            Self::write_record_inner(writer, "out", &raw_value)?;
        }

        Ok(())
    }

    fn open_new_file(&mut self) -> Result<()> {
        if self.current.is_some() {
            self.close()?;
        }

        fs::create_dir_all(&self.log_dir).with_context(|| format!("failed to create log directory {}", self.log_dir.display()))?;

        let path = self.next_log_path();
        let file = File::create(&path).with_context(|| format!("failed to create log file {}", path.display()))?;
        self.current = Some(BufWriter::new(file));
        Ok(())
    }

    fn next_log_path(&self) -> PathBuf {
        let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
        self.log_dir.join(format!("{}-{timestamp}.jsonl", self.queue.label()))
    }

    fn write_record(&mut self, dir: &'static str, data: &Value) -> Result<()> {
        let writer = self.current.as_mut().ok_or_else(|| anyhow!("attempted to log without an active game file"))?;
        Self::write_record_inner(writer, dir, data)
    }

    fn write_record_inner(writer: &mut BufWriter<File>, dir: &'static str, data: &Value) -> Result<()> {
        let record = JsonlRecord { dir, data };
        serde_json::to_writer(&mut *writer, &record).context("failed to write jsonl record")?;
        writer.write_all(b"\n").context("failed to write newline")?;
        writer.flush().context("failed to flush log writer")?;
        Ok(())
    }

    fn close(&mut self) -> Result<()> {
        if let Some(mut writer) = self.current.take() {
            writer.flush().context("failed to flush log writer")?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct JsonlRecord<'a> {
    dir: &'static str,
    data: &'a Value,
}
