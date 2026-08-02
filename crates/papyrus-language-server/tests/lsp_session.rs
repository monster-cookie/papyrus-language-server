use std::{thread, time::Duration};

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use serde_json::json;

#[test]
fn publishes_and_clears_unsaved_buffer_diagnostics() {
    let (server_connection, client_connection) = Connection::memory();
    let server_thread =
        thread::spawn(move || papyrus_language_server::run_connection(&server_connection));

    client_connection
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(1),
            method: "initialize".to_owned(),
            params: json!({ "capabilities": {} }),
        }))
        .expect("initialize should send");
    let initialize_response = client_connection
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("initialize response should arrive");
    assert!(matches!(initialize_response, Message::Response(_)));

    client_connection
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".to_owned(),
            json!({}),
        )))
        .expect("initialized should send");

    let uri = "file:///workspace/Unsaved.psc";
    client_connection
        .sender
        .send(Message::Notification(Notification::new(
            "textDocument/didOpen".to_owned(),
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "papyrus",
                    "version": 1,
                    "text": "ScriptName Test\nFunction Run()\nIf True\nEndFunction\n"
                }
            }),
        )))
        .expect("didOpen should send");
    let open_diagnostics = receive_diagnostics(&client_connection);
    assert_eq!(
        open_diagnostics["diagnostics"][0]["message"],
        "Missing EndIf before EndFunction"
    );

    client_connection
        .sender
        .send(Message::Notification(Notification::new(
            "textDocument/didChange".to_owned(),
            json!({
                "textDocument": { "uri": uri, "version": 2 },
                "contentChanges": [{
                    "text": "ScriptName Test\nFunction Run()\nIf True\nEndIf\nEndFunction\n"
                }]
            }),
        )))
        .expect("didChange should send");
    let changed_diagnostics = receive_diagnostics(&client_connection);
    assert_eq!(changed_diagnostics["diagnostics"], json!([]));

    client_connection
        .sender
        .send(Message::Request(Request {
            id: RequestId::from(2),
            method: "shutdown".to_owned(),
            params: json!(null),
        }))
        .expect("shutdown should send");
    let shutdown_response = client_connection
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("shutdown response should arrive");
    assert!(matches!(shutdown_response, Message::Response(_)));
    client_connection
        .sender
        .send(Message::Notification(Notification::new(
            "exit".to_owned(),
            json!(null),
        )))
        .expect("exit should send");

    server_thread
        .join()
        .expect("server thread should not panic")
        .expect("server session should succeed");
}

fn receive_diagnostics(connection: &Connection) -> serde_json::Value {
    let message = connection
        .receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("diagnostics should arrive");
    let Message::Notification(notification) = message else {
        panic!("expected diagnostics notification");
    };
    assert_eq!(notification.method, "textDocument/publishDiagnostics");
    notification.params
}
