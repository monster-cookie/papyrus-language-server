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

const PROJECT_SOURCE: &str = concat!(
    "ScriptName Project\n",
    "Actor Target\n",
    "Int Number\n",
    "MissingType Broken\n",
    "Duplicate AmbiguousValue\n",
    "Incomplete Pending\n",
    "Function Local(Int Required, String Optional = \"\")\n",
    "EndFunction\n",
    "Function Test()\n",
    "  MissingValue = 1\n",
    "  UnknownReceiver.Missing()\n",
    "  Target.MissingMember()\n",
    "  Number()\n",
    "  Local(Bogus = 1, Required = 2)\n",
    "  Local(Required = 1, Required = 2)\n",
    "  Local(1, \"ok\", 3)\n",
    "  Local()\n",
    "  AmbiguousValue.Anything()\n",
    "  Pending.Anything()\n",
    "EndFunction\n",
);

const CLEAN_PROJECT_SOURCE: &str = concat!(
    "ScriptName Project\n",
    "Actor Target\n",
    "Function Local(Int Required, String Optional = \"\")\n",
    "EndFunction\n",
    "Function Test()\n",
    "  Local(Required = 1)\n",
    "  Target.Known(1)\n",
    "EndFunction\n",
);

#[test]
fn publishes_semantic_diagnostics_and_revalidates_overlays() {
    let fixture = Fixture::new();
    let (client, server_thread) = start_server(&fixture.roots());
    wait_for_workspace_indexing(&client);

    let project_uri = path_uri(&fixture.project);
    open_document(&client, &project_uri, 1, PROJECT_SOURCE);
    let diagnostics = receive_diagnostics_for(&client, &project_uri, Some(1));
    assert_semantic_diagnostic_matrix(&diagnostics);

    change_document(&client, &project_uri, 2, CLEAN_PROJECT_SOURCE);
    let repaired = receive_diagnostics_for(&client, &project_uri, Some(2));
    assert_eq!(repaired["diagnostics"], json!([]));

    let syntax_error = concat!(
        "ScriptName Project\n",
        "Function Test()\n",
        "  MissingValue = 1\n",
        "  If True\n",
        "EndFunction\n",
    );
    change_document(&client, &project_uri, 3, syntax_error);
    let syntax_diagnostics = receive_diagnostics_for(&client, &project_uri, Some(3));
    let syntax_codes = diagnostic_codes(&syntax_diagnostics);
    assert!(!syntax_codes.is_empty());
    assert!(syntax_codes.keys().all(|code| {
        !code.starts_with("unresolved-")
            && code != "invalid-call-target"
            && !code.ends_with("argument")
            && code != "too-many-arguments"
    }));

    let overlay_project_uri = path_uri(&fixture.overlay_project);
    open_document(
        &client,
        &overlay_project_uri,
        1,
        &fs::read_to_string(&fixture.overlay_project).unwrap(),
    );
    let unresolved = receive_diagnostics_for(&client, &overlay_project_uri, Some(1));
    assert_eq!(
        diagnostic_codes(&unresolved),
        BTreeMap::from([("unresolved-reference".to_owned(), vec![3])])
    );

    let helper_uri = path_uri(&fixture.helper);
    let helper_overlay = concat!(
        "ScriptName Helper\n",
        "Function Help() Global\n",
        "EndFunction\n",
    );
    open_document(&client, &helper_uri, 1, helper_overlay);
    let resolved = receive_diagnostics_for(&client, &overlay_project_uri, Some(1));
    assert_eq!(resolved["diagnostics"], json!([]));

    change_document(&client, &helper_uri, 2, "ScriptName Helper\n");
    let unresolved_again = receive_diagnostics_for(&client, &overlay_project_uri, Some(1));
    assert_eq!(
        diagnostic_codes(&unresolved_again),
        BTreeMap::from([("unresolved-reference".to_owned(), vec![3])])
    );

    stop_server(client, server_thread, 90);

    let (client, server_thread) = start_server(&fixture.roots());
    wait_for_workspace_indexing(&client);
    open_document(&client, &project_uri, 1, PROJECT_SOURCE);
    let cached_diagnostics = receive_diagnostics_for(&client, &project_uri, Some(1));
    assert_semantic_diagnostic_matrix(&cached_diagnostics);
    stop_server(client, server_thread, 91);

    fixture.remove();
}

