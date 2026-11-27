// SPDX-License-Identifier: Apache-2.0

//! Adapter behaviour against a scripted loopback HTTP stub.

mod common;

use std::time::Duration;

use braga_embedder::{EmbedError, Embedder, EmbedderOptions, Ollama, OpenAiCompatible};
use common::{Reply, Stub};
use serde_json::{Value, json};

fn options(max_retries: u32) -> EmbedderOptions {
    EmbedderOptions { max_retries, timeout: Duration::from_secs(5), ..EmbedderOptions::default() }
}

#[test]
fn openai_round_trips_rows_in_index_order() {
    let stub = Stub::start(|_, _| {
        Reply::json(
            json!({
                "data": [
                    {"index": 1, "embedding": [4.0, 5.0, 6.0]},
                    {"index": 0, "embedding": [1.0, 2.0, 3.0]},
                ]
            })
            .to_string(),
        )
    });
    let embedder = OpenAiCompatible::new(stub.url("/v1/embeddings"), "text-embedding-test".into());
    assert_eq!(embedder.model(), "text-embedding-test");
    assert_eq!(embedder.dim(), None);

    let vectors = embedder.embed(&["alpha", "beta"]).expect("embed");

    assert_eq!(vectors, vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]);
    assert_eq!(embedder.dim(), Some(3));
    assert_eq!(stub.requests(), 1);
}

#[test]
fn openai_count_mismatch_is_an_invalid_response() {
    let stub = Stub::start(|_, _| {
        Reply::json(json!({"data": [{"index": 0, "embedding": [1.0]}]}).to_string())
    });
    let embedder = OpenAiCompatible::new(stub.url("/v1/embeddings"), "model".into());

    let failure = embedder.embed(&["one", "two"]).expect_err("count mismatch must fail");

    assert!(matches!(failure, EmbedError::InvalidResponse { .. }));
    assert_eq!(embedder.dim(), None);
}

#[test]
fn openai_malformed_body_is_an_invalid_response() {
    let stub = Stub::start(|_, _| Reply::json("this is not json"));
    let embedder = OpenAiCompatible::new(stub.url("/v1/embeddings"), "model".into());

    let failure = embedder.embed(&["one"]).expect_err("malformed JSON must fail");

    assert!(matches!(failure, EmbedError::InvalidResponse { .. }));
}

