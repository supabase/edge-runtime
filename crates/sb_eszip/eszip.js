const core = globalThis.Deno.core;

class Eszip {
	static async extract(bytes, dest_path) {
		await core.opAsync('op_eszip_extract', bytes, dest_path);
	}
}

export default Eszip;
