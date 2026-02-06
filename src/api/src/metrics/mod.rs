use lazy_static::lazy_static;
use prometheus::{
    Counter, Encoder, Histogram, HistogramOpts, IntCounter, IntGauge, Opts, Registry, TextEncoder,
};

lazy_static! {
    pub static ref REGISTRY: Registry = Registry::new();

    // OAuth request counters
    pub static ref OAUTH_REQUESTS_TOTAL: IntCounter = IntCounter::with_opts(
        Opts::new("oauth_requests_total", "Total number of OAuth login requests")
    ).expect("metric can be created");

    pub static ref OAUTH_CALLBACKS_TOTAL: Counter = Counter::with_opts(
        Opts::new("oauth_callbacks_total", "Total number of OAuth callbacks")
            .subsystem("auth")
    ).expect("metric can be created");

    // PKCE validation counters
    pub static ref PKCE_VALIDATIONS_TOTAL: Counter = Counter::with_opts(
        Opts::new("pkce_validations_total", "PKCE validation attempts")
            .subsystem("auth")
    ).expect("metric can be created");

    pub static ref PKCE_FAILURES_TOTAL: IntCounter = IntCounter::with_opts(
        Opts::new("pkce_failures_total", "Failed PKCE validations")
    ).expect("metric can be created");

    // OTC exchange metrics
    pub static ref OTC_EXCHANGES_TOTAL: Counter = Counter::with_opts(
        Opts::new("otc_exchanges_total", "OTC exchange attempts")
            .subsystem("auth")
    ).expect("metric can be created");

    pub static ref OTC_EXCHANGE_DURATION: Histogram = Histogram::with_opts(
        HistogramOpts::new("otc_exchange_duration_seconds", "OTC exchange duration in seconds")
            .subsystem("auth")
    ).expect("metric can be created");

    // Rate limit counters
    pub static ref RATE_LIMIT_EXCEEDED_TOTAL: IntCounter = IntCounter::with_opts(
        Opts::new("rate_limit_exceeded_total", "Rate limit exceeded count")
    ).expect("metric can be created");

    // Realtime websocket metrics
    pub static ref WS_ACTIVE_CONNECTIONS: IntGauge = IntGauge::with_opts(
        Opts::new("ws_active_connections", "Current active websocket connections")
    ).expect("metric can be created");

    pub static ref WS_CONNECTION_REJECTED_TOTAL: IntCounter = IntCounter::with_opts(
        Opts::new("ws_connection_rejected_total", "Rejected websocket connection attempts")
    ).expect("metric can be created");

    pub static ref WS_EVENTS_PUBLISHED_TOTAL: IntCounter = IntCounter::with_opts(
        Opts::new("ws_events_published_total", "Video status events published to redis")
    ).expect("metric can be created");

    pub static ref WS_EVENTS_FANOUT_TOTAL: IntCounter = IntCounter::with_opts(
        Opts::new("ws_events_fanout_total", "Video status events pushed to websocket clients")
    ).expect("metric can be created");

    pub static ref WS_CONNECTION_DROPPED_TOTAL: IntCounter = IntCounter::with_opts(
        Opts::new("ws_connection_dropped_total", "Websocket connections dropped due to overflow or send failure")
    ).expect("metric can be created");
}

pub fn register_metrics() {
    let _ = REGISTRY.register(Box::new(OAUTH_REQUESTS_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(OAUTH_CALLBACKS_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(PKCE_VALIDATIONS_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(PKCE_FAILURES_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(OTC_EXCHANGES_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(OTC_EXCHANGE_DURATION.clone()));
    let _ = REGISTRY.register(Box::new(RATE_LIMIT_EXCEEDED_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(WS_ACTIVE_CONNECTIONS.clone()));
    let _ = REGISTRY.register(Box::new(WS_CONNECTION_REJECTED_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(WS_EVENTS_PUBLISHED_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(WS_EVENTS_FANOUT_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(WS_CONNECTION_DROPPED_TOTAL.clone()));
}

pub fn metrics_handler() -> String {
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = vec![];
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}