#[test]
fn ollama_round_trips_embeddings_in_input_order() {
    let stub = Stub::start(|_, _| {
        Reply::json(json!({"embeddings": [[1.0, 2.0], [3.0, 4.0]]}).to_string())
    });
    let embedder = Ollama::new(stub.url("/api/embed"), "nomic-embed-text".into());

    let vectors = embedder.embed(&["first", "second"]).expect("embed");

    assert_eq!(vectors, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    assert_eq!(embedder.dim(), Some(2));
}

#[test]
fn ollama_count_mismatch_is_an_invalid_response() {
    let stub = Stub::start(|_, _| Reply::json(json!({"embeddings": [[1.0]]}).to_string()));
    let embedder = Ollama::new(stub.url("/api/embed"), "model".into());

    let failure = embedder.embed(&["one", "two"]).expect_err("count mismatch must fail");

    assert!(matches!(failure, EmbedError::InvalidResponse { .. }));
}

#[test]
fn ollama_malformed_body_is_an_invalid_response() {
    let stub = Stub::start(|_, _| Reply::json("not json"));
    let embedder = Ollama::new(stub.url("/api/embed"), "model".into());

    let failure = embedder.embed(&["one"]).expect_err("malformed JSON must fail");

    assert!(matches!(failure, EmbedError::InvalidResponse { .. }));
}

#[test]
fn batching_issues_one_request_per_batch() {
    let stub = Stub::start(|_, body| {
        let request: Value = serde_json::from_str(body).expect("request is JSON");
        let count = request["input"].as_array().expect("input array").len();
        let embeddings = vec![json!([1.0, 2.0]); count];
        Reply::json(json!({"embeddings": embeddings}).to_string())
    });
    let embedder = Ollama::new(stub.url("/api/embed"), "model".into());
    let inputs: Vec<String> = (0..100).map(|index| format!("text {index}")).collect();
    let refs: Vec<&str> = inputs.iter().map(String::as_str).collect();

    let vectors = embedder.embed(&refs).expect("embed");

    assert_eq!(vectors.len(), 100);
    assert_eq!(stub.requests(), 4);
}

#[test]
fn empty_input_makes_no_request() {
    let stub = Stub::start(|_, _| Reply::status(500));
    let embedder = Ollama::new(stub.url("/api/embed"), "model".into());

    let vectors = embedder.embed(&[]).expect("empty input is not an error");

    assert_eq!(vectors, Vec::<Vec<f32>>::new());
    assert_eq!(stub.requests(), 0);
    assert_eq!(embedder.dim(), None);
}

#[test]
fn retries_429_then_succeeds() {
    let stub = Stub::start(|index, _| {
        if index == 0 {
            Reply::status(429)
        } else {
            Reply::json(json!({"embeddings": [[1.0, 2.0]]}).to_string())
        }
    });
    let embedder = Ollama::with_options(stub.url("/api/embed"), "model".into(), options(3));

    let vectors = embedder.embed(&["one"]).expect("second attempt succeeds");

    assert_eq!(vectors, vec![vec![1.0, 2.0]]);
    assert_eq!(stub.requests(), 2);
}

#[test]
fn retries_5xx_until_retries_are_exhausted() {
    let stub = Stub::start(|_, _| Reply::status(500));
    let embedder = Ollama::with_options(stub.url("/api/embed"), "model".into(), options(2));

    let failure = embedder.embed(&["one"]).expect_err("500 must eventually fail");

    assert!(matches!(failure, EmbedError::Status { code: 500 }));
    assert_eq!(stub.requests(), 3);
}

#[test]
fn fails_immediately_on_non_retryable_status() {
    let stub = Stub::start(|_, _| Reply::status(401));
    let embedder = Ollama::with_options(stub.url("/api/embed"), "model".into(), options(3));

    let failure = embedder.embed(&["one"]).expect_err("401 must fail");

    assert!(matches!(failure, EmbedError::Status { code: 401 }));
    assert_eq!(stub.requests(), 1);
}

#[test]
fn a_held_response_becomes_a_transport_timeout() {
    let stub = Stub::start(|_, _| {
        Reply::json(json!({"embeddings": [[1.0]]}).to_string()).delayed(Duration::from_secs(2))
    });
    let embedder = Ollama::with_options(
        stub.url("/api/embed"),
        "model".into(),
        EmbedderOptions {
            max_retries: 0,
            timeout: Duration::from_millis(100),
            ..Default::default()
        },
    );

    let failure = embedder.embed(&["one"]).expect_err("the stub never answers in time");

    assert!(matches!(failure, EmbedError::Transport { .. }));
    assert_eq!(stub.requests(), 1);
}

#[test]
fn dim_is_learned_once_and_a_different_width_fails() {
    let stub = Stub::start(|index, _| {
        let vector = if index == 0 { json!([1.0, 2.0]) } else { json!([1.0, 2.0, 3.0]) };
        Reply::json(json!({"embeddings": [vector]}).to_string())
    });
    let embedder = Ollama::new(stub.url("/api/embed"), "model".into());

    embedder.embed(&["one"]).expect("first response pins the width");
    assert_eq!(embedder.dim(), Some(2));

    let failure = embedder.embed(&["two"]).expect_err("a wider response must fail");

    assert!(matches!(failure, EmbedError::DimensionMismatch { expected: 2, actual: 3 }));
    assert_eq!(embedder.dim(), Some(2));
}

#[test]
fn errors_never_retain_input_text_or_response_body() {
    const INPUT: &str = "SECRET-INPUT-TEXT";
    const BODY: &str = "SECRET-RESPONSE-BODY";

    let failed = Stub::start(|_, _| Reply { status: 500, body: BODY.into(), delay: None });
    let embedder =
        OpenAiCompatible::with_options(failed.url("/v1/embeddings"), "model".into(), options(0));
    let failure = embedder.embed(&[INPUT]).expect_err("500 must fail");
    let rendered = format!("{failure} {failure:?}");
    assert_eq!(failure.to_string(), "embedding request returned HTTP status 500");
    assert!(!rendered.contains("SECRET"));

    let malformed = Stub::start(|_, _| Reply::json(BODY));
    let embedder =
        OpenAiCompatible::with_options(malformed.url("/v1/embeddings"), "model".into(), options(0));
    let failure = embedder.embed(&[INPUT]).expect_err("malformed body must fail");
    let rendered = format!("{failure} {failure:?}");
    assert!(matches!(failure, EmbedError::InvalidResponse { .. }));
    assert!(!rendered.contains("SECRET"));
}
