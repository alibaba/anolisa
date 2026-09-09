//! Native `OpenTelemetry` context; no parallel trace identity or process context store.
mod fields;
mod propagation;
#[cfg(feature = "runtime")]
mod runtime;

pub use fields::{
    AgentFieldProcessor, CompatibilityCorrelation, CorrelationSnapshot, MetadataKind,
    bind_metadata, bind_trace_context_input, bounded, normalize, snapshot, validate_metadata,
};
pub use opentelemetry::Context;
use opentelemetry::trace::TraceContextExt as _;
pub use propagation::{extract_parent, inject_context, report_propagation_issues};
#[cfg(feature = "runtime")]
pub use runtime::{TelemetryRuntime, init_runtime, report_startup_error};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

/// Captures the full current context for a task/thread boundary. Create the child
/// span before spawning, then instrument its future; pass a fresh, unentered span.
/// Never hold an entered guard
/// across await. Explicit parents preserve typed extensions and late-bound baggage.
pub fn parent_span(span: tracing::Span, parent: Context) -> tracing::Span {
    let _ = span.set_parent(parent);
    span
}

#[derive(Clone, Debug)]
struct RequestSpanId(String);

/// Binds the current SDK span as the request correlation anchor. Attach this
/// context inside the request span scope, before creating its business children.
pub fn request_context() -> Context {
    let context = Context::current();
    let span = context.span();
    if span.span_context().is_valid() {
        context.with_value(RequestSpanId(span.span_context().span_id().to_string()))
    } else {
        context.clone()
    }
}

/// Emits one bounded local correlation record. Only a static reason and the
/// allowlisted snapshot are included; arbitrary messages/params are not logged.
pub fn diagnostic(reason: &'static str) {
    tracing::info!(target: "asc_observability::diagnostic", reason, correlation = %serde_json::to_string(&snapshot()).unwrap_or_default());
}

/// Sets a fixed failure category without capturing an error's potentially secret text.
pub fn mark_error(category: &'static str) {
    let context = Context::current();
    let span = context.span();
    span.set_status(opentelemetry::trace::Status::error(category));
    span.set_attribute(opentelemetry::KeyValue::new("error.type", category));
    span.set_attribute(opentelemetry::KeyValue::new(
        "agentsec.execution.status",
        "failed",
    ));
}

/// Isolated transport/protocol rejection scope, without trusting unread input.
pub fn rejection_scope<T>(reason: &'static str, work: impl FnOnce() -> T) -> T {
    let span = parent_span(
        tracing::info_span!(parent: None, "daemon.reject", otel.kind = "server"),
        Context::new(),
    );
    span.in_scope(|| {
        let _context = request_context().attach();
        mark_error(reason);
        diagnostic(reason);
        work()
    })
}

/// Marks completion without conflating accepted PAP intent with enforcement.
pub fn mark_success() {
    Context::current()
        .span()
        .set_attribute(opentelemetry::KeyValue::new(
            "agentsec.execution.status",
            "succeeded",
        ));
}
