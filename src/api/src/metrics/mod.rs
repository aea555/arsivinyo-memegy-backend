use lazy_static::lazy_static;
use prometheus::{
    Counter, Encoder, Histogram, HistogramOpts, IntCounter, Opts, Registry, TextEncoder,
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
}

pub fn register_metrics() {
    let _ = REGISTRY.register(Box::new(OAUTH_REQUESTS_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(OAUTH_CALLBACKS_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(PKCE_VALIDATIONS_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(PKCE_FAILURES_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(OTC_EXCHANGES_TOTAL.clone()));
    let _ = REGISTRY.register(Box::new(OTC_EXCHANGE_DURATION.clone()));
    let _ = REGISTRY.register(Box::new(RATE_LIMIT_EXCEEDED_TOTAL.clone()));
}

pub fn metrics_handler() -> String {
    let encoder = TextEncoder::new();
    let metric_families = REGISTRY.gather();
    let mut buffer = vec![];
    encoder.encode(&metric_families, &mut buffer).unwrap();
    String::from_utf8(buffer).unwrap()
}
