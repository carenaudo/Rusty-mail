//! Hosted-consent behavior simulated through loopback HTTP. No real keys or models.
use super::*;
use std::sync::{Arc, Mutex};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

fn store() -> Store {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    engine::migrate(&db).unwrap();
    db.execute_batch("CREATE TABLE accounts(id TEXT PRIMARY KEY,provider TEXT,email TEXT); INSERT INTO accounts VALUES('a','imap','a@example.com'),('b','imap','b@example.com'); INSERT INTO files VALUES('private','message','DoNotSend.bin','application/octet-stream',X'6174746163686d656e742d736563726574',0);").unwrap();
    Store(Arc::new(Mutex::new(db)))
}
fn message() -> engine::Mail {
    engine::Mail {
        account_id: "a".into(),
        loaded: true,
        html_body: "<p>raw-html-reader-only-marker</p>".into(),
        subject: "s".repeat(2000),
        address: "a".repeat(700),
        body: "b".repeat(14000),
        attachments: vec![engine::Attachment {
            id: "private".into(),
            name: "DoNotSend.bin".into(),
            ..Default::default()
        }],
        ..Default::default()
    }
}
fn info(url: String) -> Info {
    Info {
        configured: true,
        endpoint: url,
        model: "synthetic-model".into(),
        hosted: true,
        consent: true,
    }
}
async fn listener() -> (TcpListener, String) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", l.local_addr().unwrap());
    (l, url)
}
struct Captured {
    headers: String,
    json: serde_json::Value,
}
async fn read_request(stream: TcpStream) -> (BufReader<TcpStream>, Captured) {
    let mut io = BufReader::new(stream);
    let mut headers = String::new();
    let mut length = 0;
    loop {
        let mut line = String::new();
        assert!(io.read_line(&mut line).await.unwrap() > 0);
        headers.push_str(&line);
        assert!(headers.len() < 8192);
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().unwrap();
            }
        }
    }
    assert!(length > 0 && length < 128 * 1024);
    let mut bytes = vec![0; length];
    io.read_exact(&mut bytes).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap();
    (io, Captured { headers, json })
}
fn serve(
    l: TcpListener,
    status: &'static str,
    body: Vec<u8>,
    location: Option<String>,
    hold: bool,
) -> (tokio::task::JoinHandle<()>, oneshot::Receiver<Captured>) {
    let (sent, received) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (socket, _) = tokio::time::timeout(Duration::from_secs(3), l.accept())
            .await
            .unwrap()
            .unwrap();
        let (mut io, captured) = tokio::time::timeout(Duration::from_secs(3), read_request(socket))
            .await
            .unwrap();
        let _ = sent.send(captured);
        if hold {
            let mut byte = [0];
            let _ = tokio::time::timeout(Duration::from_secs(3), io.read(&mut byte)).await;
            return;
        }
        let location = location
            .map(|s| format!("Location: {s}\r\n"))
            .unwrap_or_default();
        let headers=format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{location}Connection: close\r\n\r\n",body.len());
        let _ = io.get_mut().write_all(headers.as_bytes()).await;
        let _ = io.get_mut().write_all(&body).await;
    });
    (task, received)
}

#[tokio::test]
async fn missing_wrong_account_or_changed_endpoint_consent_opens_no_connection() {
    for mode in 0..3 {
        let store = store();
        let runtime = AiRuntime::default();
        let (l, url) = listener().await;
        let mut m = message();
        if mode == 1 {
            set_consent(&store, &runtime, "a", &url, true).unwrap();
            m.account_id = "b".into();
        }
        if mode == 2 {
            set_consent(&store, &runtime, "a", &format!("{url}/other"), true).unwrap();
        }
        let result = request_ai(&store, &runtime, info(url), m, 0, "summary", None).await;
        assert!(result.unwrap_err().contains("permission"));
        assert!(tokio::time::timeout(Duration::from_millis(75), l.accept())
            .await
            .is_err());
    }
}

