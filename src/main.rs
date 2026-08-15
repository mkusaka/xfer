use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use clap::{Args, Parser, Subcommand, ValueEnum};
use dirs::home_dir;
use hf_hub::{Repo, RepoType, api::sync::Api};
use rustc_hash::FxHashMap;
use serde_json::Value;
use tempfile::Builder;
use tiktoken_rs::{CoreBPE, Rank};
use tokenizers::Tokenizer;
use walkdir::WalkDir;

const USER_MESSAGE_BEGIN: &str = "## My request for Codex:";
const HIDDEN_CONTEXT_TAGS: [&str; 7] = [
    "<user_instructions>",
    "<environment_context>",
    "<apps_instructions>",
    "<skills_instructions>",
    "<plugins_instructions>",
    "<collaboration_mode>",
    "<realtime_conversation>",
];
const KIMI_PATTERN: &str = concat!(
    r"[\p{Han}]+|",
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}&&[^\p{Han}]]*[\p{Ll}\p{Lm}\p{Lo}\p{M}&&[^\p{Han}]]+(?i:'s|'t|'re|'ve|'m|'ll|'d)?|",
    r"[^\r\n\p{L}\p{N}]?[\p{Lu}\p{Lt}\p{Lm}\p{Lo}\p{M}&&[^\p{Han}]]+[\p{Ll}\p{Lm}\p{Lo}\p{M}&&[^\p{Han}]]*(?i:'s|'t|'re|'ve|'m|'ll|'d)?|",
    r"\p{N}{1,3}| ?[^\s\p{L}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+"
);

#[derive(Parser)]
#[command(
    version,
    about = "Prepare a bounded session handoff for another coding agent"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a handoff from an explicit session JSONL file
    Pack {
        /// Codex or Claude session JSONL file
        session: PathBuf,

        #[command(flatten)]
        options: HandoffOptions,
    },

    /// Infer the current session JSONL from the host agent environment
    Infer(HandoffOptions),

    /// List supported destination models and their budgets
    Models,
}

#[derive(Args)]
struct HandoffOptions {
    /// Authoritative task for the receiving agent (reads stdin when omitted)
    #[arg(long, conflicts_with = "task_file")]
    task: Option<String>,

    /// Read the authoritative task from a file
    #[arg(long, value_name = "PATH", conflicts_with = "task")]
    task_file: Option<PathBuf>,

    /// Tokenizer profile used for the handoff budget
    #[arg(long, value_enum, default_value = "swe-1.7")]
    model: Model,

    /// Maximum handoff size as tokens or a model-context percentage (for example, 12.5%)
    #[arg(long, value_name = "TOKENS|PERCENT")]
    budget: Option<Budget>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Model {
    #[value(name = "swe-1.7")]
    Swe17,
    #[value(name = "glm-5.2")]
    Glm52,
}

impl Model {
    const ALL: [Self; 2] = [Self::Swe17, Self::Glm52];

    fn name(self) -> &'static str {
        match self {
            Self::Swe17 => "swe-1.7",
            Self::Glm52 => "glm-5.2",
        }
    }

    fn context_window(self) -> usize {
        match self {
            Self::Swe17 => 262_144,
            Self::Glm52 => 1_048_576,
        }
    }

    fn default_budget(self) -> usize {
        match self {
            Self::Swe17 => 32_768,
            Self::Glm52 => 65_536,
        }
    }

