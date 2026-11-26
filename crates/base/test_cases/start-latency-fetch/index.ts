// Returns a response whose headers are immutable, as a proxied fetch() response is.
Deno.serve(() => Response.redirect("https://example.com/", 302));
