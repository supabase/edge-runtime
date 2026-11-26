use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use ext_event_worker::events::EventMetadata;
use opentelemetry::global;
use opentelemetry::trace::Span as _;
use opentelemetry::trace::SpanKind;
use opentelemetry::trace::Tracer;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::Context;
use opentelemetry::KeyValue;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::BatchSpanProcessor;
use opentelemetry_sdk::trace::TracerProvider;

static PROVIDER: OnceLock<Option<TracerProvider>> = OnceLock::new();

pub fn init_if_needed() {
  PROVIDER.get_or_init(|| {
    if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_err() {
      return None;
    }

    let exporter = opentelemetry_otlp::SpanExporter::builder()
      .with_http()
      .with_export_config(opentelemetry_otlp::ExportConfig::default())
      .build()
      .expect("failed to build OTLP span exporter");

    let processor =
      BatchSpanProcessor::builder(exporter, opentelemetry_sdk::runtime::Tokio)
        .build();

    let provider = TracerProvider::builder()
      .with_span_processor(processor)
      .build();

    global::set_tracer_provider(provider.clone());
    Some(provider)
  });
}

/// Worker identity and custom otel attributes collected once at `run()` entry.
#[derive(Clone)]
pub struct WorkerSpanAttrs(Vec<KeyValue>);

impl WorkerSpanAttrs {
  pub fn from_event_metadata(meta: &EventMetadata) -> Self {
    let mut attrs = Vec::new();

    if let Some(sp) = &meta.service_path {
      attrs.push(KeyValue::new("worker.service_path", sp.clone()));
    }
    if let Some(id) = meta.execution_id {
      attrs.push(KeyValue::new("worker.key", id.to_string()));
    }
    if let Some(extra) = &meta.otel_attributes {
      for (k, v) in extra {
        attrs.push(KeyValue::new(k.clone(), v.clone()));
      }
    }

    Self(attrs)
  }
}

/// These spans exist to find workers that hold a V8 thread, so a shorter one
/// is never exported. Every exported span carries `SLOW_SPAN_ATTR`, which the
/// sampler keeps unconditionally.
const MIN_SPAN_DURATION: Duration = Duration::from_millis(50);
const SLOW_SPAN_ATTR: &str = "edge_runtime.v8.slow";

/// Time a named section of worker execution, exported as a root span with
/// worker attributes if it runs for at least `MIN_SPAN_DURATION`.
/// Returns `None` if OTLP is not configured or `attrs` is `None`.
pub fn start_span<'a>(
  name: &'static str,
  attrs: Option<&'a WorkerSpanAttrs>,
) -> Option<OtelSpanGuard<'a>> {
  let attrs = attrs?;
  let provider = PROVIDER.get()?.as_ref()?;
  Some(OtelSpanGuard {
    provider,
    name,
    attrs,
    start_time: SystemTime::now(),
    started: Instant::now(),
  })
}

/// Exports the span on drop if it ran long enough.
pub struct OtelSpanGuard<'a> {
  provider: &'static TracerProvider,
  name: &'static str,
  attrs: &'a WorkerSpanAttrs,
  start_time: SystemTime,
  started: Instant,
}

impl Drop for OtelSpanGuard<'_> {
  fn drop(&mut self) {
    if self.started.elapsed() < MIN_SPAN_DURATION {
      return;
    }

    let tracer = self.provider.tracer("edge-runtime");
    let mut attrs = self.attrs.0.clone();
    attrs.push(KeyValue::new(SLOW_SPAN_ATTR, true));
    let mut span = tracer
      .span_builder(self.name)
      .with_kind(SpanKind::Internal)
      .with_start_time(self.start_time)
      .with_attributes(attrs)
      .start_with_context(&tracer, &Context::new());
    span.end();
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;
  use std::sync::Mutex;

  use futures_util::future::BoxFuture;
  use opentelemetry::trace::SpanId;
  use opentelemetry::Value;
  use opentelemetry_sdk::export::trace::ExportResult;
  use opentelemetry_sdk::export::trace::SpanData;
  use opentelemetry_sdk::export::trace::SpanExporter;

  use super::*;

  #[derive(Debug, Clone, Default)]
  struct Recorder(Arc<Mutex<Vec<SpanData>>>);

  impl SpanExporter for Recorder {
    fn export(
      &mut self,
      batch: Vec<SpanData>,
    ) -> BoxFuture<'static, ExportResult> {
      self.0.lock().unwrap().extend(batch);
      Box::pin(async { Ok(()) })
    }
  }

  fn record(elapsed: Duration) -> Vec<SpanData> {
    let recorder = Recorder::default();
    let provider: &'static TracerProvider = Box::leak(Box::new(
      TracerProvider::builder()
        .with_simple_exporter(recorder.clone())
        .build(),
    ));
    let attrs =
      WorkerSpanAttrs(vec![KeyValue::new("worker.service_path", "fn")]);

    drop(OtelSpanGuard {
      provider,
      name: "v8.poll_event_loop",
      attrs: &attrs,
      start_time: SystemTime::now() - elapsed,
      started: Instant::now() - elapsed,
    });

    let spans = recorder.0.lock().unwrap().clone();
    spans
  }

  #[test]
  fn short_span_is_not_exported() {
    assert!(record(Duration::from_millis(10)).is_empty());
  }

  #[test]
  fn slow_span_is_exported_as_tagged_root() {
    let spans = record(MIN_SPAN_DURATION);
    assert_eq!(spans.len(), 1);

    let span = &spans[0];
    assert_eq!(span.name, "v8.poll_event_loop");
    assert_eq!(span.parent_span_id, SpanId::INVALID);
    let attr = |key: &str| {
      span
        .attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.clone())
    };
    assert_eq!(attr(SLOW_SPAN_ATTR), Some(Value::Bool(true)));
    assert_eq!(attr("worker.service_path"), Some(Value::from("fn")));
  }
}