    fn tokenizer_name(self) -> &'static str {
        match self {
            Self::Swe17 => "Kimi K2.7 proxy (+10%)",
            Self::Glm52 => "GLM-5.2",
        }
    }

    fn tokenizer_repo(self) -> &'static str {
        match self {
            Self::Swe17 => "moonshotai/Kimi-K2.7-Code",
            Self::Glm52 => "zai-org/GLM-5.2",
        }
    }

    fn tokenizer_revision(self) -> &'static str {
        match self {
            Self::Swe17 => "74797c9c62378b951a1f6fcf5c4631024e9b8bef",
            Self::Glm52 => "b4734de4facf877f85769a911abafc5283eab3d9",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Budget {
    Tokens(usize),
    Percentage(f64),
}

impl Budget {
    fn resolve(self, model: Model) -> usize {
        match self {
            Self::Tokens(tokens) => tokens,
            Self::Percentage(percent) => {
                ((model.context_window() as f64) * percent / 100.0).floor() as usize
            }
        }
    }
}

impl FromStr for Budget {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(percent) = value.strip_suffix('%') {
            let percent = percent
                .parse::<f64>()
                .map_err(|_| "percentage must be a number followed by %".to_string())?;
            if !percent.is_finite() || !(0.0 < percent && percent <= 100.0) {
                return Err("percentage must be greater than 0% and at most 100%".to_string());
            }
            return Ok(Self::Percentage(percent));
        }

        let tokens = value
            .parse::<usize>()
            .map_err(|_| "budget must be a token count or percentage such as 12.5%".to_string())?;
        if tokens == 0 {
            return Err("token budget must be greater than zero".to_string());
        }
        Ok(Self::Tokens(tokens))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Turn {
    user: String,
    assistant: Option<String>,
}

enum TokenCounter {
    Swe17(CoreBPE),
    Glm52(Box<Tokenizer>),
}

impl TokenCounter {
    fn load(model: Model) -> Result<Self> {
        let client = Api::new().context("failed to initialize the Hugging Face client")?;
        let repo = client.repo(Repo::with_revision(
            model.tokenizer_repo().to_string(),
            RepoType::Model,
            model.tokenizer_revision().to_string(),
        ));

        match model {
            Model::Swe17 => {
                let path = repo
                    .get("tiktoken.model")
                    .context("failed to download the Kimi K2.7 tokenizer for SWE-1.7")?;
                Ok(Self::Swe17(load_kimi_tokenizer(&path)?))
            }
            Model::Glm52 => {
                let path = repo
                    .get("tokenizer.json")
                    .context("failed to download the GLM-5.2 tokenizer")?;
                let tokenizer = Tokenizer::from_file(&path)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
                    .context("failed to load the GLM-5.2 tokenizer")?;
                Ok(Self::Glm52(Box::new(tokenizer)))
            }
        }
    }

    fn count(&self, text: &str) -> Result<usize> {
        match self {
            Self::Swe17(tokenizer) => {
                let tokens = tokenizer.encode_ordinary(text).len();
                Ok(tokens.saturating_mul(11).div_ceil(10))
            }
            Self::Glm52(tokenizer) => tokenizer
                .encode(text, false)
                .map(|encoding| encoding.len())
                .map_err(|error| anyhow::anyhow!(error.to_string())),
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Models => {
            print_models();
            return Ok(());
        }
        Command::Pack { session, options } => create_handoff(&session, options)?,
        Command::Infer(options) => create_handoff(&infer_session_path()?, options)?,
    }

    Ok(())
}

fn create_handoff(session: &Path, options: HandoffOptions) -> Result<()> {
    let task = read_task(options.task, options.task_file, io::stdin())?;
    let budget = options
        .budget
        .map(|budget| budget.resolve(options.model))
        .unwrap_or_else(|| options.model.default_budget());
    if budget == 0 {
        bail!("the resolved budget is zero tokens");
    }
    if budget > options.model.context_window() {
        bail!(
            "the {budget}-token budget exceeds the {}-token context window for {}",
            options.model.context_window(),
            options.model.name()
        );
    }
    let turns = parse_session(session)?;
    let counter = TokenCounter::load(options.model)?;
    let handoff = pack_handoff(&turns, task.trim(), budget, &counter)?;

    let mut file = Builder::new().prefix("xfer-").suffix(".md").tempfile()?;
    file.write_all(handoff.as_bytes())?;
    file.flush()?;
    let path = file
        .into_temp_path()
        .keep()
        .context("failed to persist the handoff file")?;
    println!("{}", path.display());

    Ok(())
}

fn read_task(
    task: Option<String>,
    task_file: Option<PathBuf>,
    mut stdin: impl Read,
) -> Result<String> {
    let task = match (task, task_file) {
        (Some(task), None) => task,
        (None, Some(path)) => fs::read_to_string(&path)
            .with_context(|| format!("failed to read task file: {}", path.display()))?,
        (None, None) => {
            let mut task = String::new();
            stdin.read_to_string(&mut task)?;
            task
        }
        (Some(_), Some(_)) => unreachable!("clap rejects conflicting task inputs"),
    };

    if task.trim().is_empty() {
        bail!("task is empty; use --task, --task-file, or pipe it through stdin");
    }
    Ok(task)
}

fn print_models() {
    println!("MODEL\tCONTEXT\tDEFAULT BUDGET\tTOKENIZER\tREVISION");
    for model in Model::ALL {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            model.name(),
            model.context_window(),
            model.default_budget(),
            model.tokenizer_name(),
            model.tokenizer_revision()
        );
    }
}

fn infer_session_path() -> Result<PathBuf> {
    if let Some(session_id) = env::var("CLAUDE_CODE_SESSION_ID")
        .ok()
        .filter(|value| !value.is_empty())
    {
        let root = env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".claude")))
            .context("could not resolve the Claude configuration directory")?
            .join("projects");
        return find_session_file(&root, &session_id, SessionKind::Claude);
    }

    if let Some(thread_id) = env::var("CODEX_THREAD_ID")
        .ok()
        .filter(|value| !value.is_empty())
    {
        let root = env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".codex")))
            .context("could not resolve CODEX_HOME")?
            .join("sessions");
        return find_session_file(&root, &thread_id, SessionKind::Codex);
    }

    bail!(
        "could not infer the current session; CLAUDE_CODE_SESSION_ID and CODEX_THREAD_ID are both unset"
    )
}

