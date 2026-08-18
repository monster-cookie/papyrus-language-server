use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use serde_json::{Value, json};

type ServerThread = thread::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>;

const INVALID_SOURCE: &str = concat!(
    "ScriptName Project\n",
    "Int Locked = 1 Const\n",
    "Int[] Values\n",
    "Function NoValue()\n",
    "EndFunction\n",
    "Int Function WrongReturn()\n",
    "  Return \"bad\"\n",
    "  Return\n",
    "EndFunction\n",
    "Function Test()\n",
    "  Int Count = \"bad\"\n",
    "  Count = \"bad\"\n",
    "  Locked = 2\n",
    "  Values[0] += 1\n",
    "  Count %= 2.0\n",
    "  Count = -\"bad\"\n",
    "  Count = 1 % 2.0\n",
    "  Count = Values[\"bad\"]\n",
    "  Count = Count[0]\n",
    "  Count = NoValue()\n",
    "  StateOnly()\n",
    "  Consume(NoValue())\n",
    "  String LeftVoid = \"result: \" + NoValue()\n",
    "  String RightVoid = NoValue() + \"result\"\n",
    "  Actor InvalidNew = New Actor\n",
    "  Int InvalidPrimitiveNew = New Int\n",
    "  GetterOnly = 2\n",
    "  Int[] Other = New Int[\"bad\"]\n",
    "  Actor InvalidCast = 1 As Actor\n",
    "  Bool InvalidTest = 1 Is Actor\n",
    "  If NoValue()\n",
    "  ElseIf 1 + Values\n",
    "  EndIf\n",
    "  Return 1\n",
    "EndFunction\n",
    "Function Consume(Int Value)\n",
    "EndFunction\n",
    "Int Property GetterOnly\n",
    "  Int Function Get()\n",
    "    Return 1\n",
    "  EndFunction\n",
    "EndProperty\n",
    "Group Settings\n",
    "  Bool Property GroupedEnabled Auto\n",
    "EndGroup\n",
    "State Active\n",
    "  Function StateOnly()\n",
    "  EndFunction\n",
    "EndState\n",
);

const VALID_SOURCE: &str = concat!(
    "ScriptName Project\n",
    "Struct Payload\n",
    "  Int Value\n",
    "EndStruct\n",
    "Group Settings\n",
    "  Bool Property GroupedEnabled Auto\n",
    "EndGroup\n",
    "Int Property Writable\n",
    "  Int Function Get()\n",
    "    Return 1\n",
    "  EndFunction\n",
    "  Function Set(Int Value)\n",
    "  EndFunction\n",
    "EndProperty\n",
    "State Active\n",
    "  Function StateOnly()\n",
    "  EndFunction\n",
    "EndState\n",
    "Int Count = 1\n",
    "Float Ratio = Count\n",
    "String Label = Count\n",
    "Bool Enabled = Label\n",
    "Int[] Values = New Int[2]\n",
    "Payload Data = New Payload\n",
    "Int Function Calculate()\n",
    "  Count += 1\n",
    "  Values[0] = Count\n",
    "  Writable = Count\n",
    "  StateOnly()\n",
    "  If Enabled && Count\n",
    "    Return Count\n",
    "  EndIf\n",
    "  Return 0\n",
    "EndFunction\n",
);

#[test]
fn publishes_and_clears_complete_type_checking_diagnostics() {
    let fixture = Fixture::new();
    let (client, server_thread) = start_server(&fixture.root);
    wait_for_workspace_indexing(&client);

    let uri = path_uri(&fixture.project);
    open_document(&client, &uri, 1, INVALID_SOURCE);
    let diagnostics = receive_diagnostics_for(&client, &uri, Some(1));
    assert_eq!(diagnostic_lines(&diagnostics), expected_diagnostic_lines());

    change_document(&client, &uri, 2, VALID_SOURCE);
    let repaired = receive_diagnostics_for(&client, &uri, Some(2));
    assert_eq!(repaired["diagnostics"], json!([]));

    stop_server(client, server_thread);

    let (client, server_thread) = start_server(&fixture.root);
    wait_for_workspace_indexing(&client);
    open_document(&client, &uri, 1, INVALID_SOURCE);
    let cached = receive_diagnostics_for(&client, &uri, Some(1));
    assert_eq!(diagnostic_lines(&cached), expected_diagnostic_lines());
    stop_server(client, server_thread);

    fixture.remove();
}

