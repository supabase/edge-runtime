const primordials = globalThis.__bootstrap.primordials;
const { SymbolAsyncIterator } = primordials;
const core = globalThis.Deno.core;

class SupabaseEventListener {
    [SymbolAsyncIterator]() {
        return {
            async next() {
                try {
                  const reqEvt = await core.opAsync("op_event_accept");
                  const done = reqEvt === "Done";

                  let value = undefined;
                  if (!done) {
                    const rawEvent = reqEvt['Event'];
                    const eventType = Object.keys(rawEvent)[0];
                    value = {
                        deployment_id: "",
                        timestamp: new Date().toISOString(),
                        event_type: eventType,
                        event: rawEvent[eventType],
                        execution_id: "",
                        region: "",
                        context: undefined
                    }
                  }

                  return { value, done }
                } catch (e) {
                  // TODO: handle errors
                  throw e;
                }
            },
        };
    }

}

export { SupabaseEventListener };
