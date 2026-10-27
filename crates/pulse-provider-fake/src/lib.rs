//! PULSE: the scripted fake provider used by tests.

use anyhow::Result;
use pulse_provider::{
    CancelToken, Login, Logouter, Message, Model, Provider, ProviderError, Request, Response, Role,
    Tool, ToolCall, Usage, register as register_provider,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::{Mutex, Once};

const NAME: &str = "fake";
const LABEL: &str = "Scripted (tests)";

/// Reply is one scripted answer from the fake provider.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    #[serde(rename = "content", default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(rename = "command", default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(rename = "report", default, skip_serializing_if = "String::is_empty")]
    pub report: String,
    /// A call to any tool by name, with `args` as its arguments; `command`
    /// is the shorthand for a `bash` call.
    #[serde(rename = "tool", default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    #[serde(
        rename = "args",
        default,
        skip_serializing_if = "serde_json::Value::is_null"
    )]
    pub args: serde_json::Value,
    #[serde(rename = "usage", default, skip_serializing_if = "is_usage_zero")]
    pub usage: Usage,
    #[serde(skip)]
    pub err: Option<ProviderError>,
    #[serde(rename = "model", default, skip_serializing_if = "String::is_empty")]
    pub model: String,
}

fn is_usage_zero(usage: &Usage) -> bool {
    usage.prompt_tokens == 0 && usage.completion_tokens == 0 && usage.cost == 0.0
}

/// Script configures the fake provider's replies and model list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Script {
    #[serde(rename = "replies", default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,
    #[serde(rename = "models", default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<Model>,
    #[serde(
        rename = "tokens_per_char",
        default,
        skip_serializing_if = "is_default_f64"
    )]
    pub tokens_per_char: f64,
    #[serde(rename = "loop", default, skip_serializing_if = "is_false")]
    pub loop_replies: bool,
}

