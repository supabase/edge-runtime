const eventManager = new globalThis.EventManager();

for await (const data of eventManager) {
	if (data) {
		console.log(data);
	}
}
