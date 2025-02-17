// @ts-ignore
import { STATUS_CODE } from "https://deno.land/std/http/status.ts";

type SessionStroage = { [key: string]: unknown };

const SESSION_HEADER_NAME = "X-Edge-Runtime-Session-Id";
const SESSIONS = new Map<string, SessionStroage>();

function makeNewSession(): [string, SessionStroage] {
    const uuid = crypto.randomUUID();
    const storage = {};

    SESSIONS.set(uuid, storage);
    return [uuid, storage];
}

function getSessionStorageFromRequest(req: Request): SessionStroage | void {
    const maybeSessionId = req.headers.get(SESSION_HEADER_NAME);

    if (typeof maybeSessionId === "string" && SESSIONS.has(maybeSessionId)) {
        return SESSIONS.get(maybeSessionId);
    }
}

export default {
    fetch(req: Request) {
        const headers = new Headers();
        let storage: SessionStroage;

        if (req.headers.get(SESSION_HEADER_NAME)) {
            const maybeStorage = getSessionStorageFromRequest(req);

            if (!maybeStorage) {
                return new Response(null, {
                    status: STATUS_CODE.BadRequest
                });
            }

            storage = maybeStorage;
        } else {
            const [sessionId, newStorage] = makeNewSession();

            headers.set(SESSION_HEADER_NAME, sessionId);

            storage = newStorage;
        }

        if (!("count" in storage)) {
            storage["count"] = 0;
        } else {
            (storage["count"] as number)++;
        }

        const count = storage["count"] as number;

        return new Response(
            JSON.stringify({ count }),
            {
                headers
            }
        );
    }
}