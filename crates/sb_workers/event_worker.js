const primordials = globalThis.__bootstrap.primordials;
const { SymbolAsyncIterator } = primordials;
const core = globalThis.Deno.core;

class SupabaseEventListener {
    async nextEvent() {
        try {
            return await core.opAsync("op_event_accept");
        } catch (e) {
            console.error(e);
        }
    }

    #buildEvent(event) {
        const rawEvent = event['Event'];
        const eventType = Object.keys(rawEvent)[0];
        return {
            deployment_id: "",
            timestamp: new Date().toISOString(),
            event_type: eventType,
            event: rawEvent[eventType],
            execution_id: "",
            region: "",
            context: undefined
        }
    }

    [SymbolAsyncIterator]() {
        const scopedClass = this;
        return {
            async next() {
                const reqEvt = await scopedClass.nextEvent();
                const isNotDoneOrEmpty = reqEvt !== "Done" && reqEvt !== "Empty";
                return { value: isNotDoneOrEmpty ? scopedClass.#buildEvent(reqEvt) : undefined, done: reqEvt === "Done" }
            },
        };
    }

}

export { SupabaseEventListener };
