use anyhow::{Context, Result, anyhow, bail};
use argh::FromArgs;
use futures_util::{SinkExt, StreamExt};
use http::Request;
use riichienv_core::action::{Action, ActionType};
use riichienv_core::observation::Observation;
use riichilab_agent_protocol::IncomingMessage;
use serde::Serialize;
use serde_json::Value;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_tungstenite::connect_async;
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
    Validate(ConnectCommand),
    Ranked(ConnectCommand),
}

#[derive(FromArgs)]
#[argh(subcommand, name = "validate", description = "connect to the validation queue")]
struct ConnectCommand {
    #[argh(option, description = "bot token; falls back to RIICHILAB_BOT_TOKEN")]
    token: Option<String>,
    #[argh(option, description = "override the websocket endpoint")]
    url: Option<String>,
    #[argh(option, description = "directory for per-game jsonl logs")]
    log_dir: Option<PathBuf>,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Args = argh::from_env();

    match args.command {
        Command::Validate(command) => run(command, QueueKind::Validate).await,
        Command::Ranked(command) => run(command, QueueKind::Ranked).await,
    }
}

async fn run(command: ConnectCommand, queue: QueueKind) -> Result<()> {
    let token = load_token(command.token)?;
    let url = command.url.unwrap_or_else(|| queue.default_url().to_owned());
    let mut logger = GameLogger::new(command.log_dir.unwrap_or_else(|| PathBuf::from("logs")));

    let request = Request::builder().uri(&url).header("Authorization", format!("Bearer {token}")).body(()).context("failed to build websocket request")?;

    let (mut socket, _) = connect_async(request).await.with_context(|| format!("failed to connect to {url}"))?;

    while let Some(frame) = socket.next().await {
        match frame.context("failed to receive websocket frame")? {
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

                if message.is_validation_result() {
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Binary(_) => {}
            Message::Ping(payload) => {
                socket.send(Message::Pong(payload)).await.context("failed to respond to ping")?;
            }
            Message::Pong(_) => {}
            Message::Frame(_) => {}
        }
    }

    Ok(())
}

fn handle_message(message: &IncomingMessage) -> Result<Option<String>> {
    let Some(request) = message.request_action()? else {
        return Ok(None);
    };

    let action = decode_observation(request.observation)?.select_action().ok_or_else(|| anyhow!("request_action contained no legal actions"))?;

    let action_mjai = action.to_mjai();
    if !request.possible_actions.iter().any(|candidate| candidate == &serde_json::from_str::<Value>(&action_mjai).expect("action mjai must be valid json")) {
        bail!("selected action not present in request_action.possible_actions: {action_mjai}");
    }

    Ok(Some(action_mjai))
}

fn decode_observation(encoded: &str) -> Result<DecodedObservation> {
    Observation::deserialize_from_base64(encoded).map(DecodedObservation).context("failed to decode 4-player observation")
}

struct DecodedObservation(Observation);

impl DecodedObservation {
    fn select_action(&self) -> Option<SelectedAction> {
        let actions = self.0.legal_actions_method();
        select_temporary_action(actions.iter().map(SelectedActionRef))
    }
}

#[derive(Debug, Clone)]
struct SelectedAction(Action);

impl SelectedAction {
    fn to_mjai(&self) -> String {
        self.0.to_mjai()
    }
}

struct SelectedActionRef<'a>(&'a Action);

impl SelectedActionRef<'_> {
    fn to_owned(&self) -> SelectedAction {
        SelectedAction(self.0.clone())
    }

    fn action_type(&self) -> ActionType {
        self.0.action_type
    }

    fn is_tsumogiri_discard(&self) -> bool {
        matches!(self.0.action_type, ActionType::Discard) && self.0.tile.is_some()
    }
}

fn select_temporary_action<'a>(possible_actions: impl IntoIterator<Item = SelectedActionRef<'a>>) -> Option<SelectedAction> {
    let actions: Vec<_> = possible_actions.into_iter().collect();
    find_action(&actions, |action| matches!(action.action_type(), ActionType::Tsumo | ActionType::Ron))
        .or_else(|| find_action(&actions, |action| matches!(action.action_type(), ActionType::Riichi)))
        .or_else(|| find_action(&actions, SelectedActionRef::is_tsumogiri_discard))
        .or_else(|| find_action(&actions, |action| matches!(action.action_type(), ActionType::Pass)))
        .or_else(|| actions.first().map(SelectedActionRef::to_owned))
}

fn find_action<'a>(possible_actions: &'a [SelectedActionRef<'a>], predicate: impl Fn(&SelectedActionRef<'a>) -> bool) -> Option<SelectedAction> {
    possible_actions.iter().find(|action| predicate(action)).map(SelectedActionRef::to_owned)
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
    current: Option<BufWriter<File>>,
    game_index: u64,
}

impl GameLogger {
    fn new(log_dir: PathBuf) -> Self {
        Self { log_dir, current: None, game_index: 0 }
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

        self.game_index += 1;
        let path = self.next_log_path()?;
        let file = File::create(&path).with_context(|| format!("failed to create log file {}", path.display()))?;
        self.current = Some(BufWriter::new(file));
        Ok(())
    }

    fn next_log_path(&self) -> Result<PathBuf> {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).context("system time is before unix epoch")?.as_secs();
        Ok(self.log_dir.join(format!("game-{timestamp}-{:04}.jsonl", self.game_index)))
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
