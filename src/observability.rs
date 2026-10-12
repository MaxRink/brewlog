//! Bounded, opt-in metrics and tracing integration.
//!
//! Request labels are route templates rather than raw paths. Request bodies,
//! query strings, cookies and user supplied coffee data are never recorded.

use std::sync::OnceLock;
use std::time::Instant;

use axum::body::Body;
use axum::extract::MatchedPath;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use metrics::{counter, histogram};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use tracing::Span;

#[cfg(feature = "otlp")]
use opentelemetry::KeyValue;
#[cfg(feature = "otlp")]
use opentelemetry_otlp::WithExportConfig;

static PROMETHEUS_HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

// Keep request latency useful for both fast page/API requests and the slower
// optional vision workflow. These bounded buckets contain no request data and
// avoid the exporter default summary, which cannot produce Prometheus p50/p95
// or heatmap queries.
const HTTP_LATENCY_BUCKETS_SECONDS: &[f64] = &[
    0.001, 0.002, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

#[cfg(feature = "otlp")]
static OTEL_PROVIDER: OnceLock<opentelemetry_sdk::trace::TracerProvider> = OnceLock::new();

/// Install the process-wide Prometheus recorder when explicitly enabled.
pub fn init_metrics() -> bool {
    if !env_enabled("BREWLOG_METRICS_ENABLED") {
        return false;
    }

    let builder = PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Full("brewlog_http_request_duration_seconds".to_owned()),
            HTTP_LATENCY_BUCKETS_SECONDS,
        )
        .expect("failed to configure HTTP latency buckets");
    let handle = builder
        .install_recorder()
        .expect("failed to install Prometheus recorder");
    let _ = PROMETHEUS_HANDLE.set(handle);
    true
}

pub async fn metrics() -> impl IntoResponse {
    PROMETHEUS_HANDLE.get().map_or_else(
        || (axum::http::StatusCode::NOT_FOUND, String::new()),
        |handle| (axum::http::StatusCode::OK, handle.render()),
    )
}

/// Record bounded HTTP telemetry. The router template prevents IDs and other
/// user data from becoming high-cardinality labels.
pub async fn http_metrics(request: Request<Body>, next: Next) -> Response {
    let route = normalized_route(&request);
    let method = safe_method_label(request.method().as_str());
    let started = Instant::now();
    let response = next.run(request).await;
    let status_class = format!("{}xx", response.status().as_u16() / 100);
    let labels = [
        ("route", route),
        ("method", method),
        ("status_class", status_class),
    ];

    counter!("brewlog_http_requests_total", &labels).increment(1);
    histogram!("brewlog_http_request_duration_seconds", &labels)
        .record(started.elapsed().as_secs_f64());
    response
}

/// Build a content-free request span for the Axum HTTP trace layer.
pub fn request_span(request: &Request<Body>) -> Span {
    let route = normalized_route(request);
    let method = safe_method_label(request.method().as_str());
    tracing::info_span!(
        "http.request",
        http.method = %method,
        http.route = %route,
    )
}

fn normalized_route(request: &Request<Body>) -> String {
    safe_route_label(
        request
            .extensions()
            .get::<MatchedPath>()
            .map(MatchedPath::as_str),
    )
}

fn safe_route_label(route: Option<&str>) -> String {
    route.map_or_else(|| "<unmatched>".to_owned(), str::to_owned)
}

fn safe_method_label(method: &str) -> String {
    match method {
        "GET" | "HEAD" | "POST" | "PUT" | "DELETE" | "CONNECT" | "OPTIONS" | "TRACE" | "PATCH" => {
            method.to_owned()
        }
        _ => "OTHER".to_owned(),
    }
}

/// Install an optional OTLP tracer. No exporter is created unless explicitly
/// enabled and an endpoint is present. Spans contain route/method metadata only.
#[cfg(feature = "otlp")]
pub fn init_otlp_tracing() -> Result<Option<opentelemetry_sdk::trace::Tracer>, String> {
    if !env_enabled("BREWLOG_OTEL_ENABLED") {
        return Ok(None);
    }
    let endpoint = configured_otlp_endpoint(std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok())?;
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .map_err(|error| format!("failed to initialize OTLP exporter: {error}"))?;
    let provider = opentelemetry_sdk::trace::TracerProvider::builder()
        .with_resource(opentelemetry_sdk::Resource::new([KeyValue::new(
            "service.name",
            std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "brewlog".to_owned()),
        )]))
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .build();
    let tracer = opentelemetry::trace::TracerProvider::tracer(&provider, "brewlog");
    let _ = OTEL_PROVIDER.set(provider);
    Ok(Some(tracer))
}

