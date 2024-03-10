mod common;

use common::{init_logging, BalanceBeam, EchoServer, Server};

/// The `setup` function initializes logging, creates an `EchoServer`, and a `BalanceBeam` instance with
/// the `EchoServer` address.
async fn setup() -> (BalanceBeam, EchoServer) {
    init_logging();
    let upstream = EchoServer::new().await;
    let balancebeam = BalanceBeam::new(&[&upstream.address], None, None).await;
    (balancebeam, upstream)
}



/// The test_simple_connections function sets up a test environment, sends GET and POST requests, and
/// checks the responses and number of requests received by the server.
#[tokio::test]
async fn test_simple_connections() {
    let (balancebeam, upstream) = setup().await;

    log::info!("Sending a GET request");
    let response_text = balancebeam
        .get("/first_url")
        .await
        .expect("Error sending request to balancebeam");
    assert!(response_text.contains("GET /first_url HTTP/1.1"));
    assert!(response_text.contains("x-sent-by: balancebeam-tests"));
    assert!(response_text.contains("x-forwarded-for: 127.0.0.1"));

    log::info!("Sending a POST request");
    let response_text = balancebeam
        .post("/first_url", "Hello world!")
        .await
        .expect("Error sending request to balancebeam");
    assert!(response_text.contains("POST /first_url HTTP/1.1"));
    assert!(response_text.contains("x-sent-by: balancebeam-tests"));
    assert!(response_text.contains("x-forwarded-for: 127.0.0.1"));
    assert!(response_text.contains("\n\nHello world!"));

    log::info!("Checking that the origin server received 2 requests");
    let num_requests_received = Box::new(upstream).stop().await;
    assert_eq!(
        num_requests_received, 2,
        "Upstream server did not receive the expected number of requests"
    );

    log::info!("All done :)");
}