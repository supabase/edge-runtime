deno_core::extension!(
  ai,
  esm_entry_point = "ext:ai/ai.js",
  esm = [
    dir "js",
    "ai.js",
    "util/event_stream_parser.mjs",
    "util/event_source_stream.mjs",
  ]
);