#[derive(Clone, Copy)]
enum SessionKind {
    Claude,
    Codex,
}

fn find_session_file(root: &Path, session_id: &str, kind: SessionKind) -> Result<PathBuf> {
    let matches = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .filter(|path| match kind {
            SessionKind::Claude => {
                path.file_stem().and_then(|name| name.to_str()) == Some(session_id)
            }
            SessionKind::Codex => path
                .file_stem()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(session_id)),
        })
        .filter(|path| session_file_matches(path, session_id, kind))
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => bail!(
            "no session JSONL with id {session_id} was found under {}",
            root.display()
        ),
        _ => bail!(
            "multiple session JSONL files with id {session_id} were found under {}",
            root.display()
        ),
    }
}

fn session_file_matches(path: &Path, session_id: &str, kind: SessionKind) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };

    BufReader::new(file)
        .lines()
        .map_while(std::result::Result::ok)
        .any(|line| {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                return false;
            };
            match kind {
                SessionKind::Claude => {
                    value.get("sessionId").and_then(Value::as_str) == Some(session_id)
                }
                SessionKind::Codex => {
                    value.get("type").and_then(Value::as_str) == Some("session_meta")
                        && value.pointer("/payload/id").and_then(Value::as_str) == Some(session_id)
                }
            }
        })
}

fn load_kimi_tokenizer(path: &Path) -> Result<CoreBPE> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read tokenizer: {}", path.display()))?;
    let mut encoder: FxHashMap<Vec<u8>, Rank> = FxHashMap::default();

    for (index, line) in contents.lines().enumerate() {
        let mut parts = line.split_whitespace();
        let token = parts
            .next()
            .with_context(|| format!("missing token on tokenizer line {}", index + 1))?;
        let rank = parts
            .next()
            .with_context(|| format!("missing rank on tokenizer line {}", index + 1))?
            .parse::<Rank>()
            .with_context(|| format!("invalid rank on tokenizer line {}", index + 1))?;
        let bytes = STANDARD
            .decode(token)
            .with_context(|| format!("invalid token on tokenizer line {}", index + 1))?;
        encoder.insert(bytes, rank);
    }

    let first_special = encoder.len() as Rank;
    let special_tokens = (0..256)
        .map(|offset| {
            (
                format!("<|reserved_token_{}|>", first_special + offset),
                first_special + offset,
            )
        })
        .collect::<FxHashMap<_, _>>();

    CoreBPE::new(encoder, special_tokens, KIMI_PATTERN)
        .context("failed to construct the Kimi K2.7 tokenizer")
}

