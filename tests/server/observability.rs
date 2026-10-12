use super::helpers::spawn_app;

#[tokio::test]
async fn http_metrics_use_route_templates_without_request_data() {
    unsafe { std::env::set_var("BREWLOG_METRICS_ENABLED", "true") };
    brewlog::observability::init_metrics();

    let app = spawn_app().await;
    let client = reqwest::Client::new();
    let response = client
        .get(app.page_url("/bags/secret-user-id?note=private"))
        .send()
        .await
        .expect("request succeeds");
    // The protected route may return 401 in the integration test; the
    // observability assertion is about the recorded route, not auth policy.
    assert!(!response.status().is_server_error());

    let metrics = client
        .get(app.page_url("/metrics"))
        .send()
        .await
        .expect("metrics request succeeds")
        .text()
        .await
        .expect("metrics body succeeds");
    assert!(metrics.contains("route=\"/bags/{id}\""));
    assert!(!metrics.contains("secret-user-id"));
    assert!(!metrics.contains("note=private"));
    unsafe { std::env::remove_var("BREWLOG_METRICS_ENABLED") };
}
