use anyhow::{Context, Result, anyhow, bail};
use argh::FromArgs;
use futures_util::{SinkExt, StreamExt};
use http::Request;
use riichilab_agent_protocol::{Action, Event};
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

    let request = Request::builder().uri(&url).header("Authorization", format!("Bearer {token}")).body(()).context("failed to build websocket request")?;

    let (mut socket, _) = connect_async(request).await.with_context(|| format!("failed to connect to {url}"))?;

    while let Some(frame) = socket.next().await {
        match frame.context("failed to receive websocket frame")? {
            Message::Text(text) => {
                let event: Event = serde_json::from_str(&text).with_context(|| format!("invalid event: {text}"))?;
                eprintln!("in  {text}");

                if let Some(action) = handle_event(&event)? {
                    let message = serde_json::to_string(&action).context("failed to serialize action")?;
                    eprintln!("out {message}");
                    socket.send(Message::Text(message.into())).await.context("failed to send action")?;
                }

                if matches!(event, Event::EndGame { .. } | Event::ValidationResult { .. }) {
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

fn handle_event(event: &Event) -> Result<Option<Action>> {
    match event {
        Event::RequestAction { possible_actions, .. } => {
            let action = select_temporary_action(possible_actions).ok_or_else(|| anyhow!("request_action contained no possible actions"))?;
            Ok(Some(action))
        }
        _ => Ok(None),
    }
}

fn select_temporary_action(possible_actions: &[Action]) -> Option<Action> {
    find_action(possible_actions, |action| matches!(action, Action::Hora { .. }))
        .or_else(|| find_action(possible_actions, |action| matches!(action, Action::Reach { .. })))
        .or_else(|| find_action(possible_actions, |action| matches!(action, Action::Dahai { tsumogiri: true, .. })))
        .or_else(|| find_action(possible_actions, |action| matches!(action, Action::None)))
        .or_else(|| possible_actions.first().cloned())
}

fn find_action(possible_actions: &[Action], predicate: impl Fn(&Action) -> bool) -> Option<Action> {
    possible_actions.iter().find(|action| predicate(action)).cloned()
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
