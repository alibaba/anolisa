use serde_json::{json, Value};

pub enum ControlRequest {
    Initialize {
        request_id: String,
    },
    CanUseTool {
        request_id: String,
        tool_name: String,
        tool_input: Value,
        tool_use_id: String,
    },
    AskUser {
        request_id: String,
        question: String,
        options: Vec<AskUserOption>,
        allow_free_text: bool,
        multi_select: bool,
    },
}

#[derive(Debug, Clone)]
pub struct AskUserOption {
    pub label: String,
    pub description: Option<String>,
}

pub fn parse_control_request(line: &str) -> Option<ControlRequest> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("type")?.as_str()? != "control_request" {
        return None;
    }
    let request = v.get("request")?;
    let subtype = request.get("subtype")?.as_str()?;
    let request_id = v.get("request_id")?.as_str()?.to_string();

    match subtype {
        "initialize" => Some(ControlRequest::Initialize { request_id }),
        "can_use_tool" => {
            let tool_name = request.get("tool_name")?.as_str()?.to_string();
            let tool_input = request.get("input")?.clone();
            let tool_use_id = request.get("tool_use_id")?.as_str()?.to_string();
            Some(ControlRequest::CanUseTool {
                request_id,
                tool_name,
                tool_input,
                tool_use_id,
            })
        }
        "ask_user" => {
            let question = request.get("question")?.as_str()?.to_string();
            let allow_free_text = request
                .get("allow_free_text")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let multi_select = request
                .get("multi_select")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let options = request
                .get("options")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            let label = item.get("label")?.as_str()?.to_string();
                            let description = item
                                .get("description")
                                .and_then(|d| d.as_str())
                                .map(|s| s.to_string());
                            Some(AskUserOption { label, description })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(ControlRequest::AskUser {
                request_id,
                question,
                options,
                allow_free_text,
                multi_select,
            })
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct QuestionResponse {
    pub request_id: String,
    pub answer: String,
}

pub fn serialize_answer(request_id: &str, answer: &str) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "answer": answer
            }
        }
    })
    .to_string()
}

#[derive(Debug, Clone)]
pub struct ApprovalResponse {
    pub request_id: String,
    pub tool_use_id: Option<String>,
    pub decision: ApprovalDecision,
}

#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    Allow,
    Deny { message: String },
}

pub fn serialize_initialize(request_id: &str) -> String {
    json!({
        "request_id": request_id,
        "type": "control_request",
        "request": { "subtype": "initialize" }
    })
    .to_string()
}

pub fn serialize_user_message(content: &str, session_id: Option<&str>) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": content },
        "parent_tool_use_id": null,
        "session_id": session_id.unwrap_or("default")
    })
    .to_string()
}

pub fn serialize_allow(request_id: &str, tool_use_id: &str) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "behavior": "allow",
                "updatedPermissions": [],
                "toolUseID": tool_use_id
            }
        }
    })
    .to_string()
}

pub fn serialize_deny(request_id: &str, message: &str) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": {
                "behavior": "deny",
                "message": message
            }
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_can_use_tool() {
        let line = r#"{"type":"control_request","request_id":"req-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"echo hello"},"tool_use_id":"toolu_xxx"}}"#;
        let req = parse_control_request(line).expect("should parse");
        match req {
            ControlRequest::CanUseTool {
                request_id,
                tool_name,
                tool_input,
                tool_use_id,
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(tool_name, "Bash");
                assert_eq!(tool_input["command"], "echo hello");
                assert_eq!(tool_use_id, "toolu_xxx");
            }
            _ => panic!("expected CanUseTool"),
        }
    }

    #[test]
    fn parse_initialize() {
        let line = r#"{"request_id":"init-1","type":"control_request","request":{"subtype":"initialize"}}"#;
        let req = parse_control_request(line).expect("should parse");
        match req {
            ControlRequest::Initialize { request_id } => {
                assert_eq!(request_id, "init-1");
            }
            _ => panic!("expected Initialize"),
        }
    }

    #[test]
    fn parse_ask_user() {
        let line = r#"{"type":"control_request","request_id":"ask-1","request":{"subtype":"ask_user","question":"What color?","options":[{"label":"Red","description":"Warm color"},{"label":"Blue"}],"allow_free_text":true,"multi_select":false}}"#;
        let req = parse_control_request(line).expect("should parse");
        match req {
            ControlRequest::AskUser {
                request_id,
                question,
                options,
                allow_free_text,
                multi_select,
            } => {
                assert_eq!(request_id, "ask-1");
                assert_eq!(question, "What color?");
                assert_eq!(options.len(), 2);
                assert_eq!(options[0].label, "Red");
                assert_eq!(
                    options[0].description.as_deref(),
                    Some("Warm color")
                );
                assert_eq!(options[1].label, "Blue");
                assert!(options[1].description.is_none());
                assert!(allow_free_text);
                assert!(!multi_select);
            }
            _ => panic!("expected AskUser"),
        }
    }

    #[test]
    fn serialize_answer_format() {
        let s = serialize_answer("ask-1", "Red");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "ask-1");
        assert_eq!(v["response"]["response"]["answer"], "Red");
    }

    #[test]
    fn parse_non_control_request_returns_none() {
        assert!(parse_control_request(r#"{"type":"assistant","message":"hi"}"#).is_none());
        assert!(parse_control_request(r#"{"type":"result","result":"done"}"#).is_none());
        assert!(parse_control_request("not json at all").is_none());
        assert!(parse_control_request("").is_none());
    }

    #[test]
    fn serialize_allow_format() {
        let s = serialize_allow("req-42", "toolu_abc");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "req-42");
        assert_eq!(v["response"]["response"]["behavior"], "allow");
        assert!(v["response"]["response"].get("updatedInput").is_none());
        assert_eq!(v["response"]["response"]["updatedPermissions"], json!([]));
        assert_eq!(v["response"]["response"]["toolUseID"], "toolu_abc");
    }

    #[test]
    fn serialize_deny_format() {
        let s = serialize_deny("req-99", "User denied");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "control_response");
        assert_eq!(v["response"]["subtype"], "success");
        assert_eq!(v["response"]["request_id"], "req-99");
        assert_eq!(v["response"]["response"]["behavior"], "deny");
        assert_eq!(v["response"]["response"]["message"], "User denied");
    }

    #[test]
    fn serialize_initialize_format() {
        let s = serialize_initialize("init-7");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "control_request");
        assert_eq!(v["request_id"], "init-7");
        assert_eq!(v["request"]["subtype"], "initialize");
    }

    #[test]
    fn serialize_user_message_format() {
        let s = serialize_user_message("hello world", Some("sess-1"));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(v["message"]["content"], "hello world");
        assert!(v["parent_tool_use_id"].is_null());
        assert_eq!(v["session_id"], "sess-1");

        let s2 = serialize_user_message("hi", None);
        let v2: Value = serde_json::from_str(&s2).unwrap();
        assert_eq!(v2["session_id"], "default");
    }
}