fn parse_session(path: &Path) -> Result<Vec<Turn>> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read session: {}", path.display()))?;
    let mut messages = Vec::new();
    let complete = contents.ends_with('\n');
    let line_count = contents.lines().count();
    let has_codex_user_events = contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|value| extract_codex_user_event(&value).is_some());

    for (index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) if !complete && index + 1 == line_count => break,
            Err(error) => {
                return Err(error).with_context(|| format!("invalid JSON on line {}", index + 1));
            }
        };
        let message = extract_codex_user_event(&value)
            .or_else(|| {
                let message = extract_codex_message(&value)?;
                if has_codex_user_events && matches!(message, Message::User(_)) {
                    None
                } else {
                    Some(message)
                }
            })
            .or_else(|| extract_claude_message(&value));
        if let Some(message) = message {
            messages.push(message);
        }
    }

    Ok(group_turns(messages))
}

#[derive(Debug, PartialEq, Eq)]
enum Message {
    User(String),
    Assistant(String),
}

fn extract_codex_user_event(value: &Value) -> Option<Message> {
    if value.get("type")?.as_str()? != "event_msg" {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "user_message" {
        return None;
    }

    let message = payload.get("message")?.as_str()?.trim();
    (!message.is_empty()).then(|| Message::User(message.to_string()))
}

fn extract_codex_message(value: &Value) -> Option<Message> {
    if value.get("type")?.as_str()? != "response_item" {
        return None;
    }

    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "message" {
        return None;
    }

    let role = payload.get("role")?.as_str()?;
    if role == "assistant"
        && payload
            .get("phase")
            .and_then(Value::as_str)
            .is_some_and(|phase| phase != "final_answer")
    {
        return None;
    }

    let text = payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str) {
            Some("input_text" | "output_text") => item.get("text").and_then(Value::as_str),
            _ => None,
        })
        .filter_map(|text| normalize_codex_text(role, text))
        .collect::<Vec<_>>()
        .join("\n");

    match (role, text.is_empty()) {
        ("user", false) => Some(Message::User(text)),
        ("assistant", false) => Some(Message::Assistant(text)),
        _ => None,
    }
}

fn normalize_codex_text(role: &str, text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || HIDDEN_CONTEXT_TAGS
            .iter()
            .any(|tag| trimmed.starts_with(tag))
    {
        return None;
    }

    let text = if role == "user" {
        trimmed
            .find(USER_MESSAGE_BEGIN)
            .map(|index| &trimmed[index + USER_MESSAGE_BEGIN.len()..])
            .unwrap_or(trimmed)
            .trim()
    } else {
        trimmed
    };

    (!text.is_empty()).then(|| text.to_string())
}

fn extract_claude_message(value: &Value) -> Option<Message> {
    let kind = value.get("type")?.as_str()?;
    if !matches!(kind, "user" | "assistant")
        || value.get("isMeta").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }

    let message = value.get("message")?;
    let content = message.get("content")?;
    let (text, has_tool_use) = match content {
        Value::String(text) => (text.trim().to_string(), false),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let has_tool_use = items
                .iter()
                .any(|item| item.get("type").and_then(Value::as_str) == Some("tool_use"));
            (text, has_tool_use)
        }
        _ => return None,
    };

    match (kind, text.is_empty(), has_tool_use) {
        ("user", false, _) => Some(Message::User(text)),
        ("assistant", false, false) => Some(Message::Assistant(text)),
        _ => None,
    }
}

