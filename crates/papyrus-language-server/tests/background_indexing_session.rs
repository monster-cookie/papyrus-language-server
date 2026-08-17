use std::{
    fs, thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use serde_json::{Value, json};

#[test]
fn initializes_before_indexing_and_replays_an_open_overlay() {
    let root = temp_root();
    fs::write(
        root.join("Actor.psc"),
        "ScriptName Actor\nFunction Jump()\nEndFunction\n",
    )
    .unwrap();
    for index in 0..500 {
        fs::write(
            root.join(format!("Library{index}.psc")),
            format!("ScriptName Library{index}\nFunction Entry()\nEndFunction\n"),
        )
        .unwrap();
    }
    let project = root.join("Project.psc");
    let project_text =
        "ScriptName Project\nActor Target\nFunction Test()\n  Target.Jump()\nEndFunction\n";
    fs::write(
        &project,
        "ScriptName Project\nFunction DiskOnly()\nEndFunction\n",
    )
    .unwrap();

    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || papyrus_language_server::run_connection(&server));
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(1),
            method: "initialize".to_owned(),
            params: json!({
                "capabilities": { "window": { "workDoneProgress": true } },
                "initializationOptions": { "papyrus": {
                    "dialect": "auto",
                    "sourceRoots": [root.to_string_lossy()]
                }}
            }),
        }))
        .unwrap();
    let first = client
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("initialize response should arrive before background messages");
    let Message::Response(response) = first else {
        panic!("initialization must complete before indexing progress");
    };
    assert_eq!(response.id, RequestId::from(1));
    assert!(response.response_result.is_ok());

    client
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".to_owned(),
            json!({}),
        )))
        .unwrap();
    let create = client
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("progress creation request should arrive after initialized");
    let Message::Request(create) = create else {
        panic!("expected work-done progress creation request");
    };
    assert_eq!(create.method, "window/workDoneProgress/create");
    client
        .sender
        .send(Message::Response(Response::new_ok(create.id, Value::Null)))
        .unwrap();

    let uri = path_uri(&project);
    client
        .sender
        .send(Message::Notification(Notification::new(
            "textDocument/didOpen".to_owned(),
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "papyrus",
                    "version": 7,
                    "text": project_text
                }
            }),
        )))
        .unwrap();
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(2),
            method: "textDocument/completion".to_owned(),
            params: json!({
                "textDocument": { "uri": uri },
                "position": { "line": 3, "character": 9 }
            }),
        }))
        .unwrap();

    let mut saw_begin = false;
    let mut saw_end = false;
    let completion = loop {
        let message = client
            .receiver
            .recv_timeout(Duration::from_secs(15))
            .expect("completion or indexing progress should arrive");
        match message {
            Message::Notification(notification) if notification.method == "$/progress" => {
                saw_begin |= notification.params["value"]["kind"] == "begin";
                saw_end |= notification.params["value"]["kind"] == "end";
            }
            Message::Response(response) if response.id == RequestId::from(2) => {
                break response.response_result.unwrap();
            }
            _ => {}
        }
    };
    assert!(
        completion
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "Jump")
    );

    while !saw_end {
        let message = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("indexing end progress should arrive");
        if let Message::Notification(notification) = message
            && notification.method == "$/progress"
        {
            saw_begin |= notification.params["value"]["kind"] == "begin";
            saw_end |= notification.params["value"]["kind"] == "end";
        }
    }
    assert!(saw_begin);

    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(3),
            method: "shutdown".to_owned(),
            params: Value::Null,
        }))
        .unwrap();
    loop {
        let Message::Response(response) = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
        else {
            continue;
        };
        if response.id == RequestId::from(3) {
            break;
        }
    }
    client
        .sender
        .send(Message::Notification(Notification::new(
            "exit".to_owned(),
            Value::Null,
        )))
        .unwrap();
    server_thread.join().unwrap().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelling_indexing_rejects_queued_and_later_semantic_requests() {
    let root = temp_root();
    for index in 0..1_000 {
        fs::write(
            root.join(format!("Library{index}.psc")),
            format!("ScriptName Library{index}\nFunction Entry()\nEndFunction\n"),
        )
        .unwrap();
    }
    let project = root.join("Project.psc");
    fs::write(&project, "ScriptName Project\n").unwrap();

    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || papyrus_language_server::run_connection(&server));
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(1),
            method: "initialize".to_owned(),
            params: json!({
                "capabilities": { "window": { "workDoneProgress": true } },
                "initializationOptions": { "papyrus": {
                    "dialect": "auto",
                    "sourceRoots": [root.to_string_lossy()]
                }}
            }),
        }))
        .unwrap();
    let Message::Response(initialize) = client
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("initialize response should arrive")
    else {
        panic!("expected initialize response");
    };
    assert!(initialize.response_result.is_ok());
    client
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".to_owned(),
            json!({}),
        )))
        .unwrap();
    let Message::Request(create) = client
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("progress creation request should arrive")
    else {
        panic!("expected progress creation request");
    };
    let token = create.params["token"].clone();
    client
        .sender
        .send(Message::Response(Response::new_ok(create.id, Value::Null)))
        .unwrap();

    let uri = path_uri(&project);
    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(2),
            method: "textDocument/completion".to_owned(),
            params: json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 0 }
            }),
        }))
        .unwrap();
    client
        .sender
        .send(Message::Notification(Notification::new(
            "window/workDoneProgress/cancel".to_owned(),
            json!({ "token": token }),
        )))
        .unwrap();

    let mut saw_cancelled_end = false;
    let queued_error = loop {
        let message = client
            .receiver
            .recv_timeout(Duration::from_secs(20))
            .expect("indexing cancellation result should arrive");
        match message {
            Message::Notification(notification) if notification.method == "$/progress" => {
                if notification.params["value"]["kind"] == "end" {
                    saw_cancelled_end =
                        notification.params["value"]["message"] == "Workspace indexing cancelled";
                }
            }
            Message::Response(response) if response.id == RequestId::from(2) => {
                break response.response_result.unwrap_err();
            }
            _ => {}
        }
    };
    assert!(saw_cancelled_end);
    assert_eq!(queued_error.code, ErrorCode::ServerCancelled as i32);

    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(3),
            method: "textDocument/completion".to_owned(),
            params: json!({
                "textDocument": { "uri": uri },
                "position": { "line": 0, "character": 0 }
            }),
        }))
        .unwrap();
    let later_error = loop {
        let Message::Response(response) = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("later semantic request response should arrive")
        else {
            continue;
        };
        if response.id == RequestId::from(3) {
            break response.response_result.unwrap_err();
        }
    };
    assert_eq!(later_error.code, ErrorCode::ServerCancelled as i32);

    client
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(4),
            method: "shutdown".to_owned(),
            params: Value::Null,
        }))
        .unwrap();
    loop {
        let Message::Response(response) = client
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
        else {
            continue;
        };
        if response.id == RequestId::from(4) {
            break;
        }
    }
    client
        .sender
        .send(Message::Notification(Notification::new(
            "exit".to_owned(),
            Value::Null,
        )))
        .unwrap();
    server_thread.join().unwrap().unwrap();
    fs::remove_dir_all(root).unwrap();
}

fn path_uri(path: &std::path::Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let prefix = if path.starts_with("//") {
        "file:"
    } else {
        "file:///"
    };
    format!("{prefix}{}", path.trim_start_matches('/'))
}

fn temp_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "papyrus-background-indexing-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}
