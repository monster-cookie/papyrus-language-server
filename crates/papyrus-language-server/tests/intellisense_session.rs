use std::{
    fs, thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::{Value, json};

#[test]
fn advertises_and_serves_source_derived_intellisense() {
    let root = std::env::temp_dir().join(format!(
        "papyrus-intellisense-session-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("Actor.psc"),
        "ScriptName Actor\n{Source evidence}\nFunction Jump(Int Height, String Label)\nEndFunction\n",
    )
    .unwrap();
    let project = root.join("Project.psc");
    fs::write(
        &project,
        "ScriptName Project\nActor Target\nFunction Test()\n  Target.Jump(1, \"test\")\nEndFunction\n",
    )
    .unwrap();

    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || papyrus_language_server::run_connection(&server));
    send_request(
        &client,
        1,
        "initialize",
        json!({
            "capabilities": {
                "workspace": {
                    "workspaceEdit": {
                        "documentChanges": true,
                        "resourceOperations": ["rename"]
                    }
                },
                "textDocument": {
                    "rename": { "prepareSupport": true }
                }
            },
            "initializationOptions": { "papyrus": {
                "dialect": "auto",
                "sourceRoots": [root.to_string_lossy()]
            }}
        }),
    );
    let capabilities = receive_response(&client);
    assert_eq!(
        capabilities["capabilities"]["completionProvider"]["triggerCharacters"][0],
        "."
    );
    assert_eq!(capabilities["capabilities"]["hoverProvider"], true);
    assert_eq!(capabilities["capabilities"]["definitionProvider"], true);
    assert_eq!(capabilities["capabilities"]["referencesProvider"], true);
    assert_eq!(
        capabilities["capabilities"]["renameProvider"]["prepareProvider"],
        true
    );
    assert_eq!(
        capabilities["capabilities"]["signatureHelpProvider"]["triggerCharacters"][0],
        "("
    );
    assert_eq!(
        capabilities["capabilities"]["signatureHelpProvider"]["retriggerCharacters"][0],
        ","
    );
    client
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".to_owned(),
            json!({}),
        )))
        .unwrap();

    let uri = path_uri(&project);
    client.sender.send(Message::Notification(Notification::new("textDocument/didOpen".to_owned(), json!({
        "textDocument": { "uri": uri, "languageId": "papyrus", "version": 1,
            "text": "ScriptName Project\nActor Target\nFunction Test()\n  Target.Jump(1, \"test\")\nEndFunction\n" }
    })))).unwrap();
    receive_notification(&client, "textDocument/publishDiagnostics");

    send_request(
        &client,
        2,
        "textDocument/completion",
        json!({
            "textDocument": { "uri": uri }, "position": { "line": 3, "character": 9 }
        }),
    );
    let completion = receive_response(&client);
    assert!(
        completion
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "Jump")
    );

    send_request(
        &client,
        3,
        "textDocument/hover",
        json!({
            "textDocument": { "uri": uri }, "position": { "line": 3, "character": 11 }
        }),
    );
    let hover = receive_response(&client);
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("Jump")
    );

    send_request(
        &client,
        4,
        "textDocument/definition",
        json!({
            "textDocument": { "uri": uri }, "position": { "line": 3, "character": 11 }
        }),
    );
    let definition = receive_response(&client);
    assert!(definition["uri"].as_str().unwrap().ends_with("Actor.psc"));

    send_request(
        &client,
        5,
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 3, "character": 11 },
            "context": { "includeDeclaration": true }
        }),
    );
    let references = receive_response(&client);
    let references = references.as_array().unwrap();
    assert_eq!(references.len(), 2);
    assert!(
        references
            .iter()
            .any(|location| location["uri"].as_str().unwrap().ends_with("Actor.psc"))
    );
    assert!(references.iter().any(|location| {
        location["uri"].as_str().unwrap().ends_with("Project.psc")
            && location["range"]["start"]["line"] == 3
    }));

    send_request(
        &client,
        6,
        "textDocument/signatureHelp",
        json!({
            "textDocument": { "uri": uri }, "position": { "line": 3, "character": 17 }
        }),
    );
    let signature_help = receive_response(&client);
    assert_eq!(
        signature_help["signatures"][0]["label"],
        "Jump(Int Height, String Label)"
    );
    assert_eq!(signature_help["activeSignature"], 0);
    assert_eq!(signature_help["activeParameter"], 1);
    assert_eq!(
        signature_help["signatures"][0]["documentation"],
        "Source evidence"
    );

    send_request(
        &client,
        7,
        "textDocument/prepareRename",
        json!({
            "textDocument": { "uri": uri }, "position": { "line": 3, "character": 11 }
        }),
    );
    let prepare_rename = receive_response(&client);
    assert_eq!(prepare_rename["placeholder"], "Jump");
    assert_eq!(prepare_rename["range"]["start"]["line"], 3);

    send_request(
        &client,
        8,
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 3, "character": 11 },
            "newName": "Leap"
        }),
    );
    let member_rename = receive_response(&client);
    let member_operations = member_rename["documentChanges"].as_array().unwrap();
    assert_eq!(member_operations.len(), 2);
    assert!(
        member_operations
            .iter()
            .all(|operation| operation["kind"].is_null())
    );
    assert_eq!(
        member_operations
            .iter()
            .flat_map(|operation| operation["edits"].as_array().unwrap())
            .filter(|edit| edit["newText"] == "Leap")
            .count(),
        2
    );

    let actor_uri = path_uri(&root.join("Actor.psc"));
    send_request(
        &client,
        9,
        "textDocument/prepareRename",
        json!({
            "textDocument": { "uri": actor_uri }, "position": { "line": 0, "character": 12 }
        }),
    );
    let prepare_script_rename = receive_response(&client);
    assert_eq!(prepare_script_rename["placeholder"], "Actor");

    send_request(
        &client,
        10,
        "textDocument/rename",
        json!({
            "textDocument": { "uri": actor_uri },
            "position": { "line": 0, "character": 12 },
            "newName": "RenamedActor"
        }),
    );
    let script_rename = receive_response(&client);
    let script_operations = script_rename["documentChanges"].as_array().unwrap();
    let file_rename = script_operations
        .iter()
        .find(|operation| operation["kind"] == "rename")
        .unwrap();
    assert!(
        file_rename["oldUri"]
            .as_str()
            .unwrap()
            .ends_with("Actor.psc")
    );
    assert!(
        file_rename["newUri"]
            .as_str()
            .unwrap()
            .ends_with("RenamedActor.psc")
    );
    assert_eq!(
        script_operations
            .iter()
            .filter(|operation| operation["kind"].is_null())
            .flat_map(|operation| operation["edits"].as_array().unwrap())
            .filter(|edit| edit["newText"] == "RenamedActor")
            .count(),
        2
    );

    send_request(&client, 11, "shutdown", json!(null));
    receive_response(&client);
    client
        .sender
        .send(Message::Notification(Notification::new(
            "exit".to_owned(),
            json!(null),
        )))
        .unwrap();
    server_thread.join().unwrap().unwrap();
    fs::remove_dir_all(root).unwrap();
}

fn send_request(connection: &Connection, id: i32, method: &str, params: Value) {
    connection
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(id),
            method: method.to_owned(),
            params,
        }))
        .unwrap();
}

fn receive_response(connection: &Connection) -> Value {
    let Message::Response(response) = connection
        .receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
    else {
        panic!("expected response");
    };
    response.response_result.unwrap()
}

fn receive_notification(connection: &Connection, method: &str) {
    let Message::Notification(notification) = connection
        .receiver
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
    else {
        panic!("expected notification");
    };
    assert_eq!(notification.method, method);
}

fn path_uri(path: &std::path::Path) -> String {
    format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
}