fn assert_semantic_diagnostic_matrix(params: &Value) {
    assert_eq!(
        diagnostic_codes(params),
        BTreeMap::from([
            ("ambiguous-member".to_owned(), vec![17]),
            ("ambiguous-type".to_owned(), vec![4]),
            ("duplicate-named-argument".to_owned(), vec![14]),
            ("invalid-call-target".to_owned(), vec![12]),
            ("missing-required-argument".to_owned(), vec![16]),
            ("too-many-arguments".to_owned(), vec![15]),
            ("unknown-named-argument".to_owned(), vec![13]),
            ("unresolved-member".to_owned(), vec![11]),
            ("unresolved-reference".to_owned(), vec![9, 10]),
            ("unresolved-type".to_owned(), vec![3]),
        ])
    );
    for diagnostic in params["diagnostics"].as_array().unwrap() {
        assert_eq!(diagnostic["severity"], 1);
        assert_eq!(diagnostic["source"], "papyrus-language-server");
    }
}

fn diagnostic_codes(params: &Value) -> BTreeMap<String, Vec<u64>> {
    let mut codes = BTreeMap::<String, Vec<u64>>::new();
    for diagnostic in params["diagnostics"].as_array().unwrap() {
        codes
            .entry(diagnostic["code"].as_str().unwrap().to_owned())
            .or_default()
            .push(diagnostic["range"]["start"]["line"].as_u64().unwrap());
    }
    codes
}

fn start_server(roots: &[PathBuf]) -> (Connection, ServerThread) {
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
                "sourceRoots": roots.iter().map(|root| root.to_string_lossy()).collect::<Vec<_>>()
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
            assert_eq!(
                notification.params["value"]["message"],
                "Workspace indexing complete"
            );
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

fn stop_server(client: Connection, server_thread: ServerThread, request_id: i32) {
    send_request(&client, request_id, "shutdown", Value::Null);
    receive_response(&client, request_id);
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
    project_root: PathBuf,
    first_duplicate_root: PathBuf,
    second_duplicate_root: PathBuf,
    project: PathBuf,
    overlay_project: PathBuf,
    helper: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "papyrus-semantic-diagnostics-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let project_root = root.join("project");
        let first_duplicate_root = root.join("first");
        let second_duplicate_root = root.join("second");
        for directory in [&project_root, &first_duplicate_root, &second_duplicate_root] {
            fs::create_dir_all(directory).unwrap();
        }

        fs::write(
            project_root.join("Base.psc"),
            concat!(
                "ScriptName Base\n",
                "Function Known(Int Required, String Optional = \"\")\n",
                "EndFunction\n",
            ),
        )
        .unwrap();
        fs::write(
            project_root.join("Actor.psc"),
            "ScriptName Actor Extends Base\n",
        )
        .unwrap();
        fs::write(
            project_root.join("Incomplete.psc"),
            "ScriptName Incomplete Extends MissingBase\n",
        )
        .unwrap();
        fs::write(
            first_duplicate_root.join("Duplicate.psc"),
            "ScriptName Duplicate\nInt First\n",
        )
        .unwrap();
        fs::write(
            second_duplicate_root.join("Duplicate.psc"),
            "ScriptName Duplicate\nString Second\n",
        )
        .unwrap();

        let project = project_root.join("Project.psc");
        fs::write(&project, PROJECT_SOURCE).unwrap();
        let helper = project_root.join("Helper.psc");
        fs::write(&helper, "ScriptName Helper\n").unwrap();
        let overlay_project = project_root.join("OverlayProject.psc");
        fs::write(
            &overlay_project,
            concat!(
                "ScriptName OverlayProject\n",
                "Import Helper\n",
                "Function Test()\n",
                "  Help()\n",
                "EndFunction\n",
            ),
        )
        .unwrap();

        Self {
            root,
            project_root,
            first_duplicate_root,
            second_duplicate_root,
            project,
            overlay_project,
            helper,
        }
    }

    fn roots(&self) -> Vec<PathBuf> {
        vec![
            self.project_root.clone(),
            self.first_duplicate_root.clone(),
            self.second_duplicate_root.clone(),
        ]
    }

    fn remove(self) {
        fs::remove_dir_all(self.root).unwrap();
    }
}
