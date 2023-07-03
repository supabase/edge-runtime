const primordials = globalThis.__bootstrap.primordials;
const { SymbolAsyncIterator } = primordials;
const core = globalThis.Deno.core;

class SupabaseEventListener {
	async nextEvent() {
		try {
			const reqEvt = await core.opAsync('op_event_accept');
			const done = reqEvt === 'Done';

			let value = undefined;
			if (!done) {
				const rawEvent = reqEvt['Event'];
				const eventType = Object.keys(rawEvent)[0];
				value = {
					timestamp: new Date().toISOString(),
					event_type: eventType,
					event: rawEvent[eventType],
				};
			}

			return { value, done };
		} catch (e) {
			// TODO: handle errors
			throw e;
		}
	}

	[SymbolAsyncIterator]() {
		const scopedClass = this;

		return {
			async next() {
				return await scopedClass.nextEvent();
			},
		};
	}
}

export { SupabaseEventListener };