#[tokio::test]
async fn approved_request_bounds_scope_excludes_attachments_and_discloses_truncation() {
    let store = store();
    let runtime = AiRuntime::default();
    let (l, url) = listener().await;
    set_consent(&store, &runtime, "a", &url, true).unwrap();
    let (server, captured) = serve(
        l,
        "200 OK",
        br#"{"choices":[{"message":{"content":"Synthetic summary"}}]}"#.to_vec(),
        None,
        false,
    );
    let result = request_ai(
        &store,
        &runtime,
        info(url),
        message(),
        0,
        "summary",
        Some("synthetic-test-key"),
    )
    .await
    .unwrap();
    let captured = captured.await.unwrap();
    server.await.unwrap();
    let request = serde_json::to_string(&captured.json).unwrap();
    assert!(captured
        .headers
        .to_ascii_lowercase()
        .contains("authorization: bearer synthetic-test-key"));
    assert!(captured.headers.starts_with("POST /v1/chat/completions "));
    assert!(!request.contains("attachment-secret"));
    assert!(!request.contains("raw-html-reader-only-marker"));
    assert!(!request.contains("DoNotSend.bin"));
    assert!(captured.json.get("tools").is_none());
    assert_eq!(captured.json["model"], "synthetic-model");
    let text = captured.json["messages"][1]["content"].as_str().unwrap();
    assert!(text.starts_with(&format!(
        "Subject: {}\nSender: {}\n\n",
        "s".repeat(1024),
        "a".repeat(320)
    )));
    assert!(text.ends_with(&"b".repeat(12000)));
    assert!(!text.contains(&"b".repeat(12001)));
    assert!(result.contains("Input truncated"));
    assert!(result.contains("Synthetic summary"));
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let store = store();
    let runtime = AiRuntime::default();
    let (source, url) = listener().await;
    let (destination, to) = listener().await;
    set_consent(&store, &runtime, "a", &url, true).unwrap();
    let (server, _) = serve(
        source,
        "307 Temporary Redirect",
        Vec::new(),
        Some(to),
        false,
    );
    let error = request_ai(&store, &runtime, info(url), message(), 0, "summary", None)
        .await
        .unwrap_err();
    server.await.unwrap();
    assert!(error.contains("307"));
    assert!(
        tokio::time::timeout(Duration::from_millis(75), destination.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn oversized_response_and_error_body_are_not_exposed() {
    for oversized in [false, true] {
        let store = store();
        let runtime = AiRuntime::default();
        let (l, url) = listener().await;
        set_consent(&store, &runtime, "a", &url, true).unwrap();
        let body = if oversized {
            vec![b'x'; 70000]
        } else {
            b"do-not-expose-provider-secret".to_vec()
        };
        let (server, _) = serve(
            l,
            if oversized {
                "200 OK"
            } else {
                "500 Internal Server Error"
            },
            body,
            None,
            false,
        );
        let error = request_ai(&store, &runtime, info(url), message(), 0, "summary", None)
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(!error.contains("do-not-expose"));
        assert!(error.contains(if oversized { "limit" } else { "500" }));
    }
}

#[tokio::test]
async fn revocation_cancels_active_generation_and_blocks_later_requests() {
    let store = store();
    let runtime = Arc::new(AiRuntime::default());
    let (l, url) = listener().await;
    set_consent(&store, &runtime, "a", &url, true).unwrap();
    let (server, captured) = serve(l, "200 OK", Vec::new(), None, true);
    let s = store.clone();
    let r = runtime.clone();
    let i = info(url.clone());
    let task =
        tokio::spawn(async move { request_ai(&s, &r, i, message(), 0, "summary", None).await });
    captured.await.unwrap();
    set_consent(&store, &runtime, "a", &url, false).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
    assert!(result.unwrap_err().contains("cancelled"));
    server.abort();
    assert!(!db(&store, |d| has_consent(d, "a", &url)).unwrap());
}