fn group_turns(messages: Vec<Message>) -> Vec<Turn> {
    let mut turns = Vec::new();
    let mut current: Option<Turn> = None;

    for message in messages {
        match message {
            Message::User(text) => {
                if current
                    .as_ref()
                    .is_some_and(|turn| turn.assistant.is_some())
                {
                    turns.push(current.take().unwrap());
                }
                match &mut current {
                    Some(turn) => {
                        turn.user.push_str("\n\n");
                        turn.user.push_str(&text);
                    }
                    None => {
                        current = Some(Turn {
                            user: text,
                            assistant: None,
                        })
                    }
                }
            }
            Message::Assistant(text) => {
                if let Some(turn) = &mut current {
                    match &mut turn.assistant {
                        Some(answer) => {
                            answer.push_str("\n\n");
                            answer.push_str(&text);
                        }
                        None => turn.assistant = Some(text),
                    }
                }
            }
        }
    }

    if let Some(turn) = current {
        turns.push(turn);
    }
    turns
}

fn pack_handoff(
    turns: &[Turn],
    task: &str,
    budget: usize,
    counter: &TokenCounter,
) -> Result<String> {
    if task.is_empty() {
        bail!("--task must not be empty");
    }

    let mut selected = Vec::new();
    let base = render_handoff(&selected, task);
    if counter.count(&base)? > budget {
        bail!("the delegated task and handoff instructions exceed the {budget}-token budget");
    }

    for turn in turns.iter().rev() {
        let mut candidate = vec![turn.clone()];
        candidate.extend(selected.iter().cloned());
        if counter.count(&render_handoff(&candidate, task))? <= budget {
            selected = candidate;
            continue;
        }

        if selected.is_empty() && turn.assistant.is_some() {
            candidate[0].assistant = None;
            if counter.count(&render_handoff(&candidate, task))? <= budget {
                selected = candidate;
                continue;
            }
        }

        if selected.is_empty() {
            bail!("the latest user message does not fit in the {budget}-token budget");
        }
        break;
    }

    Ok(render_handoff(&selected, task))
}

