use std::collections::HashMap;

use opentelemetry::Context;
use opentelemetry::propagation::TextMapPropagator as _;
use opentelemetry_sdk::propagation::TraceContextPropagator;

pub(crate) struct TraceCarrier {
    pub(crate) traceparent: Option<String>,
    pub(crate) tracestate: Option<String>,
}

pub(crate) fn inject(context: &Context) -> TraceCarrier {
    let mut carrier = HashMap::new();
    TraceContextPropagator::new().inject_context(context, &mut carrier);
    TraceCarrier {
        traceparent: carrier.remove("traceparent"),
        tracestate: carrier.remove("tracestate"),
    }
}