fn expected_diagnostic_lines() -> BTreeMap<String, Vec<u64>> {
    BTreeMap::from([
        ("incompatible-assignment".to_owned(), vec![10, 11]),
        ("incompatible-return".to_owned(), vec![6]),
        ("invalid-array-size".to_owned(), vec![27]),
        ("invalid-assignment-target".to_owned(), vec![12, 26]),
        ("invalid-binary-operands".to_owned(), vec![16, 22, 23, 31]),
        ("invalid-cast".to_owned(), vec![28]),
        ("invalid-compound-assignment".to_owned(), vec![13, 14]),
        ("invalid-condition".to_owned(), vec![30]),
        ("invalid-new-target".to_owned(), vec![24, 25]),
        ("invalid-subscript-index".to_owned(), vec![17]),
        ("invalid-subscript-target".to_owned(), vec![18]),
        ("invalid-type-test".to_owned(), vec![29]),
        ("invalid-unary-operand".to_owned(), vec![15]),
        ("missing-return-value".to_owned(), vec![7]),
        ("unexpected-return-value".to_owned(), vec![33]),
        ("void-value-use".to_owned(), vec![19, 21]),
    ])
}

fn diagnostic_lines(params: &Value) -> BTreeMap<String, Vec<u64>> {
    let mut codes = BTreeMap::<String, Vec<u64>>::new();
    for diagnostic in params["diagnostics"].as_array().unwrap() {
        codes
            .entry(diagnostic["code"].as_str().unwrap().to_owned())
            .or_default()
            .push(diagnostic["range"]["start"]["line"].as_u64().unwrap());
        assert_eq!(diagnostic["severity"], 1);
        assert_eq!(diagnostic["source"], "papyrus-language-server");
    }
    codes
}

fn start_server(root: &Path) -> (Connection, ServerThread) {
    let (server, client) = Connection::memory();
    let server_thread = thread::spawn(move || papyrus_language_server::run_connection(&server));
    send_request(
        &client,
        1,
        "initialize",
        json!({
            "capabilities": { "window": { "workDoneProgress": true } },
            "initializationOptions": { "papyrus": {
                "dialect": "auto",
                "sourceRoots": [root.to_string_lossy()]
            }}
        }),
    );
    receive_response(&client, 1);
    client
        .sender
        .send(Message::Notification(Notification::new(
            "initialized".to_owned(),
            json!({}),
        )))
        .unwrap();
    (client, server_thread)
}

fn wait_for_workspace_indexing(connection: &Connection) {
    let create = loop {
        let message = connection
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("work-done progress creation request should arrive");
        if let Message::Request(request) = message
            && request.method == "window/workDoneProgress/create"
        {
            break request;
        }
    };
    let token = create.params["token"].clone();
    connection
        .sender
        .send(Message::Response(Response::new_ok(create.id, Value::Null)))
        .unwrap();

    loop {
        let message = connection
            .receiver
            .recv_timeout(Duration::from_secs(15))
            .expect("workspace indexing progress should arrive");
        let Message::Notification(notification) = message else {
            continue;
        };
        if notification.method == "$/progress"
            && notification.params["token"] == token
            && notification.params["value"]["kind"] == "end"
        {
            return;
        }
    }
}

fn open_document(connection: &Connection, uri: &str, version: i32, text: &str) {
    connection
        .sender
        .send(Message::Notification(Notification::new(
            "textDocument/didOpen".to_owned(),
            json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "papyrus",
                    "version": version,
                    "text": text
                }
            }),
        )))
        .unwrap();
}

fn change_document(connection: &Connection, uri: &str, version: i32, text: &str) {
    connection
        .sender
        .send(Message::Notification(Notification::new(
            "textDocument/didChange".to_owned(),
            json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }]
            }),
        )))
        .unwrap();
}

fn receive_diagnostics_for(connection: &Connection, uri: &str, version: Option<i32>) -> Value {
    loop {
        let message = connection
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("diagnostics should arrive");
        let Message::Notification(notification) = message else {
            continue;
        };
        if notification.method == "textDocument/publishDiagnostics"
            && notification.params["uri"] == uri
            && notification.params["version"].as_i64() == version.map(i64::from)
        {
            return notification.params;
        }
    }
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

fn receive_response(connection: &Connection, id: i32) -> Value {
    loop {
        let message = connection
            .receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("response should arrive");
        let Message::Response(response) = message else {
            continue;
        };
        if response.id == RequestId::from(id) {
            return response.response_result.unwrap();
        }
    }
}

fn stop_server(client: Connection, server_thread: ServerThread) {
    send_request(&client, 2, "shutdown", Value::Null);
    receive_response(&client, 2);
    client
        .sender
        .send(Message::Notification(Notification::new(
            "exit".to_owned(),
            Value::Null,
        )))
        .unwrap();
    server_thread.join().unwrap().unwrap();
}

fn path_uri(path: &Path) -> String {
    format!("file:///{}", path.to_string_lossy().replace('\\', "/"))
}

struct Fixture {
    root: PathBuf,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "papyrus-type-checking-session-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Actor.psc"), "ScriptName Actor\n").unwrap();
        let project = root.join("Project.psc");
        fs::write(&project, INVALID_SOURCE).unwrap();
        Self { root, project }
    }

    fn remove(self) {
        fs::remove_dir_all(self.root).unwrap();
    }
}