fn render_handoff(turns: &[Turn], task: &str) -> String {
    let mut output = String::from(
        "# Delegated task context\n\n\
You are working as a subagent for another coding agent. Inspect the current repository and working tree yourself. Existing uncommitted changes may belong to the user or parent agent; do not revert or overwrite unrelated changes. Do not commit, push, or create a pull request unless the task explicitly requests it. The conversation below is background. The task at the end is authoritative.\n\n\
# Relevant recent conversation\n",
    );

    if turns.is_empty() {
        output.push_str("\nNo previous conversation was included.\n");
    } else {
        for turn in turns {
            output.push_str("\n## User\n\n");
            output.push_str(&turn.user);
            output.push('\n');
            if let Some(assistant) = &turn.assistant {
                output.push_str("\n## Assistant\n\n");
                output.push_str(assistant);
                output.push('\n');
            }
        }
    }

    output.push_str("\n# Your task\n\n");
    output.push_str(task);
    output.push('\n');
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_and_resolves_token_and_percentage_budgets() {
        assert_eq!("65536".parse::<Budget>().unwrap(), Budget::Tokens(65_536));
        assert_eq!("12.5%".parse::<Budget>().unwrap(), Budget::Percentage(12.5));
        assert_eq!(Budget::Percentage(12.5).resolve(Model::Swe17), 32_768);
        assert_eq!(Budget::Percentage(6.25).resolve(Model::Glm52), 65_536);
        assert!("0%".parse::<Budget>().is_err());
        assert!("101%".parse::<Budget>().is_err());
    }

    #[test]
    fn reads_task_from_stdin_when_no_flag_is_set() {
        let task = read_task(None, None, Cursor::new("implement it\n")).unwrap();
        assert_eq!(task, "implement it\n");
        assert!(read_task(None, None, Cursor::new("\n")).is_err());
    }

    #[test]
    fn reads_task_from_a_file() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "task from file").unwrap();

        let task = read_task(None, Some(file.path().to_path_buf()), Cursor::new("")).unwrap();

        assert_eq!(task, "task from file");
    }

    #[test]
    fn finds_codex_and_claude_sessions_by_embedded_id() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("2026/08/16");
        fs::create_dir_all(&nested).unwrap();

        let codex_path = nested.join("rollout-codex-id.jsonl");
        fs::write(
            &codex_path,
            serde_json::json!({
                "type": "session_meta",
                "payload": {"id": "codex-id"}
            })
            .to_string(),
        )
        .unwrap();
        let claude_path = nested.join("claude-id.jsonl");
        fs::write(
            &claude_path,
            serde_json::json!({"type": "user", "sessionId": "claude-id"}).to_string(),
        )
        .unwrap();

        assert_eq!(
            find_session_file(root.path(), "codex-id", SessionKind::Codex).unwrap(),
            codex_path
        );
        assert_eq!(
            find_session_file(root.path(), "claude-id", SessionKind::Claude).unwrap(),
            claude_path
        );
    }

    #[test]
    fn extracts_only_visible_codex_messages() {
        let hidden = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "<environment_context>hidden</environment_context>"}
            ]}
        });
        let user = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": "## My request for Codex:\nFix it"}
            ]}
        });
        let commentary = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "role": "assistant", "phase": "commentary", "content": [
                {"type": "output_text", "text": "Working"}
            ]}
        });
        let answer = serde_json::json!({
            "type": "response_item",
            "payload": {"type": "message", "role": "assistant", "phase": "final_answer", "content": [
                {"type": "output_text", "text": "Done"}
            ]}
        });

        assert_eq!(extract_codex_message(&hidden), None);
        assert_eq!(
            extract_codex_message(&user),
            Some(Message::User("Fix it".into()))
        );
        assert_eq!(extract_codex_message(&commentary), None);
        assert_eq!(
            extract_codex_message(&answer),
            Some(Message::Assistant("Done".into()))
        );
    }

    #[test]
    fn drops_claude_tool_trajectory() {
        let tool_call = serde_json::json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "text", "text": "I will inspect it"},
                {"type": "tool_use", "name": "Read"}
            ]}
        });
        let tool_result = serde_json::json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "content": "large output"}]}
        });

        assert_eq!(extract_claude_message(&tool_call), None);
        assert_eq!(extract_claude_message(&tool_result), None);
    }

    #[test]
    fn groups_messages_into_turns() {
        let turns = group_turns(vec![
            Message::User("one".into()),
            Message::Assistant("answer one".into()),
            Message::User("two".into()),
        ]);

        assert_eq!(
            turns,
            vec![
                Turn {
                    user: "one".into(),
                    assistant: Some("answer one".into())
                },
                Turn {
                    user: "two".into(),
                    assistant: None
                },
            ]
        );
    }

    #[test]
    fn drops_old_turns_to_fit_the_budget() {
        let counter = TokenCounter::Swe17(tiktoken_rs::cl100k_base().unwrap());
        let turns = vec![
            Turn {
                user: "old ".repeat(100),
                assistant: Some("old answer ".repeat(100)),
            },
            Turn {
                user: "latest request".into(),
                assistant: Some("latest answer".into()),
            },
        ];
        let latest_only = render_handoff(&turns[1..], "continue");
        let budget = counter.count(&latest_only).unwrap();

        let packed = pack_handoff(&turns, "continue", budget, &counter).unwrap();

        assert!(!packed.contains("old answer"));
        assert!(packed.contains("latest request"));
        assert!(packed.contains("latest answer"));
    }

    #[test]
    fn reads_a_complete_final_line_without_a_trailing_newline() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(
            file,
            "{}",
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "last request"}]
                }
            })
        )
        .unwrap();

        let turns = parse_session(file.path()).unwrap();

        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].user, "last request");
    }

    #[test]
    fn prefers_codex_user_events_over_injected_response_messages() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for value in [
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "# AGENTS.md instructions\ninjected"}]
                }
            }),
            serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "actual request"}
            }),
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "assistant",
                    "phase": "final_answer",
                    "content": [{"type": "output_text", "text": "answer"}]
                }
            }),
        ] {
            writeln!(file, "{value}").unwrap();
        }

        let turns = parse_session(file.path()).unwrap();

        assert_eq!(
            turns,
            vec![Turn {
                user: "actual request".into(),
                assistant: Some("answer".into())
            }]
        );
    }
}
