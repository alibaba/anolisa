use std::collections::HashMap;

use asc_daemon_protocol::DaemonRequest;
use opentelemetry::propagation::TextMapPropagator as _;
use opentelemetry::trace::TraceContextExt as _;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing::{field, info_span, warn};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

pub(crate) fn request_span(request: &DaemonRequest, request_id: &str) -> tracing::Span {
    let span = info_span!(
        "daemon.request",
        rpc_method = %request.method,
        request_id,
        otel.trace_id = field::Empty,
        otel.span_id = field::Empty,
        otel.parent_valid = field::Empty
    );

    let mut carrier = HashMap::new();
    if let Some(traceparent) = request.traceparent.as_ref() {
        carrier.insert("traceparent".to_owned(), traceparent.clone());
    }
    if let Some(tracestate) = request.tracestate.as_ref() {
        carrier.insert("tracestate".to_owned(), tracestate.clone());
    }

    if !carrier.is_empty() {
        let parent = TraceContextPropagator::new().extract(&carrier);
        let parent_is_valid = parent.span().span_context().is_valid();
        span.record("otel.parent_valid", parent_is_valid);
        if parent_is_valid {
            if let Err(problem) = span.set_parent(parent) {
                warn!(
                    request_id,
                    error = %problem,
                    "could not attach the remote OpenTelemetry parent"
                );
            }
        } else {
            warn!(
                request_id,
                "invalid W3C trace context; starting a new trace"
            );
        }
    }

    let context = span.context();
    let otel_span = context.span();
    let span_context = otel_span.span_context();
    if span_context.is_valid() {
        span.record("otel.trace_id", field::display(span_context.trace_id()));
        span.record("otel.span_id", field::display(span_context.span_id()));
    }
    span
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing_subscriber::layer::SubscriberExt as _;

    #[test]
    fn request_span_uses_w3c_parent_trace_identity() {
        let provider = SdkTracerProvider::builder().build();
        let tracer = provider.tracer("asc-daemon-test");
        let subscriber =
            tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));

        tracing::subscriber::with_default(subscriber, || {
            let request: DaemonRequest = serde_json::from_value(serde_json::json!({
                "method": "daemon.health",
                "traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
            }))
            .unwrap();
            let span = request_span(&request, "request-1");
            let context = span.context();
            let otel_span = context.span();
            let span_context = otel_span.span_context();

            assert_eq!(
                span_context.trace_id().to_string(),
                "0af7651916cd43dd8448eb211c80319c"
            );
            assert_ne!(span_context.span_id().to_string(), "b7ad6b7169203331");
        });
    }
}
