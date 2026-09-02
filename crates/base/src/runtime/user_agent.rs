use std::sync::Arc;

use deno::deno_fetch::ReqBody;
use deno_core::error::AnyError;
use http::header::HeaderValue;
use http::header::USER_AGENT;
use http::HeaderMap;
use http::Request;

pub type RequestBuilderHook =
  Arc<dyn Fn(&mut Request<ReqBody>) -> Result<(), AnyError> + Send + Sync>;

/// Builds the `deno_fetch` request hook that keeps `comment` on the
/// `User-Agent` of every request a worker sends.
///
/// `deno_fetch` already falls back to the runtime's own `User-Agent` when a
/// request carries none, so the hook only has to handle requests that set the
/// header themselves: those keep what they set, with `comment` appended, and
/// so cannot shed the project they were sent from.
///
/// Returns `None` if `comment` cannot be represented as a header value.
pub fn stamping_hook(comment: &str) -> Option<RequestBuilderHook> {
  let comment = HeaderValue::from_str(comment)
    .ok()
    .filter(|it| !it.is_empty())?;

  Some(Arc::new(move |req: &mut Request<ReqBody>| {
    append_user_agent_comment(req.headers_mut(), &comment);
    Ok(())
  }))
}

/// Appends `comment` to the `User-Agent`, if the request set one at all.
fn append_user_agent_comment(headers: &mut HeaderMap, comment: &HeaderValue) {
  let http::header::Entry::Occupied(mut entry) = headers.entry(USER_AGENT)
  else {
    return;
  };

  let current = entry.get().as_bytes();

  // Nothing to append to, and nothing to append twice to.
  if current.is_empty() {
    entry.insert(comment.clone());
    return;
  } else if current
    .windows(comment.len())
    .any(|it| it == comment.as_bytes())
  {
    return;
  }

  let mut value = Vec::with_capacity(current.len() + 1 + comment.len());

  value.extend_from_slice(current);
  value.push(b' ');
  value.extend_from_slice(comment.as_bytes());

  // The parts are header values already, so the join is one too.
  if let Ok(value) = HeaderValue::from_bytes(&value) {
    entry.insert(value);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn append(user_agent: Option<&str>) -> Option<String> {
    let comment = HeaderValue::from_static("(variant; ref=meow)");
    let mut headers = HeaderMap::new();

    if let Some(user_agent) = user_agent {
      headers.insert(USER_AGENT, HeaderValue::from_str(user_agent).unwrap());
    }

    append_user_agent_comment(&mut headers, &comment);
    headers
      .get(USER_AGENT)
      .map(|it| it.to_str().unwrap().to_string())
  }

  #[test]
  fn test_append_user_agent_comment() {
    // A request that sets no `User-Agent` is left to `deno_fetch`, which fills
    // in the runtime's own.
    assert_eq!(append(None), None);

    // One the request set itself is kept, but cannot shed the comment.
    assert_eq!(
      append(Some("curl/8.7.1")).as_deref(),
      Some("curl/8.7.1 (variant; ref=meow)")
    );

    // Already stamped (a forwarded `User-Agent`, say): left alone.
    assert_eq!(
      append(Some("curl/8.7.1 (variant; ref=meow)")).as_deref(),
      Some("curl/8.7.1 (variant; ref=meow)")
    );

    // An empty one has nothing to append to.
    assert_eq!(append(Some("")).as_deref(), Some("(variant; ref=meow)"));
  }

  #[test]
  fn test_stamping_hook_rejects_invalid_comments() {
    assert!(stamping_hook("(variant; ref=meow)").is_some());
    assert!(stamping_hook("").is_none());
    assert!(stamping_hook("new\nline").is_none());
  }
}
