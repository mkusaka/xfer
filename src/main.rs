use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use clap::{Parser, ValueEnum};
use hf_hub::api::sync::Api;
use rustc_hash::FxHashMap;
use serde_json::Value;
use tempfile::Builder;
use tiktoken_rs::{CoreBPE, Rank};
use tokenizers::Tokenizer;

const DEFAULT_BUDGET: usize = 32_768;
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
    /// Codex or Claude session JSONL file
    session: PathBuf,

    /// Authoritative task for the receiving agent
    #[arg(long)]
    task: String,

    /// Tokenizer profile used for the handoff budget
    #[arg(long, value_enum, default_value = "swe-1.7")]
    model: Model,

    /// Maximum tokens for the complete handoff
    #[arg(long, default_value_t = DEFAULT_BUDGET)]
    budget: usize,
}

#[derive(Clone, Copy, ValueEnum)]
enum Model {
    #[value(name = "swe-1.7")]
    Swe17,
    #[value(name = "glm-5.2")]
    Glm52,
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

        match model {
            Model::Swe17 => {
                let path = client
                    .model("moonshotai/Kimi-K2.7-Code".to_string())
                    .get("tiktoken.model")
                    .context("failed to download the Kimi K2.7 tokenizer for SWE-1.7")?;
                Ok(Self::Swe17(load_kimi_tokenizer(&path)?))
            }
            Model::Glm52 => {
                let path = client
                    .model("zai-org/GLM-5.2".to_string())
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
    if cli.budget == 0 {
        bail!("--budget must be greater than zero");
    }

    let turns = parse_session(&cli.session)?;
    let counter = TokenCounter::load(cli.model)?;
    let handoff = pack_handoff(&turns, cli.task.trim(), cli.budget, &counter)?;

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