#[cfg(feature = "otlp")]
fn configured_otlp_endpoint(value: Option<String>) -> Result<String, String> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "OTEL_EXPORTER_OTLP_ENDPOINT is required when OTLP is enabled".to_owned())
}

fn env_enabled(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        HTTP_LATENCY_BUCKETS_SECONDS, env_enabled, normalized_route, safe_method_label,
        safe_route_label,
    };
    use axum::body::Body;
    use axum::http::Request;
    use metrics::{histogram, with_local_recorder};
    use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};

    #[cfg(feature = "otlp")]
    #[test]
    fn otlp_requires_an_explicit_endpoint() {
        assert!(super::configured_otlp_endpoint(None).is_err());
        assert!(super::configured_otlp_endpoint(Some("   ".to_owned())).is_err());
        assert_eq!(
            super::configured_otlp_endpoint(Some("http://tempo:4317".to_owned())).unwrap(),
            "http://tempo:4317"
        );
    }

    #[test]
    fn metrics_are_opt_in() {
        unsafe { std::env::remove_var("BREWLOG_METRICS_ENABLED") };
        assert!(!env_enabled("BREWLOG_METRICS_ENABLED"));
        unsafe { std::env::set_var("BREWLOG_METRICS_ENABLED", "true") };
        assert!(env_enabled("BREWLOG_METRICS_ENABLED"));
        unsafe { std::env::remove_var("BREWLOG_METRICS_ENABLED") };
    }

    #[test]
    fn route_labels_never_use_raw_identifiers() {
        let mut request = Request::new(Body::empty());
        *request.uri_mut() = "/bags/secret-user-id?note=private".parse().unwrap();
        assert_eq!(normalized_route(&request), "<unmatched>");
        assert_eq!(safe_route_label(Some("/bags/{id}")), "/bags/{id}");
    }

    #[test]
    fn custom_methods_use_bounded_label() {
        assert_eq!(safe_method_label("GET"), "GET");
        assert_eq!(safe_method_label("X-Private-Method"), "OTHER");
    }

    #[test]
    fn rendered_histogram_contains_cumulative_buckets_and_no_private_url() {
        let recorder = PrometheusBuilder::new()
            .set_buckets_for_metric(
                Matcher::Full("brewlog_http_request_duration_seconds".to_owned()),
                HTTP_LATENCY_BUCKETS_SECONDS,
            )
            .expect("bucket configuration")
            .build_recorder();
        with_local_recorder(&recorder, || {
            histogram!(
                "brewlog_http_request_duration_seconds",
                "route" => "/bags/{id}",
                "method" => "GET",
                "status_class" => "2xx"
            )
            .record(0.025);
        });
        let rendered = recorder.handle().render();
        assert!(rendered.contains("brewlog_http_request_duration_seconds_bucket"));
        let lines: Vec<_> = rendered.lines().collect();
        assert!(
            lines
                .iter()
                .any(|line| line.contains("le=\"0.001\"") && line.ends_with(" 0"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("le=\"0.025\"") && line.ends_with(" 1"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("le=\"+Inf\"") && line.ends_with(" 1"))
        );
        assert!(lines.iter().any(|line| {
            line.starts_with("brewlog_http_request_duration_seconds_sum")
                && line.ends_with(" 0.025")
        }));
        assert!(lines.iter().any(|line| {
            line.starts_with("brewlog_http_request_duration_seconds_count") && line.ends_with(" 1")
        }));
        assert!(rendered.contains("route=\"/bags/{id}\""));
        assert!(!rendered.contains("secret-user-id"));
        assert!(!rendered.contains("note=private"));
    }

    #[test]
    fn latency_buckets_are_bounded_and_ordered() {
        assert_eq!(HTTP_LATENCY_BUCKETS_SECONDS.len(), 15);
        assert!(
            HTTP_LATENCY_BUCKETS_SECONDS
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        assert_eq!(HTTP_LATENCY_BUCKETS_SECONDS[0], 0.001);
        assert_eq!(HTTP_LATENCY_BUCKETS_SECONDS[14], 60.0);
    }
}