fn is_default_f64(v: &f64) -> bool {
    *v == 0.0
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// A thread-safe scripted provider.
#[derive(Debug)]
pub struct FakeProvider {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    script: Script,
    next: usize,
    requests: Vec<Request>,
}

/// Returns a scripted fake provider.
pub fn new(mut script: Script) -> FakeProvider {
    if script.tokens_per_char == 0.0 {
        script.tokens_per_char = 0.25;
    }
    FakeProvider {
        inner: Mutex::new(Inner {
            script,
            next: 0,
            requests: Vec::new(),
        }),
    }
}

impl FakeProvider {
    fn next_reply(inner: &mut Inner) -> Reply {
        if inner.script.replies.is_empty() {
            return Reply {
                report: "SUMMARY: script exhausted".to_string(),
                ..Reply::default()
            };
        }
        if inner.next < inner.script.replies.len() {
            let reply = inner.script.replies[inner.next].clone();
            inner.next += 1;
            return reply;
        }
        if inner.script.loop_replies {
            return inner.script.replies[inner.script.replies.len() - 1].clone();
        }
        Reply {
            report: "SUMMARY: script exhausted".to_string(),
            ..Reply::default()
        }
    }

    /// Requests returns a copy of every chat request received.
    pub fn requests(&self) -> Vec<Request> {
        self.inner
            .lock()
            .expect("fake provider poisoned")
            .requests
            .clone()
    }
}

impl Provider for FakeProvider {
    fn name(&self) -> &str {
        NAME
    }

    fn label(&self) -> &str {
        LABEL
    }

    fn chat(&self, cancel: &CancelToken, req: Request) -> Result<Response> {
        cancel.check()?;
        let mut inner = self.inner.lock().expect("fake provider poisoned");
        inner.requests.push(req.clone());
        let request_count = inner.requests.len();
        let reply = Self::next_reply(&mut inner);
        let tokens_per_char = inner.script.tokens_per_char;
        drop(inner);

        if let Some(err) = reply.err {
            return Err(err.into());
        }

        let mut msg = Message::new(Role::Assistant);
        let mut finish = "stop".to_string();
        if !reply.command.is_empty() || !reply.tool.is_empty() {
            let (name, args) = if reply.tool.is_empty() {
                (
                    "bash".to_string(),
                    pulse_store::gojson::to_string(&CommandArgs {
                        command: reply.command.clone(),
                    })?,
                )
            } else {
                let args = if reply.args.is_null() {
                    serde_json::Value::Object(Default::default())
                } else {
                    reply.args.clone()
                };
                (reply.tool.clone(), serde_json::to_string(&args)?)
            };
            msg.content = reply.content.clone();
            msg.tool_calls = vec![ToolCall {
                id: format!("call-{request_count}"),
                name,
                arguments: args,
            }];
            finish = "tool_calls".to_string();
        } else if !reply.report.is_empty() {
            msg.content = reply.report.clone();
        } else {
            msg.content = reply.content.clone();
        }

        let mut usage = reply.usage;
        if usage.prompt_tokens == 0 {
            usage.prompt_tokens = (request_chars(&req) as f64 * tokens_per_char) as i32;
        }
        if usage.completion_tokens == 0 {
            usage.completion_tokens = (reply_text(&reply).len() / 4) as i32;
        }
        let model = if reply.model.is_empty() {
            req.model
        } else {
            reply.model
        };
        Ok(Response {
            message: Some(msg),
            finish_reason: finish,
            choices: 1,
            usage,
            model,
            request_id: String::new(),
        })
    }

    fn models(&self, cancel: &CancelToken) -> Result<Vec<Model>> {
        cancel.check()?;
        let inner = self.inner.lock().expect("fake provider poisoned");
        if !inner.script.models.is_empty() {
            return Ok(inner.script.models.clone());
        }
        Ok(vec![Model {
            id: "fake-1".to_string(),
            name: "Fake 1".to_string(),
            vendor: "fake".to_string(),
            chat: true,
            tool_calls: true,
            premium: 0.0,
            preview: false,
            context_window: 128000,
            max_output: 0,
            default: true,
        }])
    }

    fn status(&self, cancel: &CancelToken) -> Result<Login> {
        cancel.check()?;
        Ok(Login {
            logged_in: true,
            user: "scripted".to_string(),
            source: "built-in".to_string(),
            ..Login::default()
        })
    }

    fn as_logouter(&self) -> Option<&dyn Logouter> {
        Some(self)
    }
}

impl Logouter for FakeProvider {
    fn logout(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Serialize)]
struct CommandArgs {
    command: String,
}

static REGISTER_ONCE: Once = Once::new();

/// Register adds the fake provider to the global registry once.
pub fn register() {
    REGISTER_ONCE.call_once(|| {
        register_provider(NAME, LABEL, |options| {
            let path = options.env("PULSE_HARNESS_FAKE_SCRIPT");
            if !path.is_empty() {
                let data = fs::read(path)?;
                let script = serde_json::from_slice(&data)?;
                return Ok(Box::new(new(script)));
            }
            Ok(Box::new(new(default_script())))
        });
    });
}

fn default_script() -> Script {
    Script {
        replies: vec![
            Reply {
                command: "echo hi".to_string(),
                ..Reply::default()
            },
            Reply {
                report: "SUMMARY: scripted run finished.".to_string(),
                ..Reply::default()
            },
        ],
        ..Script::default()
    }
}

fn request_chars(req: &Request) -> usize {
    let mut total = 0;
    for msg in &req.messages {
        total += msg.content.len();
        for call in &msg.tool_calls {
            total += call.arguments.len();
        }
    }
    total
}

fn reply_text(reply: &Reply) -> String {
    format!("{}{}{}", reply.content, reply.command, reply.report)
}

#[allow(dead_code)]
fn clone_request(req: &Request) -> Request {
    Request {
        messages: clone_messages(&req.messages),
        tools: req.tools.iter().map(clone_tool).collect(),
        ..req.clone()
    }
}

#[allow(dead_code)]
fn clone_messages(input: &[Message]) -> Vec<Message> {
    input.to_vec()
}

#[allow(dead_code)]
fn clone_tool(tool: &Tool) -> Tool {
    tool.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulse_provider::{Options, open, provider_error};

    fn user_request() -> Request {
        Request {
            model: "fake-1".to_string(),
            messages: vec![Message {
                role: Role::User,
                content: "task".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: String::new(),
                name: String::new(),
                images: Vec::new(),
            }],
            ..Request::default()
        }
    }

    #[test]
    fn script_order_and_exhaustion() {
        let p = new(Script {
            replies: vec![
                Reply {
                    content: "first".to_string(),
                    ..Reply::default()
                },
                Reply {
                    command: "echo hi".to_string(),
                    content: "running".to_string(),
                    ..Reply::default()
                },
                Reply {
                    report: "SUMMARY: done".to_string(),
                    ..Reply::default()
                },
            ],
            ..Script::default()
        });
        let cancel = CancelToken::new();
        let req = user_request();
        let resp = p.chat(&cancel, req.clone()).unwrap();
        assert_eq!(resp.message.unwrap().content, "first");

        let resp = p.chat(&cancel, req.clone()).unwrap();
        let msg = resp.message.unwrap();
        assert_eq!(msg.content, "running");
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].id, "call-2");
        assert_eq!(resp.finish_reason, "tool_calls");

        let resp = p.chat(&cancel, req.clone()).unwrap();
        assert_eq!(resp.message.unwrap().content, "SUMMARY: done");

        let resp = p.chat(&cancel, req).unwrap();
        assert_eq!(resp.message.unwrap().content, "SUMMARY: script exhausted");
        assert_eq!(p.requests().len(), 4);
    }

    #[test]
    fn a_scripted_tool_call_names_any_tool() {
        let script: Script = serde_json::from_str(
            r#"{"replies":[{"tool":"process","args":{"action":"stop","id":"bg-1"}},{"tool":"plan"}]}"#,
        )
        .unwrap();
        let p = new(script);
        let cancel = CancelToken::new();
        let msg = p.chat(&cancel, user_request()).unwrap().message.unwrap();
        assert_eq!(msg.tool_calls[0].name, "process");
        assert_eq!(
            msg.tool_calls[0].arguments,
            r#"{"action":"stop","id":"bg-1"}"#
        );
        let msg = p.chat(&cancel, user_request()).unwrap().message.unwrap();
        assert_eq!(msg.tool_calls[0].name, "plan");
        assert_eq!(msg.tool_calls[0].arguments, "{}");
    }

    #[test]
    fn tokens_per_char_and_loop() {
        let p = new(Script {
            replies: vec![Reply {
                content: "abcd".to_string(),
                ..Reply::default()
            }],
            tokens_per_char: 0.5,
            loop_replies: true,
            ..Script::default()
        });
        let req = Request {
            messages: vec![
                Message {
                    role: Role::User,
                    content: "123456".to_string(),
                    tool_calls: Vec::new(),
                    tool_call_id: String::new(),
                    name: String::new(),
                    images: Vec::new(),
                },
                Message {
                    role: Role::Assistant,
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: r#"{"command":"echo"}"#.to_string(),
                    }],
                    tool_call_id: String::new(),
                    name: String::new(),
                    images: Vec::new(),
                },
            ],
            ..Request::default()
        };
        let cancel = CancelToken::new();
        for _ in 0..2 {
            let resp = p.chat(&cancel, req.clone()).unwrap();
            assert_eq!(resp.usage.prompt_tokens, 12);
            assert_eq!(resp.usage.completion_tokens, 1);
        }
    }

    #[test]
    fn reply_error() {
        let want = ProviderError::new("fake", 0, "", "boom", false);
        let p = new(Script {
            replies: vec![Reply {
                err: Some(want.clone()),
                ..Reply::default()
            }],
            ..Script::default()
        });
        let err = p.chat(&CancelToken::new(), Request::default()).unwrap_err();
        assert_eq!(provider_error(&err), Some(&want));
    }

    #[test]
    fn register_idempotent_and_json_script() {
        register();
        register();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.json");
        let data = serde_json::to_vec(&Script {
            replies: vec![Reply {
                report: "SUMMARY: from json".to_string(),
                usage: Usage {
                    prompt_tokens: 3,
                    ..Usage::default()
                },
                ..Reply::default()
            }],
            models: vec![Model {
                id: "json-model".to_string(),
                name: "JSON Model".to_string(),
                vendor: "fake".to_string(),
                chat: true,
                tool_calls: false,
                premium: 0.0,
                preview: false,
                context_window: 0,
                max_output: 0,
                default: false,
            }],
            ..Script::default()
        })
        .unwrap();
        fs::write(&path, data).unwrap();
        let path_string = path.to_string_lossy().to_string();
        let p = open(
            "fake",
            Options {
                env: Some(std::sync::Arc::new(move |key| {
                    if key == "PULSE_HARNESS_FAKE_SCRIPT" {
                        path_string.clone()
                    } else {
                        String::new()
                    }
                })),
                ..Options::default()
            },
        )
        .unwrap();

        let models = p.models(&CancelToken::new()).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "json-model");
        let resp = p
            .chat(
                &CancelToken::new(),
                Request {
                    model: "json-model".to_string(),
                    ..Request::default()
                },
            )
            .unwrap();
        assert_eq!(resp.message.unwrap().content, "SUMMARY: from json");
        assert_eq!(resp.usage.prompt_tokens, 3);
    }
}
