use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const PROVIDERS: [Provider; 4] = [
    Provider::Anthropic,
    Provider::Openai,
    Provider::Openrouter,
    Provider::Gemini,
];
const IMAGE: &[u8] = b"synthetic-image-not-a-screenshot";

fn api(provider: Provider) -> Api {
    Api::new(
        provider,
        "test-model".into(),
        provider.default_base(),
        Zeroizing::new("dummy-key".into()),
        10,
    )
    .unwrap()
}

fn prompt(body: &Value, provider: Provider) -> &str {
    match provider {
        Provider::Anthropic => &body["system"],
        Provider::Openai | Provider::Openrouter => &body["messages"][0]["content"],
        Provider::Gemini => &body["systemInstruction"]["parts"][0]["text"],
    }
    .as_str()
    .unwrap()
}

fn response(provider: Provider, text: &str) -> Value {
    match provider {
        Provider::Anthropic => json!({"stop_reason":"end_turn","content":[
            {"type":"thinking","thinking":"private reasoning 9","signature":"test"},
            {"type":"redacted_thinking","data":"private data"}, {"type":"text","text":text}
        ]}),
        Provider::Openai | Provider::Openrouter => {
            json!({"choices":[{"finish_reason":"stop","message":{"content":text}}]})
        }
        Provider::Gemini => json!({"candidates":[{"finishReason":"STOP","content":{"parts":[
            {"thought":true,"text":"private reasoning 9"}, {"text":text}
        ]}}]}),
    }
}

#[test]
fn single_answer_contract_for_every_provider_and_display_limit() {
    for (mode, limit) in [(AnswerMode::Digits, 9), (AnswerMode::Dots, 6)] {
        for provider in PROVIDERS {
            let api = api(provider).with_mode(mode);
            let body = api.body(IMAGE);
            let prompt = prompt(&body, provider);
            assert!(prompt.contains(&format!("between 2 and {limit} choices")));
            assert!(prompt.contains("Exactly one choice is correct"));
            if mode == AnswerMode::Dots {
                assert!(prompt.contains("F=6") && !prompt.contains("G=7"));
            }
            if provider == Provider::Anthropic {
                let allowed: Vec<_> = (1..=limit)
                    .map(|n| n.to_string())
                    .chain(["?".into()])
                    .collect();
                assert_eq!(
                    body["output_config"]["format"],
                    json!({
                        "type":"json_schema", "schema": {
                            "type":"object", "properties":{"answer":{"type":"string", "enum":allowed}},
                            "required":["answer"], "additionalProperties":false
                        }
                    })
                );
                assert!(prompt.contains(JSON_FORMAT) && !prompt.contains(TEXT_FORMAT));
            } else {
                assert!(body.get("output_config").is_none());
                assert!(prompt.contains(TEXT_FORMAT));
            }
            for value in VALID_ANSWERS {
                let text = if provider == Provider::Anthropic {
                    json!({"answer":value}).to_string()
                } else {
                    value.into()
                };
                let answer = api.extract(&response(provider, &text));
                if value != "?" && value.parse::<u8>().unwrap() > limit {
                    assert!(answer.is_err());
                } else {
                    assert_eq!(answer.unwrap(), parse_answer(value).unwrap());
                }
            }
            for empty in [json!({}), json!({"choices":[]}), json!({"candidates":[]})] {
                assert!(api.extract(&empty).is_err());
            }
        }
    }
}

#[test]
fn multiple_answer_contract_for_every_provider_and_display_limit() {
    for (mode, limit) in [(AnswerMode::Digits, 9), (AnswerMode::Dots, 6)] {
        for provider in PROVIDERS {
            let api = api(provider)
                .with_mode(mode)
                .with_selection(SelectionMode::Multiple);
            let body = api.body(IMAGE);
            let prompt = prompt(&body, provider);
            assert!(prompt.contains("Select EVERY correct choice"));
            assert!(!prompt.contains("Exactly one choice is correct"));
            if provider == Provider::Anthropic {
                let schema = &body["output_config"]["format"]["schema"];
                assert_eq!(schema["required"], json!(["answers"]));
                assert_eq!(schema["additionalProperties"], false);
                let allowed: Vec<_> = (1..=limit).map(|n| n.to_string()).collect();
                assert_eq!(
                    schema["properties"]["answers"]["items"]["enum"],
                    json!(allowed)
                );
                assert!(prompt.contains(MULTIPLE_JSON_FORMAT));
            } else {
                assert!(prompt.contains(MULTIPLE_TEXT_FORMAT));
            }
            for values in [
                vec![1, 2],
                vec![3, 1, 2],
                vec![6],
                (1..=limit).collect(),
                vec![1, 7],
            ] {
                let strings: Vec<_> = values.iter().map(u8::to_string).collect();
                let text = if provider == Provider::Anthropic {
                    json!({"answers":strings}).to_string()
                } else {
                    strings.join("'")
                };
                let answer = api.extract(&response(provider, &text));
                if values.iter().any(|n| *n > limit) {
                    assert!(answer.is_err());
                } else {
                    let mut expected = values;
                    expected.sort_unstable();
                    assert_eq!(answer.unwrap().values(), expected);
                }
            }
        }
    }
}

#[test]
fn structured_multiple_answers_are_strict_and_private() {
    assert_eq!(
        parse_structured_multiple_answer(r#"{"answers":[]}"#).unwrap(),
        Answer::Unknown
    );
    assert_eq!(
        parse_structured_multiple_answer(r#"{"answers":["2","1"]}"#)
            .unwrap()
            .values(),
        vec![1, 2]
    );
    for invalid in [
        r#"{"answers":["1","1"]}"#,
        r#"{"answers":[1,2]}"#,
        r#"{"answers":["1'2"]}"#,
        r#"{"answers":["1","?"]}"#,
        r#"{"answers":["0"]}"#,
        r#"{"answers":["10"]}"#,
        r#"{"answers":[" 2 "]}"#,
        r#"{"answers":["A"]}"#,
        r#"{"answers":["private-secret"]}"#,
        r#"{"answers":null}"#,
        r#"{"answers":["1"],"extra":"private-secret"}"#,
        r#"{"answer":"1"}"#,
        r#"{"answers":"1'2"}"#,
        r#"{"answers":[],"answers":["1"]}"#,
        "1'2",
        "{}",
    ] {
        let error = parse_structured_multiple_answer(invalid)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private-secret"));
    }
    assert!(
        api(Provider::Anthropic)
            .extract(&response(Provider::Anthropic, r#"{"answers":["1","2"]}"#))
            .is_err()
    );
}

#[test]
fn structured_single_answers_are_strict_and_private() {
    for value in VALID_ANSWERS {
        let json = json!({"answer":value}).to_string();
        assert_eq!(
            parse_structured_answer(&format!(" \n{json}\n")).unwrap(),
            parse_answer(value).unwrap()
        );
    }
    for invalid in [
        "",
        "2",
        "B",
        "**2**",
        "The correct answer is 2.",
        r#"{"answer":2}"#,
        r#"{"answer":"B"}"#,
        r#"{"answer":"0"}"#,
        r#"{"answer":"10"}"#,
        r#"{"answer":"1,2"}"#,
        r#"{"answer":" 2 "}"#,
        r#"{"answer":"９"}"#,
        r#"{"answer":"2","extra":"private-content"}"#,
        r#"{"answer":"private-content"}"#,
        r#"{"answer":"2","answer":"3"}"#,
        r#"[{"answer":"2"}]"#,
        r#"{}"#,
        r#"null"#,
        "```json\n{\"answer\":\"2\"}\n```",
        r#"{"answer":"2"} {"answer":"3"}"#,
    ] {
        let error = parse_structured_answer(invalid).unwrap_err().to_string();
        assert!(!error.contains("private-content"));
    }
}

#[test]
fn anthropic_schema_preserves_thinking_and_rejects_incomplete_outputs() {
    let mut api = api(Provider::Anthropic);
    api.model = "claude-opus-5-5".into();
    let body = api.body(IMAGE);
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["max_tokens"], 4096);
    for field in ["thinking", "temperature", "top_p", "top_k"] {
        assert!(body.get(field).is_none());
    }
    let reply = response(Provider::Anthropic, r#"{"answer":"4"}"#);
    assert_eq!(api.extract(&reply).unwrap(), Answer::Choice(4));
    for stop in ["refusal", "max_tokens", "tool_use"] {
        let mut stopped = reply.clone();
        stopped["stop_reason"] = json!(stop);
        let error = api.extract(&stopped).unwrap_err().to_string();
        assert!(!error.contains("private"));
        if stop == "max_tokens" {
            assert!(error.contains("4096") && error.contains("myślenie"));
        }
    }
}

#[test]
fn url_security_and_paths() {
    for provider in PROVIDERS {
        let endpoint = endpoint_url(provider, "test-model", provider.default_base()).unwrap();
        assert!(endpoint.as_str().starts_with(provider.default_base()));
        assert!(!endpoint.as_str().contains("//messages"));
    }
    for base in [
        "http://example.com/v1",
        "https://user:secret@example.com",
        "https://example.com/?key=secret",
        "https://example.com/#secret",
        "file:///tmp/api",
    ] {
        assert!(endpoint_url(Provider::Openai, "model", base).is_err());
    }
    assert!(endpoint_url(Provider::Openai, "", Provider::Openai.default_base()).is_err());
    assert!(
        endpoint_url(
            Provider::Gemini,
            "models/name",
            Provider::Gemini.default_base()
        )
        .is_err()
    );
    assert!(
        api(Provider::Gemini)
            .endpoint
            .path()
            .ends_with("/models/test-model:generateContent")
    );
}

async fn read_request(stream: &mut TcpStream) -> (String, Value) {
    let mut received = Vec::new();
    let body_start = loop {
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0);
        received.extend_from_slice(&buf[..n]);
        if let Some(i) = received.windows(4).position(|x| x == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let headers = String::from_utf8_lossy(&received[..body_start]).to_lowercase();
    let len: usize = headers
        .lines()
        .find_map(|l| l.strip_prefix("content-length: "))
        .unwrap()
        .parse()
        .unwrap();
    while received.len() < body_start + len {
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0);
        received.extend_from_slice(&buf[..n]);
    }
    (
        headers,
        serde_json::from_slice(&received[body_start..body_start + len]).unwrap(),
    )
}

async fn send_response(stream: &mut TcpStream, status: u16, body: &str, extra: &str) {
    let response = format!(
        "HTTP/1.1 {status} Test\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}

#[tokio::test]
async fn rejects_http_errors_redirects_and_malformed_responses() {
    for (status, body) in [
        (401, "private-api-key-and-response"),
        (404, "private-api-key-and-response"),
        (429, "private-api-key-and-response"),
        (500, "private-api-key-and-response"),
        (302, "private-api-key-and-response"),
        (200, "not-json-private-api-key-and-response"),
        (200, "{}"),
        (
            200,
            r#"{"choices":[{"finish_reason":"stop","message":{"content":"1 and 2"}}]}"#,
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            // Remain loopback-only even if redirect prevention regresses.
            let extra = if status == 302 {
                format!("Location: http://{addr}/redirect\r\n")
            } else {
                String::new()
            };
            send_response(&mut stream, status, body, &extra).await;
        });
        let api = Api::new(
            Provider::Openai,
            "model".into(),
            &format!("http://{addr}/v1"),
            Zeroizing::new("dummy-key".into()),
            2,
        )
        .unwrap();
        let error = api.solve(IMAGE).await.unwrap_err().to_string();
        for private in ["private-api-key", "dummy-key", "1 and 2"] {
            assert!(!error.contains(private));
        }
        if status != 200 {
            assert!(error.contains(&format!("HTTP {status}")), "{error}");
        }
        if status == 404 {
            assert!(error.contains("nazwę") && !error.contains("klucz"));
        }
        timeout_server(server).await;
    }
}

#[tokio::test]
async fn request_timeout_is_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
    });
    let api = Api::new(
        Provider::Openai,
        "model".into(),
        &format!("http://{addr}/v1"),
        Zeroizing::new("dummy-key".into()),
        1,
    )
    .unwrap();
    let start = std::time::Instant::now();
    assert!(
        api.solve(IMAGE)
            .await
            .unwrap_err()
            .to_string()
            .contains("limit czasu")
    );
    assert!(start.elapsed() < Duration::from_secs(3));
    server.abort();
}

async fn timeout_server(server: tokio::task::JoinHandle<()>) {
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn real_http_contracts_without_external_api() {
    for provider in PROVIDERS {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (headers, payload) = read_request(&mut stream).await;
            if provider != Provider::Gemini {
                assert_eq!(payload["model"], "test-model");
            }
            assert!(!payload.to_string().contains("dummy-key"));
            let (header, path, encoded, text) = match provider {
                Provider::Anthropic => (
                    "x-api-key: dummy-key",
                    "/v1/messages",
                    payload["messages"][0]["content"][0]["source"]["data"]
                        .as_str()
                        .unwrap(),
                    r#"{"answer":"9"}"#,
                ),
                Provider::Openai | Provider::Openrouter => (
                    "authorization: bearer dummy-key",
                    "/v1/chat/completions",
                    payload["messages"][1]["content"][0]["image_url"]["url"]
                        .as_str()
                        .unwrap()
                        .strip_prefix("data:image/jpeg;base64,")
                        .unwrap(),
                    "9",
                ),
                Provider::Gemini => (
                    "x-goog-api-key: dummy-key",
                    "/v1/models/test-model:generatecontent",
                    payload["contents"][0]["parts"][0]["inlineData"]["data"]
                        .as_str()
                        .unwrap(),
                    "9",
                ),
            };
            assert_eq!(STANDARD.decode(encoded).unwrap(), IMAGE);
            if provider == Provider::Anthropic {
                assert_eq!(
                    payload["output_config"]["format"]["schema"],
                    answer_schema(AnswerMode::Digits)
                );
            }
            assert!(headers.contains(header), "{headers}");
            assert!(
                headers.starts_with(&format!("post {path} http/1.1")),
                "{headers}"
            );
            send_response(&mut stream, 200, &response(provider, text).to_string(), "").await;
        });
        let api = Api::new(
            provider,
            "test-model".into(),
            &format!("http://{addr}/v1"),
            Zeroizing::new("dummy-key".into()),
            5,
        )
        .unwrap();
        assert_eq!(api.solve(IMAGE).await.unwrap(), Answer::Choice(9));
        timeout_server(server).await;
    }
}
